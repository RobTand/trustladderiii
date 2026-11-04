//! S3 checks the delivery contracts in docs/state-table.md, not exactly-once HTTP.
#[path = "stateright_s3/model.rs"]
mod model;
#[path = "../../store/tests/support/mod.rs"]
mod support;
#[path = "stateright_s3/trace.rs"]
mod trace;

use model::{Contract, EffectModel};
use stateright::{Checker, Model};
use std::time::Instant;

#[test]
fn idempotent_effect_under_lost_response() {
    let contracts = [Contract::Once, Contract::Converges, Contract::Message];
    for first in contracts {
        for kinds in std::iter::once(vec![first])
            .chain(contracts.into_iter().map(|second| vec![first, second]))
        {
            let started = Instant::now();
            let model = EffectModel::new(kinds.clone());
            let checker = model.checker().threads(1).spawn_bfs().join();
            println!(
                "S3 {kinds:?}: {} unique states, {} generated states, depth {}, {:.3}s",
                checker.unique_state_count(),
                checker.state_count(),
                checker.max_depth(),
                started.elapsed().as_secs_f64()
            );
            checker.assert_properties();
        }
    }
}
