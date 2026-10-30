use crate::{Error, Result, http, status};
use std::{
    io::{self, Write},
    sync::{Arc, Mutex},
};
use tl3_store::Reader;

pub fn serve(config: &tl3_config::Config) -> Result<()> {
    config.validate()?;
    let reader = Arc::new(Mutex::new(StatusReader {
        settings: config.store.clone(),
        reader: Some(Reader::open(&config.store)?),
    }));
    let server = http::Server::bind(config.status.listen)?;
    println!("Status page listening on http://{}", server.address());
    std::io::stdout().flush()?;
    server.serve(move |connection| {
        let _ = handle(connection, &reader);
    })
}

struct StatusReader {
    settings: tl3_config::Store,
    reader: Option<Reader>,
}
impl StatusReader {
    fn load(&mut self) -> Result<status::Status> {
        if self.reader.is_none() {
            self.reader = Some(Reader::open(&self.settings)?);
        }
        match status::load(self.reader.as_mut().expect("opened reader")) {
            Err(Error::Store(tl3_store::Error::Database(error))) if connection_error(&error) => {
                // Drop the dead session before opening its replacement. Retry once.
                self.reader = None;
                self.reader = Some(Reader::open(&self.settings)?);
                status::load(self.reader.as_mut().expect("reopened reader"))
            }
            result => result,
        }
    }
}

fn connection_error(error: &tl3_store::DatabaseError) -> bool {
    error.is_closed()
        || error.code().is_none_or(|code| {
            code.code().starts_with("08") || matches!(code.code(), "57P01" | "57P02" | "57P03")
        })
}

fn handle(connection: &mut http::Connection, reader: &Mutex<StatusReader>) -> io::Result<()> {
    let (code, body, head) = match connection.read(0) {
        Err((code, head)) => (code, "Invalid HTTP request.\n".into(), head),
        Ok(request) => {
            let head = request.method == "HEAD";
            if !matches!(request.method.as_str(), "GET" | "HEAD") {
                (405, "Only GET and HEAD are allowed.\n".into(), head)
            } else if request.path.split('?').next() != Some("/") {
                (404, "Page not found.\n".into(), head)
            } else {
                let model = reader.lock().expect("status reader lock").load();
                match model {
                    Ok(model) => (200, status::render_html(&model), head),
                    // Neither database errors nor stored payloads belong in HTTP errors.
                    Err(error) => {
                        eprintln!("status unavailable: {error}");
                        (503, "Status is temporarily unavailable.\n".into(), head)
                    }
                }
            }
        }
    };
    let mut headers = String::from(
        "Content-Type: text/html; charset=utf-8\r\nContent-Security-Policy: default-src 'none'; style-src 'unsafe-inline'; base-uri 'none'; frame-ancestors 'none'\r\n",
    );
    if code == 405 {
        headers.push_str("Allow: GET, HEAD\r\n");
    }
    connection.respond(code, &body, head, &headers)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn serve_validates_even_programmatically_changed_configuration() {
        for address in ["127.0.0.1:0", "0.0.0.0:8033", "224.0.0.1:8033", "[::]:8033"] {
            let mut config =
                tl3_config::Config::parse(include_str!("../../config/example.toml")).unwrap();
            config.status.listen = address.parse().unwrap();
            assert!(matches!(serve(&config), Err(Error::Config(_))), "{address}");
        }
    }
}
