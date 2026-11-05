//! #835: measured costs, with each caller retaining its serialization convention.
//! Admission does not bound inferred proof receipts. No global result-size claim
//! follows from these estimates; retained fields must be measured or bounded.
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
pub const MAX_REPORT_LINE_BYTES: u64 = 4_194_304;
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
pub const MAX_RESULT_BYTES: u64 = 8_388_608;
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
pub const COST_CAP: u64 = MAX_RESULT_BYTES + 1;
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
pub const INITIAL_RESULT_BYTES: u64 = 305_538;
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
pub const PHASE_RUN_BYTES: u64 = 7_600;

pub mod arithmetic {
    use super::*;

    /// #835: fixed-width boundary, over every u128 length, without pointer assumptions.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
        out <= COST_CAP,
        out as int == if bytes > COST_CAP as u128 { COST_CAP as int } else { bytes as int },
    ))]
    pub fn measured_bytes(bytes: u128) -> u64 {
        if bytes > COST_CAP as u128 {
            COST_CAP
        } else {
            bytes as u64
        }
    }

    /// #835/#698: cap at limit+1; nonnegative additions cannot wrap or restore admission.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
        out <= COST_CAP,
        out as int == if a as int + b as int > COST_CAP as int { COST_CAP as int }
            else { a as int + b as int },
    ))]
    pub fn add_cost(a: u64, b: u64) -> u64 {
        measured_bytes(a as u128 + b as u128)
    }

    /// #835/#718: exact nonnegative multiplication below the cap, including zero.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
        out <= COST_CAP,
        out as int == if a as int * b as int > COST_CAP as int { COST_CAP as int }
            else { a as int * b as int },
    ))]
    pub fn multiply_cost(a: u64, b: u64) -> u64 {
        measured_bytes(a as u128 * b as u128)
    }

    #[cfg(verus_keep_ghost)]
    verus! {
        /// #835: witnesses supplement contracts universal over all u64/u128 inputs.
        fn witnesses() {
            let huge = measured_bytes(u128::MAX);
            let sum = add_cost(u64::MAX, u64::MAX);
            let product = multiply_cost(u64::MAX, u64::MAX);
            let zero = multiply_cost(u64::MAX, 0);
            let equal = measured_bytes(8388608);
            assert(huge == 8388609 && sum == 8388609 && product == 8388609);
            assert(zero == 0 && equal == 8388608);
        }
    }
}
pub use arithmetic::{add_cost, measured_bytes, multiply_cost};

pub mod line {
    use super::*;

    /// #835, limits.json plan/phase/collection vectors: deduplicate before measuring.
    /// The boundary supplies the unique collection and largest phase sizes.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
        out == (collection <= 4194304 && largest_phase <= 4194304),
    ))]
    pub fn plan_line_fits(collection: u64, largest_phase: u64) -> bool {
        collection <= MAX_REPORT_LINE_BYTES && largest_phase <= MAX_REPORT_LINE_BYTES
    }

    /// #835: refusal is strictly greater; equality passes, including after capping.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
        out == (bytes <= 8388608),
    ))]
    pub fn result_fits(bytes: u64) -> bool {
        bytes <= MAX_RESULT_BYTES
    }

    #[cfg(verus_keep_ghost)]
    verus! {
        /// #835: empty/deduplicated collection and both independent strict boundaries.
        fn witnesses() {
            let empty = plan_line_fits(37, 0);
            let deduplicated = plan_line_fits(70, 8225);
            let equal = plan_line_fits(4194304, 4194304);
            let collection_over = plan_line_fits(4194305, 0);
            let phase_over = plan_line_fits(37, 4194305);
            let result_equal = result_fits(8388608);
            let result_over = result_fits(8388609);
            assert(empty && deduplicated && equal && !collection_over && !phase_over);
            assert(result_equal && !result_over);
        }
    }
}
pub use line::{plan_line_fits, result_fits};

// SEAM(planner-contracts): costs only; shared Check/Proof/Forcing stay with their owner.
pub mod accumulation {
    use super::*;

    /// #835: Some means reserve a proof receipt, including absent-declaration cost
    /// chosen by the caller. Header includes any echoed forcing/symptom data.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
        out as int == capped(header as int + 1
            + (match proof_bytes { Some(bytes) => 65536int + bytes as int, None => 0int })
            + (if symptom { 393216int } else { 0int })),
    ))]
    pub fn check_cost(header: u64, proof_bytes: Option<u64>, symptom: bool) -> u64 {
        let mut cost = add_cost(header, 1);
        if let Some(bytes) = proof_bytes {
            cost = add_cost(cost, add_cost(65536, bytes));
        }
        if symptom {
            cost = add_cost(cost, 384 * 1024);
        }
        cost
    }

    /// #835/#393: charge every occurrence. Ordinary tests have two sides, forced
    /// tests three. Deduplication applies only to collection, never this charge.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
        out as int == capped(case as int + 1 + (if forced { 3int } else { 2int }) * (7600 + node as int)),
    ))]
    pub fn test_cost(case: u64, node: u64, forced: bool) -> u64 {
        add_cost(
            add_cost(case, 1),
            multiply_cost(if forced { 3 } else { 2 }, add_cost(PHASE_RUN_BYTES, node)),
        )
    }

    #[cfg(verus_keep_ghost)]
    verus! {
        pub open spec fn capped(n: int) -> int { if n > 8388609 { 8388609 } else { n } }
        pub open spec fn sum(charges: Seq<u64>) -> int
            decreases charges.len(),
        { if charges.len() == 0 { 0 } else { sum(charges.drop_last()) + charges.last() as int } }

        pub open spec fn accumulated(start: u64, charges: Seq<u64>) -> int
            decreases charges.len(),
        { if charges.len() == 0 { capped(start as int) }
          else { capped(accumulated(start, charges.drop_last()) + charges.last() as int) } }

        /// #835: induction over any finite sequence of measured check/test charges.
        /// Each fold step is add_cost's contract. No collection deduplication here.
        proof fn accumulation_step(start: u64, charges: Seq<u64>)
            ensures accumulated(start, charges) == capped(start as int + sum(charges)),
                sum(charges) >= 0,
            decreases charges.len(),
        {
            if charges.len() > 0 { accumulation_step(start, charges.drop_last()); }
        }

        /// #835 conditional accounting only: every retained byte has a charge.
        /// The inferred receipt map violates this premise today. No unconditional
        /// theorem about validate_check_plan or the real result serializer is claimed.
        fn accounted_result_fits(measured: u64, retained: u64) -> (out: bool)
            requires retained <= measured,
            ensures out ==> retained <= 8388608,
        { super::line::result_fits(measured) }

        /// #835: two/three sides and per-occurrence witnesses; universal u64 costs.
        fn witnesses() {
            let normal = test_cost(100, 20, false);
            let forced = test_cost(100, 20, true);
            let repeated = add_cost(normal, normal);
            let python = check_cost(100, Some(4), false);
            let rust = check_cost(100, Some(0), false);
            assert(normal == 15341 && forced == 22961 && repeated == 30682);
            assert(python == 65641 && rust == 65637);
        }
    }
}
pub use accumulation::{check_cost, test_cost};

pub mod delay {
    #[cfg(verus_keep_ghost)]
    use vstd::prelude::*;

    /// #835, Q27 docs/state-table.md:1002-1005: decoded IEEE-754 magnitude ordering
    /// for [0,10], plus negative zero. No rounding or floating-point decisions.
    /// All u64 bit patterns are covered, including NaN, infinities and subnormals.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
        out == ((bits <= 0x4024000000000000 || bits == 0x8000000000000000)
            && exhausted == (bits == 0x4024000000000000)),
    ))]
    pub fn delay_budget_consistent(bits: u64, exhausted: bool) -> bool {
        (bits <= 0x4024000000000000 || bits == 0x8000000000000000)
            && exhausted == (bits == 0x4024000000000000)
    }

    #[cfg(verus_keep_ghost)]
    verus! {
        /// #835: 0/false, 9.5/false, 10/true, mismatches and invalid ranges.
        fn witnesses() {
            let zero = delay_budget_consistent(0, false);
            let negative_zero = delay_budget_consistent(0x8000000000000000, false);
            let fraction = delay_budget_consistent(0x4023000000000000, false);
            let ten = delay_budget_consistent(0x4024000000000000, true);
            let mismatch = delay_budget_consistent(0x4024000000000000, false);
            let above = delay_budget_consistent(0x4024000000000001, true);
            let nan = delay_budget_consistent(0x7ff8000000000000, false);
            assert(zero && negative_zero && fraction && ten && !mismatch && !above && !nan);
        }
    }
}
pub use delay::delay_budget_consistent;

/// #835 boundary conversion only. Shape and exact JSON types precede bit decoding.
/// This predicate is not wired into shared DelayBudget admission in this slice.
#[cfg(not(verus_keep_ghost))]
pub fn decode_delay_budget(value: &serde_json::Value) -> Option<(u64, bool)> {
    let object = value.as_object()?;
    if object.len() != 2 {
        return None;
    }
    let seconds = object.get("applied_seconds")?.as_number()?.as_f64()?;
    let exhausted = object.get("exhausted")?.as_bool()?;
    Some((seconds.to_bits(), exhausted))
}
