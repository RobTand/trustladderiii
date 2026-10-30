use crate::{Error, Result, status};
use std::{
    io::{self, Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};
use tl3_store::Reader;

const HANDLERS: usize = 4;
const MAX_CONNECTIONS: usize = 8;
const MAX_HEADERS: usize = 8 * 1024;
// Socket deadlines protect the page from stuck clients. They never decide
// whether TL3's work has ended, so the work wait principle does not apply.
const SOCKET_TIMEOUT: Duration = Duration::from_secs(2);

pub fn serve(config: &tl3_config::Config) -> Result<()> {
    config.validate()?;
    let reader = Arc::new(Mutex::new(StatusReader {
        settings: config.store.clone(),
        reader: Some(Reader::open(&config.store)?),
    }));
    let listener = TcpListener::bind(config.status.listen)?;
    let (send, receive) = mpsc::sync_channel::<Connection>(MAX_CONNECTIONS);
    let receive = Arc::new(Mutex::new(receive));
    for _ in 0..HANDLERS {
        let receive = Arc::clone(&receive);
        let reader = Arc::clone(&reader);
        thread::spawn(move || {
            loop {
                let next = receive.lock().expect("status receiver lock").recv();
                let Ok(mut connection) = next else { return };
                // A disconnected browser must not stop the status process.
                let _ = handle(&mut connection.stream, &reader);
            }
        });
    }
    println!("Status page listening on http://{}", listener.local_addr()?);
    std::io::stdout().flush()?;
    let open = Arc::new(AtomicUsize::new(0));
    for stream in listener.incoming() {
        let stream = stream?;
        if open
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
                (count < MAX_CONNECTIONS).then_some(count + 1)
            })
            .is_err()
        {
            // Refuse immediately; even an overload response could block accept.
            drop(stream);
            continue;
        }
        let connection = Connection {
            stream,
            open: Arc::clone(&open),
        };
        if send.send(connection).is_err() {
            return Err(Error::Invalid("status handlers stopped"));
        }
    }
    Ok(())
}

struct Connection {
    stream: TcpStream,
    open: Arc<AtomicUsize>,
}
impl Drop for Connection {
    fn drop(&mut self) {
        self.open.fetch_sub(1, Ordering::Relaxed);
    }
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

fn handle(stream: &mut TcpStream, reader: &Mutex<StatusReader>) -> io::Result<()> {
    stream.set_write_timeout(Some(SOCKET_TIMEOUT))?;
    let (code, body, head) = match read_request(stream) {
        Err((code, head)) => (code, "Invalid HTTP request.\n".into(), head),
        Ok(request) => {
            let head = request.method == "HEAD";
            if !matches!(request.method.as_str(), "GET" | "HEAD") {
                (405, "Only GET and HEAD are allowed.\n".into(), head)
            } else if request.path != "/" {
                (404, "Page not found.\n".into(), head)
            } else {
                let model = reader.lock().expect("status reader lock").load();
                match model {
                    Ok(model) => (200, status::render_html(&model), head),
                    // Neither database errors nor stored payloads belong in HTTP errors.
                    Err(_) => (503, "Status is temporarily unavailable.\n".into(), head),
                }
            }
        }
    };
    write_response(stream, code, &body, head)
}

struct Request {
    method: String,
    path: String,
}

fn read_request(stream: &mut TcpStream) -> std::result::Result<Request, (u16, bool)> {
    let started = Instant::now();
    let mut bytes = Vec::new();
    let mut head = false;
    loop {
        let remaining = SOCKET_TIMEOUT
            .checked_sub(started.elapsed())
            .ok_or((408, head))?;
        stream
            .set_read_timeout(Some(remaining.max(Duration::from_millis(1))))
            .map_err(|_| (400, head))?;
        let mut chunk = [0; 1024];
        let count = stream.read(&mut chunk).map_err(|error| {
            (
                if matches!(
                    error.kind(),
                    io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                ) {
                    408
                } else {
                    400
                },
                head,
            )
        })?;
        if count == 0 {
            return Err((400, head));
        }
        bytes.extend_from_slice(&chunk[..count]);
        head = bytes.starts_with(b"HEAD ");
        if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            if end + 4 > MAX_HEADERS {
                return Err((431, head));
            }
            // No body or pipelined request is consumed. Every response closes the socket.
            return parse_request(&bytes[..end]).map_err(|code| (code, head));
        }
        if bytes.len() >= MAX_HEADERS {
            return Err((431, head));
        }
    }
}

fn parse_request(bytes: &[u8]) -> std::result::Result<Request, u16> {
    let source = std::str::from_utf8(bytes).map_err(|_| 400_u16)?;
    let mut lines = source.split("\r\n");
    let parts: Vec<_> = lines.next().unwrap_or("").split(' ').collect();
    let [method, path, version] = parts.as_slice() else {
        return Err(400);
    };
    if method.is_empty()
        || !method.bytes().all(|b| b.is_ascii_uppercase())
        || !path.starts_with('/')
        || path.bytes().any(|b| b.is_ascii_control())
        || !matches!(*version, "HTTP/1.0" | "HTTP/1.1")
    {
        return Err(400);
    }
    for line in lines {
        let (name, value) = line.split_once(':').ok_or(400_u16)?;
        if name.is_empty()
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
            || value.bytes().any(|b| b.is_ascii_control() && b != b'\t')
        {
            return Err(400);
        }
        if name.eq_ignore_ascii_case("transfer-encoding")
            || (name.eq_ignore_ascii_case("content-length") && value.trim() != "0")
        {
            return Err(400);
        }
    }
    Ok(Request {
        method: (*method).into(),
        path: (*path).into(),
    })
}

fn write_response(stream: &mut TcpStream, code: u16, body: &str, head: bool) -> io::Result<()> {
    let reason = match code {
        200 => "OK",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        431 => "Request Header Fields Too Large",
        503 => "Service Unavailable",
        _ => "Bad Request",
    };
    write!(
        stream,
        "HTTP/1.1 {code} {reason}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nContent-Security-Policy: default-src 'none'; style-src 'unsafe-inline'; base-uri 'none'; frame-ancestors 'none'\r\n",
        body.len()
    )?;
    if code == 405 {
        write!(stream, "Allow: GET, HEAD\r\n")?;
    }
    write!(stream, "\r\n")?;
    if !head {
        stream.write_all(body.as_bytes())?;
    }
    Ok(())
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
