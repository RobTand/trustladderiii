//! Public evidence uses the worker's #302 Redactor, packaged with this release.
use std::{
    io::{self, BufRead, BufReader, BufWriter, Write},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::Mutex,
};

include!(concat!(env!("OUT_DIR"), "/worker_redactor.rs"));

const BRIDGE: &str = r#"
import json
import sys

scope = {'__name__': 'tl3_redaction'}
exec(compile(json.loads(sys.stdin.readline()), 'tl3_workers/tracing.py', 'exec'), scope)
redactor = scope['Redactor'](None)
for line in sys.stdin:
    text = json.loads(line)
    print(json.dumps(redactor.text(text, limit=len(text))), flush=True)
    del text, line
"#;

struct Redactor {
    child: Child,
    input: BufWriter<ChildStdin>,
    output: BufReader<ChildStdout>,
}

impl Redactor {
    fn start() -> io::Result<Self> {
        // The shared implementation needs only stdlib. Neither inherited keys
        // nor Python paths from the controller belong in this child.
        let mut child = Command::new("python3")
            .env_clear()
            .env("PATH", "/usr/local/bin:/usr/bin:/bin")
            .current_dir("/")
            .args(["-I", "-u", "-c", BRIDGE])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let input = BufWriter::new(child.stdin.take().expect("piped stdin"));
        let output = BufReader::new(child.stdout.take().expect("piped stdout"));
        let mut redactor = Self {
            child,
            input,
            output,
        };
        redactor.send(REDACTOR_SOURCE)?;
        Ok(redactor)
    }

    fn send(&mut self, text: &str) -> io::Result<()> {
        serde_json::to_writer(&mut self.input, text)?;
        self.input.write_all(b"\n")?;
        self.input.flush()
    }

    fn text(&mut self, text: &str) -> io::Result<String> {
        self.send(text)?;
        let mut line = String::new();
        self.output.read_line(&mut line)?;
        Ok(serde_json::from_str(&line)?)
    }
}

impl Drop for Redactor {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub(crate) fn text(text: &str) -> String {
    // One interpreter per controller, with no retained request text or cache.
    static REDACTOR: Mutex<Option<Redactor>> = Mutex::new(None);
    let Ok(mut process) = REDACTOR.lock() else {
        return "[REDACTED]".into();
    };
    if process.is_none() {
        *process = Redactor::start().ok();
    }
    if let Some(redactor) = process.as_mut()
        && let Ok(redacted) = redactor.text(&text.replace('\0', ""))
    {
        return redacted;
    }
    // A failed helper must never fall back to publishing the original text.
    *process = None;
    "[REDACTED]".into()
}
