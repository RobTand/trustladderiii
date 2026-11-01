# TL3 state machine

Rob and I have been designing this one piece at a time since 2026-10-29. Each section says whether Rob has agreed to it or whether it is still a proposal. On 2026-10-29 and 30, I grilled him on every open branch of the specs; his 41 decisions are listed under "Decisions from the grilling", and the text cites them as Q1 to Q41. Two later decisions that day, Q42 and Q43, and Q44, Q45, Q46 and the amendment to Q29 from 2026-10-31 are listed with them, along with Q48 from 2026-11-01. The stack it runs on is in `docs/stack.md`.

**Precedence.** `docs/state-table.md` is the source of truth for the kernel: its phases, events, rules and numbers. This document keeps the reasoning and Rob's decisions. Where the two disagree, the state table wins, and the disagreement is a bug in this document.

## Shape

The kernel is the state machine, a pure function `step(world, event) -> (world, effects)`. Agent work, PrismaBuild jobs and CI run in parallel as effects. Decisions run in order, because the controller feeds the kernel one event at a time.

A node is a unit of work: a request's root, a split node, or a leaf. Each node moves through phases, and each phase is one of four kinds:

- **Agent:** the phase starts an agent run.
- **Decision:** the agent's verdict, chosen from a fixed list, picks the next phase.
- **Deterministic:** the phase runs code with an outside effect, such as merging or deleting a branch.
- **Wait:** the phase ends only on an outside event.

## Ingest

Agreed. Ingest is the first step. It collects the problem state, environment reference information and the system's design and architecture, and it queries Hindsight for relevant facts. A request can be a whole project or a small task.

Proposed details:

- The brief is capped at about 20k tokens, because every agent below carries it.
- Code does the fetching, and the agent condenses.
- The brief records which Hindsight memories went into it.
- Ingest can propose ending a request early, as a duplicate, as already done, or as not actionable. An independent audit must agree before the request closes.

## Decompose

Agreed:

- **Split lazily.** A node splits one level at a time, and only when it is about to be worked.
- **Scope never grows.** A split divides the parent's acceptance criteria among its children, and nothing else changes scope. Anything new found along the way becomes a finding, not scope.
- **Bound rework by progress, not by size.** Rob doesn't want hard limits, because some tasks will be massive. Rework spends progress credit, and only merged work earns it back, so a request can grow as large as its delivered work justifies. Rob agreed to this on 2026-10-29. The state table has the details.

Agreed for the fit limit (Q4); the rest is proposed:

- Split into the largest piece that fits, not the smallest.
- Every split has at least 2 children, and there is no upper limit.
- Code measures each child's forecast. Every child must be smaller than its parent, except when the split is by repo.
- A node is a leaf when four things hold:
  - **It fits:** forecast peak context is under about 100k of the 150k. The 100k is a starting value, tuned later from MLflow data. Slice one can't split yet, so it asks Rob to narrow a request that doesn't fit (Q4).
  - **It is checkable:** each measure has a check.
  - **It is decided:** no design questions remain.
  - **It lands alone:** it is one reviewable change that keeps main passing on its own, in one repo.
- A leaf that runs out of room partway through stops at about 85% of its context and splits.
- Work that spans several repos splits by repo first, whatever its size. That kind of request comes after slice two (Q1, Q35).

## Similar work

Agreed. There is no recompose step and no absorption.

- **Reuse links.** The decomposer's brief lists what is being built in the repo right now. The decomposer writes reuse links into its plan in the same call. A node with a reuse link depends on the node it names. It waits for that node to deliver, then builds on its code.
- **File locks.** File locks at each repo's admission gate make same-file work run one at a time.
- **Review.** Review flags any PR that reinvents something already in main.

## Dependencies without deadlock

Agreed: one node can wait on another, and waits must never stall the system.

Proposed rules:

1. **The wait graph never has a cycle.** Parents wait on their children, and nodes wait on their dependencies. Every plan and every decision is checked against the whole graph before it applies, and one that would close a cycle is rejected. An age rule alone isn't enough. Sol showed a cycle that forms when an older node later splits.
2. **Waiting holds nothing.** A leaf takes its slot and all its file locks together, and only once everything it depends on has been delivered. A leaf waiting on a dependency, on Rob or on an incident holds no slot and no lock.
3. **A started leaf never gains a dependency.** A slot is only held by work that is progressing on its own (an agent run, PrismaBuild, review or the merge queue), and each of those eventually ends. A leaf still waiting for a slot that gains a dependency leaves the gate until the dependency is delivered.
4. **Every ending is announced to the nodes waiting on it.**
   - **Delivered (merged or done):** the waiting nodes are released.
   - **Abandoned, stopped or replaced:** each waiting node gets an event, and its owner replans.
   - **Waiting on Rob or on an incident:** the waiting nodes stay blocked, and the status view shows the reason.
5. **What unblocks the most goes first.** Rob decided on 2026-10-30: "we should be pulling work off that blocks the most other work. I don't really care about starvation." (Q30) From slice two, the gate grants each free slot to the runnable leaf with the highest rank: the number of other nodes waiting on it, directly or through a chain of waits. Ties go to the older leaf (Q36). There is no rule against starvation. When two leaves want the same file, the one that unblocks more gets it (Q38). Paused leaves reserve nothing. Slice one serves leaves in ticket order, with 3 slots approved under the Q29 amendment of 2026-10-31. Q48 (Rob, 2026-11-01) lets truly disjoint younger leaves pass a blocked waiter while protecting every older runnable or overlapping waiter. The state table has the exact rule.

### Priority inheritance

Agreed, in counting form (Q30, Q36, Q37). Rob raised this on 2026-10-29, and TL2 had the same directive. There are no manual priorities in slices one and two, so a node's rank is simply the number of other nodes waiting on it, directly or further along the chain. A predecessor that many nodes wait on ranks high, which is the inheritance. The wait graph has no cycles, so the count is always finite.

The rank applies at three points:

- **Admission:** a ready predecessor gets the next free slot.
- **The merge queue:** a predecessor's PR can be given a higher Mergify queue priority.
- **Incidents:** a blocked predecessor's incident is ranked by its rank.

## Review

Agreed on 2026-10-29. Rob asked for self-review, adversarial review against predefined measures of success, and a path for a measure that can't be met to flow up to the parent. "Accept with the unmet measures listed" is an allowed outcome at the root.

**Measures of success.**

- A node gets its measures when it is created, and only their owner can change them. A leaf's measures come from its parent's split, and a request's measures come from its issue, checked by an independent audit.
- A measure is checked by a machine (a test, a PrismaBuild run or a CI check) or judged by the reviewer.
- Each repo also has standing measures that every leaf must meet. Examples: tests must be able to fail, and a PR must not reinvent something already in main.

**Self-review** is the last step of the maker's run. The maker reports each measure as met, with evidence, or says it can't be met and why. The kernel checks machine evidence itself and never takes the maker's word for it. An early "can't meet" costs less than a review round.

**The owner reviews.** Agreed: whoever set a measure owns its review. A parent owns its children's review, and it decides what happens when a child fails. The review runs as an independent session, separate from both the maker and the planner. Rob said on 2026-10-29 that the agent doing the work should try to verify its own completion, and that another agent will always verify it independently. That agent can be the same type or a different one. By default the reviewer's model comes from a different family than the maker's (Q22). MLflow records which pairs of maker and reviewer catch the most. The reviewer:

- sees the measures, the diff and the check evidence, but not the maker's reasoning, and tries to show that a measure isn't met;
- can block only on the leaf's measures and the repo's standing measures. Anything else becomes a finding, which doesn't block. That keeps review from moving the goalposts.
- approves an exact commit. TL3 then posts its approval check on that commit, and Mergify requires that check from TL3's GitHub App.

**Checks must be able to fail.** Proposed after review round 2, and agreed by Rob on 2026-10-30 (Q40). A check that only passes proves nothing, but demanding that every check fail on the old code would reject the normal case, because existing tests pass there. So each check is one of three kinds:

- **Demonstrating:** a new test that fails on the old code with only the new tests added, by a real assertion rather than a setup error, and passes on the new code.
- **Regression:** existing tests that must still pass. Failures that already happen on the old code are excused.
- **Behavior-preserving:** for refactors, the tests pass on both versions, and review judges the rest.

TL3 owns the test harness, and test configuration is protected, so the code under test can't fake its own results. A named test that is missing or skipped fails. The state table has the exact rule.

**Review fixes start with a failing test.** Agreed by Rob on 2026-10-30 (Q42), when he asked for tests to drive both building TL3 and the work TL3 does. When review finds a behavior problem, nothing showed that the fix round's new test would have caught it. So a fix round names a reproducing test for each such finding, which must fail on the commit the reviewer saw and pass on the fix. A finding no test can show, such as wording or docs, needs a stated reason, which the next review judges. Failed checks and CI failures need no new test, because their failing test already shows the problem. The maker's prompt also tells it to write each plan's tests first and see them fail before it writes the code.

**Failure types.** Agreed on 2026-10-29. Each kind of failure has its own path:

- **Out of room:** the leaf splits right away, because a retry with a bigger budget would only use more context.
- **Wrong work:** the fix round, then attempts with a bigger budget and then a different model, then a re-plan.
- **Can't meet:** the maker's "can't meet" climbs the same ladder, because a stronger model may succeed. Only a goal that no plan can meet reaches the owner.
- **Crash, quota or outage:** the step is retried, or waits, without using up an attempt. Every second failure of the same step raises an incident. A healthy probe can clear the first incident, but only Rob can clear later ones, so a crash loop can't run forever.

**Attempts.** Agreed:

1. **First attempt** with the default model and budget, then the owner's review.
2. **One fix round.** The feedback goes back to the maker's own session, so its context is kept, and the owner reviews again.
3. **Second attempt.** If the fix fails review, the leaf tries again with a bigger budget for that attempt. This spends 1 progress credit.
4. **Third attempt.** Rob said on 2026-10-29 that a failed review can also send the work back on a different model. If the second attempt fails, the third runs on the next model in the maker's ladder, which is set in config. It spends 1 more credit.

The ladder (Q22): attempts 1 and 2 use the default maker model, with attempt 2 at maximum effort and budget. Attempt 3 switches to the strongest model from a different family. The exact models come from what CLIProxyAPI serves, and the model trials adjust them.

Proposed:

- **How a new attempt starts.** It runs in a fresh session, beginning from the leaf's branch, with the measures, the brief and every finding so far. Each attempt gets its own single fix round.
- **One re-plan.** If the third attempt fails, TL3 re-plans the leaf once with everything it has learned, for 1 credit. The new plan starts again at attempt 1.
- **The reviewer resumes its own session** for the re-review in round 1, so it checks exactly what it asked for.
- **Approval binds to the exact commit reviewed.** Agreed by Rob on 2026-10-30 (Q41). CI must also pass on the exact tree being merged. Any new commit, including one from a clean rebase, goes through the checks and the review again. Round 3 showed that identical changed lines can behave differently once the code around them moves. The first three rechecks per leaf are free, and later ones spend credit. Only TL3's app can push to TL3's branches, and Mergify must not rewrite a queued PR.
- **Effort is tracked.** MLflow records the effort level of every attempt, which shows over time whether raising it rescues anything.
- **If the re-planned leaf fails too, the owner decides.** It has seen every attempt and has three choices:
  - **Split the leaf** into smaller leaves, if the attempts made partial progress.
  - **Re-divide its measures,** if the same measure failed the same way twice.
  - **Pass the impact up.**

  Splitting and re-dividing spend progress credit, as the state table lists. At a request's root, TL3 makes these choices on Rob's behalf, because none of them changes what he asked for. Only passing the impact up reaches him.

**Escalation.** Agreed: escalation runs up the chain of owners, one level at a time. Each owner resolves what it can at its own level, which keeps the right owner involved in each decision.

- A leaf can't change its own measures. A parent can re-divide its measures among its children but can't change them. Only Rob can change a request's measures.
- **Insulation.** Each level passes up only the impact, in its owner's terms, never the details of why something failed. A parent doesn't need to know why a child failed, only what it means for the parent's ongoing work and whether the parent needs to replan.
- While a parent replans, the leaves derived from the failed measure, and any leaves that depend on the escalating leaf, start no new operation. Work that is already running finishes, and a PR already in the queue can still merge. A merge can't be undone, so it is always recorded as a fact. Other branches keep working. Each pause belongs to the deciding parent, which releases it when it decides, passes up or closes.
- **Correlated failures pull evidence up.** When two or more children fail on measures that trace back to the same parent measure, the decision run also gets a bounded bundle of evidence. Short summaries could otherwise hide a shared cause, such as a bad brief. Merged work stays, and the new plan builds on main. Abandoned children give up their slots and file locks and notify the nodes waiting on them.
- A deciding parent never waits on its children, so the pause can't deadlock.
- A parent that can't meet its own measure escalates to its own parent, and so on up the tree. At the root, Rob decides: change the request, add progress credit and continue, accept it with the unmet measures listed, or stop.

**Integration.** After its last child closes, a parent's own measures go through the same checks and adversarial review. A failure there triggers a replan.

### Recursive escalation

Proposed. Escalation recurses because measures are owned per level: only the level that set a measure can change it. Four guards keep a deep tree from cascading:

1. **Escalation follows the failed measure, not the whole node.** Every child measure maps to the parent measure it came from, so the kernel can work out exactly which leaves derive from the failed one. Only those leaves pause, along with anything that depends on the escalating leaf.
2. **Escalations batch, and the batch freezes.** When a decision run starts, it gets the batch as it stands. Escalations that arrive later queue for the next batch, so a decision never misses one it was never shown.
3. **Each level makes one decision per batch: re-divide or pass up.** Re-dividing spends progress credit. Without credit, the node can only pass up. A failure costs at most one agent decision per level and climbs at most to Rob, who can add credit, change the request, accept it as it is, or stop it.
4. **A decision answers every escalation in its frozen batch,** so no escalated node is left waiting. A decision is checked against the facts as they are when it returns, not as they were when it started. A child that merged in the meantime stays merged. Restarting or re-splitting a node abandons its open children first.

The deadlock rules still hold. The wait graph stays acyclic, and a node that is deciding never waits on its children.

### Escalations to Rob

Agreed on 2026-10-29. Rob owns each request's measures and is the final point of escalation. A root agent reviews a request's final result on his behalf.

Rob added later the same day: "I want as much resolved WITHOUT ME as possible. I will be the bottleneck." Questions block work and notes don't, so TL3 asks Rob a question only when it can't carry on safely without him:

- **to change what he asked for**, such as a goal that no plan can meet;
- **for something only he can do**, such as a permission, a login or a payment;
- **when a request's credit runs out**, which means attempts keep failing without anything merging.

Everything else, TL3 decides itself. It sends him a note only when something changes a request's course: a stop, a pause, a merge TL3 didn't approve, a re-plan, a PR he closed or took out of the queue, or a commit someone else pushed. Routine steps such as fix rounds, new attempts, model switches and rebases show only in the status view (Q8). The state table lists every question and the answers each one offers.

A message to Rob must make sense without any context. It contains:

- what he asked for, in his own words, quoting the issue title;
- what's in the way, in one or two plain sentences;
- two or three options, each with what happens if he picks it;
- the recommended option, and why;
- how to answer.

It leaves out internal names and IDs, file paths, logs and TL3's own vocabulary (leaf, lineage, replan, measure). A separate check rejects any draft that breaks these rules before it is sent. The details stay available if he asks for them.

Rob's answer arrives as an event. The request waits for it with no timer, and only the affected chain of work pauses while it waits. The channel is an adapter. Discord is planned for this and will be set up later. Until then, Rob answers in comments on the request's issue.

Rob will often just comment rather than answer a question. An agent reads each of his comments on an active request and classifies it: change the request, restart, stop, pause, resume, an answer, or a remark that needs no action. "Cancel" means stop. When it is unsure, TL3 asks him one short question (Q15). A comment on a closed request that says something is wrong files a linked correction request, which carries his authorization and can include a revert (Q21).

Restart is how Rob starts a request over after something went wrong (Q49). It reads the issue's current title and body from GitHub when it applies, so he can edit the issue first. On an open request, it works like changing the request: the request stops as replaced, and a new one linked to the same issue starts on its own. On a request that closed without merging, it files a new linked request and reopens the issue if TL3 closed it. Merged and Done work isn't restarted; a complaint files a correction. A stop no longer closes the issue: the summary says to comment `restart` to start again.

Incidents often span several requests, so until Discord exists they go to one pinned "TL3 operations" issue in TL3's own repo. Questions about a request stay on that request's issue (Q17).

### Escalation reports

Agreed (Q31). The report is the interface between levels:

- **What it contains.** It names the receiver's measures that are affected, and only measures the receiver set. It says what can be delivered instead, and what that means for the receiver's ongoing work as the sender sees it. It may recommend a response.
- **What it leaves out.** It never carries diffs, logs, transcripts or anything about grandchildren. It is capped at about 2k tokens.
- **What a deciding node sees.** Its own brief, its measures and its direct children's reports. That keeps every context small at every depth.
- **Where the details go.** The raw details stay in the event log and MLflow for postmortems and Hindsight. They just aren't pushed upward.
- **Details on request.** A receiving owner can pull the evidence for one named measure, once per escalation, if the summary isn't enough.

## After merge

Agreed (Q32); the judge's postmortem and the model trials arrive in slice three. Rob asked on 2026-10-29 for two steps after a merge: delete the git worktree, and run a postmortem. The postmortem should:

- grade work against a rubric;
- suggest system prompt changes and point out issues that could have been avoided;
- be able to apply the retro skill to the agents' traces;
- send actionable recommendations, which Rob approves, back into the system prompts and possibly into new or changed phases.

**Cleanup runs first and right away.** A merge moves the leaf to Stopping. Teardown stops anything still running, removes the worktree and deletes the branch, checking before each step so that a rerun does nothing. Only then does the leaf close and its parent hear about it. The code is already on main and the transcripts are in MLflow, so the postmortem doesn't need the worktree.

**The postmortem runs in the background.** It never holds up delivery. Code grades are recorded when the node closes, starting in slice one. The judge's postmortem arrives in slice three. It is recorded as owed work, so a crash can't lose it, and it runs at low priority on spare quota.

**Port TL1's postmortem rather than redesigning it.** Start from TL1's rubric (`src/trustladder/rubric.toml`, version 1) and its judge prompt (`prompts/judge.md`):

- Grades run from 1 to 5 for each role and dimension, and each grade has a written description.
- Code grades whatever the facts settle. The judge grades the rest and cites its evidence.
- A later fact, such as an escaped defect, adds a new grade and never changes an old one.
- The rubric is versioned, so a new version can regrade history.
- Process failures are labelled by kind: prompt or skill defect, code defect, tool failure, or missing evidence.
- Each postmortem makes at most three recommendations. Each one is an exact before-and-after edit with its evidence and expected effect.
- A cheap model first summarizes each run's transcript with citations, and the judge checks any important claim against the transcript.

TL3 adds dimensions for its own steps:

- how close the forecast came to the actual peak context;
- whether the fix round fixed the problem;
- whether the raised-effort attempt rescued the leaf;
- whether an escalation report led to the right decision.

**Scope.** Every node gets one postmortem when it ends: a leaf when it merges or is abandoned, and a parent after integration. Each postmortem sees only that node's own materials, the same insulation that escalation uses. Code grades every run. The judge grades every node that needed a fix round, a later attempt or an escalation, plus a random share of the clean ones, and that share is logged.

**Ranking models and effort levels.** Grades alone can't rank them. TL1 graded everything and still couldn't compare models, because every role ran at a single effort level and nothing varied. TL3 sends a logged, random share of runs to a candidate model or effort level, starting at 1 in 10 and never on work that other work is waiting on. Later attempts, with a bigger budget or another model, don't count as comparisons by themselves, because they only happen after a failure. Results reach Rob as proposals, and he picks models and effort levels.

**The retro skill** is loaded into the postmortem worker. Its categories map onto the things TL3 can change: navigation, automated checks, coding standards, bloated steering files, tool economy, instructions that have no effect, and information access. One of its rules decides where a recommendation goes: a mechanical mistake gets an automated check, not more prompt text.

- **A prompt or skill edit** becomes a new version in MLflow's prompt registry. Until slice three, role prompts live in TL3's own repo, protected and changed only through reviewed PRs, while MLflow traces every run (Q23). A version reaches production only after Rob approves it, and TL3 records his approval with the version. It then runs on a trial share first and becomes the default if its grades hold up.
- **An automated check, coding standard or navigation doc for a served repo** becomes a request on that repo and goes through the normal pipeline.
- **A tool change, a brief change, or a new or changed phase** becomes a design proposal for TL3, built deliberately rather than changed while running.

**Approval without flooding Rob.** A curator merges duplicate recommendations across postmortems. Rob sees one plain-language proposal per change once enough postmortems point at it (three, to start). The proposal says how often the problem happened and what should change. A rejected proposal is remembered and only comes back with new evidence.

**Where results go.** Grades become MLflow assessments, tagged with role, model, effort level, prompt version and rubric version. Lessons go to Hindsight, so ingest recalls them for similar work.

## Self-improvement

Agreed on 2026-10-29: TL3 recursively improves itself, both in its instructions and through requests it files. Rob considers this one of the strongest ideas in the design. The structure below is proposed.

Self-improvement runs as three loops, sorted by risk:

| Loop | What changes | How it applies | Who approves | Safety net |
|---|---|---|---|---|
| Instructions | Prompts, skills, review standards, brief templates | A new version in MLflow's prompt registry, with no deploy | Rob, at first | A trial share, a held-out benchmark, and automatic rollback |
| Requests | Checks and docs in served repos; tools, phases and forecasting in TL3 itself | An ordinary request through the pipeline. TL3's own changes deploy as releases | Review and Mergify, plus Rob for new or changed phases | Kani laws as required checks, and one TL3 change in flight at a time |
| Constitution | The laws and the rubric | A decision | Rob, always | Nothing else can change them |

Three rules keep the recursion from running away:

1. **Every role gets postmortems, including the roles that do the improving.** The judge, the curator and the decomposer are graded the same way as the makers. Their grades come from code, measured against later facts: escaped defects, CI failures after an approval, and proposals Rob rejected. Nothing grades itself.
2. **Grades rest on facts the loop can't game.** Those facts are merges, CI results, escaped defects and Rob's decisions. Each role also has a frozen benchmark that the loop never sees, and a change that does worse on it is rejected. This is also why the rubric belongs to the constitution: a TL3 that could edit its own rubric could grade its way to success.
3. **TL3 can change itself, but it can't change itself into something that breaks its laws.** Kani's proofs run as required checks, so a change that breaks a law can't merge. A running TL3 never edits its own live code. Changes deploy as releases, with the upgrade rules in the state table.

**Earned autonomy.** Agreed (Q33). Every change starts out needing Rob's approval. Once a kind of change has a clean record, for example ten approved prompt edits that held up in trials, Rob can let that kind apply on its own. It still sits behind the trial share and automatic rollback. Rolling back to the last good version never needs approval. Each kind of change climbs its own trust ladder, and only Rob moves it up.

**Limits.**

- Only one trial runs per prompt at a time, so every grade change can be traced to a single edit.
- TL3's own repo has at most one change in flight.
- File locks stop changes from landing in the same files at once.

**Protecting the core.** Rob agreed to this on 2026-10-29. Both reviewers showed that self-modification could quietly weaken its own safety checks, for example by deleting a Kani harness while CI stays green.

- The following need Rob's approval:
  - changes to TL3's kernel, laws, proof harnesses, rubric, CI definitions and evaluation data;
  - changes to CI, Mergify and runner config in every served repo.
- The protected paths are listed in one file, which is itself protected. GitHub code owners and Mergify enforce the list.
- A change to the laws goes in its own PR, and code changes are judged by the laws already in force.
- CI checks the list of proof harnesses against a list Rob approved.
- TL3 can never post the check that approves its own core.
- Benchmarks stay out of MLflow, Hindsight and every agent's reach, so the loop can't learn the answers.

## Bounds

Rob ruled out hard size limits on 2026-10-29. Progress credit bounds rework instead, as described in `docs/state-table.md`.

**Progress credit is an experiment.** Rob said on 2026-10-29 that he had never seen anything like it and was willing to try it. Nothing published pairs a rework budget with credit earned back by merged work. The nearest precedents are retry budgets (gRPC, Finagle, Envoy), SRE error budgets, Erlang/OTP restart limits and model cascades. So TL3 measures it:

- every ledger entry is logged with its reason and the attempt it paid for;
- for each step on the ladder (the fix round, the bigger budget, the other model, and the re-plan), how often it rescues the work;
- how often Rob is asked, and whether his answer changed anything or only said "keep going";
- the credit spent per merged piece.

Agreed (Q6): after the first 25 requests, Rob reviews these numbers.

- If "keep going" is his usual answer, the reserve grows.
- If stuck work runs too long before he hears about it, the reserve shrinks.
- A step on the ladder that never rescues anything is dropped.
- If earning credit back doesn't help at all, it is switched off, which leaves a plain budget per request. The kernel's rules hold either way.

Proposed:

- **One PR per leaf.**
- **At most W leaves in flight per repo,** starting at 2 in slice two and later adapting to measured capacity (Q29). Slice one uses 1. W also caps open PRs per repo, however large a request grows. Each repo has its own admission gate holding the slots and file locks.

  Q29 amended (Rob, 2026-10-31): W may be 1 to 3; the approved slice-one deployment value is 3.
- **Plan just in time.** A repo's next node gets planned when fewer than W leaves are waiting for a slot.
- **Leaves span one repo.** A request that touches several repos splits into one child per repo, so no lock ever spans two gates.
- **The parent checks the combined result.** After its last child closes, the parent runs its own acceptance check.

The rules live in the kernel and Kani proves them. The numbers live in config and get tuned.

## Laws

The laws now live in one place: `docs/state-table.md`, under "Rules". Each one carries a label saying where its guarantee comes from: the compiler, a Kani proof, a witness test, simulation, a GitHub or Mergify setting, agent behavior, or an assumption about the outside world. Both reviewers showed that an unlabelled list overstated what Kani can prove.

## Review history

Sol and Muse reviewed the design adversarially and independently on 2026-10-29, twice. Each round, both said it wasn't ready to build, and they agreed on most of the reasons.

**Round 1** led to version 2 of the state table. That version introduced:

- facts kept separate from phases;
- an effect outbox with generations and epochs;
- progress credit;
- looks instead of silent waits;
- labelled proof claims;
- the independent audit of root measures;
- protection for TL3's core;
- staging into slices.

Rob's decisions after round 1:

- **Progress credit:** yes.
- **Verification:** the agent doing the work checks its own completion, and another agent always verifies it independently. That agent can be the same type or a different one.
- **TL3's core:** changes need Rob's approval.
- **Staging:** up to me, but progress is crucial.

**Round 2** found three blockers.

- The check rule rejected correct work.
- The kernel's function signature no longer matched what it had to do.
- Slice one sent work into phases it didn't include.

Round 2 also found merges that skipped teardown, a deadlock that forms through later splits, retries that dodged credit, effects with no failure state, unsandboxed CI, and contradictions between the two documents.

Version 3 of the state table fixes those findings:

- three kinds of check;
- a world-level kernel step that settles after each event;
- a complete slice-one table, with typed questions to Rob;
- merges that go through teardown;
- a cycle check on every structural change;
- a credit charge per child on splits;
- free retries only while work gets further, which version 4 replaced;
- delivery contracts for effects, with a permanent failure state;
- containment that includes CI;
- stall detection based on progress, not heartbeats;
- an audit of early exits;
- the approval check posted again after a clean rebase.

This document was then pruned to match.

**Round 3** found no problem with the overall shape. Both reviewers said slice one wasn't ready, for narrower reasons:

- Approval carried across a rebase by matching changed lines wasn't sound.
- Cleanup could get stuck forever if a step failed permanently, or if a late confirmation was treated as stale.
- The kernel didn't enforce the order of outside actions.
- An old answer from Rob could revive closed work.
- Several rules labelled Kani couldn't be proven as written.
- Trivial "progress" could keep resetting the retry counter.
- Code under test could fake its own results.
- Pausing had gaps.
- CI could reach the fleet's shared files.
- Upgrades stranded actions that hadn't been delivered yet.

Version 4 of the state table fixes these:

- a new commit always goes through the checks and the review again;
- a failed cleanup step waits on Rob instead of sticking;
- confirmations apply by key;
- one outside action is in flight per node at a time;
- questions to Rob carry ids;
- inductive Kani proofs over a `valid(world)` invariant, plus a progress measure and a witness that rule out dead states;
- every second failure raises an incident;
- a test harness that TL3 owns;
- explicit pause rules, which keep your decision that a PR already in the queue can still merge;
- isolated CI;
- upgrades that keep pending actions.

Slice one uses one model account and a plain status view. Rob later approved raising its gate from 1 to 3 slots under the Q29 amendment of 2026-10-31.

Both reviewers asked for proof to be tested before more design work. The next step is a kernel spike: slice one's kernel and its Kani harnesses, run on PrismaBuild.

## Closing the gaps

These were the gaps found on 2026-10-29, before building.

Decided by Rob on 2026-10-29:

- **Failure types:** agreed, as described in the Review section.
- **Upgrades:** agreed. Since review round 1, a new release migrates a snapshot with an explicit function and never replays older events.
- **No hard limits:** progress bounds the work instead.
- **GitHub App:** TL3 gets its own. Creating it needs Rob's browser session.
- **Mergify:** paid, and to be enabled on the repos TL3 serves.
- **CLIProxyAPI:** TL3 uses the existing key for now.
- **Sandbox:** `RobTand/tl3-sandbox` is private, with a small Python project. Its tests pass on PrismaBuild. CI and Mergify get wired in once the GitHub App and the runners exist.

Proposed to complete the design, and agreed where a decision is cited:

- **Findings.** A finding stays on its node. Its node's postmortem marks it either "worth a request" or noise. The curator merges findings marked worth a request across nodes, and Rob sees one plain-language proposal per finding. Only his approval turns a finding into a new request, so findings can't recreate TL1's flood of issues (Q14).
- **Stopping a request.** The teardown order is the Stopping phase in `docs/state-table.md`.
- **Root measures.** After the audit agrees, TL3 posts them to the issue in plain language as a note, and planning starts without waiting for Rob's approval (Q3). If Rob changes the request at any point, the current request stops as replaced, and a new request linked to the same issue starts on its own from his new text, under his original label (Q14).
- **Intake trust.**
  - Only issues Rob labels `tl3` enter. Labels added by anyone else are ignored, and Rob may allow other people later (Q2).
  - Every agent treats issue text, comments and repo content as material to work on, never as instructions.
  - Agents hold no secrets.
- **Agent sandbox.**
  - Each run reads and writes only its own worktree.
  - Its only tools are files, search, PrismaBuild and Hindsight recall. It has no general shell, and commands run only on PrismaBuild, inside the containment profile.
  - Its network reaches CLIProxyAPI and nothing else.
- **Incidents.**
  - An incident is raised on every second failure of the same step, or when an outside service stalls (Q7).
  - It carries a plain-language summary, shows in the status view, and goes on the pinned "TL3 operations" issue (Q17).
  - Dependent work waits while the rest carries on, and a leaf waiting on an incident gives back its slot.
  - Rob is asked only when the incident needs something only he can do, such as a login, a credential, a payment, hardware, or a step that keeps failing after its first incident.
- **Waiting on an outside job.** Agreed by Rob on 2026-10-30 (Q43). An agent waiting on a long PrismaBuild test run produces no new tool results, so the earlier rule would have stopped it as stalled. The agent now declares the job it is waiting on, and its run stays alive while that job's own record shows it has neither ended nor been lost. The wait stays open on the upstream's evidence, never on a clock, so a later wait such as a token grant fits the same rule.

Settled in the grilling:

- **Kinds of request.** Slices one and two take only code changes in one repo (Q1), and a code change is done when its PR merges with every goal confirmed (Q12). Research questions, reviews of existing PRs and work spanning several repos each get their own grilling and definition of done after slice two is solid (Q35).
- **The decomposition trial.** Before slice two's splitting goes live, a dry run decomposes a handful of real requests, changing no code, to calibrate the fit limit and the forecast (Q34).
- **W, the fit limit and the starting credit reserve:** 2 per repo in slice two (Q29), 100k (Q4), and 3 (Q6).

  Q29 amended (Rob, 2026-10-31): W may be 1 to 3; the approved slice-one deployment value is 3.
- **The status view:** a read-only web page on the LAN plus a `tl3 status` command (Q24).

Still open:

- **Discord** as the channel for escalations to Rob, set up later. Until then, comments on the request's issue.
- **These can wait, because the defaults work:**
  - scheduling that accounts for quota, and turbo mode;
  - backups.

## Decisions from the grilling

On 2026-10-29 and 30, Rob answered 41 questions in six rounds. He accepted the recommended answer each time except Q28 and Q30, where his own answer is recorded. Q36 to Q39 followed from his answer to Q30.

**Scope and intake**

- **Q1 Kinds of request:** slices one and two take only code changes in one repo.
- **Q2 Who starts a request:** only Rob's `tl3` label. Labels added by anyone else are ignored, and he may allow other people later.
- **Q3 Goals without sign-off:** once the audit agrees, TL3 posts the goals to the issue as a note and starts without waiting.
- **Q4 Size of one change:** the largest piece whose forecast fits in about 100k of the 150k context, tuned later from MLflow data. Slice one asks Rob to narrow anything bigger.
- **Q12 Done:** the PR merges, the checks and the independent review confirm every goal, and TL3 posts a plain-language summary and closes the issue. An unmet goal means it isn't done, unless Rob accepts it as is, which comes in slice two.
- **Q13 Repos:** slice one serves only `tl3-sandbox`. Slice two adds one real repo, which Rob picks then. TL3's own repo comes only after slice two.
- **Q14 Requests TL3 files:** a replacement from "change the request" starts on its own under Rob's label. Findings become requests only once he approves them.
- **Q15 Rob's comments:** an agent classifies each comment of his on an active request as a change, stop, pause, resume, answer or remark, and asks one short question when unsure.
- **Q21 Complaints after close:** a comment from Rob saying something is wrong with closed work files a linked correction request. It carries his authorization and can include a revert.
- **Q49 Restart, and stop leaves the issue open:** approved by Rob, 2026-11-01. A `restart` comment starts the request over from the issue's current title and body. On an open request it stops the request as replaced and files a new one on the same issue (Q14). On a request closed as Stopped, Abandoned, NotActionable, AlreadyDone or Duplicate, it files a new linked request and reopens the issue if TL3 closed it. It isn't offered on Merged or Done work. A root that closes as Stopped or Abandoned posts its summary, says to comment `restart` to start again, and leaves the issue open. "Cancel" is classified as stop. The controller builds this from existing kernel commands; the kernel didn't change.
- **Q35 Other kinds of request:** research questions, reviews of existing PRs and multi-repo work each get their own grilling and definition of done after slice two.

**Guarding the checks**

- **Q5 Protected config:** CI and Mergify config need Rob's approval. Test configuration needs a second, independent review aimed only at weakened checks, plus a note to Rob.
- **Q16 Test configuration:** the files that decide which tests run or how results are reported: pytest's settings, `conftest.py`, tox and nox files, and test scripts CI calls. The tests themselves don't count.
- **Q40 Checks:** every goal is checked as demonstrating, regression or behavior-preserving. TL3 runs the tests itself, and a missing or skipped test fails.
- **Q41 Approval:** an approval covers one exact commit. Any new commit, including a clean catch-up onto main, is tested and reviewed again. The first 3 catch-ups per piece are free, and later ones cost credit.
- **Q42 Review fixes:** a fix round for a review finding about behavior adds a reproducing test that fails on the reviewed commit and passes on the fix. A finding no test can show needs a stated reason. Decided after the grilling, on 2026-10-30.

**Failures, credit and Rob's attention**

- **Q6 Credit:** starting credit is the request's size in pieces plus a reserve of 3. The first 3 rechecks after rebases are free. The numbers are reviewed after the first 25 requests.
- **Q7 Repeated failures:** every second failure of the same step raises an incident. A passing health check can clear the first, and only Rob can clear a later one.
- **Q43 Declared waits:** an agent run stays alive while an outside job it has declared it is waiting on, such as a PrismaBuild test run, has by its own record neither ended nor been lost. Decided after the grilling, on 2026-10-30.
- **Q44 Reviewer out of room:** "Fresh start, then split." On the first out-of-room, restart the review in a fresh conversation on the next model in the reviewer's ladder. If that review also runs out, Planning must split the leaf, exactly as the maker's out-of-room rule does, including both credit rows. Decided on 2026-10-31.

  **Coordinator defaults, for Rob to check:** The first restart counts like a re-review, sharing the rebase allowance: free up to `free_rechecks`, then 1 each. The ladder clamps at its last entry, reusing that model in a fresh conversation. A clean rebase or outside push to a new head resets the restart flag; the shared recheck counter continues to bound restarts through credit. Ordinary fix rounds continue the selected reviewer's session. A new maker attempt resets the reviewer.

- **Q46 Publication conflicts:** Publishing plus a conflict with current main goes to Rebasing, with the same effects and costs as a conflict in Queued. Publication guards report Conflict instead of a permanent effect failure. Checks and review run again on the rebased head before it republishes. A clean main advance publishes without rebasing and lets Mergify update the merge tree. Rob approved this rule on 2026-10-31 after drill D5 found that Mergify never queues the conflicting head.

- **Q8 Notes:** only when something changes a request's course, such as a stop, a pause, a merge TL3 didn't approve or a re-plan. Routine steps show only in the status view.
- **Q17 Incidents:** they go to one pinned "TL3 operations" issue in TL3's repo until Discord exists. Questions about a request stay on its issue.
- **Q22 Model ladder:** attempts 1 and 2 use the default maker model, attempt 2 at maximum effort and budget. Attempt 3 uses the strongest model from a different family. Reviewers default to a different family from the maker.
- **Q31 Escalation reports:** only the impact on the parent's goals, at most about 2,000 tokens. The parent can pull the evidence for one goal per escalation.

**Order of work**

- **Q29 W:** 2 changes in flight per repo in slice two, tuned later.

  Q29 amended (Rob, 2026-10-31): W may be 1 to 3; the approved slice-one deployment value is 3.
- **Q30 What runs first:** Rob: "we should be pulling work off that blocks the most other work. I don't really care about starvation." So there is no rule against starvation.
- **Q36 Rank:** the number of other pieces waiting on a piece, directly or through a chain of waits, with ties going to the older piece.
- **Q37 Manual priority:** none in slices one and two. A label such as `tl3-urgent` could come later.
- **Q38 Shared files:** the age rule goes. Whichever piece unblocks more gets the file, with ties going to the older piece.
- **Q39 Agent pool:** the same ordering. Postmortems run only on spare capacity.

**Proofs**

- **Q9 When they run:** as a required Mergify check on changes to the kernel or its proofs, plus a nightly full run.
- **Q10 Bounds:** general event proofs use 2 requests, or 3 after a new request, W at most 3, and abstract commit ids. Focused gate proofs use 4 requests at W = 2 and W = 3 after the Q29 amendment.
- **Q18 Where they run:** as Mergify CI on the fleet's runners, at most 8 at a time.
- **Q27 Changing the rules:** the state table is the single source of truth. A rule change needs Rob's approval, including a rule that comes from a bug Kani or a test finds. A clarification doesn't.

**Building and proving slice one**

- **Q11 Slice one is done** when 10 real sandbox issues merge with no question to Rob, and a fault-injection run recovers every time.
- **Q19 Seed issues:** 12 on the sandbox, for Rob to label.
- **Q20 Fault drills:** kill the controller mid-step, kill a worker, drop webhooks, make CI flaky, force a conflict and lose a PrismaBuild worker. A reboot is simulated by restarting every TL3 service.
- **Q23 Prompts:** in TL3's repo, protected and changed only through reviewed PRs. MLflow traces every run, and its prompt registry takes over in slice three.
- **Q24 Status view:** a read-only web page on the LAN, served from dl380g10, plus `tl3 status`.
- **Q25 Where the controller runs:** the controller and Postgres stay on dl380g10 despite its kernel bug. This is revisited if crashes keep happening.
- **Q26 GitHub App:** the manifest is prepared so Rob only clicks "create". It is installed on `tl3-sandbox` first.
- **Q28 Committing:** Rob asked for a private GitHub repo, `RobTand/trustladderiii`, rather than local commits only.

**After the grilling**

- **Report trust (2026-10-30):** the check harness's pytest reports run in-process, so deliberately hostile code could forge them. Rob: "I'm not super worried about hostile code at this point." Slice one accepts them, each result records its custody, and tamper-proof reports wait for PB #2403.

**Slice three and later**

- **Q32 Postmortems and model trials:** they reuse TL1's rubric and judge. The judge grades everything that needed a fix, a retry or an escalation, plus a random share of clean work. 1 run in 10 tries a candidate model, never on work that other work is waiting on. A change reaches Rob as a proposal once 3 postmortems point to it.
- **Q33 Earned autonomy:** a kind of change applies without Rob only after he grants that kind, following a clean record. Rolling back never needs approval.
- **Q34 Decomposition dry run:** before splits go live, a dry run decomposes real requests without changing code, to calibrate the fit limit and the forecast.


## Q45: adopting a proposed PR

Decided by Rob on 2026-10-31, approving the associated state-table changes under Q27:

- **Trigger:** either way.
- **Fixes:** on TL3's own branch from the PR head.
- **Scope:** the issue's goals only.
- **Too big:** split into leaves.
- **Q35:** this requirement supersedes Q35's deferral of reviewing existing PRs until after slice two, for this case.

Ticket #143 implements only Rob's PR label on a same-repo PR. Its first Making
preparation adopts the pinned head once, without running the maker or earning
maker credit. Failed checks and review findings use the existing fix rounds on
TL3's branch, retaining the author's commits. Scope trimming is #145; splitting
adopted work into leaves is #147.

**Confirmed by Rob, 2026-10-31:** The two PRD choices used by #143 are:

- The closing keyword must identify exactly one issue. Zero or several linked issues get one clarification.
- The proposed head never changes. It is pinned at label time and later pushes are ignored.

The PRD's other choices are also **coordinator defaults, for Rob to check**:
the existing rebase machinery keeps up with main; protected paths count
as out of scope; and an author closing the original does not cancel the request.
The PRD also proposes noting later pushes in the summary and relabelling to adopt
again. These defaults do not expand #143 beyond its ticket. This tracer retains
the existing once-per-issue intake rule. Issue-trigger intake, fork support,
catch-up, steering, scope trimming and splitting remain later tickets.

## Q47: required checks before enqueue

Rob, 2026-11-01 approved handling CI failures during Publishing at the enqueue
step. TL3 waits for the approved head's required checks before adding
`tl3-queue`. A code failure takes the existing failed-round path, cancels the
pending enqueue and removes the label if it was added. An infrastructure failure
reruns the check, then enqueues when the existing failure rules allow.
Credit costs and infrastructure retry limits are the same as in Queued.

### Q48, Rob, 2026-11-01

> A request waiting for files may be passed by work whose files don't overlap its own. Overlapping work never jumps ahead, so the waiting request can't be passed indefinitely.

An older waiter blocks a younger leaf if it could actually run, or if their footprints overlap. Evaluate tickets oldest first: available files and a spare slot do not make a waiter runnable while an older overlapping ticket blocks it. Ticket identities, planning admission and agent-pool order remain unchanged.

A younger grant cannot add a file holder that blocks an older waiter. When the older waiter's last conflicting holder releases, that release also frees a slot. Its older ticket protects the slot while disjoint younger holders continue running. This relies on holders eventually completing or being stopped, and on older eligible waiters eventually progressing.

The per-leaf Kani proof bounds the world to four leaves at W=2 and W=3. It starts with an unpaused oldest waiter and its last conflicting holder. Younger leaves are visited before that waiter, both before and after the holder's successful teardown. The waiter must acquire a slot while disjoint overtakers remain active. It proves this bounded case, not unbounded temporal liveness with arbitrary arrivals or stalled upstreams. The covers require a disjoint bypass, including passing a waiter blocked by an older overlapping waiter. A scenario additionally repeats eight disjoint arrivals before the blocking holder releases. A store-backed scenario raises W from 1 to 3 with the oldest waiter at the highest node index; only disjoint work passes it, and it takes the next slot once its blocker releases.
