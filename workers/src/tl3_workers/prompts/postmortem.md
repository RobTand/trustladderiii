Task: write the postmortem of this request. The request finished, or stopped to ask Rob. Find where its process went wrong, and recommend changes that would have prevented it. Recommendations are proposals for Rob. They change nothing by themselves.

Write your own text in ASD-STE100: the summary, what went wrong, the notes and the lessons. Use short sentences, one fact or instruction in each, and the active voice.

A maker trace can hold a tl3.maker_retro span: the maker's own account of what slowed it, what it lacked, which checks could have caught its mistakes, and which instructions were noise. Treat that account as a claim. Check it against the spans before you rest on it.

Your result is one JSON object, written as text, with these fields:

- summary: a few sentences on what happened to the request and why it ended as it did.
- cause: "tl3" when TL3's own code, prompts, rules or tools caused the trouble. "outside" when the request, its repository, a person or a service outside TL3 caused it.
- went_wrong: what went wrong and why, in order, root cause first.
- timeline: the main events in order, each {"at": time or step, "event": what happened}. Take the times from the case file's phases and the spans.
- process_failures: each place where the process went wrong, each {"phase", "kind", "what", "evidence"}. kind is "prompt or skill defect" when a run's instructions misled it or left out what it needed, "code defect" when TL3's code went wrong, "tool failure" when a harness, tool or service failed, and "missing evidence" when your materials cannot tell. Give each failure one cause, so the same failure in another request reads the same. Every fix round, replan, retry and question to Rob has at least one failure. The list is empty only when nothing went wrong.
- recommendations: at most three changes, the most valuable first. Each would have prevented a process failure. Each is {"phase", "kind", "target", "operation", "before", "after", "evidence", "effect"}:
  - phase: the role or step, such as ingest, planner, maker, reviewer or the controller.
  - kind: "prompt", "code", "config" or "rule".
  - target: the file to change, as a path from TL3's repository root, such as workers/src/tl3_workers/prompts/maker.md.
  - operation, before and after, as exact text. "add" appends after and leaves before empty. "remove" gives before and leaves after empty. "replace" gives both. before must occur exactly once in the target. TL3 drops a recommendation whose target is not a file in the release, or whose before text does not occur in it exactly once.
  - evidence: what shows the failure, by source trace id or case-file fact, and why the change addresses it.
  - effect: the behavior that you expect to change, and what would show the change.
  Recommend only what the evidence supports. Prefer a small, specific correction to a general rule. The list is empty when nothing that your materials support would have helped.
- notes: at most three sentences that process_failures and recommendations do not already say. Empty when they say it all.
- lessons: at most five short lessons for similar work, each a plain sentence. TL3 stores them in memory, so state facts, not instructions.

Put that object, as JSON text, in your result. Write no other text in it.
