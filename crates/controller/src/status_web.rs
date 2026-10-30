use crate::{Error, Result, status};
use std::{io::Write, net::TcpListener};
use tiny_http::{Header, Method, Response, Server};
use tl3_store::Reader;

pub fn serve(config: &tl3_config::Config) -> Result<()> {
    let listener = TcpListener::bind(config.status.listen)?;
    let server = Server::from_listener(listener, None)
        .map_err(|_| Error::Invalid("cannot start status HTTP server"))?;
    let mut reader = Reader::open(&config.store)?;
    println!("Status page listening on http://{}", config.status.listen);
    std::io::stdout().flush()?;
    for request in server.incoming_requests() {
        let (code, body) = if !matches!(request.method(), Method::Get | Method::Head) {
            (405, "Only GET and HEAD are allowed.\n".into())
        } else if request.url() != "/" {
            (404, "Page not found.\n".into())
        } else {
            match status::load(&mut reader) {
                Ok(model) => (200, status::render_html(&model)),
                // Neither database errors nor stored payloads belong in HTTP errors.
                Err(_) => (503, "Status is temporarily unavailable.\n".into()),
            }
        };
        let mut response = Response::from_string(body).with_status_code(code);
        for (name, value) in [
            ("Content-Type", "text/html; charset=utf-8"),
            ("Cache-Control", "no-store"),
            ("X-Content-Type-Options", "nosniff"),
            (
                "Content-Security-Policy",
                "default-src 'none'; style-src 'unsafe-inline'; base-uri 'none'; frame-ancestors 'none'",
            ),
        ] {
            response.add_header(Header::from_bytes(name, value).expect("static HTTP header"));
        }
        if code == 405 {
            response
                .add_header(Header::from_bytes("Allow", "GET, HEAD").expect("static HTTP header"));
        }
        // A disconnected browser must not stop the status process.
        let _ = request.respond(response);
    }
    Ok(())
}
