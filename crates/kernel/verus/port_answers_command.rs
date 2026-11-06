//! #806: bridge the neutral dispatcher contract to W4's private answer model.
use super::port_answers_model::*;
use super::*;

verus! {
/// #806: command deduplication precedes the real answer-handler postcondition.
#[verifier::spinoff_prover]
pub(super) proof fn command_result(n: Node, after: Node, cmd: u64, id: u64,
    answer: Answer, next: u64, next_after: u64)
    requires if cmd <= n.last_cmd { after == n && next_after == next }
        else { answers::answer_post(Node { last_cmd: cmd, ..n }, after,
            id, answer, next, next_after) },
    ensures after == commanded(n, cmd, id, answer, next),
        next_after == next_question(n, cmd, id, answer, next),
{
    reveal(answers::answer_post); reveal(commanded); reveal(next_question);
}
/// #806: opaque forwarding envelope for the Answer event arm only.
#[verifier::opaque]
pub(super) open spec fn event_post(before: World, after: World, event: Event,
    out_before: Outbox, out_after: Outbox) -> bool {
    match event {
        Event::Command { node, id, command: Command::Answer { question, answer } } =>
            node.0 < before.nodes@.len() && answers::answer_supported(answer) ==> {
                after.nodes@ == before.nodes@.update(node.0 as int, after.nodes@[node.0 as int])
                && (if id <= before.nodes@[node.0 as int].last_cmd {
                    after.nodes@[node.0 as int] == before.nodes@[node.0 as int]
                        && after.next_question == before.next_question
                } else {
                    answers::answer_post(Node { last_cmd: id, ..before.nodes@[node.0 as int] },
                        after.nodes@[node.0 as int], question, answer,
                        before.next_question, after.next_question)
                })
                && after.file_sets@ == before.file_sets@
                && after.dependencies@ == before.dependencies@
                && (retirement::notices_only(out_before) ==> retirement::notices_only(out_after))
            },
        _ => true,
    }
}

/// #806: reconstruct the vector frame outside the large event dispatcher query.
#[verifier::spinoff_prover]
pub(super) proof fn seal_event(before: World, after: World, node: NodeId, cmd: u64,
    id: u64, answer: Answer, out_before: Outbox, out_after: Outbox)
    requires node.0 < before.nodes@.len(), answers::answer_supported(answer),
        after.nodes@.len() == before.nodes@.len(),
        forall|j: int| 0 <= j < before.nodes@.len() && j != node.0
            ==> after.nodes@[j] == before.nodes@[j],
        if cmd <= before.nodes@[node.0 as int].last_cmd {
            after.nodes@[node.0 as int] == before.nodes@[node.0 as int] && after.next_question == before.next_question
        } else { answers::answer_post(Node { last_cmd: cmd, ..before.nodes@[node.0 as int] },
            after.nodes@[node.0 as int], id, answer, before.next_question, after.next_question) },
        after.file_sets@ == before.file_sets@, after.dependencies@ == before.dependencies@,
        retirement::notices_only(out_before) ==> retirement::notices_only(out_after),
    ensures event_post(before, after, Event::Command { node, id: cmd,
        command: Command::Answer { question: id, answer } }, out_before, out_after),
{
    reveal(event_post);
    assert(after.nodes@ =~= before.nodes@.update(node.0 as int, after.nodes@[node.0 as int]));
}

/// #806: consumers open only the Answer envelope, leaving all node models opaque.
#[verifier::spinoff_prover]
pub(super) proof fn open_event(before: World, after: World, node: NodeId, cmd: u64,
    id: u64, answer: Answer, out_before: Outbox, out_after: Outbox)
    requires node.0 < before.nodes@.len(), answers::answer_supported(answer),
        event_post(before, after, Event::Command { node, id: cmd,
            command: Command::Answer { question: id, answer } }, out_before, out_after),
    ensures after.nodes@ == before.nodes@.update(node.0 as int, after.nodes@[node.0 as int]),
        if cmd <= before.nodes@[node.0 as int].last_cmd {
            after.nodes@[node.0 as int] == before.nodes@[node.0 as int] && after.next_question == before.next_question
        } else { answers::answer_post(Node { last_cmd: cmd, ..before.nodes@[node.0 as int] },
            after.nodes@[node.0 as int], id, answer, before.next_question, after.next_question) },
        after.file_sets@ == before.file_sets@, after.dependencies@ == before.dependencies@,
        retirement::notices_only(out_before) ==> retirement::notices_only(out_after),
{ reveal(event_post); }

}
