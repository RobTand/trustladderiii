//! Arithmetic shared by the CLI and controller. No clocks, I/O or floats.

#[cfg(kani)]
use std::convert::TryFrom;

/// Q4: the fit limit is exclusive.
pub fn fits(size: u64, limit: u64) -> bool {
    size < limit
}

/// #698/#718: counters never wrap and remaining allowance cannot be negative.
pub fn remaining(used: u64, allowed: u64) -> u64 {
    allowed.saturating_sub(used)
}

#[path = "cutoff.rs"]
mod threshold;
pub use self::threshold::cutoff;

pub fn estimate(characters: u64) -> u64 {
    characters.div_ceil(3)
}

/// #698: input estimates are calibrated at the latest provider sampling point.
pub fn context(estimate: u64, last_input: u64, last_estimate: Option<i64>) -> Option<u64> {
    match last_estimate {
        None => Some(estimate),
        Some(old) => {
            u64::try_from((i128::from(last_input) + i128::from(estimate) - i128::from(old)).max(0))
                .ok()
        }
    }
}

/// #698: ineffective compaction must be reported, never counted as freed room.
pub fn compacted(before: u64, after: u64) -> bool {
    after < before
}

/// #718: unfinished mandatory verification cannot be a successful maker reply.
pub fn reject_done(done: bool, unfinished: bool) -> bool {
    done && unfinished
}

/// #698/#718: finalization reserves are verification-only and survive recovery.
pub fn finalizing(
    maker: bool,
    turns: u64,
    no_progress: u64,
    allowed: u64,
    window: u64,
    reserve: u64,
) -> bool {
    maker
        && (turns.saturating_add(reserve.min(allowed)) >= allowed
            || (window != 0 && no_progress.saturating_add(reserve.min(allowed)) >= window))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum Exhaustion {
    Backstop,
    ToolTurns,
    NoProgress,
    ContextTokens,
    InputTokens,
}

/// State-table execution feasibility (#614/#692): ordered, distinct limits.
#[derive(Clone, Copy, Debug)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub struct Run {
    pub maker: bool,
    pub turns: u64,
    pub allowed: u64,
    pub stalled: bool,
    pub context_full: bool,
    pub spent: u64,
    pub predicted: u64,
    pub reserve: u64,
    pub input_limit: u64,
}

pub fn exhaustion(run: Run) -> Option<Exhaustion> {
    let Run {
        maker,
        turns,
        allowed,
        stalled,
        context_full,
        spent,
        predicted,
        reserve,
        input_limit,
    } = run;
    if turns >= allowed {
        Some(if maker {
            Exhaustion::Backstop
        } else {
            Exhaustion::ToolTurns
        })
    } else if stalled {
        Some(Exhaustion::NoProgress)
    } else if context_full {
        Some(Exhaustion::ContextTokens)
    } else if u128::from(spent) + u128::from(predicted) + u128::from(reserve)
        > u128::from(input_limit)
    {
        Some(Exhaustion::InputTokens)
    } else {
        None
    }
}

/// Nonnegative IEEE-754 values, decoded at the wire boundary. Integer operations
/// reproduce Python's round-to-nearest-even after *each* operation, including
/// int-to-float conversion. Decimal fixed point would change ceil at boundaries.
#[derive(Clone, Copy, Debug)]
pub struct Price {
    pub significand: u64,
    pub exponent: i16,
}
impl Price {
    /// #780: compare nonnegative decoded prices without floating-point decisions.
    pub fn greater(self, other: Self) -> bool {
        fn key(price: Price) -> (i32, u64) {
            if price.significand == 0 {
                (i32::MIN, 0)
            } else {
                let leading = price.significand.leading_zeros();
                (
                    i32::from(price.exponent) + (64 - leading) as i32,
                    price.significand << leading,
                )
            }
        }
        key(self) > key(other)
    }

    pub fn integer(value: u64) -> Option<Self> {
        Self::rounded(u128::from(value), 0)
    }
    fn rounded(mut value: u128, mut exponent: i16) -> Option<Self> {
        let shift = (128 - value.leading_zeros())
            .saturating_sub(53)
            .max((-1074 - exponent).max(0) as u32);
        if shift >= 128 {
            value = 0;
        } else if shift > 0 {
            let discarded = value & ((1u128 << shift) - 1);
            value >>= shift;
            let half = 1u128 << (shift - 1);
            if discarded > half || (discarded == half && value & 1 != 0) {
                value += 1;
            }
            exponent += shift as i16;
            if value >= 1u128 << 53 {
                value >>= 1;
                exponent += 1;
            }
        }
        if exponent > 971 {
            return None;
        }
        Some(Self {
            significand: value as u64,
            exponent,
        })
    }
    pub fn times(self, other: Self) -> Option<Self> {
        Self::rounded(
            u128::from(self.significand) * u128::from(other.significand),
            self.exponent + other.exponent,
        )
    }
    pub fn plus(self, other: Self) -> Option<Self> {
        if self.significand == 0 {
            return Some(other);
        }
        if other.significand == 0 {
            return Some(self);
        }
        let (high, low) = if self.exponent >= other.exponent {
            (self, other)
        } else {
            (other, self)
        };
        let shift = (high.exponent - low.exponent) as u32;
        if shift > 74 {
            return Some(high);
        }
        Self::rounded(
            (u128::from(high.significand) << shift) + u128::from(low.significand),
            low.exponent,
        )
    }
    pub fn ceil(self) -> Option<u64> {
        if self.significand == 0 {
            return Some(0);
        }
        if self.exponent >= 0 {
            let factor = 1u64.checked_shl(self.exponent as u32)?;
            self.significand.checked_mul(factor)
        } else {
            let shift = (-self.exponent) as u32;
            if shift >= 64 {
                Some(1)
            } else {
                Some(self.significand.div_ceil(1u64 << shift))
            }
        }
    }
}

pub fn charge(tokens: u64, read: u64, creation: u64, weights: [Price; 3]) -> Option<u64> {
    let (read, creation) = if u128::from(read) + u128::from(creation) > u128::from(tokens) {
        (0, 0)
    } else {
        (read, creation)
    };
    Price::integer(tokens - read - creation)?
        .times(weights[0])?
        .plus(Price::integer(creation)?.times(weights[1])?)?
        .plus(Price::integer(read)?.times(weights[2])?)?
        .ceil()
}
