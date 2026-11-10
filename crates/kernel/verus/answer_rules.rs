//! Answer node-rules lemmas (R28-Answer incident row, N1 for Answer), in their own module so their
//! reveals and context stay out of apply_command_safety's queries (kernel master 11-10).
use super::*;

verus! {

/// R28-Answer (11-10): the incident row of node_rules, as one implication. Own group.
pub(super) proof fn rules_incident(w: World, n: Node)
    ensures retirement::node_rules(w, n) ==> (n.incident_needs_rob ==> n.waiting_incident),
{
    reveal(retirement::node_rules);
}

/// N1 for Answer (11-10, rule 7): node rules across record_last_cmd. Premises are record_last_cmd's
/// per-field posts verbatim (every node field but last_cmd, and the world scalars node_rules reads).
pub(super) proof fn rules_after_last_cmd(before: World, after: World, a: Node, n: Node)
    requires
        n.phase == a.phase,
        n.pos == a.pos,
        n.repo == a.repo,
        n.footprint == a.footprint,
        n.generation == a.generation,
        n.next_seq == a.next_seq,
        n.effect == a.effect,
        n.run == a.run,
        n.job == a.job,
        n.pool == a.pool,
        n.ticket == a.ticket,
        n.holds == a.holds,
        n.paused == a.paused,
        n.question == a.question,
        n.attempt == a.attempt,
        n.round == a.round,
        n.replanned == a.replanned,
        n.plan_retries == a.plan_retries,
        n.audit_rounds == a.audit_rounds,
        n.early_end == a.early_end,
        n.run_failures == a.run_failures,
        n.run_incidents == a.run_incidents,
        n.fx_failures == a.fx_failures,
        n.fx_incidents == a.fx_incidents,
        n.waiting_incident == a.waiting_incident,
        n.incident_needs_rob == a.incident_needs_rob,
        n.proposed == a.proposed,
        n.adopted == a.adopted,
        n.head == a.head,
        n.checked == a.checked,
        n.approved == a.approved,
        n.merged == a.merged,
        n.pr_open == a.pr_open,
        n.had_pr == a.had_pr,
        n.worktree == a.worktree,
        n.rechecks == a.rechecks,
        n.review_restarted == a.review_restarted,
        n.credit == a.credit,
        n.priority == a.priority,
        n.review_stop == a.review_stop,
        n.planning_started == a.planning_started,
        n.waiting_prerequisite == a.waiting_prerequisite,
        after.next_question == before.next_question,
        after.next_ticket == before.next_ticket,
        after.gates@ == before.gates@,
        after.file_sets@ == before.file_sets@,
    ensures retirement::node_rules(before, a) ==> retirement::node_rules(after, n),
        dependencies::owner_waiting_ok(a) ==> dependencies::owner_waiting_ok(n),
{
    if retirement::node_rules(before, a) {
        assert(n == (Node { last_cmd: n.last_cmd, ..a }));
        handler_safety::rules_to_v(before, a);
        handler_safety::last_cmd_rules_v(a, before.next_question, n.last_cmd);
        handler_safety::rules_world_update(before, after, a, n);
    }
}


/// E2.N4c (CONTRACTS.md v2.41): a waiting owner's rows over an answered node (form rule 2: opaque inside
/// apply; consumers see only the frozen N4c clause).
#[verifier::opaque]
pub(super) open spec fn owner_rows(r: Node, b: Node, id: u64, answer: Answer) -> bool {
    !r.planning_started && dependencies::waits(r.question) && dependencies::blocked_phase(r.phase)
        && (r.phase == Phase::Unplanned ==> !r.holds && !r.pool && r.ticket.is_none() && r.run.is_none())
        && (dependencies::early(r.phase) ==> dependencies::zero_credit(r.credit)
            || (handler_safety::answer_withholds(b, id, answer) && r.credit.ready))
}

/// E2.N4c (rule 6): with node rules, a waiting owner's open question fits only an early or stop continuation
/// (question_fits), so every answer arm keeps the owner rows over the exact answered value.
pub(super) proof fn answer_owner_rows(b: Node, nq: u64, id: u64, answer: Answer)
    ensures (handler_safety::node_rules_v(b, nq) && dependencies::owner_waiting_ok(b))
        ==> owner_rows(apply_answer_values::answered(b, nq, id, answer).0, b, id, answer),
{
    reveal(owner_rows);
    reveal(apply_answer_values::answered);
    if handler_safety::node_rules_v(b, nq) && dependencies::owner_waiting_ok(b) {
        reveal(handler_safety::node_rules_v);
        reveal(dependencies::zero_credit);
        let r = apply_answer_values::answered(b, nq, id, answer).0;
        match b.question {
            None => { assert(r == b); }
            Some(q) => {
                if q.id != id {
                    assert(r == b);
                } else {
                    match b.phase {
                        Phase::Stopping(_) => {
                            // Retry / MarkHandled keep the Stopping phase and planning_started.
                        }
                        Phase::AwaitingRob => {
                            assert(question_fits(q));
                            assert(dependencies::waits(Some(q)));
                            reveal(retirement::stopped);
                            if answer == Answer::Continue && apply_answer_values::permits(q.kind, answer) {
                                assert(q.kind == QKind::AuditDisagreement || q.kind == QKind::EffectFailed);
                            }
                        }
                        _ => { assert(r == b); }
                    }
                }
            }
        }
    }
}

/// E2.N4c (rule 7), withhold branch: premises = answer_node's atom and withhold_dependency_credit's post
/// verbatim (d = prerequisites_delivered of the world it ran on).
pub(super) proof fn owner_after_withhold(a: Node, f: Node, d: bool, b: Node, id: u64, answer: Answer)
    requires f == (Node {
            credit: Credit {
                withheld: a.credit.withheld
                    || (!d && !a.planning_started
                        && (match a.phase {
                            Phase::Ingesting | Phase::Auditing | Phase::Unplanned => true,
                            _ => false,
                        })
                        && a.credit.ready),
                ..a.credit
            },
            ..a
        }),
    ensures owner_rows(a, b, id, answer) && handler_safety::answer_withholds(b, id, answer)
        ==> d || dependencies::owner_waiting_ok(f),
{
    reveal(owner_rows);
    reveal(dependencies::zero_credit);
}

/// E2.N4c (rule 7), no-withhold branch.
pub(super) proof fn owner_without_withhold(a: Node, b: Node, id: u64, answer: Answer)
    ensures owner_rows(a, b, id, answer) && !handler_safety::answer_withholds(b, id, answer)
        ==> dependencies::owner_waiting_ok(a),
{
    reveal(owner_rows);
}

}
