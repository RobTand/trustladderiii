# Rust merge gate evidence

The gate builds every workspace test target once, then runs four binaries concurrently. Native libtest tests run one thread per binary and report their own execution times. Custom harnesses keep their existing CLI and assertions. The GitHub harness has two independent groups, each with its own Postgres. Plain `cargo test --workspace --all-targets --locked` still runs every scenario.

`tools/ci/check.py rust` runs fmt, clippy and the enforcing runner. Development runs of that command must go through PrismaBuild. Mergify runs it directly in its admitted fleet runner.

## What was slow

- `workers.rs` repeatedly imports the full DeepAgents/LangChain runtime even for raw protocol failures and blocked-process fixtures. Those branches now execute before those imports. Four planner controller cases use a protocol worker. Real ingest/audit, valid and rejected planning, maker and reviewer execution remain contracts.
- GitHub steering runs several workers per scenario. Each classification, writer turn and planning turn previously started the full agent runtime. Steering now uses a stdlib protocol fixture. Its Git commits, controller, worker supervision, Postgres, GitHub HTTP server and restart behavior remain real. Six writer structural-retry cases use that same transport; the multiline writer and classifier retain real runtime contracts.
- Worker deadlines and quota allowances already use injected supervisor time. There was no multi-second deadline sleep to replace there. Remaining 10 ms fixture loops synchronize process output or deliberate blocking, rather than simulate elapsed business time. This change does not raise any timeout.
- Postgres was already shared once per custom binary. Each scenario still has an isolated database, with real migrations and fsync. Recovery tests retain fsync and their process-crash boundaries.
- Publishing, crash safety, Mergify and PrismaBuild still perform their original subprocess, Git, database and crash trials. Scheduling them alongside other binaries removes their serial contribution to gate time. Unnamed setup and trailing work are charged to `__tail__`, rather than disappearing from the budget.

The protocol worker shares scripted reply construction with the real-model fixture. Planning still measures real token counts. Steering supplies internally consistent forecast counts because token sizing is covered by the separate planning contracts. The fake worker does not decide controller phases or retry policy.

## Measurement method

All actions reserve four CPUs and bound native numerical, Rayon and Tokio worker threads to one. Host pins are used only for this required cross-host measurement. The fleet remains shared, so contention varies. Dependency caches were populated; each snapshot has a fresh Cargo target directory. Initial baselines run binaries serially with one native test thread. Final runs use four concurrent binaries, with one native test thread each. Baseline and final custom timings conservatively include setup since the previous completion marker.

Baselines use original main `87119190` plus the measurement driver. The final source includes main through `b2313f6b`, including request vocabulary and dequeue evidence work merged during this ticket. Those additional scenarios remain enabled. Final timings include fmt and clippy; the baseline action measured compilation and tests. This makes total-action comparison conservative, but it is not an isolated benchmark of the test transport alone.

`cargo-nextest` was unavailable. The Cargo-artifact runner handles native tests and this repository's many `harness = false` binaries without passing them unsupported libtest flags. It emits every duration and writes `target/ci-test-times.json`.

## Enforced budgets

The default is 10 seconds. The initial explicit slow list uses the maximum observed duration across dl380g10, sparky and sparklina, multiplied by 1.5 and rounded up to five seconds. Tests with that margin above 10 seconds are listed explicitly. The list includes fixture tails, which expose remaining work to optimize.

A PR cannot increase the default, add an exception or raise an exception's limit. The runner compares the policy against the PR base SHA or merge-group base SHA, fetching missing target history in shallow CI checkouts. Initial installation is allowed only when the target has no policy. Missing or skipped native test results fail. Diagnostics stay on stderr so partial background messages cannot swallow stdout completion records.

## Required coordinator workflow change

The Rust job uses `actions/checkout` with `persist-credentials: false` and its default shallow depth. Enforcing the target branch's budget needs that target commit. A private-repository fetch after checkout has no retained credential. In `.github/workflows/light-checks.yml`, add only the following key to the existing Rust job's checkout `with` block:

```yaml
fetch-depth: 0
```

Keep `persist-credentials: false`. Full checkout happens within the authenticated checkout action, so both the PR target and merge-queue target history are available afterward. The runner's fetch fallback remains useful for authenticated developer checkouts and is covered by the shallow-clone regression. It fails closed when history cannot be fetched. This stack deliberately leaves the fleet-runner agent's workflow and deployment files untouched; the coordinator must apply this checkout change before enabling the new gate. No cargo-nextest installation is required.

## Real worker contracts and ARM runner investigation

The named `--real-worker-contracts` selectors cover ingest, audit, planner, maker, reviewer, writer and classifier. They keep real Python runtime execution with a scripted model, real schemas and tools, and real maker Git/check/review flows. They also execute in the default suites.

The failing runner run was https://github.com/RobTand/trustladderiii/actions/runs/41221386685, job 124602540969, at `1bc998e78dabf18fbf4f9bfe0891328b0ac44171`. The missing `reviewed_sha` means the rebased check plan had lost its replayed reviewed tree. The adapter deliberately drops that field when reviewed-tree replay fails; the original log did not retain the Git failure. The added assertion now prints the entire disposable check plan, including any `reproducing_unavailable` reason.

I read the #176 runner sources without editing them. Qualification used the same ARM runner image generation, `sha256:097c463c42e1021b8223b0bc2698f7426d6f87ea72f51372eb1da733af62442a`, through PB's Docker ownership shim. The container has the runner's 6 GiB memory limit, 1024 PID limit, dropped capabilities and no-new-privileges. PB preserves its assigned CPU affinity. This is the same image under PB-owned Docker, not a replay of the live rootless Podman service. The host's managed CPython 3.14 is mounted read-only because Actions setup-python supplies it in CI; the image alone does not contain that interpreter.

All seven role contracts passed in that ARM image. Twenty consecutive focused rebase trials passed. The original failed source also passed both a focused replay and its full worker ordering in the same image. The historical and current production Git launchers differ because main already contains the separate Git hardening PR. No deterministic ARM incompatibility or runner-image defect was demonstrated. **The original intermittent failure's root cause remains unproven.** These passes support contract compatibility, not a claim that the intermittent failure is fixed. There is no evidence-based image change to request, and re-enabling Spark CI runners remains the coordinator's decision.

The `worker retirement retained memory-store-error: Postgres operation failed` message is expected fault injection in `memory_store_error_retries_retirement`. That scenario installs a failing INSERT trigger, checks that evidence and the retirement retry remain, removes the trigger, then checks successful idempotent retirement. It is unrelated to reviewed-tree replay. The intentional GitHub `FailingSource` thread panic likewise tests ingress-source failure handling.

## Reproduce

Run only inside an admitted PB action:

```sh
python3 tools/ci/check.py rust
python3 -m unittest discover -s tools/ci -p 'test_*.py' -v
python3 tools/ci/gate_mutants.py all
```

PB snapshots omit `origin`. For enforcing qualification, provide a temporary `GITHUB_EVENT_PATH` containing the frozen base SHA `b2313f6b`, as CI does. Do not disable enforcement. For image qualification declare the immutable image with PB's `--container-image`, then run:

```sh
python3 tools/ci/worker_contract_container.py sha256:097c463c42e1021b8223b0bc2698f7426d6f87ea72f51372eb1da733af62442a 20
```

No workflow, runner deployment, kernel, state table or production file changes belong to this stack. Shared `github.rs` changes are limited to fixture selection and CLI dispatch; #173 request-vocabulary and #174 catch-up selectors remain intact. Lower stack layers isolate workers and GitHub changes so they can merge independently.

## Before and after

Seconds, from admitted action start through completion. All final actions include fmt, clippy, compilation and workspace tests. Every test binary passed. The sparklina driver rejected provisional budget limits after test completion; the finalized policy is checked separately against every recorded duration. No failed scenario is represented as a pass.

| Host | Before total | After total | Before test wall | After test wall |
|---|---:|---:|---:|---:|
| dl380g10 | 1758.867 | 650.513 | 1625.769 | 471.233 |
| sparky | 952.857 | 328.713 | 883.124 | 234.396 |
| sparklina | unmeasured | 406.535 | unmeasured | 315.325 |

| Host | Measurement | PrismaBuild action key |
|---|---|---|
| dl380g10 | Before, driver exit 0, all test binaries pass | `1d549e1fe221be242f42ae8729add9c9dbb7879c52d4896aa594746d9a896917` |
| sparky | Before, driver exit 0, all test binaries pass | `1cc2c41066c0d49c6583b4c2c76f402a4b3c2c83a70295ff5ff0f7c8ec2d3765` |
| dl380g10 | After, driver exit 0, all test binaries pass | `ef0e60f9b21b96156c833c8f6962060c8a006a1b29dd77bfc67a99e642782145` |
| sparky | After, driver exit 0, all test binaries pass | `9c87393bcb6ef6a66537ae344247911b0e8e4f9c34a16e8c6b4449d180e258c2` |
| sparklina | After, driver exit 1, all test binaries pass | `6c09b2e1b9566478b9c23f652c2d34d6547de50df3f4d54f0ba01cbc3c7f149a` |

### Per binary

These are execution durations, excluding compilation. GitHub is split after the change; the two durations overlap, so their sum is not gate wall time. `n/a` means that target name was absent in that measurement.

| Package / target | dl380 before | dl380 after | sparky before | sparky after | sparklina after |
|---|---:|---:|---:|---:|---:|
| `tl3-config/config` | 0.696 | 0.685 | 0.297 | 0.283 | 0.279 |
| `tl3-config/tl3_config` | 0.001 | 0.001 | 0.001 | 0.001 | 0.001 |
| `tl3-controller/core_defects` | 5.997 | 5.814 | 10.024 | 8.556 | 12.869 |
| `tl3-controller/crash_safety` | 95.887 | 101.914 | 34.733 | 43.277 | 43.966 |
| `tl3-controller/fixture_contracts` | 1.073 | 0.974 | 2.520 | 1.441 | 2.203 |
| `tl3-controller/github` | 527.651 | n/a | 324.051 | n/a | n/a |
| `tl3-controller/github-flow` | n/a | 174.526 | n/a | 129.575 | 174.844 |
| `tl3-controller/github-notices` | n/a | 263.339 | n/a | 154.106 | 183.932 |
| `tl3-controller/hindsight` | 36.706 | 39.362 | 31.399 | 40.705 | 55.183 |
| `tl3-controller/ingress_loom` | 0.067 | 0.070 | 0.029 | 0.029 | 0.029 |
| `tl3-controller/looks` | 28.760 | 33.270 | 20.169 | 35.105 | 61.558 |
| `tl3-controller/mergify` | 51.895 | 100.653 | 33.487 | 81.713 | 135.319 |
| `tl3-controller/pb_trial` | 0.001 | 0.001 | 0.001 | 0.001 | 0.001 |
| `tl3-controller/prismabuild` | 67.712 | 65.054 | 56.840 | 56.779 | 70.804 |
| `tl3-controller/production` | 18.965 | 23.613 | 12.460 | 13.871 | 7.687 |
| `tl3-controller/publishing` | 133.191 | 192.282 | 64.465 | 68.960 | 89.281 |
| `tl3-controller/reproducing_rebase` | 1.129 | 0.738 | 0.361 | 0.724 | 0.815 |
| `tl3-controller/sandbox_gate` | 0.044 | 0.033 | 0.028 | 0.047 | 0.050 |
| `tl3-controller/scope` | n/a | 1.862 | n/a | 1.530 | 1.133 |
| `tl3-controller/simulation` | 43.628 | 37.890 | 31.742 | 23.977 | 57.233 |
| `tl3-controller/tl3` | 0.001 | 0.001 | 0.001 | 0.001 | 0.001 |
| `tl3-controller/tl3-deploy` | 0.017 | 0.016 | 0.085 | 0.085 | 0.025 |
| `tl3-controller/tl3_controller` | 11.965 | 16.500 | 7.665 | 16.217 | 14.980 |
| `tl3-controller/tl3d` | 0.002 | 0.001 | 0.001 | 0.001 | 0.001 |
| `tl3-controller/workers` | 517.870 | 471.232 | 196.244 | 184.890 | 227.406 |
| `tl3-controller/worktrees` | 12.646 | 9.219 | 9.887 | 7.908 | 17.981 |
| `tl3-controller/writer_quote` | 0.076 | 0.053 | 0.030 | 0.033 | 0.032 |
| `tl3-controller/writer_references` | 21.247 | 18.596 | 5.740 | 5.788 | 5.080 |
| `tl3-github/check_prose` | 0.002 | 0.001 | 0.001 | 0.001 | 0.002 |
| `tl3-github/messages` | 0.188 | 0.166 | 0.097 | 0.104 | 0.103 |
| `tl3-github/models` | 0.002 | 0.002 | 0.002 | 0.002 | 0.003 |
| `tl3-github/rest` | 10.223 | 9.842 | 8.625 | 9.955 | 13.950 |
| `tl3-github/tl3_github` | 0.002 | 0.002 | 0.001 | 0.002 | 0.002 |
| `tl3-github/webhooks` | 0.007 | 0.008 | 0.005 | 0.005 | 0.006 |
| `tl3-kernel/footprint_sets` | 0.711 | 0.734 | 0.428 | 0.437 | 0.440 |
| `tl3-kernel/proof_map` | 0.002 | 0.002 | 0.002 | 0.002 | 0.002 |
| `tl3-kernel/random_walks` | 2.720 | 2.465 | 1.524 | 1.508 | 1.506 |
| `tl3-kernel/read_queries` | 0.002 | 0.002 | 0.001 | 0.001 | 0.001 |
| `tl3-kernel/scenarios` | 0.008 | 0.012 | 0.005 | 0.005 | 0.006 |
| `tl3-kernel/tl3_kernel` | 0.001 | 0.001 | 0.001 | 0.001 | 0.001 |
| `tl3-mergify/client` | 0.108 | 0.121 | 0.078 | 0.083 | 0.083 |
| `tl3-mergify/sandbox_gate` | 0.022 | 0.020 | 0.021 | 0.028 | 0.025 |
| `tl3-mergify/tl3_mergify` | 0.001 | 0.002 | 0.001 | 0.001 | 0.001 |
| `tl3-prismabuild/client` | 3.274 | 3.158 | 2.881 | 2.828 | 2.782 |
| `tl3-prismabuild/tl3_prismabuild` | 0.001 | 0.001 | 0.001 | 0.001 | 0.001 |
| `tl3-store/fixture_lifecycle` | 16.333 | 14.499 | 9.117 | 12.416 | 15.663 |
| `tl3-store/postgres` | 10.374 | 11.190 | 16.240 | 24.493 | 37.958 |
| `tl3-store/tl3_store` | 0.002 | 0.002 | 0.002 | 0.003 | 0.001 |
| `tl3-worktrees/tl3_worktrees` | 4.555 | 5.403 | 1.826 | 2.804 | 2.203 |

### Rewritten test claims and mutation evidence

Before/after seconds below use sparky. Worker tests retain their original aggregate names. GitHub names gain the `github-flow` group prefix; writer structural cases gain the `writer_transport_` prefix. The same original assertions remain, with three explicit assertions added to expose defects directly instead of an unwrap or fixture timeout.

`M` is the final mutation action, recorded below. Every row names its killed mutation; only a test-source assertion counts. Compile errors, fixture timeouts and unrelated process exits do not count.

| Test | Claim | Red evidence | Before | After |
|---|---|---|---:|---:|
| `steering_commands_remark_authorization_and_dedup` | Authorization, pause/resume, remarks and dedup | M: pause / commands | 8.269 | 5.126 |
| `steering_question_ids_one_clarification_and_stale_answer` | One clarification; stale answers cannot answer the current question | M: clarify / answers | 8.901 | 5.843 |
| `steering_replacement_reuses_issue_and_latest_binding` | Replacement retains issue and latest binding | M: pause / replacement | 4.279 | 1.970 |
| `steering_closed_complaint_files_one_correction_with_revert` | Closed complaint files one correction and carries revert instructions | M: revert / correction | 9.361 | 7.194 |
| `steering_pause_allows_queued_pr_to_merge` | Pause preserves a queued PR merge | M: pause / queued_pause | 7.211 | 5.660 |
| `steering_named_answer_applies_after_controller_recovery` | Named answer applies after recovery and reaches Merged | M: continue / answer_recovery | 12.281 | 7.263 |
| `steering_classifier_retries_and_rephrase_on_failure` | Classification failures retry and eventually ask to rephrase | M: pause / classifier_failures | 9.510 | 3.673 |
| `steering_classifies_per_request_with_durable_liveness` | Concurrent requests retain separate classifications and liveness | M: pause / concurrent_requests | 7.930 | 2.881 |
| `steering_try_again_passes_guidance_to_planner` | Retry passes the exact requested guidance | M: guidance / guidance | 5.605 | 3.063 |
| `steering_add_credit_resumes_request` | Credit answer adds the exact requested amount and resumes work | M: credit / credit | 14.351 | 14.758 |
| `steering_answer_change_request_reuses_issue` | Change-request answer replaces work on the same issue | M: pause / answer_replacement | 7.484 | 6.856 |
| `writer_transport_wrong-quote` | Reject paraphrased request quote, retry with feedback, post once | M: writer-policy / writer:wrong-quote | 1.458 | 0.133 |
| `writer_transport_no-draft` | Reject the wrong question shape, retry, post once | M: writer-policy / writer:no-draft | 1.451 | 0.132 |
| `writer_transport_recommendation` | Reject recommendation outside the supplied options | M: writer-policy / writer:recommendation | 1.452 | 0.125 |
| `writer_transport_no-excerpt` | Require the comment excerpt in clarification | M: writer-shape / writer:no-excerpt | 1.447 | 0.134 |
| `writer_transport_no-text` | Require text for clarification | M: writer-shape / writer:no-text | 1.472 | 0.118 |
| `writer_transport_always-multiline` | After three multiline failures, use the bounded fallback | M: writer-shape / writer:always-multiline | 2.243 | 0.147 |
| `schema_missing_nonzero_and_duplicate_replies_fail` | Reject malformed, missing, forged and duplicate output and nonzero exits | M: exit / workers:wire | 12.262 | 8.420 |
| `planner_leaf_narrowing_and_rule_retry_with_findings` | Persist valid plans, preserve split context/credit, reject malformed output, retry rejected plans once with findings and alternate model | M: planner-evidence / workers:planning | 23.288 | 16.806 |

Raw wire fixture import reordering and shared scripted-reply extraction preserve existing test bodies. The full worker suite retains its original real maker/reviewer, crash, replay and retry scenarios, including every assertion and fsync boundary. The planner mutation demonstrates rejection in the aggregate six-mode test; it does not claim exhaustive mutation coverage of every planner branch.

## Verification records

| Evidence | Result | PrismaBuild action key |
|---|---|---|
| Current-source 19-case mutation campaign | Exit 0, 250.034s; every mutant killed by an assertion | `5d4d9ab55b356d59895a5a7e8bb24991f714eac729fd3db2406c3322faaf5d59` |
| CI unit tests and Python static checks | Exit 0; 15 unit tests and Ruff E9/F clean | `c42a953309ca0ea8a8acabe11d2cf693275a093a9a5956ad28b5bdac96de1919` |
| ARM image, all seven real worker roles and focused maker/reviewer rebase | Exit 0, 95.690s | `dd0091980f3e434af5820823b9a9b19a5ce71bfff41992249364e8dd3d099fb1` |
| ARM image, 20 consecutive focused maker/reviewer rebases | Exit 0, 230.978s; 20/20 | `8b05e9108c30747a3f5699739a4bab2bab9d208fb50718bf8be67516fae1bb85` |
| ARM image, original failed source, focused rebase | Exit 0, 58.382s | `ca7bf4752c4bf7f22524bb12774be5a0757dabfbd6b2157b1b533e28372399b2` |
| ARM image, original failed source, full worker ordering | Exit 0, 364.101s | `6ff91f024b280784f4891ce97d419bdc3b9bce55cfb44c8f3f03001980b8acd0` |

The assertion-failure commits remain reachable through history-only merges on the CI layer. Those merges preserve the final tree on `b2313f6b`; their older production changes do not appear in this stack's diff.

| CI regression | Red commit | Red action, expected exit 1 | Intended failure | Green |
|---|---|---|---|---|
| Budget rejection/monotonic limits, bounded overlap, native/custom timing | `940b81ef` | `a6097aa4e3c5fa9dc4dbbf7b8a68a716ccc90f9f6b7377d6b4ab20466dd90fbb` | Five `NotImplementedError` failures in new runner stubs | CI unit action above |
| Shallow PR checkout compares the target policy | `c37d85d2` | `0886c06758e8ce61a11b1cfa9c5c8d2fe29d058c9bae947148bc62470724f654` | Missing target history blocks policy lookup | CI unit action above; real shallow clone fetches target and rejects expanded PR exceptions |
| Partial stderr cannot hide a custom completion record | `a745d215` | `f063f554b090b5214703e22cd84320959e5e1a76e73b5d4f8af0c12646c40797` | Assertion: `fixture::named_scenario` missing from results | CI unit action above |

Terminal exit codes, retained logs and CAS receipts were checked with `pbwait.py --endings`. Successful actions have verified result receipts; expected red actions have verified failure endings. `docs/fast-gate-evidence.json` records the full keys, measurement log hashes and receipt hashes. No required Rust scenario was skipped. cargo-nextest was absent and was not required.

The first enforcing sparklina run, `a327647a580947c82263122f1326dc4dbdf501bf21852ff33c6245bda9ced60f`, passed all scenarios but failed the provisional 25-second GitHub notice tail limit at 26.490 seconds. Before publishing the initial policy, that measured limit received the same 1.5 margin and five-second rounding, initially giving 40 seconds. A later observed shutdown extended the final initial value to 50 seconds using the same formula. This is initialization of an unmerged policy, not an expansion of a merged slow list. No test timeout was changed.

Excluded trials include initial custom-harness invocations with unsupported libtest flags, a mutation attempt without its Python environment, an early assertion detector that missed a custom message, and planner-fixture runs that lacked progress events. None counts as passing or mutation evidence. The missing planner progress fixture was fixed and all original assertions now pass. Two initial enforcing snapshots stopped before tests because PB omits `origin`; final snapshots supplied frozen PR-base event metadata. A queued dl380g10 copy of that setup mistake was withdrawn before execution. The earlier dl380g10 measurement with merged stdout/stderr lost one completion record and is superseded by the separated-stream qualification.

## Cached-dependency gate

The exact `python3 tools/ci/check.py rust` command passed in **264.527 seconds, 4m25s**, on sparky. Test execution took 217.719 seconds. PB action `e40d92c2c880beeee09f34be3921a73184b207e5cae080cab92d83d07b4a0117` exited 0 and includes the dependency-priming phase, so its total action time is 356.623 seconds.

To model the existing Swatinem dependency cache, the action first populated Cargo dependency artifacts, then ran `cargo clean -p` for every workspace package before timing the exact CI command. Local workspace crates were rebuilt. The timing includes fmt, clippy, test compilation and all tests, and excludes Actions setup and cache restoration. This meets the five-minute command target with cached dependencies. The fresh-target gate remains 328.713 seconds on the same host; a five-minute cold gate has not been demonstrated.

A second sparklina action, `6c09b2e1b9566478b9c23f652c2d34d6547de50df3f4d54f0ba01cbc3c7f149a`, again passed every test binary but rejected a 24.944-second GitHub flow tail and a 28.824-second generic Mergify dequeue case against provisional limits. The final initial policy uses all collected measurements, with the same 1.5 factor and five-second rounding. Its 88 explicit exceptions include slow fixture tails. Structured evidence records every observation key, log hash and the observed maximum used for each exception. These policy initialization changes precede the first publication; after merge the list and limits may only shrink.

## Final policy verification

PB action `6357bd8445d3ca1b42f5214db79fa8f9197b98591033bb23a11929ad29510840` exited 0. It checked every retained stdout SHA-256 before parsing, required every test binary to have exited 0, then applied the final budget to **6,710 duration records across eight actions**. All passed. The same action passed all 15 CI unit tests and Ruff E9/F checks. Only initial policy limits and evidence documentation changed after the matrix; no Rust or worker fixture source changed. This replay validates that configuration change without selecting a quieter rerun of unchanged tests. Failed provisional driver exits remain explicit in the measurement table.
