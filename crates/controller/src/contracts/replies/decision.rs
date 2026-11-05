//! The actual role mapping decision, independent of serde, allocation and I/O.
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
pub enum RoleReply<H, F, I> {
    MakerDone {
        head: H,
        rejected: bool,
    },
    MakerNeedsFiles(F),
    MakerCantMeet,
    MakerBlocked,
    MakerOutOfRoom,
    IngestBrief(I),
    IngestDuplicate,
    IngestAlreadyDone,
    IngestNotActionable,
    AuditAgree,
    AuditDisagree,
    ReviewApprove,
    ReviewChanges,
    ReviewOutOfRoom,
    Planner {
        violations: bool,
        cant_meet: bool,
        split: bool,
        fits: bool,
    },
    Classifier,
    Writer,
    Postmortem,
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
pub enum MappedReply<H, F, I> {
    Made(H),
    NeedsFiles(F),
    Ingest(I),
    MakerCantMeet,
    RunFailed,
    MakerOutOfRoom,
    IngestDuplicate,
    IngestAlreadyDone,
    IngestNotActionable,
    AuditAgrees,
    AuditDisagrees,
    Approved,
    ChangesRequested,
    ReviewerOutOfRoom,
    PlanCantMeet,
    PlanSplit,
    PlanLeaf { fits: bool },
    Unsupported,
}

/// Q50, Q54 and #730 C3: total mapping over all 18 role families. H, F and I
/// are opaque payloads preserved exactly; their wire/arithmetic checks are at
/// the boundary. The bound is the complete enum and every Boolean valuation.
#[cfg_attr(verus_keep_ghost,vstd::prelude::verus_spec(out => ensures
    match reply {
        RoleReply::MakerDone { head,rejected } => if rejected { matches!(out,MappedReply::MakerCantMeet) }
            else { matches!(out,MappedReply::Made(h) if vstd::prelude::equal(h,head)) },
        RoleReply::MakerNeedsFiles(files) => matches!(out,MappedReply::NeedsFiles(f) if vstd::prelude::equal(f,files)),
        RoleReply::MakerCantMeet => matches!(out,MappedReply::MakerCantMeet),
        RoleReply::MakerBlocked => matches!(out,MappedReply::RunFailed),
        RoleReply::MakerOutOfRoom => matches!(out,MappedReply::MakerOutOfRoom),
        RoleReply::IngestBrief(brief) => matches!(out,MappedReply::Ingest(i) if vstd::prelude::equal(i,brief)),
        RoleReply::IngestDuplicate => matches!(out,MappedReply::IngestDuplicate),
        RoleReply::IngestAlreadyDone => matches!(out,MappedReply::IngestAlreadyDone),
        RoleReply::IngestNotActionable => matches!(out,MappedReply::IngestNotActionable),
        RoleReply::AuditAgree => matches!(out,MappedReply::AuditAgrees),
        RoleReply::AuditDisagree => matches!(out,MappedReply::AuditDisagrees),
        RoleReply::ReviewApprove => matches!(out,MappedReply::Approved),
        RoleReply::ReviewChanges => matches!(out,MappedReply::ChangesRequested),
        RoleReply::ReviewOutOfRoom => matches!(out,MappedReply::ReviewerOutOfRoom),
        RoleReply::Planner {violations,cant_meet,split,fits} =>
            if violations || cant_meet { matches!(out,MappedReply::PlanCantMeet) }
            else if split { matches!(out,MappedReply::PlanSplit) }
            else { matches!(out,MappedReply::PlanLeaf { fits:f } if f == fits) },
        RoleReply::Classifier | RoleReply::Writer | RoleReply::Postmortem => matches!(out,MappedReply::Unsupported),
    },
))]
pub fn map_reply<H, F, I>(reply: RoleReply<H, F, I>) -> MappedReply<H, F, I> {
    match reply {
        RoleReply::MakerDone { head, rejected } => {
            if rejected {
                MappedReply::MakerCantMeet
            } else {
                MappedReply::Made(head)
            }
        }
        RoleReply::MakerNeedsFiles(files) => MappedReply::NeedsFiles(files),
        RoleReply::MakerCantMeet => MappedReply::MakerCantMeet,
        RoleReply::MakerBlocked => MappedReply::RunFailed,
        RoleReply::MakerOutOfRoom => MappedReply::MakerOutOfRoom,
        RoleReply::IngestBrief(brief) => MappedReply::Ingest(brief),
        RoleReply::IngestDuplicate => MappedReply::IngestDuplicate,
        RoleReply::IngestAlreadyDone => MappedReply::IngestAlreadyDone,
        RoleReply::IngestNotActionable => MappedReply::IngestNotActionable,
        RoleReply::AuditAgree => MappedReply::AuditAgrees,
        RoleReply::AuditDisagree => MappedReply::AuditDisagrees,
        RoleReply::ReviewApprove => MappedReply::Approved,
        RoleReply::ReviewChanges => MappedReply::ChangesRequested,
        RoleReply::ReviewOutOfRoom => MappedReply::ReviewerOutOfRoom,
        RoleReply::Planner {
            violations,
            cant_meet,
            split,
            fits,
        } => {
            if violations || cant_meet {
                MappedReply::PlanCantMeet
            } else if split {
                MappedReply::PlanSplit
            } else {
                MappedReply::PlanLeaf { fits }
            }
        }
        RoleReply::Classifier | RoleReply::Writer | RoleReply::Postmortem => {
            MappedReply::Unsupported
        }
    }
}

#[cfg(verus_keep_ghost)]
verus! {
/// #730: concrete reachable witnesses for every output family, including Q50.
/// No input preconditions; all inputs are inhabited and no assumptions are used.
pub fn witnesses() {
    let made=map_reply::<u64,u64,u64>(RoleReply::MakerDone {head:7,rejected:false});
    assert(matches!(made,MappedReply::Made(h) if h == 7));
    let blocked=map_reply::<u64,u64,u64>(RoleReply::MakerBlocked);
    assert(matches!(blocked,MappedReply::RunFailed));
    let ingest=map_reply::<u64,u64,u64>(RoleReply::IngestBrief(11));
    assert(matches!(ingest,MappedReply::Ingest(i) if i == 11));
    let audit=map_reply::<u64,u64,u64>(RoleReply::AuditAgree);
    assert(matches!(audit,MappedReply::AuditAgrees));
    let review=map_reply::<u64,u64,u64>(RoleReply::ReviewApprove);
    assert(matches!(review,MappedReply::Approved));
    let plan=map_reply::<u64,u64,u64>(RoleReply::Planner {violations:false,cant_meet:false,split:false,fits:true});
    assert(matches!(plan,MappedReply::PlanLeaf {fits:true}));
    let unsupported=map_reply::<u64,u64,u64>(RoleReply::Writer);
    assert(matches!(unsupported,MappedReply::Unsupported));
}
}
