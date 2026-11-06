//! #806: opaque exact frames for the four maker reply rows.
use super::*;

verus! {
/// #806 slice 2: all maker payloads retain arbitrary heads, files and fit decisions.
pub(super) open spec fn selected(reply: Reply) -> bool {
    matches!(reply, Reply::MakerDone { .. } | Reply::MakerCantMeet
        | Reply::MakerOutOfRoom | Reply::MakerNeedsFiles { .. })
}

/// Maker completion/ladder/exhaustion / #806: the exact existing node/question pair.
#[verifier::opaque]
pub(super) open spec fn work_result(source: Node, fast: bool, id: u64, reply: Reply) -> (Node, u64) {
    match reply {
        Reply::MakerDone { head } => (retirement::phased(Node {
            head: Some(head), checked: None, ..source }, Phase::Checking), id),
        Reply::MakerCantMeet => port_review_retry::rung(source, fast, id, ReviewStop::MakerCantMeet),
        Reply::MakerOutOfRoom => port_review_retry::room(source, fast, id, ReviewStop::MakerOutOfRoom),
        _ => (source, id),
    }
}

/// File extension row / #806: actual union storage growth, exact for all inline inputs.
#[verifier::opaque]
pub(super) open spec fn union_post(before: Seq<Vec<FileId>>, after: Seq<Vec<FileId>>, a: Footprint,
    b: Footprint, joined: Footprint) -> bool {
    after.len() >= before.len()
        && (forall|j: int| 0 <= j < before.len()
            ==> after[j] == before[j])
        && (forall|a_bits: u64, b_bits: u64|
            a == footprint::inline_footprint(a_bits) && b == footprint::inline_footprint(b_bits) ==> {
                joined == footprint::inline_footprint(a_bits | b_bits)
                    && after == before
                    && (!footprint::footprint_empty(a) || !footprint::footprint_empty(b)
                        ==> !footprint::footprint_empty(joined))
            })
}

/// Q54 funding / #806: exact funded, unfunded-fast and retry file-extension branches.
/// Interned inputs retain the real append-only table relation; no spec Vec is built.
#[verifier::opaque]
pub(super) open spec fn files_post(before: World, after: World, i: int, source: Node,
    files: Footprint, fits: bool, out_before: Outbox, out_after: Outbox) -> bool {
    (retirement::notices_only(out_before) ==> retirement::notices_only(out_after))
        && if fits && !footprint::footprint_empty(files)
            && (port_review_retry::funded(source) || before.fail_fast) {
            union_post(before.file_sets@, after.file_sets@, source.footprint, files, after.nodes@[i].footprint)
                && if port_review_retry::funded(source) {
                    after.nodes@[i] == retirement::phased(Node {
                        footprint: after.nodes@[i].footprint, holds: false,
                        ..port_review_retry::paid(source) }, Phase::Ready(Resume::Making))
                        && after.next_question == before.next_question && out_after == out_before
                } else {
                    after.nodes@[i] == port_review_retry::asked(Node {
                        footprint: after.nodes@[i].footprint, ..source }, before.next_question,
                        QKind::OutOfCredit, Cont::Ready(Resume::Making))
                        && after.next_question as int == before.next_question as int + 1
                        && out_after.len == out_before.len + 1
                }
        } else {
            after.nodes@[i] == port_review_retry::room(source, before.fail_fast,
                before.next_question, ReviewStop::MakerNeedsFiles).0
                && after.next_question == port_review_retry::room(source, before.fail_fast,
                    before.next_question, ReviewStop::MakerNeedsFiles).1
                && after.file_sets@ == before.file_sets@
                && (before.fail_fast ==> out_after.len == out_before.len)
        }
}

/// Maker rows / #806: the accepted ended-source relation forwarded by the shared API.
#[verifier::opaque]
pub(super) open spec fn transition_post(before: World, after: World, i: int, source: Node,
    reply: Reply, out_before: Outbox, out_after: Outbox) -> bool {
    selected(reply) && port_review_reply::accepted_reply(before, source, reply) ==> {
        if let Reply::MakerNeedsFiles { files, fits } = reply {
            files_post(before, after, i, source, files, fits, out_before, out_after)
        } else {
            after.nodes@[i] == work_result(source, before.fail_fast, before.next_question, reply).0
                && after.next_question == work_result(source, before.fail_fast, before.next_question, reply).1
                && after.file_sets@ == before.file_sets@
                && (retirement::notices_only(out_before) ==> retirement::notices_only(out_after))
                && (before.fail_fast && matches!(reply, Reply::MakerCantMeet | Reply::MakerOutOfRoom)
                    ==> out_after.len == out_before.len)
        }
    }
}
/// #806 maker admission: combine the real worker result with W2's review-only frame.
pub(super) proof fn unselected_maker(before: World, after: World, i: int, source: Node,
    reply: Reply, out_before: Outbox, out_after: Outbox)
    requires selected(reply),
        transition_post(before, after, i, source, reply, out_before, out_after),
    ensures port_review_reply::transition_post(before, after, i, source, reply, out_before, out_after),
{
    port_review_admission::unselected(before, after, i, source, reply, out_before, out_after);
    reveal(port_review_reply::transition_post);
    reveal(port_replies_contracts::transition_post);
}

/// File extension / #806: compose the funded branch without exposing other cases.
pub(super) proof fn files_paid(before: World, after: World, i: int, files: Footprint,
    fits: bool, out_before: Outbox, out_after: Outbox)
    requires fits, !footprint::footprint_empty(files), port_review_retry::funded(before.nodes@[i]),
        union_post(before.file_sets@, after.file_sets@, before.nodes@[i].footprint, files, after.nodes@[i].footprint),
        after.nodes@[i] == retirement::phased(Node { footprint: after.nodes@[i].footprint,
            holds: false, ..port_review_retry::paid(before.nodes@[i]) }, Phase::Ready(Resume::Making)),
        after.next_question == before.next_question, out_after == out_before,
    ensures files_post(before, after, i, before.nodes@[i], files, fits, out_before, out_after),
{ reveal(files_post); }

/// Q54 funding / #806: compose the unfunded extension's credit question.
pub(super) proof fn files_ask(before: World, after: World, i: int, files: Footprint,
    fits: bool, out_before: Outbox, out_after: Outbox)
    requires fits, !footprint::footprint_empty(files), !port_review_retry::funded(before.nodes@[i]),
        before.fail_fast,
        union_post(before.file_sets@, after.file_sets@, before.nodes@[i].footprint, files, after.nodes@[i].footprint),
        after.nodes@[i] == port_review_retry::asked(Node { footprint: after.nodes@[i].footprint,
            ..before.nodes@[i] }, before.next_question, QKind::OutOfCredit, Cont::Ready(Resume::Making)),
        after.next_question as int == before.next_question as int + 1,
        out_after.len == out_before.len + 1,
        retirement::notices_only(out_before) ==> retirement::notices_only(out_after),
    ensures files_post(before, after, i, before.nodes@[i], files, fits, out_before, out_after),
{ reveal(files_post); }

/// Q54 exhaustion / #806: forward the exact room result and empty fast output.
pub(super) proof fn files_room(before: World, after: World, i: int, files: Footprint,
    fits: bool, out_before: Outbox, out_after: Outbox)
    requires !(fits && !footprint::footprint_empty(files)
            && (port_review_retry::funded(before.nodes@[i]) || before.fail_fast)),
        after.file_sets@ == before.file_sets@,
        after.nodes@[i] == port_review_retry::room(before.nodes@[i], before.fail_fast,
            before.next_question, ReviewStop::MakerNeedsFiles).0,
        after.next_question == port_review_retry::room(before.nodes@[i], before.fail_fast,
            before.next_question, ReviewStop::MakerNeedsFiles).1,
        before.fail_fast ==> out_after.len == out_before.len,
        retirement::notices_only(out_before) ==> retirement::notices_only(out_after),
    ensures files_post(before, after, i, before.nodes@[i], files, fits, out_before, out_after),
{ reveal(files_post); }

/// File union / #806: package only the real helper's published table and inline clauses.
pub(super) proof fn union_intro(before: Seq<Vec<FileId>>, after: Seq<Vec<FileId>>,
    a: Footprint, b: Footprint, joined: Footprint)
    requires after.len() >= before.len(),
        forall|j: int| 0 <= j < before.len() ==> after[j] == before[j],
        forall|a_bits: u64, b_bits: u64|
            a == footprint::inline_footprint(a_bits) && b == footprint::inline_footprint(b_bits) ==> {
                joined == footprint::inline_footprint(a_bits | b_bits) && after == before
                    && (!footprint::footprint_empty(a) || !footprint::footprint_empty(b)
                        ==> !footprint::footprint_empty(joined))
            },
    ensures union_post(before, after, a, b, joined),
{ reveal(union_post); }

/// Reply admission / #806: end_operation changes no input of the maker file relation.
pub(super) proof fn files_transport(before: World, middle: World, after: World, i: int,
    source: Node, files: Footprint, fits: bool, out_before: Outbox, out_after: Outbox)
    requires before.file_sets@ == middle.file_sets@, before.fail_fast == middle.fail_fast,
        before.next_question == middle.next_question,
        files_post(middle, after, i, source, files, fits, out_before, out_after),
    ensures files_post(before, after, i, source, files, fits, out_before, out_after),
{ reveal(files_post); }

/// #806 maker admission: carry the exact relation across end_operation's table frame.
pub(super) proof fn transport(before: World, middle: World, after: World, i: int,
    source: Node, reply: Reply, out_before: Outbox, out_after: Outbox)
    requires selected(reply), source.phase == Phase::Making,
        before.file_sets@ == middle.file_sets@, before.fail_fast == middle.fail_fast,
        before.next_question == middle.next_question,
        transition_post(middle, after, i, source, reply, out_before, out_after),
    ensures transition_post(before, after, i, source, reply, out_before, out_after),
{
    reveal(transition_post); reveal(port_review_reply::accepted_reply);
    if let Reply::MakerNeedsFiles { files, fits } = reply {
        footprint::storage_frame(before, middle, files, files);
        if footprint::valid_footprint(before, files) {
            files_transport(before, middle, after, i, source, files, fits, out_before, out_after);
        }
    }
}

}
