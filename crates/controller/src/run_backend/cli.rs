//! One JSON command on stdin, NDJSON result/event frames on stdout.
use super::*;
use crate::adapters::run_backend::flux::Flux;
use std::io::{BufRead, BufReader, Read, Write};

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    BeginContainerLaunch {
        owner: String,
        correlation: String,
        launcher: Box<RuntimeOwnership>,
    },
    BindContainerLaunch {
        owner: String,
        correlation: String,
        observed: Box<RuntimeOwnership>,
    },
    PrepareRuntimeBundle {
        input: Box<PrepareBundleInput>,
    },
    PrepareRuntime {
        bundle: PathBuf,
        reservation: Reservation,
        arguments: Vec<String>,
    },
    PrepareIntent {
        owner: String,
        correlation: String,
        request: Box<Request>,
    },
    Submit {
        request: Box<Request>,
        intent: SubmissionIntent,
    },
    Attach {
        owner: String,
        correlation: String,
    },
    Cancel {
        owner: String,
        correlation: String,
    },
    Liveness {
        owner: String,
        correlation: String,
    },
    Probe {
        reservation: Reservation,
        placement: Placement,
    },
    LaunchContext {
        owner: String,
        correlation: String,
        job: String,
    },
    ValidateRuntimeBinding {
        owner: String,
        correlation: String,
        facts: Box<RuntimeBindingInput>,
    },
    CollectWorker {
        owner: String,
        correlation: String,
        facts: Box<CollectorInput>,
    },
    WorkerPayload {
        owner: String,
        correlation: String,
        terminal: Terminal,
    },
    CollectRunner {
        owner: String,
        correlation: String,
        facts: Box<CollectorInput>,
    },
    DaytimePayload {
        owner: String,
        correlation: String,
        terminal: Terminal,
    },
    ValidateAllocation {
        owner: String,
        correlation: String,
        facts: Box<AllocationInput>,
    },
    RetainRuntime {
        owner: String,
        correlation: String,
        ownership: Box<RuntimeOwnership>,
    },
    RetainCleanup {
        owner: String,
        correlation: String,
        evidence: BlobRef,
    },
    CleanupRuntime {
        owner: String,
        correlation: String,
    },
    StopWatch,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Frame {
    pub schema: FrameSchema,
    pub sequence: u64,
    #[serde(flatten)]
    pub body: FrameBody,
}
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub enum FrameSchema {
    #[serde(rename = "tl3.run_backend.frame.v1")]
    V1,
}
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FrameBody {
    Event { event: Box<Event> },
    Result { result: Box<CliResult> },
    Error { code: ErrorCode, message: String },
}
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum CliResult {
    BeginContainerLaunch {
        launch: Box<ContainerLaunchIntent>,
    },
    BindContainerLaunch {
        binding: Box<ContainerLaunchBinding>,
    },
    PrepareRuntimeBundle {
        bundle: BlobRef,
    },
    PrepareRuntime {
        prepared: Box<PreparedRuntime>,
    },
    PrepareIntent {
        intent: SubmissionIntent,
    },
    Submit {
        submission: Submission,
    },
    Attach {
        observation: Observation,
    },
    Cancel {
        acknowledgement: CancelAck,
    },
    Liveness {
        liveness: Liveness,
    },
    Probe {
        probe: Probe,
    },
    LaunchContext {
        context: Box<LaunchContext>,
    },
    ValidateRuntimeBinding {
        qualification: RuntimeQualification,
    },
    CollectWorker {
        collection: WorkerCollection,
    },
    WorkerPayload {
        payload: WorkerPayload,
    },
    CollectRunner {
        report: BlobRef,
        payload: BlobRef,
    },
    ValidateAllocation {
        grant: CpuGrant,
    },
    RetainRuntime,
    RetainCleanup {
        cleanup: CleanupResult,
    },
    CleanupRuntime {
        cleanup: CleanupResult,
    },
    DaytimePayload {
        report: crate::contracts::report::CheckReport,
    },
}

struct Writer {
    sequence: u64,
}
impl Writer {
    fn frame(&mut self, body: FrameBody) -> Result<()> {
        let frame = Frame {
            schema: FrameSchema::V1,
            sequence: self.sequence,
            body,
        };
        let mut stdout = std::io::stdout().lock();
        serde_json::to_writer(&mut stdout, &frame)?;
        stdout.write_all(b"\n")?;
        stdout.flush()?;
        self.sequence += 1;
        Ok(())
    }
}

pub fn main() -> i32 {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut writer = Writer { sequence: 0 };
    let result = (|| -> Result<()> {
        if args == ["schema"] {
            print!("{}", schema::cli()?);
            return Ok(());
        }
        if args == ["launch-schema"] {
            print!("{}", schema::launch_context()?);
            return Ok(());
        }
        if args.len() == 2 && args[0] == "write-schemas" {
            let root = PathBuf::from(&args[1]);
            std::fs::create_dir_all(&root)?;
            durable::atomic(&root.join("run-backend.json"), schema::cli()?.as_bytes())?;
            durable::atomic(
                &root.join("run-backend-launch-context.json"),
                schema::launch_context()?.as_bytes(),
            )?;
            return Ok(());
        }
        let mut reader = BufReader::new(std::io::stdin());
        let mut line = Vec::new();
        std::io::Read::by_ref(&mut reader)
            .take(durable::MAX_BYTES as u64 + 1)
            .read_until(b'\n', &mut line)?;
        if line.len() > durable::MAX_BYTES {
            return Err(BackendError::invalid("command exceeds byte bound"));
        }
        let command: Command = serde_json::from_slice(&line)
            .map_err(|_| BackendError::invalid("command does not match generated Rust schema"))?;
        if args.len() != 2 || args[0] != "--config" {
            return Err(BackendError::invalid(
                "use --config FILE with explicit provisioned Options JSON",
            ));
        }
        let options: Options =
            serde_json::from_slice(&durable::read(PathBuf::from(&args[1]).as_path())?)?;
        let backend = Flux::new(options)?;
        let result = match command {
            Command::BeginContainerLaunch {
                owner,
                correlation,
                launcher,
            } => CliResult::BeginContainerLaunch {
                launch: Box::new(
                    backend.begin_container_launch(&Owner { owner, correlation }, &launcher)?,
                ),
            },
            Command::BindContainerLaunch {
                owner,
                correlation,
                observed,
            } => CliResult::BindContainerLaunch {
                binding: Box::new(
                    backend.bind_container_launch(&Owner { owner, correlation }, *observed)?,
                ),
            },
            Command::PrepareRuntimeBundle { input } => CliResult::PrepareRuntimeBundle {
                bundle: prepared::prepare_bundle(&input)?,
            },
            Command::PrepareRuntime {
                bundle,
                reservation,
                arguments,
            } => CliResult::PrepareRuntime {
                prepared: Box::new(prepared::prepare_runtime(&bundle, &reservation, arguments)?),
            },
            Command::PrepareIntent {
                owner,
                correlation,
                request,
            } => CliResult::PrepareIntent {
                intent: backend.prepare_intent(&request, &Owner { owner, correlation })?,
            },
            Command::Submit { request, intent } => CliResult::Submit {
                submission: backend.submit(*request, intent)?,
            },
            Command::Attach { owner, correlation } => {
                let stop = WatchStop::default();
                let control = stop.clone();
                // EOF is normal for a one-shot command. A second control frame stops only this watcher.
                std::thread::spawn(move || {
                    let mut line = Vec::new();
                    if std::io::Read::by_ref(&mut reader)
                        .take(1025)
                        .read_until(b'\n', &mut line)
                        .is_ok()
                        && line.len() <= 1024
                        && matches!(
                            serde_json::from_slice::<Command>(&line),
                            Ok(Command::StopWatch)
                        )
                    {
                        control.stop();
                    }
                });
                CliResult::Attach {
                    observation: backend.attach(
                        &Owner { owner, correlation },
                        &stop,
                        &mut |event| {
                            writer.frame(FrameBody::Event {
                                event: Box::new(event.clone()),
                            })
                        },
                    )?,
                }
            }
            Command::Cancel { owner, correlation } => CliResult::Cancel {
                acknowledgement: backend.cancel(&Owner { owner, correlation })?,
            },
            Command::Liveness { owner, correlation } => CliResult::Liveness {
                liveness: backend.liveness(&Owner { owner, correlation })?,
            },
            Command::Probe {
                reservation,
                placement,
            } => CliResult::Probe {
                probe: backend.probe(&reservation, &placement)?,
            },
            Command::LaunchContext {
                owner,
                correlation,
                job,
            } => CliResult::LaunchContext {
                context: Box::new(backend.launch_context(&Owner { owner, correlation }, &job)?),
            },
            Command::ValidateRuntimeBinding {
                owner,
                correlation,
                facts,
            } => CliResult::ValidateRuntimeBinding {
                qualification: backend
                    .validate_runtime_binding(&Owner { owner, correlation }, &facts)?,
            },
            Command::CollectWorker {
                owner,
                correlation,
                facts,
            } => CliResult::CollectWorker {
                collection: backend.collect_worker(&Owner { owner, correlation }, &facts)?,
            },
            Command::WorkerPayload {
                owner,
                correlation,
                terminal,
            } => CliResult::WorkerPayload {
                payload: backend.worker_payload(&Owner { owner, correlation }, &terminal)?,
            },
            Command::CollectRunner {
                owner,
                correlation,
                facts,
            } => {
                let (report, payload) =
                    backend.collect_runner(&Owner { owner, correlation }, &facts)?;
                CliResult::CollectRunner { report, payload }
            }
            Command::DaytimePayload {
                owner,
                correlation,
                terminal,
            } => CliResult::DaytimePayload {
                report: backend.daytime_payload(&Owner { owner, correlation }, &terminal)?,
            },
            Command::ValidateAllocation {
                owner,
                correlation,
                facts,
            } => CliResult::ValidateAllocation {
                grant: backend.validate_allocation(&Owner { owner, correlation }, &facts)?,
            },
            Command::RetainRuntime {
                owner,
                correlation,
                ownership,
            } => {
                backend.retain_runtime(&Owner { owner, correlation }, *ownership)?;
                CliResult::RetainRuntime
            }
            Command::RetainCleanup {
                owner,
                correlation,
                evidence,
            } => CliResult::RetainCleanup {
                cleanup: backend.retain_cleanup(&Owner { owner, correlation }, &evidence)?,
            },
            Command::CleanupRuntime { owner, correlation } => CliResult::CleanupRuntime {
                cleanup: backend.cleanup_runtime(&Owner { owner, correlation })?,
            },
            Command::StopWatch => {
                return Err(BackendError::invalid(
                    "stop_watch is a second-line attach control frame only",
                ));
            }
        };
        writer.frame(FrameBody::Result {
            result: Box::new(result),
        })
    })();
    match result {
        Ok(()) => 0,
        Err(error) => {
            let _ = writer.frame(FrameBody::Error {
                code: error.code,
                message: error.message,
            });
            1
        }
    }
}
