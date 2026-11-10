use std::{
    cell::RefCell,
    collections::VecDeque,
    io::{self, Read},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Call {
    Open(std::path::PathBuf),
    Limit(u64),
    Read(usize),
}
#[derive(Clone, Debug)]
pub(super) enum Step {
    Bytes(Vec<u8>),
    Fail,
}
struct Recording {
    opens: VecDeque<bool>,
    reads: VecDeque<Step>,
    calls: Vec<Call>,
}
thread_local! { static STATE: RefCell<Option<Recording>> = const { RefCell::new(None) }; }
pub(super) fn run<T>(
    opens: Vec<bool>,
    reads: Vec<Step>,
    action: impl FnOnce() -> T,
) -> (T, Vec<Call>) {
    struct Restore(Option<Recording>);
    impl Drop for Restore {
        fn drop(&mut self) {
            STATE.with(|state| *state.borrow_mut() = self.0.take());
        }
    }
    let previous = STATE.with(|state| {
        state.replace(Some(Recording {
            opens: opens.into(),
            reads: reads.into(),
            calls: vec![],
        }))
    });
    let _restore = Restore(previous);
    let value = action();
    let calls = STATE.with(|state| state.borrow().as_ref().unwrap().calls.clone());
    (value, calls)
}
pub(crate) struct File(Option<std::fs::File>);
impl File {
    pub(crate) fn open(path: &std::path::Path) -> io::Result<Self> {
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            let Some(state) = state.as_mut() else {
                return std::fs::File::open(path).map(|file| Self(Some(file)));
            };
            state.calls.push(Call::Open(path.to_path_buf()));
            if state.opens.pop_front().unwrap_or(false) {
                return Err(io::Error::other("recorded open failure"));
            }
            Ok(Self(None))
        })
    }
    pub(crate) fn take(self, limit: u64) -> io::Take<Self> {
        if self.0.is_none() {
            STATE.with(|state| {
                state
                    .borrow_mut()
                    .as_mut()
                    .unwrap()
                    .calls
                    .push(Call::Limit(limit))
            });
        }
        Read::take(self, limit)
    }
}
impl Read for File {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if let Some(file) = self.0.as_mut() {
            return file.read(bytes);
        }
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            let state = state.as_mut().unwrap();
            state.calls.push(Call::Read(bytes.len()));
            match state.reads.pop_front() {
                Some(Step::Fail) => Err(io::Error::other("recorded read failure")),
                Some(Step::Bytes(chunk)) => {
                    let count = bytes.len().min(chunk.len());
                    bytes[..count].copy_from_slice(&chunk[..count]);
                    if count < chunk.len() {
                        state.reads.push_front(Step::Bytes(chunk[count..].to_vec()));
                    }
                    Ok(count)
                }
                None => Ok(0),
            }
        })
    }
}
