use std::{
    io::{BufRead, BufReader, Write},
    net::{TcpListener, TcpStream},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};
pub struct BellRequest {
    pub line: String,
    pub response: mpsc::SyncSender<serde_json::Value>,
}
impl BellRequest {
    pub fn reply(self, seq: u64, events: serde_json::Value) {
        self.response
            .send(serde_json::json!({
                "epoch": "0123456789abcdef0123456789abcdef", "seq": seq, "events": events
            }))
            .unwrap();
    }
}
pub struct Bell {
    pub url: String,
    address: std::net::SocketAddr,
    pub requests: Option<mpsc::Receiver<BellRequest>>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}
impl Bell {
    pub fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let (sender, requests) = mpsc::channel();
        let thread = thread::spawn(move || {
            let mut handlers = vec![];
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                if stopping.load(Ordering::Relaxed) {
                    break;
                }
                let sender = sender.clone();
                handlers.push(thread::spawn(move || {
                    stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
                    let mut reader = BufReader::new(&stream);
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 { return; }
                    loop {
                        let mut header = String::new();
                        if reader.read_line(&mut header).unwrap_or(0) == 0 { return; }
                        if header == "\r\n" { break; }
                    }
                    let (response, receiver) = mpsc::sync_channel(0);
                    if sender.send(BellRequest { line, response }).is_err() { return; }
                    let Ok(response) = receiver.recv() else { return; };
                    let body = response.to_string();
                    let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                }));
            }
            for handler in handlers {
                handler.join().unwrap();
            }
        });
        Self {
            url: format!("http://{address}/"),
            address,
            requests: Some(requests),
            stop,
            thread: Some(thread),
        }
    }
    pub fn request(&self) -> BellRequest {
        self.requests
            .as_ref()
            .unwrap()
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
    }
}
impl Drop for Bell {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.requests.take();
        let _ = TcpStream::connect(self.address);
        self.thread.take().unwrap().join().unwrap();
    }
}
