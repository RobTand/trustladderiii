# Create TL3's GitHub App

Run `./deploy/github-app/wizard.sh` on dl380g10. It needs Bash and Python 3.10 or later, with no packages.

1. On celestia, run the SSH tunnel command printed by the wizard and open its loopback URL.
2. Sign in as `RobTand`, click **Continue to GitHub**, then **Create GitHub App** within one hour.
3. Follow the installation link. Choose **Only select repositories**, select **tl3-sandbox** only, then click **Install**.

If port 8765 is busy, pass `--port 8766` and use the printed tunnel command.
Keep both terminals open until the wizard confirms that it saved the credentials.
The wizard does not create or install the app until you act in the browser.
GitHub's installation page controls repository selection; the manifest cannot restrict it to one repository.

The callback writes `~/.config/tl3/github-app.json` on dl380g10. The directory has mode 0700 and the file has mode 0600.
It contains GitHub's `id`, `slug`, `pem`, `webhook_secret`, `client_id`, and `client_secret` fields.
The controller can load these fields without another credential format or environment variables.
Do not copy this file into the repo, paste it into chat, or collect it in logs.
The wizard disables request logging, suppresses upstream error details, and refuses to overwrite an existing file.
If setup fails after creation, check GitHub's app settings before retrying. Regenerate credentials there if the conversion code was consumed.
If the file already exists, use the existing app's **Install App** page in GitHub settings.

Replace `https://tl3-webhook.invalid/github/events` in the app's webhook settings when the deploy ticket provides the receiver URL.
Enable the webhook at that point. It stays inactive while the placeholder is in use.

`manifest.json` requests actions, issues, pull requests, checks, and contents with write access, plus read-only metadata.
It also requests `merge_queues: read`, which GitHub requires for the `merge_group` event.
The app is private, so only its owning account can install it.
The wizard replaces the manifest's redirect URL with its actual loopback port and checks a random state on the callback.

GitHub documents `merge_group` for its native merge queue. It does not provide a dedicated Mergify queue event.
The controller must reconcile Mergify queue state through its integration; check and pull-request events alone are not a complete Mergify queue feed.

To verify this directory, submit `python3 -m unittest discover -s deploy/github-app -p 'test_*.py' -v` through PrismaBuild from a clean commit.
The suite runs the actual wizard with a loopback fake conversion API and temporary config directories.
It does not contact GitHub or read real credentials. `--fake-api-url` accepts only a loopback HTTP endpoint.

GitHub references:

- [Manifest flow and documented fields](https://docs.github.com/en/apps/sharing-github-apps/registering-a-github-app-from-a-manifest)
- [Conversion endpoint and response](https://docs.github.com/en/rest/apps/apps#create-a-github-app-from-a-manifest)
- [Merge-group event and required permission](https://docs.github.com/en/webhooks/webhook-events-and-payloads#merge_group)
- [Install your own app and select repositories](https://docs.github.com/en/apps/using-github-apps/installing-your-own-github-app)

## Workflow rerun permission

Recovery of cancelled, timed-out, or infrastructure-failed required checks needs
repository permission **Actions: Read and write** on TL3's GitHub App. The original
manifest omitted this permission; the current manifest requests it for new installations.
Rob must still grant it in the existing App's
repository permissions and approve the updated permissions for the installation,
including sandbox installation `176903725`.

An Actions API permission refusal raises a setup incident with that instruction.
It prevents re-enqueueing until access is restored and the failed effect is retried.
The controller does not change App permissions.

## Served repository command permissions

Before serving a repo, allow `tl3-robtand[bot]` in its `.mergify.yml`
`commands_restrictions` for both `queue` and `requeue`. The App installation's
write permissions do not satisfy Mergify's default sender restriction. Follow
the [served repository configuration](../../crates/mergify/README.md#served-repository-configuration).
Mergify's "Command disallowed" or "command restrictions" reply raises a setup
incident; correct the configuration before clearing the incident and retrying.
Other replies remain under the normal queue acceptance allowance.
