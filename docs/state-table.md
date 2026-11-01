# TL3 state table

Version 5, 2026-10-30, amended the same day with Rob's decisions Q42 and Q43, and on 2026-10-31 with Q44, Q45, Q46 and Q29 amended. This branch proposes Q48, which needs Rob's approval before queueing. Version 4 came out of three rounds of adversarial review by Sol and Muse. Version 5 adds Rob's decisions from the grilling of 2026-10-29 and 30, numbered Q1 to Q41 in `docs/state-machine.md`. This file is the source of truth for the kernel. Where `docs/state-machine.md` disagrees with it, this file wins. `docs/state-machine.md` keeps the reasoning and Rob's decisions.

**Changing the rules.** Any change to a rule in this file needs Rob's approval. That includes a new rule that comes from a bug Kani or a test finds. A clarification that changes no rule doesn't need it (Q27).

## Where each guarantee comes from

| Label | Backed by |
|---|---|
| **Compiler** | Rust's exhaustive matching. No catch-all arms, so an unhandled pair of state and event doesn't compile. |
| **Kani** | A bounded proof over the pure kernel, in the form under "How Kani proves the rules". |
| **Witness** | A test that drives the kernel from the start to a given phase. |
| **Simulation** | Deterministic tests of the controller against fakes of GitHub, Mergify, PrismaBuild, the agents and the clock, with a crash injected at every save point. loom covers the controller's concurrent code. |
| **Setting** | GitHub, Mergify and runner configuration, checked by a script against a copy Rob approved. |
| **Agent** | A prompt and an output schema, measured by postmortems. Not proven. |
| **Assumption** | Something about the outside world, listed at the end. |

## The kernel's shape

The kernel is one pure function, `step(world, event) -> (world, effects)`.

The world holds:

- every request's node tree;
- the gates;
- the agent pool;
- the effect outbox;
- the credit ledger.

It is a set of tables indexed by id and stored in `Vec`s, so production has no size bound.

Gates and the agent pool are part of the world. Granting a slot, releasing one and admitting a run are updates inside `step`, made atomically with the event that caused them. They are never effects.

The controller handles each event in three steps:

1. It loads the world and applies the event.
2. In one Postgres transaction, it writes the event, the changed rows and the new effects.
3. It runs the effects.

### Settle

After applying the event, `step` runs `settle`. Settle keeps a worklist of the nodes the event touched. For each node on the list, it re-evaluates that node's conditions against current facts and takes any transition they enable. It then adds every node that transition touched to the worklist: the parent, the waiters and the gate.

Settle takes only forward transitions:

- moving to Stopping on a merge;
- closing once teardown is done;
- leaving Children once every child has closed;
- granting slots and admissions;
- running a pending start once a pause lifts or an incident clears.

Failed checks, review findings, conflicts and Rob's answers are outside events, never stored facts, so settle can't loop through them. Each settle transition lowers the progress measure, defined below, so settle always ends.

### How Kani proves the rules

Kani proves the rules inductively. A harness builds an arbitrary world within the bounds and assumes `valid(world)`. It then applies an arbitrary event and asserts that the rule and `valid` both still hold.

Because the starting world is arbitrary, a counter can start at any value. No harness has to replay a long history to reach a threshold. Thresholds are parameters, and harnesses check them at 1 and 2.

For slice one, the bounds are 2 requests, or 3 after a new one arrives; 2 repos of 4 files each; W at most 3 and the agent pool at most 2; and abstract commit ids. Four-request gate harnesses cover W = 2 and W = 3, including full gates and waiters (Q29 amended, Rob 2026-10-31).

**When the proofs run.** The proofs are a required Mergify check on any change that touches the kernel or its proofs, and a nightly run covers them all. Other changes skip them. Both run as Mergify CI on the fleet's runners, at most 8 proofs at a time (Q9, Q18).

`valid(world)` says:

- every leaf in a holding phase, or in Stopping before its release step, holds a slot and its files, and nothing else does;
- the gate's file locks are exactly the footprints of the leaves holding slots;
- credit is the sum of its ledger entries and is never negative;
- effect keys are unique, and each node has at most one effect in flight;
- the wait graph has no cycle;
- every pause belongs either to a node that is deciding or to Rob;
- every node waiting on Rob has an open question with an id;
- a closed node holds nothing.

The holding phases are Making, Checking, Reviewing, Publishing, Queued and Rebasing.

**The progress measure** sums, over every open node, a distance to Closed, taken from its phase and its position in any effect sequence. It then adds the tokens that settle hasn't consumed yet.

**The progress witness** is a function that names one outside event for any valid world with an open node. The event is one of:

- a successful reply;
- a confirmation;
- a fact, such as an incident clearing;
- an answer from Rob.

Kani proves that applying the witness's event lowers the measure. That is how the kernel shows it has no dead states: from every valid world, some event moves it closer to done.

## Rules

| Rule | Label |
|---|---|
| Every phase handles every event. | Compiler |
| `valid(world)` holds after every step. | Kani |
| Settle ends, and afterwards no forward transition is left untaken. | Kani |
| No dead states: the progress witness always names an event that lowers the progress measure. | Kani |
| From every phase, stop followed by teardown reaches Closed. A teardown step that fails permanently waits on Rob, keeping what it holds. | Kani |
| Closed is reached only from Stopping, once every teardown step has confirmed or Rob has marked it handled. | Kani |
| Every phase can be reached from the start. | Witness |
| Every new operation gets a new generation, so no two operations share an effect key. | Kani |
| A node has at most one effect in flight, and emits the next only when the previous one confirms. | Kani |
| Credit changes only through the ledger entries under "Progress credit", and each entry applies once. | Kani |
| An operation's failures are bounded. Every second failure raises an incident, and only Rob can clear an operation's second incident. | Kani |
| No step adds scope. Each of a child's measures traces to one of its parent's measures, and together the children's measures cover all of them. | Kani for the tracing; Agent for the meaning |
| TL3 enqueues a head only after that exact head has passed its checks and review. | Kani |
| Mergify merges only after CI passes on the exact tree it merges, only with the approval check from TL3's GitHub App, and without rewriting the PR's head. | Setting |
| A merge TL3 observes is recorded truthfully, and the node closes as Merged after teardown. A merge of a head TL3 didn't approve also raises an incident. | Kani |
| A paused leaf starts no new operation. | Kani |
| A decision is checked against current facts before it applies, and it answers every escalation in its frozen batch. | Kani |
| Each of Rob's answers applies only to the question it names, and only once. | Kani |
| Escalations go only to the parent. | Kani |
| A gate grants at most W slots and never gives the same file to two leaves holding slots. | Kani |
| From slice two, each free slot goes to the runnable leaf with the highest rank, with ties going to the older leaf. | Kani |
| Each effect follows its delivery contract. | Kani for the outbox; Simulation for delivery |
| A new kernel version starts from a migrated snapshot, never replays older events, and keeps every pending effect's identity. | Simulation |
| Every run of agent-written code is contained. | Setting |
| Only TL3's GitHub App can push to TL3's branches. | Setting |
| Changes to TL3's core, and to CI and Mergify configuration in served repos, need Rob's approval. | Setting |
| A change to a served repo's test configuration needs a second, independent review that looks only for weakened checks. | Setting for the required check; Agent for the review |

Every rule above is a safety property, except the progress witness. Liveness under the assumptions at the end can be checked later with TLA+.

## Machines

| Machine | One per | Purpose |
|---|---|---|
| Node | Request root, split node, leaf | The work |
| Gate | Repo | Slots, file locks and planning admission |
| Agent pool | Fleet | Admission for every agent run. Slice one uses one account, first come, first served. |
| Effect | Outside action | Delivering one action under its contract |

## Events

| Kind | Examples | Rule |
|---|---|---|
| **Request** | An issue labelled `tl3` by Rob, or a same-repo proposed PR labelled by Rob for adoption (Q45), in a repo TL3 serves | Creates a request whose root starts in Ingesting. Labels added by anyone else are ignored. Rob may allow other people later (Q2). |
| **Reply** | A run's verdict: ingest done, audit verdict, plan, maker done, checks done, review verdict, decision, rebase result | Carries its operation id: node, generation and kind. A reply is stale unless it matches the node's current operation. A reply that breaks its schema, or names the wrong commit, counts as a failed run. |
| **Acknowledgement** | An effect confirmed, failed retryably, or failed permanently | Applied to the effect by its key, whatever the node's current operation is, so a late confirmation always settles its effect. The node acts on it only if it's waiting on that effect. |
| **Fact** | Child closed or escalated, dependency delivered or abandoned, PR merged, PR closed or dequeued by a person, a push by someone other than TL3, incident cleared | Never stale and never dropped. It updates facts in any phase, and settle applies whatever it enables. |
| **Command** | Stop, pause, resume, change the request, an owner's decision, Rob's answer | Carries a command id. A repeated id is ignored. Rob's answer also names the question it answers. |
| **Look** | Liveness or progress went quiet past its allowance | Reads the upstream's own record. See "Looks and stalls". |

**Rob's comments.** An agent reads each comment Rob leaves on an active request's issue. It classifies the comment as one of these: change the request, stop, pause, resume, an answer to the open question, or a remark that needs no action. A remark changes nothing, and anything else enters the kernel as that command or answer. When the agent is unsure, TL3 asks Rob one short question and classifies his reply the same way (Q15). For comments on a closed request, see "Corrections after close".

Workers report liveness, as a heartbeat, and progress, such as new tool results, to a side table. That table is a cache, not state. Losing it loses nothing, because a look reads the upstream itself.

## Node facts

- **Children:** each child's status.
- **Dependencies:** each one names a node and is pending, delivered or abandoned.
- **Measures:** each with its lineage and status: unmet, provisionally met, or confirmed met.
- **Escalations:** the frozen batch, and the queue behind it.
- **PR:** every PR number the node has used, the base commit, the current head, and every head that passed checks and review, with its tree.
- **Adoption:** the proposed head supplied with the request, and the adopted bit, which never resets (Q45).
- **Merge:** once observed, the merged head and tree.
- **Operation:** the generation, attempt and round, and whether the leaf has been re-planned. It also records the failure count and the incidents across the operation's whole lineage of generations.
- **Effect sequence:** the phase's remaining effects, and which one is in flight.
- **Question:** while the node waits on Rob, the question's id, kind and continuation.
- **Resume point:** where a leaf waiting in Ready picks up again.
- **Pending start:** an operation of a node that isn't a leaf, deferred while the node is paused.
- **Pauses:** each pause, with its owner.
- **Footprint:** the declared files, plus a token count measured by code.

## Progress credit

Agreed with Rob on 2026-10-29, with the numbers below confirmed on 2026-10-30 and reviewed after the first 25 requests (Q6). Credit belongs to the request and lives on its root, as a ledger. Each entry is keyed by its node and its reason, so it applies only once.

**Starting credit:** the request's measured size in leaves, plus a reserve of 3. Size in leaves is the root forecast divided by the fit limit, rounded up.

**Debits:**

| Action | Cost |
|---|---|
| A split into k children, where k is at least 2 | k − 1 |
| Out of room. This pays for the wasted attempt; the split that follows is charged like any split. | 1 |
| A new attempt: the second gets a bigger budget, the third a different model | 1 |
| A re-plan after the third attempt, once per leaf | 1 |
| A conflict fix that changes code | 1 |
| A recheck after a clean rebase or a fresh review after reviewer exhaustion, beyond the first `free_rechecks` (3 by default) for the leaf | 1 |
| A footprint extension | 1 |
| A restart, by an owner or by Rob | 1 |
| Re-dividing, in slice two | 1 per child abandoned or added |

**Credits:**

- Each merged leaf earns 1, once, whether it merged before closing or after. A fully delivered tree of L leaves spends L − 1 on its splits and earns L back.
- Rob can add any amount.

**When credit runs short**, a node can't take an action that costs more than the credit left, so it passes up. At the root, Rob is asked in plain language.

Rob's answers never fail for lack of credit. An answer that needs credit adds what it needs first, and the ledger records both the addition and the spend.

## Fit and splits

A node's forecast is computed by code: the token count of its slice of the brief and of its declared files, plus an overhead calibrated from MLflow. The starting constants are set in config.

**A node is a leaf** when all of these hold:

- the forecast fits, under about 100k of the 150k. That is a starting value, tuned later from MLflow data (Q4);
- it is in one repo;
- every measure has a check;
- its footprint avoids protected paths, which in slice one include CI, Mergify and, until its second review arrives, test configuration;
- the planner states that no design questions remain and that the change lands on its own (Agent).

**A split is allowed** for one of three reasons, and the plan must name which:

- the forecast doesn't fit;
- the work spans several repos, in which case it must split by repo, one child per repo;
- a leaf ran out of room.

Every split has at least 2 children. Every child's forecast must be smaller than its parent's, except in a repo split.

**A plan breaks a rule** when it does any of these:

- fails the leaf or split rules;
- returns a leaf when the node must split;
- leaves a measure untraced;
- declares a dependency that would close a cycle in the wait graph.

A plan that breaks a rule is re-run once with the reason. A second failure counts as "can't meet".

**Footprint.** At Checking, code compares the files the commit changed with the footprint, and any change outside it fails the round. A maker that needs more files asks for them while it works.

## Checks

Each leaf's plan lists checks of three kinds, which Rob confirmed on 2026-10-30 (Q40). A fix round after review adds a fourth kind (Q42). Each check names at least one test.

| Kind | Used for | Passes when |
|---|---|---|
| **Demonstrating** | A measure that changes behavior | Each named test fails on the base and passes on the leaf's commit. The base run applies only the leaf's test changes. On the base, the failure must be an assertion, or a missing name the leaf adds. |
| **Regression** | Showing that nothing which worked is now broken | It passes on the leaf's commit. A test already failing on the base is recorded as pre-existing and excused. Base results are cached per base commit. |
| **Behavior-preserving** | A measure that changes no behavior, such as a refactor | It passes on both trees. Review judges the rest. |
| **Reproducing** | A review finding about behavior, fixed in a fix round | Each named test fails on the commit that was reviewed and passes on the fixed commit. The run on the reviewed commit applies only the round's test changes. On the reviewed commit, the failure must be an assertion, or a missing name the round adds. |

In a fix round after review, the maker names the tests that reproduce each finding about behavior. A finding no test can show, such as wording or docs, needs a stated reason instead, which the next review judges. Failed checks and CI failures need no new test, because the failing test already shows the problem.

TL3 owns the check harness, and the code under test can't change it:

- The harness's runner and its test-reporting plugin are mounted read-only, from outside the checkout.
- The test command comes from the repo's test configuration, which a leaf can't change without a second review. See "Containment".
- The harness selects the named tests itself. It collects and parses the report outside the container.
- A named test that is missing, skipped or not collected fails its check.
- One check job runs both trees, and the reviewed commit as a third when the round has reproducing checks.
- The harness commits progress to PrismaBuild after each test, so a long suite stays visibly alive.

A job that produces no report never ran its tests, so it is an infrastructure failure. A demonstrating test that errors on the base for any other reason shows nothing, so the round fails.

## Failures

**Infrastructure failures** are crashes, setup errors, lost workers, stalls and outages. Each operation counts its failures across its whole lineage, through every generation.

**Retries and incidents,** as Rob confirmed on 2026-10-30 (Q7):

- A failure retries in a new generation, resuming from the last checkpoint where there is one.
- Every second failure raises an incident, and the operation waits for "incident cleared" before it retries.
- An operation's first incident clears when a probe of the upstream succeeds, or when Rob clears it.
- Only Rob can clear an operation's later incidents, and he gets a plain-language note asking him to.
- A leaf whose operation waits on an incident gives back its slot and files. It waits in Ready, and resumes at the same step.
- When quota runs out, the operation waits for the reset time the provider announced. That isn't a failure.

Code computes a signature for each failure from the upstream's error class, and uses it only to group incidents.

**Delivery failures** follow the same rule, counted per effect. Every second retryable failure raises an incident, and only Rob can clear the effect's second incident.

**Work failures** are failed checks, review findings and CI test failures. They follow the rules for rounds and attempts under "A failed round".

## Node phases, slice one

Slice one runs one issue whose root plans as a single leaf. The root's owner is Rob, so in slice one every pass-up goes to AwaitingRob. From slice two on, a node with a parent goes to Escalated instead.

Each table lists only the events its phase acts on. Anything else is either a stale reply, or a fact or command covered under "Facts and commands in every phase".

A phase's effects run as a sequence. The kernel emits the first effect, then emits each following one when the one before it confirms.

**A request starts** when Rob labels an issue `tl3` (Q2). Q45 also permits a
proposed PR to seed the request. In #143, only Rob's `tl3` label on a same-repo
PR qualifies. **Confirmed by Rob, 2026-10-31:** its closing keyword
must identify exactly one issue. The issue supplies the goals. The root starts
in Ingesting and requests an ingest run, retaining the proposed head.

### Ingesting (root)

| Event | Condition | Next | Effects |
|---|---|---|---|
| Ingest done (brief and measures, which the ingest agent has checked against the issue) | | Auditing | Request an audit run |
| Ingest proposes an early end | Duplicate, already done, or not actionable | Auditing | Request an audit of that verdict |

### Auditing (root)

An independent agent checks ingest's output against the original issue. It can be the same type of agent as ingest or a different one.

| Event | Condition | Next | Effects |
|---|---|---|---|
| Audit agrees with the measures | | Unplanned | Post the measures to the issue in plain language |
| Audit agrees with an early end | | Stopping(Duplicate, AlreadyDone or NotActionable) | |
| Audit disagrees | First time | Ingesting | Re-run ingest with the audit's objections, in a new generation |
| Audit disagrees | Second time | AwaitingRob(audit disagreement) | Ask Rob, showing both readings |

If Rob keeps ingest's reading, the request continues to Unplanned when the dispute was about measures. When it was about an early end, it goes to Stopping with ingest's proposed outcome.

### Unplanned

Settle admits the node to planning when all three hold:

- it isn't paused;
- nothing it depends on is pending;
- its repo's gate has room, meaning fewer than W leaves are waiting for a slot.

A node that spans several repos is admitted at once, because its plan must split it by repo. Admission moves the node to Planning and requests a plan run.

### Planning

The controller supplies the planner with these inputs, alongside the brief,
goals and forecast settings:

- `must_split`: true after a maker ran out of room or a fresh reviewer ran out of room, otherwise false. A leaf
  reply is rejected when it is true.
- `findings`: everything learned before a re-plan, including failed checks and
  review findings, as a list of text reports. It is empty for initial planning.
  The planner uses these findings when choosing its next plan.
- `protected_paths`: repo-relative glob patterns for files a leaf or split may
  not declare. Code rejects matching declarations. The controller's defaults
  include `.github/**`, `.mergify.yml`, `**/conftest.py`, `pytest.ini`, `tox.ini`,
  `noxfile.py`, `setup.cfg` and `pyproject.toml`. `**` matches zero or more path
  segments, including a root `conftest.py`.

| Event | Condition | Next | Effects |
|---|---|---|---|
| Plan: leaf | The leaf rules hold | Ready(fresh) | |
| Plan: split | Slice one | AwaitingRob(needs splitting) | Ask Rob to narrow the request, showing the proposed pieces |
| Plan: can't meet, with a report | First time | Planning | Re-run with the next model on the planner's ladder, in a new generation |
| Plan: can't meet, with a report | Second time | AwaitingRob(can't meet, continue to Planning) | |
| Plan breaks a rule | First time | Planning | Re-run with the reason, in a new generation |
| Plan breaks a rule | Second time | AwaitingRob(can't meet, continue to Planning) | |

### Ready(resume point)

A leaf waits here for a slot, holding nothing. Settle grants it a slot and its files when all four hold:

- it isn't paused;
- it isn't waiting on an incident;
- the gate has room;
- all its files are free.

**Proposed Q48, needs Rob's approval:** an older waiter blocks a younger leaf if a slot is free for it, or if their footprints overlap. Only truly disjoint leaves pass a file-blocked waiter. A waiter keeps its ticket and holds nothing. A grant moves the leaf straight to its resume point.

| Resume point | Next | Effects, in order |
|---|---|---|
| Fresh, without a proposed head | Making(attempt 1, round 0) | Create the branch from main if it's missing, recording the base commit; create the worktree if it's missing; start the maker run |
| Fresh, with an unadopted proposed head (Q45) | Making(attempt 1, round 0) | Fetch the pinned pull head; create TL3's branch there, recording the main base and a separate start ref; prepare the worktree |
| Making(attempt, round) | That Making | Create the worktree if it's missing; start or resume the maker run |
| Checking | Checking | Bring the worktree up to the branch's head; submit the check job |
| Reviewing | Reviewing | Start the review run |
| Publishing, from a given step | Publishing | Continue the publishing sequence from that step |
| Rebasing | Rebasing | Rebase onto main, in a new generation |

### Making(attempt, round)

The attempt picks the model and budget from the maker's ladder in config (Q22):

- attempt 1 uses the default model and budget;
- attempt 2 uses the same model at maximum effort and budget;
- attempt 3 runs on the strongest model from a different family.

The exact models come from what CLIProxyAPI serves. They are set in config, and model trials adjust them.

| Event | Condition | Next | Effects |
|---|---|---|---|
| Worktree preparation confirms (Q45) | Proposed head present and not yet adopted | Checking | Mark adopted; set head to the proposed head; clear checked head; submit checks. Keep attempt, round and credit unchanged; do not start a maker. |
| Worktree preparation confirms | No proposed head, or already adopted | Making at the next step | Start or resume the maker run |
| Maker done (a commit, and its own check that every measure is met) | | Checking | Submit the check job |
| Maker needs more files (a list) | Credit at least 1, and the forecast with the new files still fits | Ready(the same Making) | Spend 1; record the larger footprint; release the slot and files. The worktree and session stay. |
| Maker needs more files | Otherwise | As for out of room | |
| Maker can't meet, with a report | | The next attempt, as for a failed round 1 | |
| Maker out of room | Credit at least 1 | Planning, which must split | Spend 1; release the slot and files |
| Maker out of room | Credit 0 | AwaitingRob(out of credit, continue to Planning, which must split) | Release the slot and files |

**Confirmed by Rob, 2026-10-31:** the proposed head never changes; it
is pinned at label time. The adopted bit can only become true and is
never reset, including on a re-plan. Repeated preparations cannot readopt the
original head. Adoption holds the normal slot and footprint, uses no agent pool
slot, and earns or spends no maker credit. Subsequent fixes use the existing
failed-round rules.

### Checking(attempt, round)

| Event | Condition | Next | Effects |
|---|---|---|---|
| Checks done | Every check passes for its kind, and no file is outside the footprint | Reviewing | Start the review run |
| Checks done | A check fails for its kind, or a file is outside the footprint | A failed round | |

### Reviewing(attempt, round)

The review runs in its own session, independent of the maker and the planner. It can use the same type of agent or a different one. By default its model comes from a different family than the maker's (Q22). In round 1, the reviewer resumes its round-0 session. If it runs out of room, it gets one fresh conversation on the next model in `models.reviewer`, without resuming the exhausted session (Q44).

Coordinator defaults, for Rob to check: the ladder clamps at its last entry. A clean rebase or outside push to a new head starts a new review and resets its restart flag; it keeps the shared recheck counter. Ordinary fix rounds keep the selected reviewer and its session. A new maker attempt starts with the default reviewer again. The first restart uses the re-review cost rule in the rows below.

| Event | Condition | Next | Effects |
|---|---|---|---|
| Approved (head) | The head is the one that passed the checks | Publishing | Record the head and its tree as approved; start the publishing sequence |
| Changes requested (findings, each tied to a measure) | | A failed round | |
| Reviewer out of room | First time for this review; within `free_rechecks`, or credit at least 1 | Reviewing | Start a fresh conversation, without session resume, on the next reviewer model, clamped at the last entry. Count it as a recheck: free up to `free_rechecks`, then spend 1. |
| Reviewer out of room | First time for this review; beyond `free_rechecks`, and credit 0 | AwaitingRob(out of credit, continue to Ready(Reviewing)) | Release the slot and files; after funding, start the fresh review on the next reviewer model, clamped at the last entry, without session resume |
| Reviewer out of room | Again, on the fresh review; credit at least 1 | Planning, which must split | Spend 1; release the slot and files; set `must_split` true |
| Reviewer out of room | Again, on the fresh review; credit 0 | AwaitingRob(out of credit, continue to Planning, which must split) | Release the slot and files; preserve `must_split` true in the continuation |

### Publishing

The publishing sequence has four steps:

1. Push the branch.
2. Post the approval check on the head.
3. Open the PR, or reopen it.
4. Enqueue it.

When the last step confirms, the leaf moves to Queued.

| Event | Condition | Next | Effects |
|---|---|---|---|
| Conflict with current main | | Rebasing | Rebase onto main, in a new generation (Q46, Rob 2026-10-31) |

Publication guards report a Conflict fact when the checked head conflicts with current main, including before the push or PR opening completes and before enqueue. Rebasing uses the same credit and recheck rules as a conflict in Queued. A clean main advance needs no rebase; publish the checked head and let Mergify update the merge tree.

### A failed round

- **Round 0:** go to Making(round 1), and resume the maker's session with the evidence.
- **Round 1 fails, or the maker says it can't meet:** move to the next step below. Each step costs 1. Each new attempt runs in a fresh session, starting from the branch, with every finding so far.
  - After attempt 1: Making(attempt 2, round 0), with a bigger budget.
  - After attempt 2: Making(attempt 3, round 0), on the next model in the maker's ladder.
  - After attempt 3, if the leaf hasn't been re-planned: release the slot and files, and go to Planning with every finding in the planner's `findings` input. `must_split` is false unless the maker or the fresh reviewer ran out of room. The new plan starts again at attempt 1.
  - After attempt 3 of a re-planned leaf: release the slot and files, and go to AwaitingRob(can't meet, continue to Ready(Making, new attempt)).
- **Not enough credit for the next step:** release the slot and files, and go to AwaitingRob(out of credit, continue to that step).

### Queued

| Event | Condition | Next | Effects |
|---|---|---|---|
| CI failed | A test failed on the code | A failed round | Dequeue |
| CI failed | The runner or its setup failed | See "Failures" | Enqueue again, when the failure rules allow |
| Conflict | | Rebasing | Rebase onto main, in a new generation |
| GitHub or Mergify stalled | No incident is already open | Queued, waiting on an incident | Raise an incident. A probe or Rob clears the first; only Rob clears later incidents. See "Looks and stalls" and Q7. |

### Rebasing

| Event | Condition | Next | Effects |
|---|---|---|---|
| Rebase clean | | Checking, same attempt and round | Submit the check job on the new head. Review runs again afterwards, because an approval covers one exact commit (Q41). This is free up to `free_rechecks` per leaf, and costs 1 each time after that. |
| Rebase needs changes | Credit at least 1 | Making, same attempt and round | Spend 1. Checks and review run again. |
| Rebase needs changes | Credit 0 | AwaitingRob(out of credit, continue to Ready(Making, same attempt and round)) | Release the slot and files |

### AwaitingRob(question)

Rob said on 2026-10-29: "I want as much resolved WITHOUT ME as possible. I will be the bottleneck." A question blocks work, so TL3 asks him one only when it can't carry on safely without him:

- to change what he asked for;
- for something only he can do;
- when a request's credit runs out.

Everything else, TL3 decides itself. A note is a plain-language message on the issue that needs no answer. TL3 sends one only when something changes the request's course (Q8):

- a stop or a pause;
- a merge TL3 didn't approve;
- a PR Rob closed or took out of the queue;
- a push by someone other than TL3;
- a re-plan.

Routine steps, such as fix rounds, new attempts, model switches and rebases, show only in the status view. Questions and notes about a request go on its issue. Incidents go to one pinned "TL3 operations" issue in TL3's own repo until Discord exists, because they often span several requests (Q17).

A question has an id, a kind and a continuation. The continuation is one of these:

- Unplanned;
- Planning, which must split where noted;
- Ready, with a resume point;
- the same phase, for a node that holds no slot;
- Stopping, with an outcome.

The node holds no slot and no files while it waits. Each question is in plain language, and offers only the answers listed for its kind. An answer naming another question, or giving an answer its question doesn't offer, is recorded and ignored.

| Question | Answers it offers |
|---|---|
| Audit disagreement | Continue (ingest's reading stands), change the request, stop |
| Needs splitting (slice one only) | Change the request, stop |
| Can't meet | Try again with guidance, change the request, stop |
| Out of credit | Add credit, change the request, stop |
| Effect failed | Continue (Rob has fixed the cause), stop |
| Cleanup can't finish | Retry (Rob has fixed the cause), mark the step handled |
| Unclear answer | The same answers as the question it clarifies |

| Answer | Next | Effects |
|---|---|---|
| Continue | The continuation | |
| Try again with guidance | The continuation. A continuation into Making starts over at attempt 1, round 0. | Add Rob's guidance to the brief; spend 1 |
| Add credit (an amount) | The continuation | Add the credit; take the debit the continuation needs |
| Change the request (new text) | Stopping(Replaced) | File a new request, linked to the same issue, with the new text. It starts on its own, under Rob's original label (Q14). |
| Stop | Stopping(Stopped) | |
| Unclear | AwaitingRob(unclear answer) | Ask one clarifying question. If the second answer is unclear too, the question stays open. |

### Stopping(outcome)

Teardown is a sequence. It runs even while the node is paused, and each step is fenced by the node's current generation:

1. Stop the node's runs, and confirm they've stopped.
2. Withdraw its PrismaBuild jobs.
3. Cancel its outstanding effects, and wait until each one is final.
4. Dequeue its PR and close it.
5. Stop its open children.
6. Release its slot and files.
7. Remove the worktree, then delete the branch. Deleting a branch closes any PR still on it.

Each effect's contract lists which answers mean the target is already gone, such as "not found", and those confirm at once. The PR step confirms with the PR's final state. If the PR merged, the outcome becomes Merged.

A step can fail permanently, for example with a permission error. When that happens, the node stays in Stopping and keeps everything it hasn't released yet. TL3 raises an incident and asks Rob, in plain language, to either fix the cause and retry the step, or mark the step handled. That question has an id, like any other.

Settle moves the node to Closed once two things hold:

- every step has confirmed or been marked handled;
- every child has closed.

### Closed(outcome)

The outcome is one of these: Merged, Done, AcceptedPartial, Stopped, Abandoned, Replaced, Duplicate, AlreadyDone or NotActionable.

Closed is final. It accepts only two kinds of event: a late merge, covered under "Corrections after close", and looks. Every other event is recorded and ignored.

## Facts and commands in every phase

| Event | Facts | Then |
|---|---|---|
| PR merged | Record the merged head and tree | From any phase except Stopping and Closed, go to Stopping(Merged). In Stopping, the outcome becomes Merged. If the merged head wasn't approved, or the required checks didn't pass on the merged tree, also raise an incident and tell Rob. |
| Push by someone other than TL3 | Record the new head | In Publishing or Queued: dequeue, release the slot and files, and go to Ready(Checking), so the new head is tested and reviewed. This counts against the leaf's free rechecks, and Rob gets a note. |
| PR closed by a person | Record it | Stopping(Stopped), from any phase except Stopping and Closed. Rob gets a note saying how to start the request again. |
| PR dequeued by a person | Record it | In Queued: release the slot and files, go to Ready(Publishing from step 4), and add a pause owned by Rob. Rob gets a note. Resuming puts the PR back in the queue. |
| Effect permanently failed | Raise an incident | Outside Stopping: release any slot and files, and go to AwaitingRob(effect failed). The continuation retries the failed effect and carries on. A leaf that held a slot carries on through Ready; any other node returns to the same phase. For Stopping, see Stopping. |
| Dependency delivered or abandoned | Mark it | Settle decides what follows |
| Incident cleared | Clear the wait | The waiting operation runs again, first requesting a slot if it gave one back |
| Run failed | | See "Failures" |
| Look | Read the upstream's record | What the look finds enters as replies, acknowledgements and facts |
| Pause, with an owner | Add the pause | See "Pausing" below |
| Resume, or the owner releases its pause | Remove the pause | Once no pause remains, settle requests a slot for a leaf in Ready, or runs the pending start |
| Change the request (Rob, any time) | | Stopping(Replaced), and a new request linked to the same issue, which starts on its own (Q14) |
| Stop or abandon | | Stopping(Stopped) or Stopping(Abandoned), from any phase except Stopping and Closed |

**Pausing.** While a pause is in place, no new operation starts. What happens to work already under way depends on where the node is:

- **A leaf with a run or effect in flight** lets it finish. It then releases its slot and files, and waits in Ready, resuming at the next step.
- **A leaf whose finished effect put its PR in the queue** stays in Queued instead, because a PR already in the queue can still merge.
- **A leaf in Queued** stays too. If the queue sends it back, it waits in Ready instead of starting the fix or the rebase.
- **A leaf with nothing in flight** releases its slot and files at once, and cancels any run it requested that hasn't started.
- **A node in any other phase** stores its next start as its pending start.

## The closing function

Every move into Closed runs `close(outcome)`. It:

- adds the merged-leaf credit to the ledger, for a Merged leaf;
- releases every pause this node owns;
- tells the parent the outcome;
- for a root, posts a plain-language summary to the issue, listing any unmet measures, and closes the issue. After Replaced, the issue stays open for the new request;
- tells waiting nodes whether the work was delivered (Merged or Done) or abandoned;
- records the code grades: rounds, attempts, credit spent, outcome, and time from start to merge;
- from slice three on, marks a judge postmortem as owed.

## Corrections after close

**A late merge** can arrive after a node closed as Stopped, Abandoned or Replaced. It runs `correct(Merged)`, which:

- changes the outcome to Merged;
- adds the merged-leaf credit to the ledger, once;
- tells the parent, the waiters and any replacing request that the work was delivered after all;
- posts a plain-language note to the issue;
- raises an incident that records the race.

**Something wrong after close** comes back as a correction request, linked to the old one. It can include a revert. It is filed in either of two ways:

- Rob comments on the closed request that something is wrong. The correction is filed automatically and carries his authorization, so it starts like a request he labelled (Q21).
- Rob approves a finding that files it.

## The effect machine

| State | Event | Next |
|---|---|---|
| Intended | Picked up | Sent |
| Sent | Confirmed | Confirmed (final) |
| Sent | Retryable failure | Intended, counted under "Failures" |
| Sent | Permanent failure | Failed (final). This raises an incident, and the node gets "effect permanently failed". |
| Sent | Crash or no answer | Uncertain |
| Uncertain | Reconciliation finds it | Confirmed |
| Uncertain | Reconciliation finds it absent | Intended |
| Intended | Cancelled, or its epoch is old | Cancelled (final) |
| Sent or Uncertain | Cancelled, or its epoch is old | Cancelling |
| Cancelling | The executor's attempt has ended, and reconciliation finds it | Confirmed (final), and teardown undoes it |
| Cancelling | The executor's attempt has ended, and reconciliation finds it absent | Cancelled (final) |

An executor's attempt has ended once it replies, or once its process or epoch has ended. Acknowledgements are applied by effect key, so a late confirmation always reaches its effect.

TL3 promises deduplication, not exactly-once delivery. Each effect has a delivery contract:

| Effect | Contract | How reconciliation finds it | Counts as already gone |
|---|---|---|---|
| Submit a PrismaBuild job | Once | The action key | |
| Withdraw a PrismaBuild job | Converges | The job's state | The job is finished or unknown |
| Approval check | Once per head | The check run's external id | |
| Push a branch | Converges | The branch's head on GitHub | |
| Open or reopen a PR | Converges | Any PR for the branch, open or closed | |
| Enqueue, dequeue | Converges | The PR's queue state | Dequeue: the PR isn't queued |
| Close a PR | Converges | The PR's state | The PR is closed or merged |
| Worktree, branch | Converges | Whether they exist | Missing |
| Start or stop an agent run | Once per operation | The worker supervisor's record for the operation id | Stop: the run has ended |
| Comment, or message to Rob | At least once, then deduplicated | A hidden marker. In a rare race, a message can post twice. The next reconciliation deletes the extra copy. | |

A request that times out can still land later. Teardown deletes the branch last, which closes any PR left on it. After every restart, a look checks closed nodes for leftover branches and PRs.

Every effect carries the controller's epoch, and executors refuse effects from an older epoch.

## Gate (one per repo)

The gate is part of the world, so its changes are atomic with the step that makes them.

**Slice one (Q29 amended, Rob 2026-10-31):** W is configurable from 1 through 3. Rob approved raising W from 1 to 3 slots per repo once built and proven.

A leaf gets its slot and all its files together. **Proposed Q48, needs Rob's approval:** an older waiter blocks a younger leaf if a slot is free for it, or if their footprints overlap. Only truly disjoint leaves pass a file-blocked waiter. A leaf waiting for a slot holds neither a slot nor files and keeps its ticket.

**Slice two:**

- W is 2 per repo to start, tuned later from measured capacity (Q29).

  Q29 amended (Rob, 2026-10-31): W may be 1 to 3; the approved slice-one deployment value is 3.
- A queued leaf is eligible when it isn't paused and isn't waiting on an incident. A leaf that isn't eligible reserves nothing.
- A leaf is runnable when it is eligible and all its files are free.
- Each free slot goes to the runnable leaf with the highest rank, with ties going to the older leaf. The rank is defined under "Status, bottlenecks and ranking". When two leaves want the same file, the one that unblocks more gets it (Q30, Q36, Q38).
- There is no rule against starvation. Rob chose to always pull the work that blocks the most (Q30).
- Planning is admitted when fewer than W leaves are waiting for a slot.

## Agent pool

Slice one uses one account, and serves runs first come, first served.

From slice two on:

- Runs are granted in the gate's order, by rank with ties going to the older run, within each account's capacity (Q39).
- Postmortem runs come after every delivery run, so they use only spare capacity. Under constant load they wait (Q39).
- A quota reset the provider announces is a wake-up event.

## Looks and stalls

Each pending operation has two allowances, set in config for each upstream: one for silence and one for no progress. When either runs out, a look reads the upstream's own record:

| Upstream | What the look reads | Progress means |
|---|---|---|
| Agent worker | The supervisor's record, and the record of any outside job the agent has declared it is waiting on | New tool results, or an outside job the agent has declared it is waiting on (such as a PrismaBuild test run) that, by that job's own record, has neither ended nor been lost (Q43) |
| PrismaBuild | The job's record | Tests completed, as committed by TL3's harness under PrismaBuild's progress contract |
| Mergify and GitHub | The PR, its checks and the queue | Checks finishing, or the queue moving |

What the look finds enters the kernel as replies, acknowledgements and facts.

**A stall** is an operation that is alive, but whose progress hasn't moved since the previous look. What happens next depends on the upstream:

- **Agent runs:** TL3 stops the run and counts it as an infrastructure failure.
- **PrismaBuild jobs:** TL3 sets no deadline on them. Under PrismaBuild's progress contract, PrismaBuild ends a quiet job itself, and the ending arrives as a failed run.
- **Mergify and GitHub:** the stall raises an incident.

A look ends work only when the upstream's own record shows no progress.

After a restart or a gap in webhooks, every pending operation gets a look.

## Containment

Every run of code that an agent wrote or changed is treated as untrusted. That includes checks, PrismaBuild commands an agent requests, and CI.

Wherever it runs, untrusted code gets:

- an isolated container with explicit mounts only: the checkout, plus TL3's harness, read-only;
- no shared fleet mount;
- no host sockets;
- no credentials inside the process;
- on PrismaBuild, no network;
- in CI, only what the job needs, and no access to the fleet's internal network.

On PrismaBuild, agents can submit only through this profile. PB #2403 will make it PrismaBuild's own default.

Mergify runners use short-lived tokens with minimal scopes. The runner holds those tokens, not the job's code.

In every served repo, changes to CI, Mergify and runner configuration need Rob's approval. GitHub code owners and branch protection enforce this.

A change to test configuration needs a second, independent review instead, aimed only at whether it weakens any check, and Rob gets a note (Q5). That review arrives in slice two. Until then, leaves keep out of test configuration.

**Test configuration** means the files that decide which tests run or how their results are reported (Q16):

- pytest's settings, wherever pytest reads them, such as `pytest.ini` or the pytest section of `pyproject.toml`;
- `conftest.py` files;
- tox and nox files;
- test scripts that CI calls.

The tests themselves are not test configuration.

## Upgrades

1. Stop starting new operations.
2. Wait until every effect that is Sent or Uncertain reaches a final state.
3. Snapshot the world.
4. Migrate the snapshot with an explicit, tested function.
5. Start the new kernel with a new epoch. Every effect that is still Intended keeps its key, and is delivered under the new epoch.

A newer kernel never replays older events. They stay for audit.

## Status, bottlenecks and ranking

**Slice one** ships a plain status view: a read-only web page on the LAN, served from dl380g10, and a `tl3 status` command. It shows each request's phase, what it is waiting on, its credit, its last progress and any open incidents (Q24).

**From slice two**, code recomputes two things from the wait graph after every event:

- **Rank:** the number of other nodes waiting on a node, directly or through a chain of waits. Higher ranks go first, and ties go to the older node (Q36). The wait graph has no cycles, so the count is always finite.
- **Wait reason:** what every open node is waiting on.

There are no manual priorities in slices one and two. A label such as `tl3-urgent` could be added later if Rob finds he wants one (Q37). In slice one there are no dependencies between leaves. Leaves can wait on a file holder or an older gate ticket; the proposed Q48 gate protects older runnable or overlapping waiters. Ranking starts in slice two.

File waits and pauses count in the wait graph, and neither can close a cycle:

- Only leaves with no pending dependencies wait on files. A file wait always points at the leaf holding that file, which holds a slot.
- A pause always points at a deciding ancestor, which waits only for its decision run.

## Assumptions

- Outside services either answer or keep a record TL3 can read.
- Records from PrismaBuild, GitHub and Mergify are authentic.
- CLIProxyAPI routes each call to the model requested.
- Rob eventually answers.
- Mergify merges only through the queue, with the required checks, and doesn't rewrite queued PRs. This needs confirming when Mergify is wired.
- GitHub closes a PR when its branch is deleted.
- Code under test doesn't forge its own test reports. pytest's reporter runs in the tests' own interpreter, so the harness can't rule it out. Rob decided on 2026-10-30 that hostile code isn't a concern at this point. Tamper-proof reports wait for PB #2403.

## Staging

**One kind of request.** Slices one and two take only one kind of request: a code change in one repo (Q1). Research questions, reviews of existing PRs and work spanning several repos come after slice two is solid, each with its own grilling and definition of done (Q35).

**A request is done** when its PR merges, the checks and the independent review confirm every goal, and TL3 posts a plain-language summary to the issue and closes it. A goal left unmet means the request isn't done, unless Rob accepts it as is, which arrives in slice two (Q12).

**Slice one** runs one issue as one leaf, and serves only `RobTand/tl3-sandbox` (Q13). It includes:

- every phase under "Node phases, slice one";
- Rob's pause, resume, stop and request changes, by answer or by comment;
- one gate with an approved W of 3, with the proposed Q48 disjoint-overtaking rule pending Rob's approval;
- one model account, first come, first served;
- the effect machine, progress credit, looks and stalls, and containment;
- the plain status view, and code grades.

It leaves out ranking, bottleneck scoring, Hindsight writes and the orphan sweep.

**Slice one is done** when 10 real sandbox issues go from Rob's label to merged with no question to Rob, and a fault-injection run recovers every time (Q11).

- **Seed issues.** 12 issues on the sandbox, written for Rob to label: small bug fixes, a small feature, a refactor, two issues that conflict with each other, one too big to fit, which tests narrowing, and one duplicate (Q19).
- **Fault drills.** Kill the controller mid-step, kill an agent worker, drop GitHub webhooks, make Mergify CI flaky, force a merge conflict and lose a PrismaBuild worker. A box reboot is simulated by restarting every TL3 service, not by rebooting dl380g10, because of its kernel bug (Q20).

**Before the controller is built,** a kernel spike tests whether the rules above can be proven as stated, at the stated bounds, in reasonable time. The spike is slice one's kernel in pure Rust, with its Kani harnesses, run on PrismaBuild.

**Before the sandbox trial,** slice one needs:

- the dedicated Postgres instance;
- a GitHub App for TL3. Its manifest is prepared so Rob only clicks "create" in his browser. It asks for issues, pull requests, checks and contents, plus read-only metadata, and subscribes to webhooks for issues, comments, pull requests, checks and the merge queue. It is installed on `tl3-sandbox` only at first (Q26).
- Mergify config and runners on the sandbox. They use minimal tokens, require the approval check from TL3's app, and never rewrite PR heads.
- a branch ruleset, so only TL3's app can push to TL3's branches;
- code owners on the sandbox's CI and Mergify configuration;
- the pinned "TL3 operations" issue for incidents (Q17);
- the agent that classifies Rob's comments (Q15);
- the webhook tunnel;
- issue comments as Rob's answer channel, until Discord;
- a worker supervisor whose starts and stops are idempotent per operation;
- a tokenizer and the starting forecast constants;
- TL3's check harness, and PrismaBuild's container profile for agent code;
- the Kani harnesses, the witnesses, and the crash and race simulations, all passing on PrismaBuild;
- the proofs wired as a required Mergify check on kernel and proof changes, with a nightly full run (Q9, Q18).

**Slice two** adds:

- one real repo, which Rob picks when slice two starts (Q13);
- splits, after a dry run that decomposes a handful of real requests, changing no code, to calibrate the fit limit and the forecast (Q34);
- the parent phases and Escalated;
- dependencies;
- pauses owned by deciders;
- the slice-two gate, the multi-account agent pool, ranking and bottleneck scoring;
- recursive escalation;
- decision batches checked against current facts;
- "accept as is";
- an approval path for changes to protected paths, and the second review for test configuration (Q5).

**Slice three** adds:

- judge postmortems, which reuse TL1's rubric and judge (Q32);
- model trials, on 1 run in 10 and never on work that other work is waiting on (Q32);
- proposals to Rob, once 3 postmortems point at the same change (Q32);
- MLflow's prompt registry, which takes the role prompts over from TL3's repo (Q23);
- writing back to Hindsight.

**After slice two:** TL3's own repo as a served repo (Q13), and the other kinds of request (Q35).

**Later:** earned autonomy, once Rob grants a kind of change (Q33); adaptive W; critical-path analysis.

## Slice two phases

These phases carry forward with the review fixes:

- facts are handled everywhere;
- settle runs after every transition;
- a frozen batch is checked against current facts before it applies;
- every structural change is checked for cycles.

| Phase | Waits for | Moves on when |
|---|---|---|
| Children | Its children | Settle sees the last child close. The node goes to Integrating when every measure is covered, and to Deciding when one isn't. An escalation freezes the queue as a batch and moves the node to Deciding. |
| Deciding | A decision run | The decision is checked against current facts and credit, and it must keep the wait graph acyclic. A valid decision applies, and the node goes to Children. Otherwise the node passes up, to Escalated, or at the root to AwaitingRob. Leaving Deciding either way releases the decider's pauses. |
| Integrating | Its own checks, and its owner's review | Pass: Stopping(Done). Fail: Deciding. |
| Escalated | The owner's decision | Restart, which costs 1: Ready, as a new attempt. Split: Planning, which must split. Abandon: Stopping(Abandoned). The node holds nothing while it waits. |

Three more slice-two rules:

- A valid split in Planning spends k − 1 credit, creates the children in Unplanned, and moves the node to Children.
- A leaf that gains a dependency leaves the gate until the dependency is delivered.
- At the root, Rob's questions also offer "accept as is". It goes to Stopping(AcceptedPartial), with the unmet measures listed.
