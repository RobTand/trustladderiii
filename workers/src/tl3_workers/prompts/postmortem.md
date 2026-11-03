Task: write the postmortem of this request. It has finished, or stopped to ask Rob. Find where its process went wrong, and recommend changes that would have prevented it. Recommendations are proposals for Rob: they change nothing by themselves.

Your result is one JSON object, written as text, with exactly these fields:

- summary: a few sentences on what happened to the request and why it ended as it did.
- cause: "tl3" when TL3's own code, prompts, rules or tools caused the trouble, or "outside" when the request, its repository, a person or a service outside TL3 did.
- went_wrong: one string of at most 8000 characters describing what went wrong and why, in order, with the root cause first.
- timeline: the request's main events in order, each {"at": time or step, "event": what happened}. Take times from the case file's phases and the spans.
- process_failures: each place the process went wrong, each {"phase", "kind", "what", "evidence"}. The kind is "prompt or skill defect" when a run's instructions misled it or left out what it needed, "code defect" when TL3's code went wrong, "tool failure" when a harness, tool or service did, and "missing evidence" when your materials can't tell. Name each separately, one cause each, so the same failure in another request reads the same. Every fix round, replan, retry and question to Rob has at least one. Empty only when nothing went wrong.
- recommendations: at most three changes, the most valuable first, each one that would have prevented a process failure. Each is {"phase", "kind", "target", "operation", "before", "after", "evidence", "effect"}:
  - phase: the role or step the change is for, such as ingest, planner, maker, reviewer or the controller;
  - kind: "prompt", "code", "config" or "rule";
  - target: the file to change, as a path from TL3's repository root, such as workers/src/tl3_workers/prompts/maker.md;
  - operation, before and after, as exact text: "add" appends after and leaves before empty; "remove" gives before and leaves after empty; "replace" gives both. before must appear exactly once in the target. A recommendation whose target isn't a file in the release, or whose before text isn't in it exactly once, is dropped;
  - evidence: what shows the failure, by source trace id or case-file fact, and why the change addresses it;
  - effect: the behavior you expect to change, and what would show it did.
  Recommend only what the evidence supports. Prefer a small, specific correction to a general rule. Empty when nothing your materials support would have helped.
- notes: at most three sentences that process_failures and recommendations don't already say. Empty when they say it all.
- lessons: at most five short lessons worth remembering for similar work, each a plain sentence. They are stored in TL3's memory, so state facts, not instructions.

Put that object, as JSON text, in your result. Write no other text in it.
