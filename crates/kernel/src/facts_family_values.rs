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

/// Exact ordered list, including the unoccupied slots.
#[verifier::opaque]
pub(super) open spec fn merge_notices(n: Node, notes: Outbox, i: usize,
    head: HeadId, checks_passed: bool) -> bool {
    let late = matches!(n.phase, Phase::Closed(Outcome::Stopped) | Phase::Closed(Outcome::Replaced));
    let active = n.had_pr && n.merged.is_none() && (!(n.phase is Closed) || late);
    let unapproved = !(n.approved == Some(head) && checks_passed);
    let items = if !active { seq![] } else {
        let prefix = if late { seq![
            Some(Out::Notice { node: NodeId(i as u64), notice: Notice::Note(NoteKind::LateMerge) }),
            Some(Out::Notice { node: NodeId(i as u64), notice: Notice::Incident { needs_rob: false } }),
        ] } else { seq![] };
        if unapproved { prefix + seq![
            Some(Out::Notice { node: NodeId(i as u64), notice: Notice::Incident { needs_rob: false } }),
            Some(Out::Notice { node: NodeId(i as u64), notice: Notice::Note(NoteKind::UnapprovedMerge) }),
        ] } else { prefix }
    };
    notes.len == items.len()
        && notes.items@ == Seq::new(OUTBOX as nat, |k: int| if k < items.len() { items[k] } else { None })
}

/// Safety follows from the actual exact update, independently of the world entry.
pub(super) proof fn value_node_rows(b: Node, n: Node, fast: bool, i: usize, fact: Fact)
    requires node_headroom(b), n == retirement::fact_node(b, fact),
        matches!(fact, Fact::Priority(_) | Fact::PrClosedByPerson | Fact::Conflict
            | Fact::IncidentCleared { .. } | Fact::PrMerged { .. }),
    ensures node_rows(b, n, fast, i, fact),
{
    reveal(node_rows);
    reveal(retirement::fact_node);
    reveal(retirement::stopped);
    assert forall|origin: World| #[trigger] retirement::node_rules(origin, b)
        implies transition_safety::fail_fast_node(b, n, fast,
            Event::Fact { node: NodeId(i as u64), fact }, i as int) by {}
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
] proof [reveal(notices); reveal(retirement::fact_node); reveal(empty_outbox);
    reveal(retirement::stopped); reveal(retirement::notices_only); reveal(distance);] {
    let mut value = n;
    let mut notes = Outbox::new();
    let out = &mut notes;
    match fact {
        Fact::Priority(priority) => {
            if !value.phase.is_closed() {
                value.priority = priority;
            }
        }
        Fact::PrClosedByPerson => {
            let m = &mut value;
            if !m.pr_open {
                kernel_proof! { value_node_rows(n, value, fast, i, fact); }
                return (value, notes);
            }
            m.pr_open = false;
            if !m.phase.is_stopping() && !m.phase.is_closed() {
                notice(out, i, Notice::Note(NoteKind::PrClosed));
                enter_stopping(m, Outcome::Stopped);
            }
        }
        Fact::Conflict => {
            let m = &mut value;
            if m.phase == Phase::AwaitingDelivery {
                set_phase(m, Phase::Ready(Resume::Rebasing));
            } else if m.phase == Phase::Publishing {
                set_phase(m, Phase::Rebasing);
            }
        }
        Fact::IncidentCleared { by_rob } => {
            let m = &mut value;
            if m.waiting_incident && (by_rob || !m.incident_needs_rob) {
                m.waiting_incident = false;
                m.incident_needs_rob = false;
            }
        }
        _ => {}
    }
    kernel_proof! { value_node_rows(n, value, fast, i, fact); }
    (value, notes)
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
] proof [reveal(notices); reveal(merge_notices); reveal(empty_outbox); reveal(retirement::fact_node);
    reveal(retirement::stopped); reveal(retirement::notices_only);
    reveal(retirement::unapproved_note);] {
    let mut value = n;
    let mut notes = Outbox::new();
    let out = &mut notes;
    kernel_snapshot! { before_merge = n }
    let m = &mut value;
    if !m.had_pr || m.merged.is_some() {
        kernel_proof! { facts_merge_lemmas::ignored_result(before_merge@, head, checks_passed); }
        kernel_proof! { value_node_rows(n, value, fast, i, Fact::PrMerged { head, checks_passed }); }
        return (value, notes);
    }
    let approved = m.approved == Some(head) && checks_passed;
    match m.phase {
        Phase::Closed(o) => {
            if !matches!(o, Outcome::Stopped | Outcome::Replaced) {
                kernel_proof! { facts_merge_lemmas::ignored_result(before_merge@, head, checks_passed); }
                kernel_proof! { value_node_rows(n, value, fast, i, Fact::PrMerged { head, checks_passed }); }
                return (value, notes);
            }
            m.merged = Some(head);
            m.phase = Phase::Closed(Outcome::Merged);
            merge_credit(m);
            kernel_proof! { facts_merge_lemmas::late_result(before_merge@, *m, head, checks_passed); }
            notice(out, i, Notice::Note(NoteKind::LateMerge));
            notice(out, i, Notice::Incident { needs_rob: false });
        }
        _ => {
            merge_open(m, head);
            kernel_proof! { facts_merge_lemmas::open_result(before_merge@, *m, head, checks_passed); }
        }
    }
    if !approved {
        notice(out, i, Notice::Incident { needs_rob: false });
        notice(out, i, Notice::Note(NoteKind::UnapprovedMerge));
    }
    kernel_proof! { value_node_rows(n, value, fast, i, Fact::PrMerged { head, checks_passed }); }
    (value, notes)
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
        invariant k <= notes.len, out.len == before_append@.len + k,
            before_append@.len + notes.len <= OUTBOX,
            forall|j: int| 0 <= j < OUTBOX ==> out.items@[j]
                == if before_append@.len <= j < out.len {
                    notes.items@[j - before_append@.len]
                } else { before_append@.items@[j] },
        decreases notes.len - k,
    ] {
        match notes.items[k] {
            Some(item) => out.push(item),
            None => {}
        }
        k += 1;
    } }
    kernel_proof! {
        if retirement::unapproved_note(notes, i) {
            let note = Some(Out::Notice { node: NodeId(i as u64),
                notice: Notice::Note(NoteKind::UnapprovedMerge) });
            let j = choose|j: int| 0 <= j < notes.items@.len() && notes.items@[j] == note;
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
    ensures retirement::fact_frame(*old(w), *final(w), i, *old(out), *final(out)),
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
    let (value, notes) = ordinary_fact_value(w.nodes[i], w.fail_fast, i, fact);
    w.nodes[i] = value;
    append_fact_notices(out, notes, i);
}
}

kernel_fn! {
pub(super) fn fact_dependency_entry(w: &mut World, i: usize, fact: Fact,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), i <= u64::MAX as int,
        old(out).len + 1 <= OUTBOX, dependencies::edges_indexable(*old(w)),
        fact is Dependency || fact is DependencyRemove,
    ensures dependencies::dependency_frame(*old(w), *final(w), i as int),
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
    match fact {
        Fact::DependencyRemove { kind, number } => {
            w.remove_dependency(i, kind, number);
            kernel_proof! { reveal(dependencies::dependency_frame); }
        }
        Fact::Dependency { target, status } => w.dependency_fact(i, target, status, out),
        _ => {}
    }
}
}
