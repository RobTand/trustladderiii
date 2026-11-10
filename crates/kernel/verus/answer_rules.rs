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
{
    if retirement::node_rules(before, a) {
        assert(n == (Node { last_cmd: n.last_cmd, ..a }));
        handler_safety::rules_to_v(before, a);
        handler_safety::last_cmd_rules_v(a, before.next_question, n.last_cmd);
        handler_safety::rules_world_update(before, after, a, n);
    }
}


/// N1 for on_answer (rule 7; on_answer fix 11-10): the node-rules chain across answer_node, the dependency
/// withhold and the world write, out of on_answer's query. Premises are on_answer's facts verbatim:
/// answer_node's node_rules_v / repo / footprint / ticket posts, the withhold write's post over the mid world
/// (or the unchanged node), and the world-scalar frames.
pub(super) proof fn on_answer_rules(before: World, mid: World, after: World, b: Node, a: Node, f: Node,
    nq1: u64, withhold: bool, node: NodeId)
    requires
        handler_safety::node_rules_v(b, before.next_question) ==> handler_safety::node_rules_v(a, nq1),
        a.repo == b.repo,
        a.footprint == b.footprint,
        a.ticket == b.ticket || a.ticket.is_none(),
        !withhold ==> f == a,
        withhold ==> f == (Node {
            credit: Credit {
                withheld: a.credit.withheld
                    || (!dependencies::prerequisites_delivered(mid, node)
                        && !a.planning_started
                        && (match a.phase {
                            Phase::Ingesting | Phase::Auditing | Phase::Unplanned => true,
                            _ => false,
                        })
                        && a.credit.ready),
                ..a.credit
            },
            ..a
        }),
        after.next_question == nq1,
        after.gates@ == before.gates@,
        after.file_sets@ == before.file_sets@,
        after.next_ticket == before.next_ticket,
    ensures retirement::node_rules(before, b) ==> retirement::node_rules(after, f),
{
    if retirement::node_rules(before, b) {
        handler_safety::rules_to_v(before, b);
        if withhold {
            dependencies::prerequisite_definition(mid, node);
            apply_command_safety::withhold_rules_v(a, nq1, mid.dependencies@, node);
            reveal(dependencies::withheld_after);
            assert(f == handler_safety::unwithheld(a, dependencies::withheld_after(a.credit.withheld,
                a.planning_started, a.phase, a.credit.ready, mid.dependencies@, node)));
        }
        handler_safety::rules_world_update(before, after, b, f);
    }
}

}
