#[path = "../../store/tests/support/mod.rs"]
mod support;

use serde_json::Value;
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use support::Database;
use tl3_controller::{Bootstrap, Controller, LabelledIssue, Repository, adapters, board};
use tl3_store::{Reader, Store};

const ROLE: &str = "tl3_console_reader";

fn main() {
    let mut database = Database::start();
    let settings = database.create("console");
    let records = tempfile::tempdir().unwrap();
    let mut controller = open(&settings, records.path());
    controller.intake(issue(42)).unwrap();
    let reader = read_only_role(&settings);
    let assets = tempfile::tempdir().unwrap();
    std::fs::write(
        assets.path().join("index.html"),
        "<!doctype html><title>TL3 console</title>",
    )
    .unwrap();
    let console = tl3_console::start(
        TcpListener::bind("127.0.0.1:0").unwrap(),
        tl3_console::Options {
            store: reader.clone(),
            assets: assets.path().into(),
            models: None,
        },
    )
    .unwrap();
    let address = console.address();

    commit_reaches_open_stream(address, &settings, &mut controller);
    println!("test commit_reaches_open_stream ... ok");
    last_event_id_gets_a_fresh_consistent_snapshot(address, &settings);
    println!("test last_event_id_gets_a_fresh_consistent_snapshot ... ok");
    serves_board_and_web_app(address, &settings);
    println!("test serves_board_and_web_app ... ok");
    console_reads_through_a_role_that_cannot_write(&settings);
    println!("test console_reads_through_a_role_that_cannot_write ... ok");
    drop(controller);
    streams_again_after_postgres_restarts(address, &mut database, &settings, records.path());
    println!("test streams_again_after_postgres_restarts ... ok");
    database.stop();
    println!("5 console scenarios passed; no skips");
}

fn bootstrap() -> Bootstrap {
    Bootstrap {
        operations: None,
        repos: vec![Repository {
            owner: "RobTand".into(),
            name: "tl3-sandbox".into(),
            installation: 1,
        }],
        rob: 24923402,
        slots: 1,
        pool: 1,
    }
}

fn issue(number: u64) -> LabelledIssue {
    LabelledIssue {
        delivery: format!("console-delivery-{number}"),
        owner: "RobTand".into(),
        repo: "tl3-sandbox".into(),
        installation: 1,
        number,
        title: format!("Console fixture {number}"),
        actor: 24923402,
        label: "tl3".into(),
    }
}

fn open(settings: &tl3_config::Store, records: &Path) -> Controller {
    Controller::open(
        Store::open(settings).unwrap(),
        adapters::simulation(records).unwrap(),
        bootstrap(),
    )
    .unwrap()
}

fn raw(settings: &tl3_config::Store) -> postgres::Client {
    postgres::Config::new()
        .host(&settings.host)
        .port(settings.port)
        .user(&settings.user)
        .dbname(&settings.database)
        .password("tl3-disposable-test-password")
        .connect(postgres::NoTls)
        .unwrap()
}

/// The console runs as a role that may only read, so any write would fail.
fn read_only_role(settings: &tl3_config::Store) -> tl3_config::Store {
    let mut client = raw(settings);
    client
        .batch_execute(&format!(
            "CREATE ROLE {ROLE} LOGIN PASSWORD 'tl3-disposable-test-password';
             GRANT SELECT ON ALL TABLES IN SCHEMA public TO {ROLE};"
        ))
        .unwrap();
    tl3_config::Store {
        user: ROLE.into(),
        ..settings.clone()
    }
}

fn revision(settings: &tl3_config::Store) -> i64 {
    Reader::open(settings)
        .unwrap()
        .load()
        .unwrap()
        .revision
        .get()
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

fn card_column(cards: &Value, issue: u64) -> Option<String> {
    cards
        .as_array()
        .unwrap()
        .iter()
        .find(|card| card["issue"] == issue)
        .map(|card| card["column"].as_str().unwrap().to_string())
}

fn commit_reaches_open_stream(
    address: SocketAddr,
    settings: &tl3_config::Store,
    controller: &mut Controller,
) {
    let mut stream = EventStream::open(address, None);
    let first = stream.board_event();
    assert_eq!(first.name, "snapshot");
    assert_eq!(first.data["revision"], revision(settings));
    assert_eq!(
        card_column(&first.data["cards"], 42).as_deref(),
        Some("Intake")
    );
    let mut column = "Intake".to_string();
    let mut seen = first.data["revision"].as_i64().unwrap();
    for _ in 0..100 {
        if column == "Making" {
            break;
        }
        assert!(controller.drive_once().unwrap());
        let committed = revision(settings);
        while seen < committed {
            let event = stream.board_event();
            assert_eq!(event.name, "patch", "{event:?}");
            assert!(
                event
                    .id
                    .starts_with(&format!("{}.", event.data["revision"]))
            );
            seen = event.data["revision"].as_i64().unwrap();
            if let Some(moved) = card_column(&event.data["upserts"], 42) {
                column = moved;
            }
        }
    }
    assert_eq!(column, "Making", "the card moved live without a reload");
}

fn last_event_id_gets_a_fresh_consistent_snapshot(
    address: SocketAddr,
    settings: &tl3_config::Store,
) {
    let mut stream = EventStream::open(address, Some("1.1"));
    let event = stream.board_event();
    assert_eq!(event.name, "snapshot");
    let mut reader = Reader::open(settings).unwrap();
    let mut tx = reader.transaction().unwrap();
    let expected = board::read(&mut tx, now_ms(), None).unwrap();
    tx.commit().unwrap();
    assert_eq!(event.data["revision"], expected.revision);
    assert_eq!(
        event.data["cards"],
        serde_json::to_value(&expected.cards).unwrap()
    );
    assert_eq!(
        event.data["flow"]["columns"],
        serde_json::to_value(&expected.flow.columns).unwrap()
    );
}

fn serves_board_and_web_app(address: SocketAddr, settings: &tl3_config::Store) {
    let (status, headers, body) = get(address, "/api/board");
    assert_eq!(status, 200);
    assert!(headers.contains("content-type: application/json"));
    let board: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(board["revision"], revision(settings));
    assert_eq!(card_column(&board["cards"], 42).as_deref(), Some("Making"));
    for path in ["/", "/board/RobTand/tl3-sandbox"] {
        let (status, headers, body) = get(address, path);
        assert_eq!(status, 200, "{path}");
        assert!(headers.contains("content-type: text/html"), "{path}");
        assert!(headers.contains("content-security-policy: default-src 'self'"));
        assert_eq!(body, b"<!doctype html><title>TL3 console</title>", "{path}");
    }
    assert_eq!(get(address, "/api/unknown").0, 404);
}

fn console_reads_through_a_role_that_cannot_write(settings: &tl3_config::Store) {
    let mut client = raw(settings);
    for table in ["tl3_world", "tl3_events", "tl3_meta", "tl3_node_phases"] {
        for privilege in ["INSERT", "UPDATE", "DELETE", "TRUNCATE"] {
            assert!(
                !client
                    .query_one(
                        "SELECT has_table_privilege($1, $2, $3)",
                        &[&ROLE, &table, &privilege],
                    )
                    .unwrap()
                    .get::<_, bool>(0),
                "{ROLE} must not {privilege} {table}"
            );
        }
    }
    let sessions: i64 = client
        .query_one(
            "SELECT count(*) FROM pg_stat_activity WHERE usename = $1",
            &[&ROLE],
        )
        .unwrap()
        .get(0);
    assert!(
        sessions >= 1,
        "the console's sessions use the read-only role"
    );
}

fn streams_again_after_postgres_restarts(
    address: SocketAddr,
    database: &mut Database,
    settings: &tl3_config::Store,
    records: &Path,
) {
    let mut stream = EventStream::open(address, None);
    assert_eq!(stream.board_event().name, "snapshot");
    database.restart();
    let mut controller = open(settings, records);
    controller.intake(issue(43)).unwrap();
    let committed = revision(settings);
    loop {
        let event = stream.board_event();
        let cards = if event.name == "snapshot" {
            &event.data["cards"]
        } else {
            &event.data["upserts"]
        };
        if card_column(cards, 43).is_some() {
            assert_eq!(event.data["revision"], committed);
            break;
        }
    }
    let status = stream
        .last_database
        .expect("the stream reports database state");
    assert_eq!(status["connected"], true);
}

#[derive(Debug)]
struct Event {
    name: String,
    id: String,
    data: Value,
}

/// A browser's view of `/api/stream`: Server-Sent Events over chunked HTTP/1.1.
struct EventStream {
    body: ChunkedBody,
    last_database: Option<Value>,
}

impl EventStream {
    fn open(address: SocketAddr, last_event_id: Option<&str>) -> Self {
        let mut socket = TcpStream::connect(address).unwrap();
        // A hang guard only; events arrive as soon as Postgres commits.
        socket
            .set_read_timeout(Some(Duration::from_secs(60)))
            .unwrap();
        let resume = last_event_id
            .map(|id| format!("Last-Event-ID: {id}\r\n"))
            .unwrap_or_default();
        write!(
            socket,
            "GET /api/stream HTTP/1.1\r\nHost: console\r\nAccept: text/event-stream\r\n{resume}\r\n"
        )
        .unwrap();
        let mut reader = BufReader::new(socket);
        let (status, headers) = response_head(&mut reader);
        assert_eq!(status, 200);
        assert!(headers.contains("content-type: text/event-stream"));
        assert!(headers.contains("transfer-encoding: chunked"));
        Self {
            body: ChunkedBody::new(reader),
            last_database: None,
        }
    }

    fn event(&mut self) -> Event {
        let (mut name, mut id, mut data) = (String::new(), String::new(), String::new());
        loop {
            let line = self.body.line();
            if line.is_empty() {
                if !data.is_empty() {
                    return Event {
                        name,
                        id,
                        data: serde_json::from_str(&data).unwrap(),
                    };
                }
                continue;
            }
            if let Some(value) = line.strip_prefix("event:") {
                name = value.trim().into();
            } else if let Some(value) = line.strip_prefix("id:") {
                id = value.trim().into();
            } else if let Some(value) = line.strip_prefix("data:") {
                data.push_str(value.trim_start());
            }
        }
    }

    /// The next board snapshot or patch, noting database events on the way.
    fn board_event(&mut self) -> Event {
        loop {
            let event = self.event();
            if event.name == "database" {
                self.last_database = Some(event.data);
                continue;
            }
            return event;
        }
    }
}

fn response_head(reader: &mut BufReader<TcpStream>) -> (u16, String) {
    let mut status = String::new();
    reader.read_line(&mut status).unwrap();
    let code = status.split_whitespace().nth(1).unwrap().parse().unwrap();
    let mut headers = String::new();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        if line == "\r\n" {
            return (code, headers.to_ascii_lowercase());
        }
        headers.push_str(&line);
    }
}

struct ChunkedBody {
    reader: BufReader<TcpStream>,
    pending: Vec<u8>,
}

impl ChunkedBody {
    fn new(reader: BufReader<TcpStream>) -> Self {
        Self {
            reader,
            pending: Vec::new(),
        }
    }

    /// Next chunk's bytes, or None at the end of the body.
    fn chunk(&mut self) -> Option<Vec<u8>> {
        let mut size = String::new();
        self.reader.read_line(&mut size).unwrap();
        let size = usize::from_str_radix(size.trim(), 16).unwrap();
        let mut data = vec![0; size + 2];
        self.reader.read_exact(&mut data).unwrap();
        assert_eq!(&data[size..], b"\r\n");
        data.truncate(size);
        (size > 0).then_some(data)
    }

    fn line(&mut self) -> String {
        loop {
            if let Some(end) = self.pending.iter().position(|byte| *byte == b'\n') {
                let line: Vec<u8> = self.pending.drain(..=end).collect();
                return String::from_utf8(line)
                    .unwrap()
                    .trim_end_matches(['\r', '\n'])
                    .into();
            }
            let chunk = self.chunk().expect("the event stream stays open");
            self.pending.extend(chunk);
        }
    }

    fn rest(&mut self) -> Vec<u8> {
        while let Some(chunk) = self.chunk() {
            self.pending.extend(chunk);
        }
        std::mem::take(&mut self.pending)
    }
}

fn get(address: SocketAddr, path: &str) -> (u16, String, Vec<u8>) {
    let mut socket = TcpStream::connect(address).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(60)))
        .unwrap();
    write!(
        socket,
        "GET {path} HTTP/1.1\r\nHost: console\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut reader = BufReader::new(socket);
    let (status, headers) = response_head(&mut reader);
    let body = if headers.contains("transfer-encoding: chunked") {
        ChunkedBody::new(reader).rest()
    } else {
        let mut body = Vec::new();
        reader.read_to_end(&mut body).unwrap();
        body
    };
    (status, headers, body)
}
