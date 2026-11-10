//! Node facts for the unchanged Apply command updates.
use super::*;

verus! {
/// Resume: every per-node fact resume_command exports, over the exact node update,
/// so the world query never reveals resumed / resume_post.
pub(super) proof fn resume_facts(w: World, b: Node, id: u64)
    requires id > b.last_cmd,
    ensures ({ let r = Node { paused: false, ..Node { last_cmd: id, ..b } };
        progress_gate_apply::resume_post(b, r, id, Command::Resume)
        && handler_safety::command_step(b, r)
        && (node_checked(w, b) && command_progress_input(b, Command::Resume)
            ==> distance(r) < distance(b))
        && (retirement::node_rules(w, b) ==> handler_safety::node_rules_v(r, w.next_question)) }),
{
    let a = Node { last_cmd: id, ..b };
    let r = Node { paused: false, ..a };
    reveal(progress_gate_apply::resumed);
    reveal(progress_gate_apply::resume_post);
    if node_checked(w, b) && command_progress_input(b, Command::Resume) {
        resume_decreases(w, b);
        handler_safety::distance_last_cmd(Node { paused: false, ..b }, id);
    }
    handler_safety::field_step(a, r);
    handler_safety::last_cmd_step(b, a, r);
    if retirement::node_rules(w, b) {
        handler_safety::rules_to_v(w, b);
        handler_safety::last_cmd_rules_v(b, w.next_question, id);
        handler_safety::field_rules_v(a, w.next_question, false);
    }
}

pub(super) proof fn pause_facts(w: World, b: Node, id: u64)
    requires id > b.last_cmd,
    ensures ({ let a = Node { last_cmd: id, ..b };
        let p = Node { paused: true, ..a };
        progress_gate_apply::resume_post(b, a, id, Command::Pause)
        && progress_gate_apply::resume_post(b, p, id, Command::Pause)
        && handler_safety::command_step(b, a)
        && handler_safety::command_step(b, p)
        && !command_progress_input(b, Command::Pause)
        && (retirement::node_rules(w, b) ==> handler_safety::node_rules_v(a, w.next_question)
            && handler_safety::node_rules_v(p, w.next_question)) }),
{
    let a = Node { last_cmd: id, ..b };
    let p = Node { paused: true, ..a };
    reveal(progress_gate_apply::resume_post);
    handler_safety::same_step(a);
    handler_safety::last_cmd_step(b, a, a);
    handler_safety::field_step(a, p);
    handler_safety::last_cmd_step(b, a, p);
    if retirement::node_rules(w, b) {
        handler_safety::rules_to_v(w, b);
        handler_safety::last_cmd_rules_v(b, w.next_question, id);
        handler_safety::field_rules_v(a, w.next_question, true);
    }
}

}
