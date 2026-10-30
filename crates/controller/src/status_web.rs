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
    accept_connections(listener.incoming(), send, open)
}

fn accept_connections(
    incoming: impl Iterator<Item = io::Result<TcpStream>>,
    send: mpsc::SyncSender<Connection>,
    open: Arc<AtomicUsize>,
) -> Result<()> {
    for stream in incoming {
        let stream = match stream {
            Ok(stream) => stream,
            Err(error) => {
                eprintln!("status accept failed: {error}");
                // File-table exhaustion must not turn the accept loop into a busy loop.
                if matches!(error.raw_os_error(), Some(23 | 24)) {
                    thread::sleep(Duration::from_millis(100));
                }
                continue;
            }
        };
        if open
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
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
    let (code, body, head) = match read_request(stream) {
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
    let started = Instant::now();
    let mut headers = format!(
        "HTTP/1.1 {code} {reason}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nContent-Security-Policy: default-src 'none'; style-src 'unsafe-inline'; base-uri 'none'; frame-ancestors 'none'\r\n",
        body.len()
    );
    if code == 405 {
        headers.push_str("Allow: GET, HEAD\r\n");
    }
    headers.push_str("\r\n");
    for mut bytes in [headers.as_bytes(), if head { b"" } else { body.as_bytes() }] {
        while !bytes.is_empty() {
            let remaining = SOCKET_TIMEOUT
                .checked_sub(started.elapsed())
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::TimedOut, "status response deadline")
                })?;
            stream.set_write_timeout(Some(remaining.max(Duration::from_millis(1))))?;
            match stream.write(&bytes[..bytes.len().min(4096)]) {
                Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
                Ok(count) => bytes = &bytes[count..],
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accept_errors_do_not_stop_later_connections() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let _client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (stream, _) = listener.accept().unwrap();
        let (send, receive) = mpsc::sync_channel(1);
        let open = Arc::new(AtomicUsize::new(0));
        let incoming = [
            Err(io::ErrorKind::ConnectionAborted.into()),
            Err(io::Error::from_raw_os_error(24)),
            Err(io::Error::from_raw_os_error(23)),
            Ok(stream),
        ];
        let started = Instant::now();
        accept_connections(incoming.into_iter(), send, Arc::clone(&open)).unwrap();
        assert!(started.elapsed() >= Duration::from_millis(200));
        let connection = receive.recv().unwrap();
        assert_eq!(open.load(Ordering::Relaxed), 1);
        drop(connection);
        assert_eq!(open.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn trickle_reader_cannot_extend_total_response_deadline() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let socket = socket2::Socket::new(
            socket2::Domain::IPV4,
            socket2::Type::STREAM,
            Some(socket2::Protocol::TCP),
        )
        .unwrap();
        socket.set_recv_buffer_size(1024).unwrap();
        socket
            .connect(&listener.local_addr().unwrap().into())
            .unwrap();
        let mut client: TcpStream = socket.into();
        client
            .set_read_timeout(Some(Duration::from_millis(200)))
            .unwrap();
        let (mut stream, _) = listener.accept().unwrap();
        socket2::SockRef::from(&stream)
            .set_send_buffer_size(1024)
            .unwrap();
        let (send, receive) = mpsc::channel();
        let writer = thread::spawn(move || {
            let started = Instant::now();
            let result = write_response(&mut stream, 200, &"x".repeat(256 * 1024), false);
            send.send((result, started.elapsed())).unwrap();
        });
        let started = Instant::now();
        let mut reads = 0;
        let (result, elapsed) = loop {
            if let Ok(ending) = receive.try_recv() {
                break ending;
            }
            assert!(
                started.elapsed() < SOCKET_TIMEOUT + Duration::from_secs(2),
                "trickle reads extended the response deadline"
            );
            let mut bytes = [0; 1024];
            match client.read(&mut bytes) {
                Ok(count) if count > 0 => reads += 1,
                Ok(_) => {}
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock
                            | io::ErrorKind::TimedOut
                            | io::ErrorKind::ConnectionReset
                    ) => {}
                Err(error) => panic!("trickle read: {error}"),
            }
            thread::sleep(Duration::from_millis(50));
        };
        assert!(reads > 2, "the client must make progress during the write");
        assert!(elapsed >= SOCKET_TIMEOUT);
        assert!(matches!(
            result.unwrap_err().kind(),
            io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
        ));
        drop(client);
        writer.join().unwrap();
    }

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
