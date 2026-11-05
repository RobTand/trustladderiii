//! Finite classification facts, parsed from schema-checked JSON at the boundary.
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum Kind {
    ChangeRequest,
    Answer,
    Restart,
    Complaint,
    Remark,
    Unsure,
    Stop,
    Pause,
    Resume,
    Reconcile,
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum Answer {
    ChangeRequest,
    AddCredit,
    Unclear,
    Other,
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[derive(Clone, Copy, Debug)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub struct Classification {
    pub kind: Kind,
    pub answer: Answer,
    pub reconcile_issue: bool,
    pub assessments_exact: bool,
    pub selected_matches: bool,
    pub has_applicable: bool,
    pub supersedes_present: bool,
    pub supersedes_applicable: bool,
    pub closed: bool,
    pub restartable: bool,
    pub question_current: bool,
    pub answer_offered: bool,
    pub credit_integer: bool,
    pub text_nonblank: bool,
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Violation {
    ReconcileRequired,
    Assessments,
    SelectedAnswer,
    MissingSupersedes,
    InapplicableDecision,
    RestartUnavailable,
    Closed,
    Question,
    UnofferedAnswer,
    Credit,
    Complaint,
    EmptyChange,
}

impl Violation {
    pub fn message(self) -> &'static str {
        match self {
            Self::ReconcileRequired => "restart must reconcile the current issue",
            Self::Assessments => {
                "reconciliation must assess every applicable decision exactly once"
            }
            Self::SelectedAnswer => "classifier changed a selected console answer",
            Self::MissingSupersedes => "replacement must assess applicable decisions",
            Self::InapplicableDecision => "supersedes names a decision that is not applicable",
            Self::RestartUnavailable => "restart is not offered for this request",
            Self::Closed => "closed requests allow only complaint, remark, unsure or restart",
            Self::Question => "reply answers a question that is not open",
            Self::UnofferedAnswer => "reply selects an answer the question does not offer",
            Self::Credit => "credit amount must be an integer",
            Self::Complaint => "complaints require a closed request",
            Self::EmptyChange => "a change requires new request text",
        }
    }
}

/// Return the first violation, in the worker's repair-message order.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result => ensures
    result == expected(c),
))]
pub fn decide(c: Classification) -> Result<(), Violation> {
    use self::Violation::*;
    if c.reconcile_issue || c.kind == Kind::Reconcile {
        return if !c.reconcile_issue || c.kind != Kind::Reconcile {
            Err(ReconcileRequired)
        } else if !c.assessments_exact {
            Err(Assessments)
        } else {
            Ok(())
        };
    }
    if !c.selected_matches {
        return Err(SelectedAnswer);
    }
    let change = c.kind == Kind::ChangeRequest
        || (c.kind == Kind::Answer && c.answer == Answer::ChangeRequest);
    if change && c.has_applicable && !c.supersedes_present {
        return Err(MissingSupersedes);
    }
    if !c.supersedes_applicable {
        return Err(InapplicableDecision);
    }
    if c.kind == Kind::Restart {
        if !c.restartable {
            return Err(RestartUnavailable);
        }
    } else if c.closed && !matches!(c.kind, Kind::Complaint | Kind::Remark | Kind::Unsure) {
        return Err(Closed);
    }
    if c.kind == Kind::Answer {
        if !c.question_current {
            return Err(Question);
        }
        if c.answer != Answer::Unclear && !c.answer_offered {
            return Err(UnofferedAnswer);
        }
        if c.answer == Answer::AddCredit && !c.credit_integer {
            return Err(Credit);
        }
    }
    if c.kind == Kind::Complaint && !c.closed {
        return Err(Complaint);
    }
    if change && !c.text_nonblank {
        return Err(EmptyChange);
    }
    Ok(())
}

#[cfg(verus_keep_ghost)]
verus! {
/// Q15/Q49/#743: the Kani ordered rule table, over all finite parsed facts.
pub open spec fn expected(c: Classification) -> Result<(), Violation> {
    let reconcile = c.reconcile_issue || c.kind == Kind::Reconcile;
    let change = c.kind == Kind::ChangeRequest || (c.kind == Kind::Answer && c.answer == Answer::ChangeRequest);
    if reconcile && !(c.reconcile_issue && c.kind == Kind::Reconcile) { Err(Violation::ReconcileRequired) }
    else if reconcile && !c.assessments_exact { Err(Violation::Assessments) }
    else if !reconcile && !c.selected_matches { Err(Violation::SelectedAnswer) }
    else if !reconcile && change && c.has_applicable && !c.supersedes_present { Err(Violation::MissingSupersedes) }
    else if !reconcile && !c.supersedes_applicable { Err(Violation::InapplicableDecision) }
    else if !reconcile && c.kind == Kind::Restart && !c.restartable { Err(Violation::RestartUnavailable) }
    else if !reconcile && c.closed && !(c.kind == Kind::Restart || c.kind == Kind::Complaint || c.kind == Kind::Remark || c.kind == Kind::Unsure) { Err(Violation::Closed) }
    else if !reconcile && c.kind == Kind::Answer && !c.question_current { Err(Violation::Question) }
    else if !reconcile && c.kind == Kind::Answer && c.answer != Answer::Unclear && !c.answer_offered { Err(Violation::UnofferedAnswer) }
    else if !reconcile && c.kind == Kind::Answer && c.answer == Answer::AddCredit && !c.credit_integer { Err(Violation::Credit) }
    else if !reconcile && c.kind == Kind::Complaint && !c.closed { Err(Violation::Complaint) }
    else if !reconcile && change && !c.text_nonblank { Err(Violation::EmptyChange) }
    else { Ok(()) }
}
}
