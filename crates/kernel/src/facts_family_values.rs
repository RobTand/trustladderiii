//! Fact value producers and exact bounded notice transport.
use super::*;

#[cfg(verus_keep_ghost)]
verus! {
#[verifier::opaque]
pub(super) open spec fn node_rows(b: Node, n: Node, fast: bool, i: usize, fact: Fact) -> bool {
    transition_safety::node_step(b, n)
        && (forall|origin: World| #[trigger] retirement::node_rules(origin, b)
            ==> transition_safety::fail_fast_node(b, n, fast,
                Event::Fact { node: NodeId(i as u64), fact }, i as int))
        && transition_safety::node_event_rules(b, n, fast,
            Event::Fact { node: NodeId(i as u64), fact })
}

#[verifier::opaque]
pub(super) open spec fn notices(notes: Outbox, i: usize, bound: int) -> bool {
    notes.len <= bound
        && (forall|k: int| 0 <= k < notes.len ==> match #[trigger] notes.items@[k] {
            Some(Out::Notice { node, .. }) => node == NodeId(i as u64),
            _ => false,
        })
        && (forall|k: int| notes.len <= k < OUTBOX ==> #[trigger] notes.items@[k].is_none())
}

pub(super) open spec fn priority_node(n: Node, priority: Priority) -> Node {
    if n.phase is Closed { n } else { Node { priority, ..n } }
}

pub(super) open spec fn closed_pr_node(n: Node) -> Node {
    if !n.pr_open { n } else {
        let closed = Node { pr_open: false, ..n };
        if n.phase is Stopping || n.phase is Closed { closed }
        else { retirement::stopped(closed, Outcome::Stopped) }
    }
}

pub(super) open spec fn conflict_node(n: Node) -> Node {
    if n.phase == Phase::AwaitingDelivery {
        retirement::phased(n, Phase::Ready(Resume::Rebasing))
    } else if n.phase == Phase::Publishing {
        retirement::phased(n, Phase::Rebasing)
    } else { n }
}

pub(super) open spec fn cleared_node(n: Node, by_rob: bool) -> Node {
    if n.waiting_incident && (by_rob || !n.incident_needs_rob) {
        Node { waiting_incident: false, incident_needs_rob: false, ..n }
    } else { n }
}

pub(super) open spec fn merged_credit(c: Credit) -> Credit {
    Credit { merge_credited: true,
        earned: if c.merge_credited { c.earned } else { (c.earned + 1) as u64 }, ..c }
}

pub(super) open spec fn merged_node(n: Node, head: HeadId) -> Node {
    if !n.had_pr || n.merged.is_some() { n } else { match n.phase {
        Phase::Closed(Outcome::Stopped) | Phase::Closed(Outcome::Replaced) => Node {
            merged: Some(head), phase: Phase::Closed(Outcome::Merged),
            credit: merged_credit(n.credit), ..n
        },
        Phase::Closed(_) => n,
        _ => retirement::stopped(Node { merged: Some(head), pr_open: false, ..n }, Outcome::Merged),
    } }
}

pub(super) open spec fn fact_value_node(n: Node, fact: Fact) -> Node {
    match fact {
        Fact::Priority(priority) => priority_node(n, priority),
        Fact::PrClosedByPerson => closed_pr_node(n),
        Fact::Conflict => conflict_node(n),
        Fact::IncidentCleared { by_rob } => cleared_node(n, by_rob),
        Fact::PrMerged { head, .. } => merged_node(n, head),
        _ => n,
    }
}

pub(super) open spec fn ordinary_items(n: Node, i: usize, fact: Fact) -> Seq<Option<Out>> {
    if fact == Fact::PrClosedByPerson && n.pr_open
        && !(n.phase is Stopping || n.phase is Closed) {
        seq![Some(Out::Notice { node: NodeId(i as u64), notice: Notice::Note(NoteKind::PrClosed) })]
    } else { seq![] }
}

pub(super) open spec fn padded_notices(items: Seq<Option<Out>>) -> Seq<Option<Out>> {
    Seq::new(OUTBOX as nat, |k: int| if k < items.len() { items[k] } else { None })
}

pub(super) proof fn fact_value_bridge(n: Node, fact: Fact)
    requires matches!(fact, Fact::Priority(_) | Fact::PrClosedByPerson | Fact::Conflict
        | Fact::IncidentCleared { .. } | Fact::PrMerged { .. }),
    ensures fact_value_node(n, fact) == retirement::fact_node(n, fact),
{
    reveal(retirement::fact_node);
}

pub(super) open spec fn merge_items(n: Node, i: usize,
    head: HeadId, checks_passed: bool) -> Seq<Option<Out>> {
    let late = matches!(n.phase, Phase::Closed(Outcome::Stopped) | Phase::Closed(Outcome::Replaced));
    let active = n.had_pr && n.merged.is_none() && (!(n.phase is Closed) || late);
    let unapproved = !(n.approved == Some(head) && checks_passed);
    if !active { seq![] } else {
        let prefix = if late { seq![
            Some(Out::Notice { node: NodeId(i as u64), notice: Notice::Note(NoteKind::LateMerge) }),
            Some(Out::Notice { node: NodeId(i as u64), notice: Notice::Incident { needs_rob: false } }),
        ] } else { seq![] };
        if unapproved { prefix + seq![
            Some(Out::Notice { node: NodeId(i as u64), notice: Notice::Incident { needs_rob: false } }),
            Some(Out::Notice { node: NodeId(i as u64), notice: Notice::Note(NoteKind::UnapprovedMerge) }),
        ] } else { prefix }
    }
}

/// Exact ordered list, including the unoccupied slots.
#[verifier::opaque]
pub(super) open spec fn merge_notices(n: Node, notes: Outbox, i: usize,
    head: HeadId, checks_passed: bool) -> bool {
    notes.len == merge_items(n, i, head, checks_passed).len()
        && notes.items@ =~= padded_notices(merge_items(n, i, head, checks_passed))
}

pub(super) proof fn value_node_step(b: Node, n: Node, fast: bool, i: usize, fact: Fact)
    requires node_headroom(b), n == retirement::fact_node(b, fact),
        matches!(fact, Fact::Priority(_) | Fact::PrClosedByPerson | Fact::Conflict
            | Fact::IncidentCleared { .. } | Fact::PrMerged { .. }),
    ensures transition_safety::node_step(b, n),
{
    reveal(retirement::fact_node);
    match fact {
        Fact::PrClosedByPerson | Fact::PrMerged { .. } => { reveal(retirement::stopped); },
        _ => {},
    }
}

pub(super) proof fn value_fail_fast(b: Node, n: Node, fast: bool, i: usize, fact: Fact)
    requires node_headroom(b), n == retirement::fact_node(b, fact),
        matches!(fact, Fact::Priority(_) | Fact::PrClosedByPerson | Fact::Conflict
            | Fact::IncidentCleared { .. } | Fact::PrMerged { .. }),
    ensures transition_safety::fail_fast_node(b, n, fast,
        Event::Fact { node: NodeId(i as u64), fact }, i as int),
{
    reveal(retirement::fact_node);
    match fact {
        Fact::PrClosedByPerson | Fact::PrMerged { .. } => { reveal(retirement::stopped); },
        _ => {},
    }
}

pub(super) proof fn value_event_rules(b: Node, n: Node, fast: bool, i: usize, fact: Fact)
    requires node_headroom(b), n == retirement::fact_node(b, fact),
        matches!(fact, Fact::Priority(_) | Fact::PrClosedByPerson | Fact::Conflict
            | Fact::IncidentCleared { .. } | Fact::PrMerged { .. }),
    ensures transition_safety::node_event_rules(b, n, fast,
        Event::Fact { node: NodeId(i as u64), fact }),
{
    reveal(retirement::fact_node);
    match fact {
        Fact::PrClosedByPerson | Fact::PrMerged { .. } => { reveal(retirement::stopped); },
        _ => {},
    }
}

/// Compose separately measured node properties without exposing their updates.
pub(super) proof fn value_node_rows(b: Node, n: Node, fast: bool, i: usize, fact: Fact)
    requires node_headroom(b), n == retirement::fact_node(b, fact),
        matches!(fact, Fact::Priority(_) | Fact::PrClosedByPerson | Fact::Conflict
            | Fact::IncidentCleared { .. } | Fact::PrMerged { .. }),
    ensures node_rows(b, n, fast, i, fact),
{
    value_node_step(b, n, fast, i, fact);
    value_fail_fast(b, n, fast, i, fact);
    value_event_rules(b, n, fast, i, fact);
    reveal(node_rows);
}

pub(super) proof fn ordinary_frame(b: Node, n: Node, fact: Fact)
    requires node_headroom(b), n == retirement::fact_node(b, fact),
        matches!(fact, Fact::Priority(_) | Fact::PrClosedByPerson
            | Fact::Conflict | Fact::IncidentCleared { .. }),
    ensures n.repo == b.repo, n.footprint == b.footprint,
        settle_shape(b) ==> settle_shape(n),
        n.generation as int <= b.generation as int + 1,
        n.next_seq == b.next_seq, n.credit == b.credit, n.last_cmd == b.last_cmd,
        b.phase is Closed ==> n.phase is Closed,
{
    reveal(retirement::fact_node);
    match fact {
        Fact::PrClosedByPerson => { reveal(retirement::stopped); },
        _ => {},
    }
}

pub(super) proof fn ordinary_distance(b: Node, n: Node, fact: Fact)
    requires n == retirement::fact_node(b, fact), arithmetic_consistent(b),
        fact == (Fact::IncidentCleared { by_rob: true }),
        b.waiting_incident, b.effect.is_none(), (!b.paused || b.phase is Stopping),
        !(b.phase is Closed),
    ensures distance(n) < distance(b),
{
    reveal(retirement::fact_node);
    reveal(distance);
}

pub(super) proof fn empty_merge_notices(n: Node, notes: Outbox, i: usize,
    head: HeadId, checks_passed: bool)
    requires empty_outbox(notes),
        !(n.had_pr && n.merged.is_none() && (!(n.phase is Closed)
            || matches!(n.phase, Phase::Closed(Outcome::Stopped) | Phase::Closed(Outcome::Replaced)))),
    ensures merge_notices(n, notes, i, head, checks_passed),
{
    reveal(empty_outbox);
    reveal(merge_notices);
    assert(notes.items@ =~= Seq::new(OUTBOX as nat, |k: int|
        if k < Seq::<Option<Out>>::empty().len() { Seq::<Option<Out>>::empty()[k] } else { None }));
}

pub(super) proof fn ordinary_outputs(n: Node, notes: Outbox, i: usize, fact: Fact)
    requires matches!(fact, Fact::Priority(_) | Fact::PrClosedByPerson
            | Fact::Conflict | Fact::IncidentCleared { .. }),
        notes.len == ordinary_items(n, i, fact).len(),
        notes.items@ =~= padded_notices(ordinary_items(n, i, fact)),
    ensures notices(notes, i, 1), retirement::notices_only(notes),
        fact is Priority || fact is Conflict || fact is IncidentCleared ==> notes.len == 0,
        !(fact == Fact::PrClosedByPerson && n.pr_open
            && !(n.phase is Stopping || n.phase is Closed)) ==> notes.len == 0,
        fact == Fact::PrClosedByPerson && n.pr_open
            && !(n.phase is Stopping || n.phase is Closed)
            ==> notes.len == 1 && notes.items@[0] == Some(Out::Notice {
                node: NodeId(i as u64), notice: Notice::Note(NoteKind::PrClosed) }),
{
    reveal(notices);
    reveal(retirement::notices_only);
}

pub(super) proof fn merge_frame(n: Node, value: Node, head: HeadId, checks_passed: bool)
    requires node_headroom(n), value == retirement::fact_node(n, Fact::PrMerged { head, checks_passed }),
    ensures value.repo == n.repo, value.footprint == n.footprint,
        settle_shape(n) ==> settle_shape(value),
        n.phase is Closed ==> value.phase is Closed,
        value.generation as int <= n.generation as int + 1,
        value.next_seq == n.next_seq,
        value.credit.start == n.credit.start,
        value.credit.earned as int <= n.credit.earned as int + 1,
        value.credit.added == n.credit.added, value.credit.spent == n.credit.spent,
        value.credit.earned as int + merge_capacity(value) <= n.credit.earned as int + 1,
        value.last_cmd == n.last_cmd,
{
    reveal(retirement::fact_node);
    reveal(retirement::stopped);
}

pub(super) proof fn merge_distance(n: Node, value: Node, head: HeadId, checks_passed: bool)
    requires n.had_pr, n.merged.is_none(), n.phase == Phase::AwaitingDelivery,
        !n.paused, !n.waiting_incident, n.effect.is_none(),
        value == retirement::fact_node(n, Fact::PrMerged { head, checks_passed }),
    ensures distance(value) < distance(n),
{
    reveal(retirement::fact_node);
    let prepared = Node { merged: Some(head), pr_open: false, ..n };
    retirement::merge_input_distance(n, head);
    allocation_stopping_lemmas::stopping_progress(prepared, value, Outcome::Merged);
}

pub(super) proof fn merge_outputs(n: Node, notes: Outbox, i: usize,
    head: HeadId, checks_passed: bool)
    requires notes.len == merge_items(n, i, head, checks_passed).len(),
        notes.items@ =~= padded_notices(merge_items(n, i, head, checks_passed)),
    ensures merge_notices(n, notes, i, head, checks_passed),
        notices(notes, i, 4), retirement::notices_only(notes),
        n.had_pr && n.merged.is_none() && !(n.phase is Closed)
            && !(n.approved == Some(head) && checks_passed)
            ==> retirement::unapproved_note(notes, i),
{
    reveal(merge_notices);
    reveal(notices);
    reveal(retirement::notices_only);
    reveal(retirement::unapproved_note);
    if n.had_pr && n.merged.is_none() && !(n.phase is Closed)
        && !(n.approved == Some(head) && checks_passed) {
        assert(notes.items@[1] == Some(Out::Notice { node: NodeId(i as u64),
            notice: Notice::Note(NoteKind::UnapprovedMerge) }));
    }
}

#[verifier::opaque]
pub(super) open spec fn appended(ob: Outbox, oa: Outbox, notes: Outbox) -> bool {
    oa.len == ob.len + notes.len
        && forall|k: int| 0 <= k < OUTBOX ==> #[trigger] oa.items@[k]
            == if ob.len <= k < oa.len { notes.items@[k - ob.len] } else { ob.items@[k] }
}

#[verifier::opaque]
pub(super) open spec fn notice_suffix(ob: Outbox, oa: Outbox, i: usize) -> bool {
    ob.len <= oa.len <= OUTBOX
        && forall|k: int| ob.len <= k < oa.len ==> match #[trigger] oa.items@[k] {
            Some(Out::Notice { node, .. }) => node == NodeId(i as u64),
            _ => false,
        }
}
}

kernel_fn! {
pub(super) fn priority_fact_arm(n: Node, i: usize, priority: Priority)
    -> (result: (Node, Outbox)) [
    requires node_headroom(n),
    ensures result.0 == priority_node(n, priority),
        result.1.len == ordinary_items(n, i, Fact::Priority(priority)).len(),
        result.1.items@ =~= padded_notices(ordinary_items(n, i, Fact::Priority(priority))),
] proof [reveal(empty_outbox);] {
    let mut value = n;
    let mut notes = Outbox::new();
    let out = &mut notes;
    if !value.phase.is_closed() {
        value.priority = priority;
    }
    kernel_proof! {
        assert(notes.items@ =~= padded_notices(ordinary_items(n, i, Fact::Priority(priority))));
    }
    (value, notes)
}
}

kernel_fn! {
pub(super) fn closed_pr_fact_arm(n: Node, i: usize)
    -> (result: (Node, Outbox)) [
    requires node_headroom(n),
    ensures result.0 == closed_pr_node(n),
        result.1.len == ordinary_items(n, i, Fact::PrClosedByPerson).len(),
        result.1.items@ =~= padded_notices(ordinary_items(n, i, Fact::PrClosedByPerson)),
] proof [reveal(empty_outbox);] {
    let mut value = n;
    let mut notes = Outbox::new();
    let out = &mut notes;
    let m = &mut value;
    if !m.pr_open {
        kernel_proof! {
            assert(notes.items@ =~= padded_notices(ordinary_items(n, i, Fact::PrClosedByPerson)));
        }
        return (value, notes);
    }
    m.pr_open = false;
    if !m.phase.is_stopping() && !m.phase.is_closed() {
        notice(out, i, Notice::Note(NoteKind::PrClosed));
        enter_stopping(m, Outcome::Stopped);
    }
    kernel_proof! {
        assert(notes.items@ =~= padded_notices(ordinary_items(n, i, Fact::PrClosedByPerson)));
    }
    (value, notes)
}
}

kernel_fn! {
pub(super) fn conflict_fact_arm(n: Node, i: usize)
    -> (result: (Node, Outbox)) [
    requires node_headroom(n),
    ensures result.0 == conflict_node(n),
        result.1.len == ordinary_items(n, i, Fact::Conflict).len(),
        result.1.items@ =~= padded_notices(ordinary_items(n, i, Fact::Conflict)),
] proof [reveal(empty_outbox);] {
    let mut value = n;
    let mut notes = Outbox::new();
    let out = &mut notes;
    let m = &mut value;
    if m.phase == Phase::AwaitingDelivery {
        set_phase(m, Phase::Ready(Resume::Rebasing));
    } else if m.phase == Phase::Publishing {
        set_phase(m, Phase::Rebasing);
    }
    kernel_proof! {
        assert(notes.items@ =~= padded_notices(ordinary_items(n, i, Fact::Conflict)));
    }
    (value, notes)
}
}

kernel_fn! {
pub(super) fn cleared_fact_arm(n: Node, i: usize, by_rob: bool)
    -> (result: (Node, Outbox)) [
    requires node_headroom(n),
    ensures result.0 == cleared_node(n, by_rob),
        result.1.len == ordinary_items(n, i, Fact::IncidentCleared { by_rob }).len(),
        result.1.items@ =~= padded_notices(ordinary_items(n, i, Fact::IncidentCleared { by_rob })),
] proof [reveal(empty_outbox);] {
    let mut value = n;
    let mut notes = Outbox::new();
    let out = &mut notes;
    let m = &mut value;
    if m.waiting_incident && (by_rob || !m.incident_needs_rob) {
        m.waiting_incident = false;
        m.incident_needs_rob = false;
    }
    kernel_proof! {
        assert(notes.items@ =~= padded_notices(ordinary_items(n, i, Fact::IncidentCleared { by_rob })));
    }
    (value, notes)
}
}

kernel_fn! {
/// Give the merge caller a whole credit value in its exact node equation.
pub(super) fn merge_credit_value(n: Node) -> (result: Node) [
    requires n.credit.merge_credited || n.credit.earned < u64::MAX,
    ensures result == (Node { credit: merged_credit(n.credit), ..n }),
] {
    let mut value = n;
    merge_credit(&mut value);
    value
}
}

kernel_fn! {
pub(super) fn merged_fact_arm(n: Node, i: usize, head: HeadId, checks_passed: bool)
    -> (result: (Node, Outbox)) [
    requires node_headroom(n),
    ensures result.0 == merged_node(n, head),
        result.1.len == merge_items(n, i, head, checks_passed).len(),
        result.1.items@ =~= padded_notices(merge_items(n, i, head, checks_passed)),
] proof [reveal(empty_outbox);] {
    let mut value = n;
    let mut notes = Outbox::new();
    let out = &mut notes;
    let m = &mut value;
    if !m.had_pr || m.merged.is_some() {
        kernel_proof! {
            assert(value == merged_node(n, head));
            assert(notes.items@ =~= padded_notices(merge_items(n, i, head, checks_passed)));
        }
        return (value, notes);
    }
    let approved = m.approved == Some(head) && checks_passed;
    match m.phase {
        Phase::Closed(o) => {
            if !matches!(o, Outcome::Stopped | Outcome::Replaced) {
                kernel_proof! {
                    assert(value == merged_node(n, head));
                    assert(notes.items@ =~= padded_notices(merge_items(n, i, head, checks_passed)));
                }
                return (value, notes);
            }
            m.merged = Some(head);
            m.phase = Phase::Closed(Outcome::Merged);
            *m = merge_credit_value(*m);
            kernel_proof! { assert(*m == merged_node(n, head)); }
            notice(out, i, Notice::Note(NoteKind::LateMerge));
            notice(out, i, Notice::Incident { needs_rob: false });
        }
        _ => {
            merge_open(m, head);
            kernel_proof! { assert(*m == merged_node(n, head)); }
        }
    }
    if !approved {
        notice(out, i, Notice::Incident { needs_rob: false });
        notice(out, i, Notice::Note(NoteKind::UnapprovedMerge));
    }
    kernel_proof! {
        assert(notes.items@ =~= padded_notices(merge_items(n, i, head, checks_passed)));
    }
    (value, notes)
}
}

kernel_fn! {
pub(super) fn ordinary_fact_value(n: Node, fast: bool, i: usize, fact: Fact)
    -> (result: (Node, Outbox)) [
    requires node_headroom(n), arithmetic_consistent(n),
        matches!(fact, Fact::Priority(_) | Fact::PrClosedByPerson
            | Fact::Conflict | Fact::IncidentCleared { .. }),
    ensures matches!(fact, Fact::Priority(_) | Fact::PrClosedByPerson
            | Fact::Conflict | Fact::IncidentCleared { .. })
            ==> node_rows(n, result.0, fast, i, fact),
        notices(result.1, i, 1),
        result.0 == retirement::fact_node(n, fact),
        result.0.repo == n.repo, result.0.footprint == n.footprint,
        settle_shape(n) ==> settle_shape(result.0),
        result.0.generation as int <= n.generation as int + 1,
        result.0.next_seq == n.next_seq, result.0.credit == n.credit,
        result.0.last_cmd == n.last_cmd,
        (n.phase is Closed ==> result.0.phase is Closed),
        fact is Priority || fact is Conflict || fact is IncidentCleared ==> result.1.len == 0,
        !(fact == Fact::PrClosedByPerson && n.pr_open
            && !(n.phase is Stopping || n.phase is Closed)) ==> result.1.len == 0,
        fact == Fact::PrClosedByPerson && n.pr_open
            && !(n.phase is Stopping || n.phase is Closed)
            ==> result.1.len == 1 && result.1.items@[0] == Some(Out::Notice {
                node: NodeId(i as u64), notice: Notice::Note(NoteKind::PrClosed) }),
        (fact == Fact::IncidentCleared { by_rob: true } && n.waiting_incident
            && n.effect.is_none() && (!n.paused || n.phase is Stopping)
            && !(n.phase is Closed) ==> distance(result.0) < distance(n)),
        retirement::notices_only(result.1),
] {
    let result = match fact {
        Fact::Priority(priority) => priority_fact_arm(n, i, priority),
        Fact::PrClosedByPerson => closed_pr_fact_arm(n, i),
        Fact::Conflict => conflict_fact_arm(n, i),
        Fact::IncidentCleared { by_rob } => cleared_fact_arm(n, i, by_rob),
        _ => (n, Outbox::new()),
    };
    kernel_proof! {
        fact_value_bridge(n, fact);
        ordinary_frame(n, result.0, fact);
        ordinary_outputs(n, result.1, i, fact);
        value_node_rows(n, result.0, fast, i, fact);
        if fact == (Fact::IncidentCleared { by_rob: true }) && n.waiting_incident
            && n.effect.is_none() && (!n.paused || n.phase is Stopping) && !(n.phase is Closed) {
            ordinary_distance(n, result.0, fact);
        }
    }
    result
}
}

kernel_fn! {
pub(super) fn merge_fact_value(n: Node, fast: bool, i: usize,
    head: HeadId, checks_passed: bool) -> (result: (Node, Outbox)) [
    requires node_headroom(n),
    ensures n.had_pr && n.merged.is_none()
            ==> node_rows(n, result.0, fast, i, Fact::PrMerged { head, checks_passed }),
        !n.had_pr || n.merged.is_some() ==> result.0 == n,
        merge_notices(n, result.1, i, head, checks_passed),
        notices(result.1, i, 4), retirement::notices_only(result.1),
        result.0 == retirement::fact_node(n, Fact::PrMerged { head, checks_passed }),
        result.0.repo == n.repo, result.0.footprint == n.footprint,
        settle_shape(n) ==> settle_shape(result.0),
        (n.phase is Closed ==> result.0.phase is Closed),
        result.0.generation as int <= n.generation as int + 1,
        result.0.next_seq == n.next_seq,
        result.0.credit.start == n.credit.start,
        result.0.credit.earned as int <= n.credit.earned as int + 1,
        result.0.credit.added == n.credit.added, result.0.credit.spent == n.credit.spent,
        result.0.credit.earned as int + merge_capacity(result.0) <= n.credit.earned as int + 1,
        result.0.last_cmd == n.last_cmd,
        (n.had_pr && n.merged.is_none() && n.phase == Phase::AwaitingDelivery
            && !n.paused && !n.waiting_incident && n.effect.is_none()
            ==> distance(result.0) < distance(n)),
        n.had_pr && n.merged.is_none() && !(n.phase is Closed)
            && !(n.approved == Some(head) && checks_passed)
            ==> retirement::unapproved_note(result.1, i),
] {
    let result = merged_fact_arm(n, i, head, checks_passed);
    kernel_proof! {
        let fact = Fact::PrMerged { head, checks_passed };
        fact_value_bridge(n, fact);
        merge_frame(n, result.0, head, checks_passed);
        merge_outputs(n, result.1, i, head, checks_passed);
        value_node_rows(n, result.0, fast, i, fact);
        if !n.had_pr || n.merged.is_some() { assert(result.0 == n); }
        if n.had_pr && n.merged.is_none() && n.phase == Phase::AwaitingDelivery
            && !n.paused && !n.waiting_incident && n.effect.is_none() {
            merge_distance(n, result.0, head, checks_passed);
        }
    }
    result
}
}

kernel_fn! {
pub(super) fn append_fact_notices(out: &mut Outbox, notes: Outbox, i: usize) -> (result: ()) [
    requires notices(notes, i, 4), old(out).len + notes.len <= OUTBOX,
    ensures appended(*old(out), *final(out), notes), notice_suffix(*old(out), *final(out), i),
        final(out).len == old(out).len + notes.len,
        notes.len == 0 ==> *final(out) == *old(out),
        retirement::notices_only(*old(out)) ==> retirement::notices_only(*final(out)),
        retirement::unapproved_note(notes, i) ==> retirement::unapproved_note(*final(out), i),
] proof [reveal(notices); reveal(appended); reveal(notice_suffix);
    reveal(retirement::notices_only); reveal(retirement::unapproved_note);] {
    kernel_snapshot! { before_append = *out }
    let mut k = 0usize;
    kernel_while! { (k < notes.len) [
        invariant notices(notes, i, 4), k <= notes.len, out.len == before_append@.len + k,
            notes.len == 0 ==> out.items == before_append@.items,
            before_append@.len + notes.len <= OUTBOX,
            forall|j: int| 0 <= j < OUTBOX ==> out.items@[j]
                == if before_append@.len <= j < out.len {
                    notes.items@[j - before_append@.len]
                } else { before_append@.items@[j] },
        decreases notes.len - k,
    ] {
        kernel_proof! { reveal(notices); assert(notes.items@[k as int].is_some()); }
        match notes.items[k] {
            Some(item) => out.push(item),
            None => {}
        }
        k += 1;
    } }
    kernel_proof! {
        if notes.len == 0 {
            assert(out.items@ =~= before_append@.items@);
            assert(out.len == before_append@.len);
        }
        if retirement::unapproved_note(notes, i) {
            let note = Some(Out::Notice { node: NodeId(i as u64),
                notice: Notice::Note(NoteKind::UnapprovedMerge) });
            let j = choose|j: int| 0 <= j < notes.items@.len() && notes.items@[j] == note;
            reveal(notices);
            if j >= notes.len { assert(notes.items@[j].is_none()); }
            assert(j < notes.len);
            assert(out.items@[before_append@.len + j] == note);
        }
    }
}
}

kernel_fn! {
pub(super) fn ordinary_fact_entry(w: &mut World, i: usize, fact: Fact,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len + 1 <= OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]),
        matches!(fact, Fact::Priority(_) | Fact::PrClosedByPerson
            | Fact::Conflict | Fact::IncidentCleared { .. }),
    ensures prefix_preserved(*old(out), *final(out)),
        retirement::fact_frame(*old(w), *final(w), i, *old(out), *final(out)),
        final(w).nodes@[i as int] == retirement::fact_node(old(w).nodes@[i as int], fact),
        matches!(fact, Fact::Priority(_) | Fact::PrClosedByPerson
            | Fact::Conflict | Fact::IncidentCleared { .. })
            ==> node_rows(old(w).nodes@[i as int], final(w).nodes@[i as int], old(w).fail_fast, i, fact),
        notice_suffix(*old(out), *final(out), i),
        final(w).next_question == old(w).next_question,
        retirement::notices_only(*old(out)) ==> retirement::notices_only(*final(out)),
        fact is Priority ==> *final(out) == *old(out),
        (fact == Fact::IncidentCleared { by_rob: true }
            && old(w).nodes@[i as int].waiting_incident && old(w).nodes@[i as int].effect.is_none()
            && (!old(w).nodes@[i as int].paused || old(w).nodes@[i as int].phase is Stopping)
            && !(old(w).nodes@[i as int].phase is Closed)
            ==> distance(final(w).nodes@[i as int]) < distance(old(w).nodes@[i as int])),
] proof [reveal(retirement::fact_frame); reveal(notices);] {
    kernel_snapshot! { before_out = *out }
    let (value, notes) = ordinary_fact_value(w.nodes[i], w.fail_fast, i, fact);
    w.nodes[i] = value;
    append_fact_notices(out, notes, i);
    kernel_proof! { facts_family_tail::append_prefix(before_out@, *out, notes); }
}
}

kernel_fn! {
pub(super) fn fact_dependency_entry(w: &mut World, i: usize, fact: Fact,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), i <= u64::MAX as int,
        old(out).len + 1 <= OUTBOX, dependencies::edges_indexable(*old(w)),
        fact is Dependency || fact is DependencyRemove,
    ensures node_rows(old(w).nodes@[i as int], final(w).nodes@[i as int], old(w).fail_fast, i, fact),
        notice_suffix(*old(out), *final(out), i), prefix_preserved(*old(out), *final(out)),
        dependencies::dependency_frame(*old(w), *final(w), i as int),
        match fact {
            Fact::Dependency { target, status } =>
                dependencies::fact_result(*old(w), *final(w), i as int, target, status),
            Fact::DependencyRemove { kind, number } =>
                dependencies::remove_result(*old(w), *final(w), i as int, kind, number),
            _ => true,
        },
        final(out).len <= old(out).len + 1,
        fact is DependencyRemove ==> *final(out) == *old(out),
        (match fact {
            Fact::Dependency { target, status: DependencyStatus::Delivered } =>
                dependencies::delivery_input(*old(w), i as int, target),
            _ => false,
        } ==> progress::progress_measure(*final(w)) + 1 == progress::progress_measure(*old(w))
            && dependencies::edges_indexable(*final(w))
            && match fact { Fact::Dependency { target, .. } =>
                dependencies::delivery_changed(*old(w), *final(w), i as int, target), _ => false }),
] {
    kernel_snapshot! { before = *w }
    match fact {
        Fact::DependencyRemove { kind, number } => {
            w.remove_dependency(i, kind, number);
            kernel_proof! { reveal(dependencies::dependency_frame); }
        }
        Fact::Dependency { target, status } => w.dependency_fact(i, target, status, out),
        _ => {}
    }
    kernel_proof! {
        reveal(notice_suffix); reveal(prefix_preserved);
        facts_family_tail::dependency_rows(before@, *w, i, fact);
    }
}
}

#[cfg(verus_keep_ghost)]
verus! {
pub(super) open spec fn infrastructure_retry_arm_result(n: Node) -> (Node, Option<bool>) {
    (retirement::phased(Node { run_failures: (n.run_failures + 1) as u8, ..n }, Phase::Publishing), None)
}

pub(super) open spec fn infrastructure_incident_count_result(c: u8) -> u8 {
    if c < 2 { (c + 1) as u8 } else { 2u8 }
}

pub(super) open spec fn infrastructure_incident_arm_result(n: Node) -> (Node, Option<bool>) {
    let count = infrastructure_incident_count_result(n.run_incidents);
    (Node { run_failures: 0, run_incidents: count, ..n }, Some(count >= 2))
}

pub(super) open spec fn infrastructure_retry_result(n: Node) -> (Node, Option<bool>) {
    if n.run_failures == 0 { infrastructure_retry_arm_result(n) }
    else { infrastructure_incident_arm_result(n) }
}

pub(super) proof fn infrastructure_retry_bridge(n: Node)
    requires node_headroom(n), arithmetic_consistent(n),
    ensures infrastructure_retry_result(n) == retirement::infrastructure_retry(n),
        infrastructure_retry_result(n).0.generation as int <= n.generation as int + 1,
{
    reveal(retirement::infrastructure_retry);
}
}

kernel_fn! {
/// The original saturated incident count, isolated from the node result.
pub(super) fn infrastructure_incident_count(c: u8) -> (result: u8) [
    requires c <= 2,
    ensures result == infrastructure_incident_count_result(c),
] {
    (c + 1).min(2)
}
}

kernel_fn! {
/// The first infrastructure failure retries publishing with the original writes.
pub(super) fn infrastructure_retry_arm(n: Node) -> (result: (Node, Option<bool>)) [
    requires n.generation < u64::MAX, n.run_failures == 0,
    ensures result == infrastructure_retry_arm_result(n),
] {
    let mut value = n;
    value.run_failures += 1;
    set_phase(&mut value, Phase::Publishing); // enqueue again
    (value, None)
}
}

kernel_fn! {
/// The second failure resets its counter and computes the incident report.
pub(super) fn infrastructure_incident_arm(n: Node) -> (result: (Node, Option<bool>)) [
    requires n.run_failures < u8::MAX, n.run_incidents <= 2,
    ensures result == infrastructure_incident_arm_result(n),
] {
    let mut value = n;
    value.run_failures += 1;
    value.run_failures = 0;
    value.run_incidents = infrastructure_incident_count(value.run_incidents);
    let needs_rob = value.run_incidents >= 2;
    (value, Some(needs_rob))
}
}

kernel_fn! {
/// Dispatch to exact value arms, then bridge to the unchanged public model.
pub(super) fn infrastructure_retry_value(n: Node) -> (result: (Node, Option<bool>)) [
    requires node_headroom(n), arithmetic_consistent(n),
    ensures result == infrastructure_retry_result(n),
        result == retirement::infrastructure_retry(n),
        result.0.generation as int <= n.generation as int + 1,
] {
    let result = if n.run_failures == 0 {
        infrastructure_retry_arm(n)
    } else {
        infrastructure_incident_arm(n)
    };
    kernel_proof! { infrastructure_retry_bridge(n); }
    result
}
}

#[cfg(verus_keep_ghost)]
verus! {
pub(super) open spec fn ci_before_enqueue(n: Node) -> bool {
    n.phase == Phase::Publishing && current_item(n) == Some(Item::Do(EffectKind::Enqueue))
}

pub(super) open spec fn ci_active(n: Node, infra: bool) -> bool {
    !(n.phase == Phase::AwaitingDelivery && infra)
        && (n.phase == Phase::AwaitingDelivery || ci_before_enqueue(n))
}

pub(super) open spec fn ci_prepared(n: Node, infra: bool) -> Node {
    if ci_active(n, infra) && ci_before_enqueue(n) {
        Node { effect: match n.effect {
            Some(effect) => Some(InFlight { cancelling: true, ..effect }),
            None => None,
        }, ..n }
    } else { n }
}
}

kernel_fn! {
/// Return the full effect value after the original cancellation write.
fn ci_cancel_effect_value(effect: InFlight) -> (result: InFlight) [
    ensures result == (InFlight { cancelling: true, ..effect }),
] {
    let mut value = effect;
    value.cancelling = true;
    value
}
}

kernel_fn! {
/// The original CI guards and pre-enqueue cancellation, before either retry path.
pub(super) fn ci_prefix_value(n: Node, infra: bool) -> (result: (Node, bool)) [
    ensures result.0 == ci_prepared(n, infra), result.1 == ci_active(n, infra),
] {
    let mut value = n;
    if value.phase == Phase::AwaitingDelivery && infra {
        return (value, false);
    }
    let before_enqueue = value.phase == Phase::Publishing
        && value.current() == Some(Item::Do(EffectKind::Enqueue));
    if value.phase != Phase::AwaitingDelivery && !before_enqueue {
        return (value, false);
    }
    if before_enqueue {
        if let Some(effect) = value.effect {
            value.effect = Some(ci_cancel_effect_value(effect));
        }
    }
    (value, true)
}
}

pub(super) mod tail_values {
use super::*;

#[cfg(verus_keep_ghost)]
verus! {
#[verifier::opaque]
pub(in super::super) open spec fn ci_change(n: Node, fast: bool, infra: bool) -> ReplyChange {
    if !ci_active(n, infra) { ReplyChange::Done { node: n, note: None } }
    else if infra {
        let prepared = ci_prepared(n, infra);
        let retry = retirement::infrastructure_retry(prepared);
        match retry.1 {
            Some(needs_rob) => ReplyChange::Incident { node: retry.0, needs_rob },
            None => ReplyChange::Done { node: retry.0, note: None },
        }
    } else {
        reply_family_values::failed_round(ci_prepared(n, infra), fast, ReviewStop::CiFailed)
    }
}

pub(in super::super) open spec fn outside_active(n: Node) -> bool {
    n.had_pr && matches!(n.phase,
        Phase::Publishing | Phase::AwaitingDelivery | Phase::Ready(Resume::Publishing))
}

pub(in super::super) open spec fn outside_prepared(n: Node, head: HeadId) -> Node {
    Node { head: Some(head), checked: None,
        review_restarted: n.head == Some(head) && n.review_restarted, ..n }
}

#[verifier::opaque]
pub(in super::super) open spec fn outside_change(n: Node, head: HeadId) -> ReplyChange {
    if outside_active(n) {
        reply_family_values::recheck(outside_prepared(n, head), Phase::Ready(Resume::Checking))
    } else { ReplyChange::Done { node: n, note: None } }
}

pub(in super::super) open spec fn outside_items(n: Node, nq: u64, i: usize,
    head: HeadId) -> Seq<Option<Out>> {
    if !n.had_pr { Seq::empty() } else {
        seq![Some(Out::Notice { node: NodeId(i as u64),
            notice: Notice::Note(NoteKind::OutsidePush) })]
        + match reply_family_values::item(nq, outside_change(n, head)) {
            Some(note) => seq![Some(Out::Notice { node: NodeId(i as u64), notice: note })],
            None => seq![],
        }
    }
}
}

kernel_fn! {
/// Preserve the local report work before resolving the canonical failed round.
pub(in super::super) fn failed_round_unreported_value(n: Node, nq: u64, fast: bool,
    i: usize, step: ReviewStop) -> (result: (Node, u64, Outbox)) [
    requires node_headroom(n), arithmetic_consistent(n), nq < u64::MAX,
    ensures result.0 == reply_family_values::finish(nq,
            reply_family_values::failed_round(n, fast, step)),
        result.1 == reply_family_values::next_question(nq,
            reply_family_values::failed_round(n, fast, step)),
        result.2.len == match reply_family_values::item(nq,
            reply_family_values::failed_round(n, fast, step)) {
            Some(_) => 1usize, None => 0usize,
        },
        result.2.items@ =~= facts_reply_adapter::reply_items(nq,
            reply_family_values::failed_round(n, fast, step), i),
        notices(result.2, i, 1), tail_frame(n, result.0),
        result.1 as int <= nq as int + 1,
] {
    let mut unused_report = Report::new();
    let change = failed_round_node(n, i, !fast, nq, step, &mut unused_report);
    let result = facts_reply_adapter::resolve_reply_value(change, nq, i);
    kernel_proof! {
        facts_family_tail::failed_round_frame(n, nq, fast, step);
        facts_family_tail::reply_notice_bound(result.2, nq, change, i);
    }
    result
}
}

kernel_fn! {
/// The original CI guards, cancellation and selected canonical retry path.
pub(in super::super) fn ci_fact_arm(n: Node, nq: u64, fast: bool, i: usize, infra: bool)
    -> (result: (Node, u64, Outbox)) [
    requires node_headroom(n), arithmetic_consistent(n), nq < u64::MAX,
    ensures result.0 == reply_family_values::finish(nq, ci_change(n, fast, infra)),
        result.1 == reply_family_values::next_question(nq, ci_change(n, fast, infra)),
        result.2.len == match reply_family_values::item(nq, ci_change(n, fast, infra)) {
            Some(_) => 1usize, None => 0usize,
        },
        result.2.items@ =~= facts_reply_adapter::reply_items(nq, ci_change(n, fast, infra), i),
        notices(result.2, i, 1),
] {
    kernel_proof! { facts_family_tail::ci_prefix_frame(n, infra); }
    let (prepared, active) = ci_prefix_value(n, infra);
    kernel_proof! { reveal(ci_change); }
    if !active {
        let result = facts_reply_adapter::resolve_reply_value(
            ReplyChange::Done { node: prepared, note: None }, nq, i);
        kernel_proof! { reveal(notices); }
        return result;
    }
    if !infra {
        let result = failed_round_unreported_value(prepared, nq, fast, i, ReviewStop::CiFailed);
        kernel_proof! { reveal(notices); }
        return result;
    }
    let (node, escalation) = infrastructure_retry_value(prepared);
    let change = match escalation {
        Some(needs_rob) => ReplyChange::Incident { node, needs_rob },
        None => ReplyChange::Done { node, note: None },
    };
    let result = facts_reply_adapter::resolve_reply_value(change, nq, i);
    kernel_proof! { reveal(notices); }
    result
}
}

kernel_fn! {
/// Preserve OutsidePush before the canonical head update and optional question.
pub(in super::super) fn outside_push_arm(n: Node, nq: u64, fast: bool, i: usize, head: HeadId)
    -> (result: (Node, u64, Outbox)) [
    requires node_headroom(n), arithmetic_consistent(n), nq < u64::MAX,
    ensures result.0 == reply_family_values::finish(nq, outside_change(n, head)),
        result.1 == reply_family_values::next_question(nq, outside_change(n, head)),
        result.2.len == outside_items(n, nq, i, head).len(),
        result.2.items@ =~= padded_notices(outside_items(n, nq, i, head)),
        notices(result.2, i, 2),
] {
    let mut value = n;
    let mut next_question = nq;
    let mut notes = Outbox::new();
    kernel_proof! { reveal(empty_outbox); reveal(outside_change); }
    if !value.had_pr {
        kernel_proof! {
            reveal(notices);
            assert(notes.items@ =~= padded_notices(outside_items(n, nq, i, head)));
        }
        return (value, next_question, notes);
    }
    notice(&mut notes, i, Notice::Note(NoteKind::OutsidePush));
    let published = matches!(value.phase,
        Phase::Publishing | Phase::AwaitingDelivery | Phase::Ready(Resume::Publishing));
    if published {
        update_recheck_head(&mut value, head);
        kernel_proof! { facts_family_tail::outside_prefix_frame(n, head); }
        let change = recheck_node(value, next_question, Phase::Ready(Resume::Checking));
        let (node, counter, tail) = facts_reply_adapter::resolve_reply_value(change, next_question, i);
        value = node;
        next_question = counter;
        if let Some(item) = tail.items[0] {
            notes.push(item);
        }
    }
    kernel_proof! {
        reveal(notices);
        assert(notes.items@ =~= padded_notices(outside_items(n, nq, i, head)));
    }
    (value, next_question, notes)
}
}

kernel_fn! {
pub(in super::super) fn ci_fact_value(n: Node, nq: u64, fast: bool, i: usize, infra: bool)
    -> (result: (Node, u64, Outbox)) [
    requires node_headroom(n), arithmetic_consistent(n), nq < u64::MAX,
    ensures (n.phase is Closed ==> result.0.phase is Closed), tail_frame(n, result.0), node_rows(n, result.0, fast, i, Fact::CiFailed { infra }),
        notices(result.2, i, 1), result.1 as int <= nq as int + 1,
        infra ==> result.0 == retirement::fact_node(n, Fact::CiFailed { infra: true }),
        infra ==> result.1 == nq,
] {
    let result = ci_fact_arm(n, nq, fast, i, infra);
    kernel_proof! {
        facts_family_tail::ci_scalar_posts(n, nq, fast, infra);
        facts_family_tail::ci_node_rows(n, nq, fast, infra, i);
    }
    result
}
}

kernel_fn! {
pub(in super::super) fn outside_push_value(n: Node, nq: u64, fast: bool, i: usize, head: HeadId)
    -> (result: (Node, u64, Outbox)) [
    requires node_headroom(n), arithmetic_consistent(n), nq < u64::MAX,
    ensures (n.phase is Closed ==> result.0.phase is Closed), tail_frame(n, result.0), node_rows(n, result.0, fast, i, Fact::PushByOther { head }),
        notices(result.2, i, 2), result.1 as int <= nq as int + 1,
] {
    let result = outside_push_arm(n, nq, fast, i, head);
    kernel_proof! {
        facts_family_tail::outside_scalar_posts(n, nq, head);
        facts_family_tail::outside_node_rows(n, nq, fast, head, i);
    }
    result
}
}

kernel_fn! {
pub(in super::super) fn outside_fact_entry(w: &mut World, i: usize, head: HeadId, out: &mut Outbox)
    -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len + 2 <= OUTBOX,
        node_headroom(old(w).nodes@[i as int]), arithmetic_consistent(old(w).nodes@[i as int]),
        old(w).next_question < u64::MAX,
    ensures retirement::fact_frame(*old(w), *final(w), i, *old(out), *final(out)),
        node_rows(old(w).nodes@[i as int], final(w).nodes@[i as int], old(w).fail_fast, i,
            Fact::PushByOther { head }),
        notice_suffix(*old(out), *final(out), i), prefix_preserved(*old(out), *final(out)),
] proof [reveal(retirement::fact_frame); reveal(notices);] {
    kernel_snapshot! { before = w.nodes@[i as int] }
    kernel_snapshot! { before_out = *out }
    let (value, counter, notes) = outside_push_value(w.nodes[i], w.next_question, w.fail_fast, i, head);
    kernel_proof! { facts_family_tail::frame_fields(before@, value); }
    w.nodes[i] = value;
    w.next_question = counter;
    append_fact_notices(out, notes, i);
    kernel_proof! { facts_family_tail::append_prefix(before_out@, *out, notes); }
}
}

}

#[cfg(verus_keep_ghost)]
verus! {
#[verifier::opaque]
pub(super) open spec fn tail_frame(b: Node, n: Node) -> bool {
    n.repo == b.repo && n.footprint == b.footprint
        && (settle_shape(b) ==> settle_shape(n))
        && n.generation as int <= b.generation as int + 1
        && n.next_seq == b.next_seq && n.credit.start == b.credit.start
        && n.credit.earned == b.credit.earned && n.credit.added == b.credit.added
        && n.credit.spent as int <= b.credit.spent as int + 1 && n.last_cmd == b.last_cmd
}

#[verifier::opaque]
pub(super) open spec fn prefix_preserved(ob: Outbox, oa: Outbox) -> bool {
    forall|k: int| 0 <= k < ob.len ==> #[trigger] oa.items@[k] == ob.items@[k]
}
}
