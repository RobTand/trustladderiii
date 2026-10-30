"""Receive GitHub's manifest callback and save the app credentials locally."""

import argparse
import html
from http.server import BaseHTTPRequestHandler, HTTPServer
import json
import os
from pathlib import Path
import re
import secrets
import sys
from urllib.parse import parse_qs, urlencode, urlsplit
from urllib.request import HTTPRedirectHandler, ProxyHandler, Request, build_opener


class SetupError(Exception):
    pass


def load_manifest(port):
    manifest = json.loads(Path(__file__).with_name("manifest.json").read_text())
    manifest["redirect_url"] = f"http://127.0.0.1:{port}/callback"
    return manifest


def prepare_directory(path):
    path.mkdir(mode=0o700, parents=True, exist_ok=True)
    fd = os.open(path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        if os.fstat(fd).st_uid != os.getuid():
            raise SetupError("The config directory must belong to the current user.")
        os.fchmod(fd, 0o700)
        # Never follow or read an existing credential file, including a symlink.
        if "github-app.json" in os.listdir(fd):
            raise SetupError("github-app.json already exists. Keep the existing app; setup will not overwrite it.")
        return fd
    except BaseException:
        os.close(fd)
        raise


def save_credentials(directory_fd, credentials):
    temporary = ".github-app-" + secrets.token_hex(16)
    fd = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600,
                 dir_fd=directory_fd)
    try:
        with os.fdopen(fd, "w") as output:
            os.fchmod(output.fileno(), 0o600)
            json.dump(credentials, output, indent=2)
            output.write("\n")
            output.flush()
            os.fsync(output.fileno())
        # Publish the complete file atomically without replacing an existing app.
        os.link(temporary, "github-app.json", src_dir_fd=directory_fd,
                dst_dir_fd=directory_fd, follow_symlinks=False)
        os.fsync(directory_fd)
    finally:
        os.unlink(temporary, dir_fd=directory_fd)


class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def convert(code, api_url):
    request = Request(f"{api_url}/app-manifests/{code}/conversions", data=b"",
                      method="POST", headers={
                          "Accept": "application/vnd.github+json",
                          "X-GitHub-Api-Version": "2026-03-10",
                          "User-Agent": "tl3-github-app-setup",
                      })
    # Do not forward this one-use code to redirects or environment proxies.
    with build_opener(ProxyHandler({}), NoRedirect()).open(request, timeout=30) as response:
        if response.status != 201:
            raise SetupError("GitHub did not confirm app creation.")
        body = response.read(1024 * 1024 + 1)
    if len(body) > 1024 * 1024:
        raise SetupError("GitHub returned an oversized response.")
    data = json.loads(body)
    if not isinstance(data, dict):
        raise SetupError("GitHub returned an invalid response.")
    if type(data.get("id")) is not int or data["id"] <= 0:
        raise SetupError("GitHub returned an invalid app id.")
    if not isinstance(data.get("slug"), str) or not re.fullmatch(r"[a-z0-9-]+", data["slug"]):
        raise SetupError("GitHub returned an invalid app name.")
    for name in ("pem", "webhook_secret", "client_id", "client_secret"):
        if not isinstance(data.get(name), str) or not data[name].strip():
            raise SetupError("GitHub returned incomplete credentials.")
    owner = data.get("owner")
    if not isinstance(owner, dict) or owner.get("login", "").lower() != "robtand":
        raise SetupError("Create the private app while signed in as RobTand.")
    return {name: data[name] for name in
            ("id", "slug", "pem", "webhook_secret", "client_id", "client_secret")}


def make_handler(state, api_url, directory_fd):
    class Handler(BaseHTTPRequestHandler):
        def log_message(self, format, *args):
            # Access logs would expose the callback's one-use code and state.
            pass

        def page(self, status, body):
            encoded = ("<!doctype html><html lang='en'><meta charset='utf-8'>"
                       "<title>TL3 GitHub App setup</title>" + body + "</html>").encode()
            self.send_response(status)
            self.send_header("Content-Type", "text/html; charset=utf-8")
            self.send_header("Content-Length", str(len(encoded)))
            self.send_header("Cache-Control", "no-store")
            self.send_header("Referrer-Policy", "no-referrer")
            self.send_header("X-Content-Type-Options", "nosniff")
            self.send_header("Content-Security-Policy", "default-src 'none'; form-action https://github.com; frame-ancestors 'none'; base-uri 'none'")
            self.end_headers()
            self.wfile.write(encoded)

        def do_GET(self):
            expected_host = f"127.0.0.1:{self.server.server_port}"
            if self.headers.get("Host") != expected_host:
                self.page(403, "<p>Open the loopback URL printed by the wizard.</p>")
                return
            url = urlsplit(self.path)
            if url.path == f"/{state}" and not url.query:
                action = "https://github.com/settings/apps/new?" + urlencode({"state": state})
                manifest = html.escape(json.dumps(load_manifest(self.server.server_port)), quote=True)
                self.page(200, "<h1>Create TL3's private GitHub App</h1>"
                          "<p>Sign in as RobTand. GitHub will ask you to confirm creation.</p>"
                          "<p>Afterward, install on Only select repositories: tl3-sandbox.</p>"
                          f'<form method="post" action="{html.escape(action, quote=True)}">'
                          f'<input type="hidden" name="manifest" value="{manifest}">'
                          '<button type="submit">Continue to GitHub</button></form>')
                return
            if url.path != "/callback":
                self.page(404, "<p>Open the URL printed by the wizard.</p>")
                return
            query = parse_qs(url.query, keep_blank_values=True)
            if (len(query.get("state", [])) != 1
                    or not secrets.compare_digest(query["state"][0].encode(), state.encode())
                    or len(query.get("code", [])) != 1
                    or not re.fullmatch(r"[A-Za-z0-9_-]{1,256}", query["code"][0])):
                self.page(400, "<p>Invalid callback. Return to this wizard's GitHub form.</p>")
                return
            try:
                credentials = convert(query["code"][0], api_url)
                save_credentials(directory_fd, credentials)
            except Exception:
                # Exception text and upstream response bodies may contain secrets.
                self.server.failed = True
                self.page(502, "<p>Setup failed. Check the terminal for recovery instructions.</p>")
                return
            self.server.install_url = f"https://github.com/apps/{credentials['slug']}/installations/new"
            self.page(200, "<h1>Credentials saved on dl380g10</h1>"
                      f'<p><a href="{self.server.install_url}">Install the app</a>.</p>'
                      "<p>Choose Only select repositories, select tl3-sandbox only, then click Install.</p>")

    return Handler


def parse_args():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=8765,
                        help="loopback port; use the same port in the SSH tunnel")
    parser.add_argument("--config-dir", type=Path, default=Path.home() / ".config/tl3")
    parser.add_argument("--fake-api-url", help="tests only: loopback HTTP conversion endpoint")
    args = parser.parse_args()
    if not 0 <= args.port <= 65535:
        parser.error("port must be between 0 and 65535")
    if args.fake_api_url:
        url = urlsplit(args.fake_api_url)
        if (url.scheme != "http" or url.hostname != "127.0.0.1" or not url.port
                or url.path or url.query or url.fragment or url.username or url.password):
            parser.error("fake API must be http://127.0.0.1:PORT")
    return args


def main():
    args = parse_args()
    directory_fd = prepare_directory(args.config_dir)
    try:
        state = secrets.token_urlsafe(32)
        handler = make_handler(state,
                               args.fake_api_url or "https://api.github.com", directory_fd)
        with HTTPServer(("127.0.0.1", args.port), handler) as server:
            server.failed = False
            server.install_url = None
            server.timeout = 1
            print(f"On celestia: ssh -N -L {server.server_port}:127.0.0.1:{server.server_port} rob@dl380g10", flush=True)
            print(f"Open on celestia: http://127.0.0.1:{server.server_port}/{state}", flush=True)
            print("Sign in as RobTand, continue to GitHub, then click Create GitHub App within one hour.", flush=True)
            while not server.failed and server.install_url is None:
                server.handle_request()
            if server.failed:
                raise SetupError("Setup failed; no secrets were printed. Check GitHub Settings > Developer settings > GitHub Apps before retrying. If the app exists, regenerate its credentials there rather than creating another app.")
            print("Credentials saved under the config directory with owner-only permissions.")
            print("Step 2/2: Install on Only select repositories, tl3-sandbox only:")
            print(server.install_url)
    finally:
        os.close(directory_fd)


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        print("\nSetup cancelled.", file=sys.stderr)
        sys.exit(130)
    except SetupError as error:
        print(str(error), file=sys.stderr)
        sys.exit(1)
    except Exception:
        print("Setup failed. Check the loopback port and config directory permissions.", file=sys.stderr)
        sys.exit(1)
