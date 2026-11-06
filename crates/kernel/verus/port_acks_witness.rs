//! #806: executable witnesses in the exact original two-node settled domain.
use super::port_acks_specs::*;
use super::retirement::{accepted, bounded, domain, node_rules};
use super::*;

verus! {
/// #806: every bound in Kani's any_settled_world constructor, including small heads.
pub(super) open spec fn original_bounds(w: World) -> bool {
    domain(w) && bounded(w) && w.file_sets@.len() == 0
        && w.pool_capacity <= 2 && (forall|r: int| 0 <= r < 2 ==> w.gates@[r].w <= 3)
        && forall|i: int| 0 <= i < 2 ==> {
            let n = #[trigger] w.nodes@[i];
            n.repo.0 < 2 && n.last_cmd < 1099511627776u64
                && n.credit.start < 1073741824u64 && n.credit.earned < 1073741824u64
                && n.credit.added < 1073741824u64 && n.credit.spent < 1073741824u64
                && (match n.proposed { Some(h) => h.0 < 8, None => true })
                && (match n.head { Some(h) => h.0 < 8, None => true })
                && (match n.checked { Some(h) => h.0 < 8, None => true })
                && (match n.approved { Some(h) => h.0 < 8, None => true })
                && (match n.merged { Some(h) => h.0 < 8, None => true })
                && (match n.ticket { Some(t) => t < 1099511627776u64, None => true })
                && (match n.run { Some(g) => g < 1099511627776u64, None => true })
                && (match n.job { Some(g) => g < 1099511627776u64, None => true })
                && (match n.effect { Some(e) => e.seq < 1099511627776u64, None => true })
                && (match n.question { Some(q) => q.id < 1099511627776u64, None => true })
        }
}
}

kernel_fn! {
/// #806 / #629: bounded consistent settled Publishing worlds cover all eight events.
/// Cases 4/5/6 select stale, cancelled and non-OpenPr body refusals respectively.
pub(super) fn original_world(case: u8, fail_fast: bool) -> (result: World) [
    requires case < 11,
    ensures original_bounds(result), accepted(result), storage_ordered(result), settled_world(result),
        result.nodes@[0].phase == Phase::Publishing,
        result.nodes@[0].pos == if case == 6 { 0u8 } else { 2u8 },
        result.nodes@[0].effect == Some(InFlight { seq: 0,
            kind: if case == 6 { EffectKind::Push } else { EffectKind::OpenPr }, cancelling: case == 5 }),
        result.nodes@[0].next_seq == 1, result.nodes@[0].last_cmd == 0,
        result.nodes@[0].fx_failures == 0, result.nodes@[0].fx_incidents == 0,
        result.nodes@[0].paused == (case == 9), result.fail_fast == fail_fast,
        result.nodes@[0].question.is_none(), result.next_question == 0,
] proof [reveal(initial_node); reveal(accepted); reveal(node_rules); reveal(bounded);
    reveal(original_bounds); reveal(storage_ordered); reveal(footprint::ordered_table);] {
    let mut first = Node::new(RepoId(0));
    let mut second = Node::new(RepoId(1));
    second.phase = Phase::Closed(Outcome::Stopped);
    first.phase = Phase::Publishing;
    first.pos = if case == 6 { 0 } else { 2 };
    first.credit.ready = true;
    first.credit.start = 4;
    first.attempt = 1;
    first.footprint = Footprint::inline(1);
    first.head = Some(HeadId(1));
    first.checked = first.head;
    first.approved = first.head;
    first.holds = true;
    first.pr_open = true;
    first.had_pr = true;
    first.next_seq = 1;
    first.effect = Some(InFlight { seq: 0,
        kind: if case == 6 { EffectKind::Push } else { EffectKind::OpenPr }, cancelling: case == 5 });
    first.paused = case == 9;
    let mut nodes = Vec::new();
    nodes.push(first);
    nodes.push(second);
    let mut gates = Vec::new();
    gates.push(Gate { w: 1 });
    gates.push(Gate { w: 1 });
    let w = World { nodes, gates, pool_capacity: 1, next_ticket: 0, next_question: 0,
        file_sets: Vec::new(), dependencies: Vec::new(), fail_fast };
    kernel_proof! {
        footprint::inline_is_valid(w, 0);
        footprint::inline_is_valid(w, 1);
        assert forall|i: int| 0 <= i < 2 implies node_rules(w, w.nodes@[i]) by { assert(i == 0 || i == 1); }
        assert(accepted(w));
        assert(original_bounds(w)) by { assert forall|i: int| 0 <= i < 2 implies {
            let n = #[trigger] w.nodes@[i];
            n.generation < 1099511627776u64 && n.next_seq < 1099511627776u64
                && n.credit.start < 1099511627776u64 && n.credit.earned < 1099511627776u64
                && n.credit.added < 1099511627776u64 && n.credit.spent < 1099511627776u64
        } by { assert(i == 0 || i == 1); } }
        reveal(settled_world); reveal(movable);
        assert forall|i: int| 0 <= i < 2 implies !movable(w, i) by { assert(i == 0 || i == 1); }
    }
    w
}
}

kernel_fn! {
/// #806: copy an original-domain settled world once, before any concrete cover transition.
pub(super) fn prepared_original(case: u8, fail_fast: bool) -> (result: (World, World)) [
    requires case < 11,
    ensures port_acks_apply::same_world(result.0, result.1),
        original_bounds(result.0), accepted(result.0), settled_world(result.0),
        original_bounds(result.1), accepted(result.1), storage_ordered(result.1),
        result.0.nodes@[0].phase == Phase::Publishing,
        result.0.nodes@[0].pos == if case == 6 { 0u8 } else { 2u8 },
        result.0.nodes@[0].effect == Some(InFlight { seq: 0,
            kind: if case == 6 { EffectKind::Push } else { EffectKind::OpenPr }, cancelling: case == 5 }),
        result.0.nodes@[0].next_seq == 1, result.0.nodes@[0].last_cmd == 0,
        result.0.nodes@[0].fx_failures == 0, result.0.nodes@[0].fx_incidents == 0,
        result.0.nodes@[0].paused == (case == 9), result.0.fail_fast == fail_fast,
        result.0.nodes@[0].question.is_none(), result.0.next_question == 0,
 ] {
    let w = original_world(case, fail_fast);
    let before = snapshot_world(&w);
    kernel_proof! {
        reveal(accepted); reveal(bounded); reveal(original_bounds); reveal(settled_world);
        assert(before.file_sets@ =~= w.file_sets@);
        retirement::node_frame(w, before, w.nodes@[0]);
        retirement::node_frame(w, before, w.nodes@[1]);
        footprint::storage_frame(w, before, w.nodes@[0].footprint, w.nodes@[1].footprint);
        assert forall|i: int| 0 <= i < before.nodes@.len() implies !movable(before, i) by {
            movable_frame(w, before, i);
        }
    }
    (before, w)
}
}

kernel_fn! {
/// #806 / #629: real Confirmed application supplies this original-domain reachability case.
pub(super) fn confirmed_cover(fail_fast: bool) -> (result: (World, World, Outbox)) [
    ensures original_bounds(result.0), accepted(result.0), settled_world(result.0), accepted(result.1),
        result.1.nodes@[0] != result.0.nodes@[0],
        result.1.nodes@[0].pos == 3,
] proof [reveal(ack_output); reveal(retry_output); reveal(ack_node); reveal(world_frame); reveal(appended); reveal(ack_emissions); reveal(port_acks_specs::empty); reveal(confirmed_node); reveal(confirmed_cleanup); reveal(confirmed_agenda_node);] {
    let (before, w) = prepared_original(0, fail_fast);
    let (after, out) = port_acks_apply::verify_ack_apply(w, 0, Ack::Confirmed);
    (before, after, out)
}
}

kernel_fn! {
/// #806 / #629: real Retryable application supplies this original-domain reachability case.
pub(super) fn retryable_cover(fail_fast: bool) -> (result: (World, World, Outbox)) [
    ensures original_bounds(result.0), accepted(result.0), settled_world(result.0), accepted(result.1),
        result.1.nodes@[0] != result.0.nodes@[0],
        result.1.nodes@[0].next_seq == 2,
] proof [reveal(ack_output); reveal(retry_output); reveal(ack_node); reveal(world_frame); reveal(appended); reveal(ack_emissions); reveal(port_acks_specs::empty); reveal(retry_node); reveal(failed_delivery); reveal(emitted_node);] {
    let (before, w) = prepared_original(1, fail_fast);
    let (after, out) = port_acks_apply::verify_ack_apply(w, 0, Ack::Retryable);
    (before, after, out)
}
}

kernel_fn! {
/// #806 / #629: real Permanent application supplies this original-domain reachability case.
pub(super) fn permanent_cover(fail_fast: bool) -> (result: (World, World, Outbox)) [
    ensures original_bounds(result.0), accepted(result.0), settled_world(result.0), accepted(result.1),
        result.1.nodes@[0] != result.0.nodes@[0],
        result.1.nodes@[0].phase == Phase::AwaitingRob && result.2.len == 2,
] proof [reveal(ack_output); reveal(retry_output); reveal(ack_node); reveal(world_frame); reveal(appended); reveal(ack_emissions); reveal(port_acks_specs::empty); reveal(permanent_node);] {
    let (before, w) = prepared_original(2, fail_fast);
    let (after, out) = port_acks_apply::verify_ack_apply(w, 0, Ack::Permanent);
    (before, after, out)
}
}

kernel_fn! {
/// #806 / #629: real BodyPolicyRefused application supplies this original-domain reachability case.
pub(super) fn open_pr_cover(fail_fast: bool) -> (result: (World, World, Outbox)) [
    ensures original_bounds(result.0), accepted(result.0), settled_world(result.0), accepted(result.1),
        result.1.nodes@[0] != result.0.nodes@[0],
        publication(result.0.nodes@[0], 0, Ack::BodyPolicyRefused) && result.0.fail_fast == fail_fast && result.2.len == 0 && result.1.nodes@[0].phase == Phase::Stopping(Outcome::Stopped) && result.1.nodes@[0].review_stop == Some(ReviewStop::PublicationBody),
] proof [reveal(ack_output); reveal(retry_output); reveal(ack_node); reveal(world_frame); reveal(appended); reveal(ack_emissions); reveal(port_acks_specs::empty); reveal(retirement::stopped);] {
    let (before, w) = prepared_original(3, fail_fast);
    let (after, out) = port_acks_apply::verify_ack_apply(w, 0, Ack::BodyPolicyRefused);
    (before, after, out)
}
}

kernel_fn! {
/// #806 / #629: real BodyPolicyRefused application supplies this original-domain reachability case.
pub(super) fn stale_refusal_cover(fail_fast: bool) -> (result: (World, World, Outbox)) [
    ensures original_bounds(result.0), accepted(result.0), settled_world(result.0), accepted(result.1),
        // The real event below uses seq=1, so this exact seq=0 delivery is stale.
        result.0.nodes@[0].effect == Some(InFlight { seq: 0, kind: EffectKind::OpenPr, cancelling: false }),
        result.1.nodes@ == result.0.nodes@ && result.2.len == 0,
] proof [reveal(ack_output); reveal(retry_output); reveal(ack_node); reveal(world_frame); reveal(appended); reveal(ack_emissions); reveal(port_acks_specs::empty); ] {
    let (before, w) = prepared_original(4, fail_fast);
    let (after, out) = port_acks_apply::verify_ack_apply(w, 1, Ack::BodyPolicyRefused);
    kernel_proof! { assert(after.nodes@ =~= before.nodes@); }
    (before, after, out)
}
}

kernel_fn! {
/// #806 / #629: real BodyPolicyRefused application supplies this original-domain reachability case.
pub(super) fn cancelled_refusal_cover(fail_fast: bool) -> (result: (World, World, Outbox)) [
    ensures original_bounds(result.0), accepted(result.0), settled_world(result.0), accepted(result.1),
        // The real event below uses the matching seq=0 cancellation.
        result.0.nodes@[0].effect == Some(InFlight { seq: 0, kind: EffectKind::OpenPr, cancelling: true }),
        result.1.nodes@[0] != result.0.nodes@[0],
        result.1.nodes@[0] == (Node { effect: None, ..result.0.nodes@[0] }) && result.2.len == 0,
] proof [reveal(ack_output); reveal(retry_output); reveal(ack_node); reveal(world_frame); reveal(appended); reveal(ack_emissions); reveal(port_acks_specs::empty); ] {
    let (before, w) = prepared_original(5, fail_fast);
    let (after, out) = port_acks_apply::verify_ack_apply(w, 0, Ack::BodyPolicyRefused);
    (before, after, out)
}
}

kernel_fn! {
/// #806 / #629: real BodyPolicyRefused application supplies this original-domain reachability case.
pub(super) fn other_refusal_cover(fail_fast: bool) -> (result: (World, World, Outbox)) [
    ensures original_bounds(result.0), accepted(result.0), settled_world(result.0), accepted(result.1),
        result.1.nodes@[0] != result.0.nodes@[0],
        result.0.nodes@[0].effect == Some(InFlight { seq: 0, kind: EffectKind::Push, cancelling: false }) && result.1.nodes@[0].phase == Phase::AwaitingRob && result.2.len == 2,
] proof [reveal(ack_output); reveal(retry_output); reveal(ack_node); reveal(world_frame); reveal(appended); reveal(ack_emissions); reveal(port_acks_specs::empty); reveal(permanent_node);] {
    let (before, w) = prepared_original(6, fail_fast);
    let (after, out) = port_acks_apply::verify_ack_apply(w, 0, Ack::BodyPolicyRefused);
    (before, after, out)
}
}

kernel_fn! {
/// #806 covers: real Confirmed, Retryable, Permanent and OpenPr refusal change the world.
/// #629 covers: OpenPr refusal with both flags, stale, cancelled and other effect kinds.
pub(super) fn ack_cover(case: u8, fail_fast: bool) -> (result: (World, World, Outbox)) [
    requires case < 7,
    ensures original_bounds(result.0), accepted(result.0), settled_world(result.0),
        accepted(result.1),
        case != 4 ==> result.1.nodes@[0] != result.0.nodes@[0],
        case == 0 ==> result.1.nodes@[0].pos == 3,
        case == 1 ==> result.1.nodes@[0].next_seq == 2,
        case == 2 || case == 6 ==> result.1.nodes@[0].phase == Phase::AwaitingRob && result.2.len == 2,
        case == 3 ==> publication(result.0.nodes@[0], 0, Ack::BodyPolicyRefused)
            && result.0.fail_fast == fail_fast && result.2.len == 0
            && result.1.nodes@[0].phase == Phase::Stopping(Outcome::Stopped)
            && result.1.nodes@[0].review_stop == Some(ReviewStop::PublicationBody),
        case == 4 ==> result.0.nodes@[0].effect == Some(InFlight { seq: 0, kind: EffectKind::OpenPr, cancelling: false })
            && result.1.nodes@ == result.0.nodes@ && result.2.len == 0,
        case == 5 ==> result.0.nodes@[0].effect == Some(InFlight { seq: 0, kind: EffectKind::OpenPr, cancelling: true })
            && result.1.nodes@[0] == (Node { effect: None, ..result.0.nodes@[0] }) && result.2.len == 0,
        case == 6 ==> result.0.nodes@[0].effect == Some(InFlight { seq: 0, kind: EffectKind::Push, cancelling: false }),
] {
    match case {
        0 => confirmed_cover(fail_fast),
        1 => retryable_cover(fail_fast),
        2 => permanent_cover(fail_fast),
        3 => open_pr_cover(fail_fast),
        4 => stale_refusal_cover(fail_fast),
        5 => cancelled_refusal_cover(fail_fast),
        _ => other_refusal_cover(fail_fast),
    }
}
}

kernel_fn! {
/// #806 covers: each real owner command changes a bounded, consistent settled world.
pub(super) fn command_cover(case: u8) -> (result: (World, World, Outbox)) [
    requires 7 <= case < 11,
    ensures original_bounds(result.0), accepted(result.0), settled_world(result.0), accepted(result.1),
        result.1.nodes@[0] != result.0.nodes@[0], result.2.len == 0,
        case == 7 ==> result.1.nodes@[0].phase == Phase::Stopping(Outcome::Stopped),
        case == 8 ==> !result.0.nodes@[0].paused && result.1.nodes@[0].paused,
        case == 9 ==> result.0.nodes@[0].paused && !result.1.nodes@[0].paused,
        case == 10 ==> result.1.nodes@[0].phase == Phase::Stopping(Outcome::Replaced),
] proof [reveal(command_node); reveal(retirement::stopped);] {
    let w = original_world(case, false);
    let before = snapshot_world(&w);
    kernel_proof! {
        reveal(accepted); reveal(bounded); reveal(original_bounds); reveal(settled_world);
        assert(before.file_sets@ =~= w.file_sets@);
        retirement::node_frame(w, before, w.nodes@[0]);
        retirement::node_frame(w, before, w.nodes@[1]);
        footprint::storage_frame(w, before, w.nodes@[0].footprint, w.nodes@[1].footprint);
        assert forall|i: int| 0 <= i < before.nodes@.len() implies !movable(before, i) by {
            movable_frame(w, before, i);
        }
    }
    let command = match case { 7 => Command::Stop, 8 => Command::Pause, 9 => Command::Resume, _ => Command::ChangeRequest };
    let (after, out) = port_acks_commands::verify_command_apply(w, 1, command);
    (before, after, out)
}
}
