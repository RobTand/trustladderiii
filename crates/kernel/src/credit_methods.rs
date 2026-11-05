//! #806: unchanged kernel helpers, independently gated for bounded solver time.
use super::*;

impl Credit {
    kernel_fn! {
        pub fn balance(&self) -> (balance: i128) [
            ensures balance as int == if self.withheld { 0int } else {
                self.start as int + self.earned as int + self.added as int - self.spent as int
            },
        ] {
            if self.withheld {
                0
            } else {
                self.ledger_balance()
            }
        }
    }

    kernel_fn! {
        pub(super) fn ledger_balance(&self) -> (balance: i128) [
            ensures balance as int == self.start as int + self.earned as int
                + self.added as int - self.spent as int,
        ] {
            self.start as i128 + self.earned as i128 + self.added as i128 - self.spent as i128
        }
    }

    kernel_fn! {
    /// Q18 / #771: compare spendable credit, including dependency withholding.
    pub(super) fn can_spend(&self, cost: u64) -> (result: bool) [
        ensures result == (!self.withheld && self.start as int + self.earned as int
            + self.added as int >= self.spent as int + cost as int
            || self.withheld && cost == 0),
    ] {
        self.balance() >= cost as i128
    }
    }

    kernel_fn! {
        /// State-table credit ledger: one unit per paid retry or continuation.
        pub(super) fn spend(&mut self, cost: u64) -> (result: ()) [
            requires old(self).spent as int + cost as int <= u64::MAX as int,
            ensures final(self).spent as int == old(self).spent as int + cost as int,
                final(self).start == old(self).start,
                final(self).earned == old(self).earned,
                final(self).added == old(self).added,
                final(self).withheld == old(self).withheld,
                *final(self) == (Credit { spent: (old(self).spent + cost) as u64, ..*old(self) }),
        ] {
            self.spent += cost;
        }
    }

    kernel_fn! {
        /// State-table credit ledger: Rob's answers top up the shortfall first.
        /// #771: consistent ledgers are nonnegative; withholding permits only
        /// zero-cost continuations. Hence automatic top-up adds at most cost.
        pub(super) fn pay(&mut self, cost: u64) -> (result: ()) [
            requires
                old(self).start as int + old(self).earned as int + old(self).added as int
                    >= old(self).spent as int,
                old(self).withheld ==> cost == 0,
                old(self).added as int + (if cost as int > (old(self).start as int + old(self).earned as int + old(self).added as int - old(self).spent as int) { cost as int - (old(self).start as int + old(self).earned as int + old(self).added as int - old(self).spent as int) } else { 0int }) <= u64::MAX as int,
                old(self).spent as int + cost as int <= u64::MAX as int,
            ensures
                final(self).added as int == old(self).added as int + (if cost as int > (old(self).start as int + old(self).earned as int + old(self).added as int - old(self).spent as int) { cost as int - (old(self).start as int + old(self).earned as int + old(self).added as int - old(self).spent as int) } else { 0int }),
                final(self).ready == old(self).ready,
                final(self).merge_credited == old(self).merge_credited,
                final(self).added as int >= old(self).added as int,
                final(self).added as int <= old(self).added as int + cost as int,
                final(self).spent as int == old(self).spent as int + cost as int,
                final(self).start == old(self).start,
                final(self).earned == old(self).earned,
                final(self).withheld == old(self).withheld,
                final(self).start as int + final(self).earned as int + final(self).added as int
                    >= final(self).spent as int,
        ] {
            let short = cost as i128 - self.balance();
            if short > 0 {
                self.added += short as u64;
            }
            self.spent += cost;
        }
    }
    kernel_fn! {
        /// Q18 / #771: a u8 AddCredit answer and its automatic top-up together
        /// add at most 255. Valid questions cost at most one credit.
        pub(super) fn fund_and_pay(&mut self, deposit: u8, cost: u8) -> (result: ()) [
            requires cost <= 1,
                old(self).start as int + old(self).earned as int + old(self).added as int
                    >= old(self).spent as int,
                old(self).withheld ==> cost == 0,
                old(self).added as int + 255 <= u64::MAX as int,
                old(self).spent as int + cost as int <= u64::MAX as int,
            ensures final(self).added as int >= old(self).added as int,
                final(self).added as int <= old(self).added as int + 255,
                final(self).spent as int == old(self).spent as int + cost as int,
                final(self).start == old(self).start, final(self).earned == old(self).earned,
                final(self).ready == old(self).ready, final(self).withheld == old(self).withheld,
                final(self).merge_credited == old(self).merge_credited,
                final(self).start as int + final(self).earned as int + final(self).added as int
                    >= final(self).spent as int,
                final(self).added as int == old(self).added as int + deposit as int
                    + if cost as int > old(self).start as int + old(self).earned as int
                        + old(self).added as int + deposit as int - old(self).spent as int {
                        cost as int - (old(self).start as int + old(self).earned as int
                            + old(self).added as int + deposit as int - old(self).spent as int)
                    } else { 0int },
        ] {
            self.added += deposit as u64;
            self.pay(cost as u64);
        }
    }
}
