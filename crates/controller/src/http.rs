use crate::{Error, Result};
use std::{
    io::{self, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

const HANDLERS: usize = 4;
const MAX_CONNECTIONS: usize = 8;
const MAX_HEADERS: usize = 8 * 1024;
const SEND_BUFFER: usize = 32 * 1024;
pub(crate) const MAX_BODY: usize = 25 * 1024 * 1024;
// Socket deadlines bound untrusted clients, never the duration of TL3's work.
pub(crate) const SOCKET_TIMEOUT: Duration = Duration::from_secs(2);

pub(crate) struct Server {
    listener: TcpListener,
    control: Control,
}
#[derive(Clone)]
pub(crate) struct Control {
    address: SocketAddr,
    stopped: Arc<AtomicBool>,
}
impl Control {
    pub(crate) fn stop(&self) {
        self.stop_with(|| {
            // Wake blocking accept without polling when an owner shuts the server down.
            let _ = TcpStream::connect_timeout(&self.address, Duration::from_millis(100));
        });
    }
    fn stop_with(&self, wake: impl FnOnce()) {
        self.stopped.store(true, Ordering::Release);
        wake();
    }
}
impl Server {
    pub(crate) fn bind(address: SocketAddr) -> io::Result<Self> {
        let listener = TcpListener::bind(address)?;
        Ok(Self {
            control: Control {
                address: listener.local_addr()?,
                stopped: Arc::new(AtomicBool::new(false)),
            },
            listener,
        })
    }
    pub(crate) fn address(&self) -> SocketAddr {
        self.control.address
    }
    pub(crate) fn control(&self) -> Control {
        self.control.clone()
    }
    pub(crate) fn serve(
        self,
        handler: impl Fn(&mut Connection) + Send + Sync + 'static,
    ) -> Result<()> {
        let (send, receive) = mpsc::sync_channel::<Connection>(MAX_CONNECTIONS);
        let receive = Arc::new(Mutex::new(receive));
        let handler = Arc::new(handler);
        let workers: Vec<_> = (0..HANDLERS)
            .map(|_| {
                let receive = Arc::clone(&receive);
                let handler = Arc::clone(&handler);
                thread::spawn(move || {
                    loop {
                        let next = receive.lock().expect("HTTP receiver lock").recv();
                        let Ok(mut connection) = next else { return };
                        handler(&mut connection);
                    }
                })
            })
            .collect();
        let result = accept_connections(
            self.listener.incoming(),
            send,
            Arc::new(AtomicUsize::new(0)),
            &self.control.stopped,
        );
        for worker in workers {
            worker
                .join()
                .map_err(|_| Error::Invalid("HTTP handler panicked"))?;
        }
        result
    }
}

fn accept_connections(
    incoming: impl Iterator<Item = io::Result<TcpStream>>,
    send: mpsc::SyncSender<Connection>,
    open: Arc<AtomicUsize>,
    stopped: &AtomicBool,
) -> Result<()> {
    for stream in incoming {
        if stopped.load(Ordering::Acquire) {
            break;
        }
        let stream = match stream {
            Ok(stream) => stream,
            Err(error) => {
                eprintln!("HTTP accept failed: {error}");
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
            drop(stream);
            continue;
        }
        let connection = Connection {
            stream,
            open: Arc::clone(&open),
        };
        if send.send(connection).is_err() {
            return Err(Error::Invalid("HTTP handlers stopped"));
        }
    }
    Ok(())
}

pub(crate) struct Connection {
    stream: TcpStream,
    open: Arc<AtomicUsize>,
}
impl Connection {
    pub(crate) fn read(&mut self, max_body: usize) -> std::result::Result<Request, (u16, bool)> {
        read_request(&mut self.stream, max_body)
    }
    pub(crate) fn respond(
        &mut self,
        code: u16,
        body: &str,
        head: bool,
        headers: &str,
    ) -> io::Result<()> {
        socket2::SockRef::from(&self.stream).set_send_buffer_size(SEND_BUFFER)?;
        write_response(&mut self.stream, code, body, head, headers)
    }
}
impl Drop for Connection {
    fn drop(&mut self) {
        self.open.fetch_sub(1, Ordering::Relaxed);
    }
}

pub(crate) struct Request {
    pub(crate) method: String,
    pub(crate) path: String,
    headers: Vec<(String, String)>,
    pub(crate) body: Vec<u8>,
}
impl Request {
    #[cfg(all(test, loom))]
    pub(crate) fn for_model(body: Vec<u8>, headers: Vec<(String, String)>) -> Self {
        Self {
            method: "POST".into(),
            path: "/github/events".into(),
            body,
            headers,
        }
    }

    pub(crate) fn header(&self, name: &str) -> &str {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map_or("", |(_, value)| value)
    }
}

fn read_request(
    stream: &mut TcpStream,
    max_body: usize,
) -> std::result::Result<Request, (u16, bool)> {
    let headers_deadline = Instant::now() + SOCKET_TIMEOUT;
    let mut bytes = Vec::new();
    let mut head = false;
    let end = loop {
        let mut chunk = [0; 1024];
        let count =
            read_before(stream, &mut chunk, headers_deadline).map_err(|code| (code, head))?;
        if count == 0 {
            return Err((400, head));
        }
        bytes.extend_from_slice(&chunk[..count]);
        head = bytes.starts_with(b"HEAD ");
        if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            if end + 4 > MAX_HEADERS {
                return Err((431, head));
            }
            break end;
        }
        if bytes.len() >= MAX_HEADERS {
            return Err((431, head));
        }
    };
    if Instant::now() >= headers_deadline {
        return Err((408, head));
    }
    let (mut request, length) = parse_request(&bytes[..end]).map_err(|code| (code, head))?;
    if max_body > 0 && request.method == "POST" && length.is_none() {
        return Err((411, head));
    }
    let length = length.unwrap_or(0);
    if length > max_body.min(MAX_BODY) {
        return Err((if max_body == 0 { 400 } else { 413 }, head));
    }
    let body_deadline = Instant::now() + body_timeout(length);
    request
        .body
        .extend_from_slice(&bytes[end + 4..][..length.min(bytes.len() - end - 4)]);
    while request.body.len() < length {
        let mut chunk = [0; 4096];
        let limit = chunk.len().min(length - request.body.len());
        let count =
            read_before(stream, &mut chunk[..limit], body_deadline).map_err(|code| (code, head))?;
        if count == 0 {
            return Err((400, head));
        }
        request.body.extend_from_slice(&chunk[..count]);
    }
    if Instant::now() >= body_deadline {
        return Err((408, head));
    }
    Ok(request)
}

// The webhook socket is loopback-only behind cloudflared. Allow at least 1 MiB/s,
// capped at GitHub's 25 MiB payload limit, with a two-second minimum for small bodies.
fn body_timeout(length: usize) -> Duration {
    SOCKET_TIMEOUT.max(Duration::from_secs(
        length.min(MAX_BODY).div_ceil(1024 * 1024) as u64,
    ))
}

fn read_before(
    stream: &mut TcpStream,
    bytes: &mut [u8],
    deadline: Instant,
) -> std::result::Result<usize, u16> {
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or(408_u16)?;
        stream
            .set_read_timeout(Some(remaining.max(Duration::from_millis(1))))
            .map_err(|_| 400_u16)?;
        match stream.read(bytes) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            result => {
                return result.map_err(|error| {
                    if matches!(
                        error.kind(),
                        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                    ) {
                        408
                    } else {
                        400
                    }
                });
            }
        }
    }
}

fn parse_request(bytes: &[u8]) -> std::result::Result<(Request, Option<usize>), u16> {
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
    let mut headers: Vec<(String, String)> = Vec::new();
    let mut length = None;
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
        if name.eq_ignore_ascii_case("transfer-encoding") {
            return Err(400);
        }
        let value = value.trim_matches([' ', '\t']);
        if name.eq_ignore_ascii_case("content-length") {
            if length.is_some()
                || value.is_empty()
                || !value.bytes().all(|b| b.is_ascii_digit())
                || (value.len() > 1 && value.starts_with('0'))
            {
                return Err(400);
            }
            length = Some(value.parse::<usize>().map_err(|_| 413_u16)?);
        }
        headers.push((name.into(), value.into()));
    }
    Ok((
        Request {
            method: (*method).into(),
            path: (*path).into(),
            headers,
            body: Vec::new(),
        },
        length,
    ))
}

fn write_response(
    stream: &mut TcpStream,
    code: u16,
    body: &str,
    head: bool,
    extra_headers: &str,
) -> io::Result<()> {
    let reason = match code {
        200 => "OK",
        202 => "Accepted",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        411 => "Length Required",
        413 => "Content Too Large",
        431 => "Request Header Fields Too Large",
        503 => "Service Unavailable",
        _ => "Bad Request",
    };
    let started = Instant::now();
    let headers = format!(
        "HTTP/1.1 {code} {reason}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\n{extra_headers}\r\n",
        body.len()
    );
    for mut bytes in [headers.as_bytes(), if head { b"" } else { body.as_bytes() }] {
        while !bytes.is_empty() {
            let remaining = SOCKET_TIMEOUT
                .checked_sub(started.elapsed())
                .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "HTTP response deadline"))?;
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
    fn body_deadline_scales_with_declared_length_and_is_capped() {
        assert_eq!(body_timeout(0), SOCKET_TIMEOUT);
        assert_eq!(body_timeout(100), SOCKET_TIMEOUT);
        assert_eq!(body_timeout(2 * 1024 * 1024 + 1), Duration::from_secs(3));
        assert_eq!(body_timeout(MAX_BODY), Duration::from_secs(25));
        assert_eq!(body_timeout(usize::MAX), Duration::from_secs(25));
    }

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
        accept_connections(
            incoming.into_iter(),
            send,
            Arc::clone(&open),
            &AtomicBool::new(false),
        )
        .unwrap();
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
            let result = write_response(&mut stream, 200, &"x".repeat(256 * 1024), false, "");
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
}

#[cfg(all(test, loom))]
#[path = "../tests/loom/http.rs"]
mod loom_tests;
