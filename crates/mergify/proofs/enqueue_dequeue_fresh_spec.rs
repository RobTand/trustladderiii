use crate::enqueue::*;
use crate::Time;
use vstd::prelude::*;

verus! {
/// #608 retains prior-dequeue history rather than trusting current labels;
/// #751 enqueue_dequeue_fresh, preserved by the #760 lifecycle extraction.
/// Context: Publishing/Queued, docs/state-table.md:526,570,574,667,669.
/// Timestamp/ID freshness and equality boundaries are preserved existing
/// behaviour (no rule); these rows do not specify the comparison encoding.
/// Time derives Ord over (secs: i64, nanos: u32), in that field order.
/// Full integer and Option domains, including negative secs and leap nanos.
pub open spec fn fresh(dequeued: Option<LabelEvent>, queued: Option<u64>, finished: Option<Time>) -> bool {
    match dequeued {
        None => false,
        Some(event) => event.labeled && match queued {
            Some(queued) => event.id > queued,
            None => match finished {
                Some(finished) => event.at.secs > finished.secs
                    || event.at.secs == finished.secs && event.at.nanos >= finished.nanos,
                None => false,
            },
        },
    }
}

pub open spec fn expected(retry: bool, concluded: bool, labeled: bool,
    dequeued: Option<LabelEvent>, queued: Option<u64>, finished: Option<Time>) -> bool {
    retry && !concluded && (!labeled || !fresh(dequeued, queued, finished))
}


}
