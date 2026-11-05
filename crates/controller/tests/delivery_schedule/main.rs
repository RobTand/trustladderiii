//! CC-E / state-table Effects: the verbatim record shell and production shell
//! receive the same changing reads and crash after every durable write.
#![allow(dead_code)]

use proptest::prelude::*;
use proptest::test_runner::{Config, FileFailurePersistence, RngSeed, TestRunner};
use serde_json::{Value, json};
use std::{cell::RefCell, collections::BTreeMap, io, path::PathBuf};
pub use tl3_controller::{Error, Result, executor};

mod reference {
    use crate::recorded_std as std;
    include!("base-record.rs.txt");
}
mod current {
    use crate::recorded_std as std;
    include!("../../src/adapters/record.rs");
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Call {
    Read(PathBuf, std::result::Result<Vec<u8>, io::ErrorKind>),
    Exists(PathBuf, bool),
    Create(PathBuf),
    Write(PathBuf, Vec<u8>),
    Sync(PathBuf),
    Rename(PathBuf, PathBuf),
    Open(PathBuf),
}
#[derive(Clone, Debug, Default)]
struct Io {
    samples: Vec<u8>,
    cursor: usize,
    calls: Vec<Call>,
    files: BTreeMap<PathBuf, Vec<u8>>,
    writes: usize,
    crash_at: usize,
    crashed: bool,
}
thread_local! {
    static IO: RefCell<Io> = RefCell::new(Io::default());
}
impl Io {
    fn sample(&mut self) -> u8 {
        let value = self.samples.get(self.cursor).copied().unwrap_or(0);
        self.cursor += 1;
        value
    }
    fn running(&self) -> io::Result<()> {
        if self.crashed {
            Err(io::Error::other("scheduled crash"))
        } else {
            Ok(())
        }
    }
    fn write(&mut self, call: Call, mutation: impl FnOnce(&mut Self)) -> io::Result<()> {
        self.running()?;
        self.calls.push(call);
        mutation(self);
        self.writes += 1;
        if self.crash_at == self.writes {
            self.crashed = true;
        }
        self.running()
    }
}

mod recorded_std {
    pub use ::std::{cmp, collections, io};
    pub mod path {
        pub type Path = crate::ScriptPath;
        pub type PathBuf = crate::ScriptPath;
    }
    pub mod fs {
        use crate::{Call, IO, json};
        use ::std::{
            io,
            path::{Path, PathBuf},
        };
        pub use std::fs::read_dir;
        pub fn read(path: impl AsRef<Path>) -> io::Result<Vec<u8>> {
            IO.with_borrow_mut(|io| {
                io.running()?;
                let path = path.as_ref().to_owned();
                let sample = io.sample();
                let result = match sample % 4 {
                    0 => io.files.get(&path).cloned().ok_or(io::ErrorKind::NotFound),
                    1 => Err(io::ErrorKind::NotFound),
                    2 if path.ends_with("epoch.fence") => {
                        Ok((i64::from(sample) - 128).to_string().into_bytes())
                    }
                    2 => Ok(serde_json::to_vec(&json!({
                        "key":"recorded-key", "deliveries":sample, "epochs":[1,2],
                        "completion":{"ack":"Confirmed", "failure_class":null,
                            "progress":null, "observations":[]}, "note":"previous"
                    }))
                    .unwrap()),
                    _ => Err(io::ErrorKind::PermissionDenied),
                };
                io.calls.push(Call::Read(path, result.clone()));
                result.map_err(io::Error::from)
            })
        }
        pub fn rename(from: impl AsRef<Path>, to: impl AsRef<Path>) -> io::Result<()> {
            let (from, to) = (from.as_ref().to_owned(), to.as_ref().to_owned());
            IO.with_borrow_mut(|io| {
                io.write(Call::Rename(from.clone(), to.clone()), |io| {
                    let bytes = io.files.remove(&from).unwrap();
                    io.files.insert(to, bytes);
                })
            })
        }
        pub struct File(PathBuf);
        impl File {
            pub fn create(path: impl AsRef<Path>) -> io::Result<Self> {
                let path = path.as_ref().to_owned();
                IO.with_borrow_mut(|io| {
                    io.write(Call::Create(path.clone()), |io| {
                        io.files.insert(path.clone(), Vec::new());
                    })
                })?;
                Ok(Self(path))
            }
            pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
                let path = path.as_ref().to_owned();
                IO.with_borrow_mut(|io| {
                    io.running()?;
                    io.calls.push(Call::Open(path.clone()));
                    Ok::<(), io::Error>(())
                })?;
                Ok(Self(path))
            }
            pub fn sync_all(&self) -> io::Result<()> {
                IO.with_borrow_mut(|io| io.write(Call::Sync(self.0.clone()), |_| {}))
            }
        }
        impl io::Write for File {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                IO.with_borrow_mut(|io| {
                    io.write(Call::Write(self.0.clone(), bytes.to_vec()), |io| {
                        io.files.get_mut(&self.0).unwrap().extend_from_slice(bytes);
                    })
                })?;
                Ok(bytes.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
    }
}

// Redirect the path type as well as fs so the inherent exists sampling point
// remains observable without changing either included record implementation.
#[derive(Clone)]
pub struct ScriptPath(PathBuf);
impl AsRef<std::path::Path> for ScriptPath {
    fn as_ref(&self) -> &std::path::Path {
        self.0.as_ref()
    }
}
impl ScriptPath {
    fn join(&self, path: impl AsRef<std::path::Path>) -> Self {
        Self(self.0.join(path))
    }
    fn with_extension(&self, extension: impl AsRef<std::ffi::OsStr>) -> Self {
        Self(self.0.with_extension(extension))
    }
    fn exists(&self) -> bool {
        IO.with_borrow_mut(|io| {
            let result = if io.sample() & 1 == 0 {
                io.files.contains_key(&self.0)
            } else {
                true
            };
            io.calls.push(Call::Exists(self.0.clone(), result));
            result
        })
    }
}

fn delivery() -> executor::Delivery {
    serde_json::from_value(json!({
        "key":"recorded-key", "epoch":5,
        "output":{"Effect":{"node":0,"seq":1,"kind":"PostApproval","head":null}},
        "generation":1,"head_sha":"a".repeat(40),
        "repository":{"owner":"owner","name":"sandbox","installation":3},
        "issue":12,"pull_request":null,"paths":[],"attempt":1,"round":0
    }))
    .unwrap()
}
#[derive(Debug, PartialEq, Eq)]
struct Outcome {
    returns: Vec<std::result::Result<Value, String>>,
    calls: Vec<Call>,
    persisted: BTreeMap<PathBuf, Vec<u8>>,
    writes: usize,
}
fn run(reference: bool, samples: &[u8], program: &[u8], crash_at: usize) -> Outcome {
    IO.set(Io {
        samples: samples.to_vec(),
        crash_at,
        ..Io::default()
    });
    let root = ScriptPath(PathBuf::from("records"));
    let old = reference::Records::new(&root);
    let new = current::Records::new(&root);
    let mut returns = vec![];
    for operation in program {
        let mut delivery = delivery();
        delivery.epoch = i64::from(*operation) - 32;
        let completion = executor::Completion::confirmed(vec![]);
        let result = match operation % 4 {
            0 => if reference {
                old.fence(delivery.epoch)
            } else {
                new.fence(delivery.epoch)
            }
            .map(|()| json!(null)),
            1 => if reference {
                old.deliver(&delivery, completion)
            } else {
                new.deliver(&delivery, completion)
            }
            .map(|c| json!(c)),
            2 => if reference {
                old.remember(&delivery, completion)
            } else {
                new.remember(&delivery, completion)
            }
            .map(|c| json!(c)),
            _ => if reference {
                old.reconcile(&delivery)
            } else {
                new.reconcile(&delivery)
            }
            .map(|r| match r {
                executor::Reconciliation::Found(c) => json!({"found":c}),
                executor::Reconciliation::Absent => json!("absent"),
                executor::Reconciliation::Pending => json!("pending"),
            }),
        };
        returns.push(result.map_err(|e| e.to_string()));
        if IO.with_borrow(|io| io.crashed) {
            break;
        }
    }
    IO.with_borrow(|io| Outcome {
        returns,
        calls: io.calls.clone(),
        persisted: io.files.clone(),
        writes: io.writes,
    })
}
fn compare(samples: &[u8], program: &[u8]) -> std::result::Result<(), TestCaseError> {
    let uninterrupted = run(true, samples, program, 0);
    for crash_at in 0..=uninterrupted.writes {
        let expected = run(true, samples, program, crash_at);
        let actual = run(false, samples, program, crash_at);
        prop_assert_eq!(actual, expected, "crash after write {}", crash_at);
    }
    Ok(())
}
/// State-table Effects :755: a missing fence accepts a nonnegative epoch.
#[test]
fn fence_reference_agrees_at_every_write() {
    compare(&[], &[40, 41, 42, 43]).unwrap();
}
/// CC-E extraction equivalence, unbounded values via generated changing samples.
/// Each generated finite schedule is checked at every executed write boundary.
#[test]
fn changing_reads_and_crashes_preserve_calls_results_and_bytes() {
    let output = std::env::var_os("TEST_UNDECLARED_OUTPUTS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("delivery-io-regressions.txt");
    std::fs::write(&output, include_bytes!("io_schedule-regressions.txt")).unwrap();
    let path = Box::leak(output.to_string_lossy().into_owned().into_boxed_str());
    let mut runner = TestRunner::new(Config {
        cases: 64,
        rng_seed: RngSeed::Fixed(73005),
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(path))),
        ..Config::default()
    });
    runner
        .run(
            &(
                prop::collection::vec(any::<u8>(), 0..24),
                prop::collection::vec(any::<u8>(), 1..8),
            ),
            |(samples, program)| compare(&samples, &program),
        )
        .unwrap();
}
