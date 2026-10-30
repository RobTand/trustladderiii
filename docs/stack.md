# TL3 stack

Rob and I settled this on 2026-10-29. The state machine gets its own document, `docs/state-machine.md`, once Rob lays it out.

## Hardware

| Box | Hardware | Role in TL3 |
|---|---|---|
| dl380g10 | 2× Xeon Gold 6230 with 40 cores and 80 threads, 306 GB RAM, ZFS file server | Runs the controller, Postgres, MLflow and Hindsight. Hosts the PrismaBuild queue and an x86 worker. |
| sparky, sparklina | DGX Spark, GB10, arm64 | PrismaBuild GPU workers. |
| beelink | Small home server | Runs CLIProxyAPI at 192.168.1.120:8317. |
| celestia | Ryzen 9800X3D, RX 9070 XT, Ubuntu | Rob's workstation. Outside the first loop. |

## Components

| Layer | Choice | Why |
|---|---|---|
| Kernel | Rust, with its laws proven by Kani | Decided 2026-10-29 after the trial in `spikes/kernel-trial`. The kernel is the state machine, a pure function `step(world, event) -> (world, effects)`, specified in `docs/state-table.md`. Ordinary builds have no standard library, so core logic cannot slip into I/O code. |
| Controller | A Rust process under a systemd user unit that feeds events to the kernel, keeps the event log in Postgres and carries out effects | Decided 2026-10-29. With the state machine in the kernel, the per-item LangGraph graph is gone. LangGraph stays inside the agent workers. |
| Agent nodes | LangChain Deep Agents, one Python 3.14 worker per agent run | TL3 controls every system prompt. A harness profile drops optional middleware, turns off the default general-purpose sub-agent and overrides tool descriptions. A crashed agent cannot take the controller down. |
| Models | LangChain chat models that call CLIProxyAPI | One endpoint spreads requests over several subscriptions. |
| State | A dedicated Postgres instance that holds the event log, world snapshots and effect records, plus the agent workers' LangGraph checkpoints | It can move to a bigger host or add replicas without code changes. |
| Tracing, prompts, grading | MLflow, with its own database on the TL3 Postgres instance | It traces every node, model call and tool call. It keeps role prompts as numbered versions, and it grades responses with model judges, code checks and Rob's feedback. |
| Memory | Hindsight, read and written | Recall nodes fetch past context before agent work. TL3 writes outcomes, decisions and postmortems back. |
| Development tests and trials | PrismaBuild | Agents test their work on PrismaBuild while they work. PB's main job is to keep every fleet machine busy to its capacity and to feed the Sparks data over InfiniBand on time. |
| CI and merging | Mergify's merge queue, running GitHub checks on Mergify runners that live on fleet machines and share their resources | Mergify is the CI (Rob, 2026-10-29). A change reaches main only through the queue. The controller reads check results itself and never takes an agent's word for them. |
| GitHub | REST calls, plus webhooks for issues, pull requests, check runs and the merge queue that reach a receiver on dl380g10 through a Cloudflare tunnel | Events arrive without polling. Rob's GraphQL budget is shared, so TL3 stays off GraphQL. |
| Config | One TOML file | Nothing environment-specific goes in code. |

## Waits

Every wait is event-driven, the way TL1 used Temporal to wake a node when a computation finished. A waiting item is a state in the kernel, and nothing runs while it waits. The event that carries the result is the kernel's next input.

Work ends on evidence, never on a clock. Rob's rules:

- Work runs as long as it needs while its upstream shows it is alive. No deadline applies to duration.
- When liveness signals stop, TL3 reads the upstream's own record before it decides anything. Silence never fails an item by itself.
- Results stay upstream under a key, so TL3 still uses a result that lands late.
- A retry reattaches by key. TL3 never starts a second copy of work that may still be running.
- Only the owner declares work lost. For PrismaBuild, that means lease expiry.
- The limit applies to stalls, meaning work that is alive but makes no progress. `docs/state-table.md` under "Looks and stalls" says what happens to each kind. In short:
  - A stalled agent run is stopped and counts as a failure. Every second failure of the same step raises an incident.
  - PrismaBuild ends a quiet job itself, under its progress contract.
  - A stall in Mergify or GitHub raises an incident.

A wait is a state, not a paused function, so nothing reruns when it ends. The effect that starts the work is emitted on entering the state, and recovery asserts it again by key.

| Upstream | Sign of life | How TL3 hears it |
|---|---|---|
| PrismaBuild | Workers renew each claim's lease every 30 s by rewriting `claimed/<key>.lease`. PB declares a claim lost after 300 s without a renewal. | The bell's long-poll pushes state changes, including a lost lease: the reaper writes an attempt record marked `lease_lost`. Renewals are not on the bell yet (PB #2426). The cursor resets when the bell restarts or its buffer overflows, and on a reset TL3 rereads the record of every action it waits on. TL3 submits jobs under PB's progress contract (`--progress-phase`) and sets no `--timeout-s`, so PB judges a job by whether it keeps committing progress, not by how long it runs. |
| Mergify CI | Check runs finishing, and the queue moving | GitHub webhooks for check runs and the merge queue. If neither moves past its allowance, a look reads the PR and the queue, and a confirmed stall raises an incident. |
| Model calls | The token stream | The streaming response. A dropped stream retries only that turn, from the agent's last checkpoint. |
| GitHub | Webhook deliveries | The Cloudflare tunnel. GitHub does not redeliver failed webhooks, so TL3 runs one catch-up pass after any outage. |
| Hindsight | Its reply | A Hindsight outage makes the item wait. TL3 never treats an outage as an empty answer. |

## Left out

Temporal, vendor agent CLIs such as Codex CLI and Claude Code, SQLite, Dolt, Beads, Zuul, ZooKeeper, Langfuse, Podman Compose, free-threaded Python, and any coordinating agent. Each one was tried or weighed in an earlier attempt, or lost to a choice above.

## Open

- PB #2426 asks the bell to publish lease renewals. Until it lands, TL3 hears a lost lease but not each renewal.
- Pick a retention window for MLflow traces. Deep Agents resends the whole conversation each turn, so traces grow fast. TL2 kept bulk payloads for 30 days.
- Check that MLflow's judges can call models through CLIProxyAPI.
- Decide the first repo, whether celestia joins PrismaBuild, and whether any work type gets a runaway cap.
