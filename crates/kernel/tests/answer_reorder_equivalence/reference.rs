//! Verbatim pre-reorder executable bodies (K5-L1 0db78386): answer_command writes last_cmd, then
//! calls on_answer; on_command dispatches to it. Ghost code dropped (it compiles to nothing).
use super::*;

pub(super) fn answer_command(w: &mut World, i: usize, id: u64, question: u64, answer: Answer,
    out: &mut Outbox) {
    w.nodes[i].last_cmd = id;
    crate::answers::on_answer(w, i, question, answer, out);
}

pub(super) fn on_command(w: &mut World, i: usize, id: u64, command: Command, out: &mut Outbox) {
    if id <= w.nodes[i].last_cmd {
        return; // a repeated command
    }
    match command {
        Command::Stop => crate::commands::stop_command(w, i, id),
        Command::ChangeRequest => crate::commands::change_request_command(w, i, id),
        Command::Pause => crate::commands::pause_command(w, i, id),
        Command::Resume => crate::commands::resume_command(w, i, id),
        Command::Answer { question, answer } => answer_command(w, i, id, question, answer, out),
    }
}
