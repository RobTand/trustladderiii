//! #698/#718: wire round trips and arithmetic outside the finite proofs.
use proptest::prelude::*;
use serde_json::json;
use tl3_controller::budget::{Request, decide};

proptest! {
    #[test]
    fn charge_preserves_binary64_rounding(tokens in any::<u32>(), read in any::<u32>(), creation in any::<u32>(), weights in prop::array::uniform3(0u64..10_000)) {
        let weights=weights.map(|w|w as f64/1000.0);
        let (r,c)=if u64::from(read)+u64::from(creation)>u64::from(tokens) {(0,0)}else{(read,creation)};
        let expected=((tokens-r-c) as f64*weights[0]+c as f64*weights[1]+r as f64*weights[2]).ceil() as u64;
        let request=Request::Charge {tokens:tokens.into(),read:read.into(),creation:creation.into(),weights};
        let encoded=serde_json::to_value(&request).unwrap();
        let decoded:Request=serde_json::from_value(encoded.clone()).unwrap();
        prop_assert_eq!(serde_json::to_value(&decoded).unwrap(),encoded);
        prop_assert_eq!(decide(decoded).unwrap(),json!(expected));
    }
    #[test]
    fn malformed_boundary_counts_fail_closed(count in -10000i64..0) {
        let wire=json!({"kind":"context","estimate":count,"last_input":0,"last_estimate":null});
        prop_assert!(serde_json::from_value::<Request>(wire).is_err());
    }
}

#[test]
fn all_budget_sums_reject_overflow() {
    assert!(
        decide(Request::Dropped {
            spent: u64::MAX,
            in_flight: 1,
            reserve: 0,
            allowed: u64::MAX
        })
        .is_err()
    );
    assert!(
        decide(Request::Forecast {
            brief_tokens: u64::MAX,
            file_tokens: Default::default(),
            overhead_tokens: 1,
            fit_tokens: 100,
            context_tokens: 200,
            problems: 0
        })
        .is_err()
    );
}

#[test]
fn generated_wire_contract_matches_rust() {
    let wire = include_str!("../../../workers/src/tl3_workers/schemas/budget-wire.json");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(wire).unwrap(),
        tl3_controller::budget::wire_schema()
    );
}

#[test]
fn huge_finite_cache_price_cannot_wrap_to_a_free_call() {
    assert!(
        decide(Request::Charge {
            tokens: 1,
            read: 0,
            creation: 0,
            weights: [2.0f64.powi(128), 1.25, 0.1]
        })
        .is_err()
    );
}

#[test]
fn incident_355_and_reasoning_cutoff_goldens() {
    let cutoff = |input, output, reasoning, message_characters, prompt_characters, context| {
        decide(Request::Cutoff {
            input,
            output,
            reasoning,
            message_characters,
            prompt_characters,
            context,
        })
        .unwrap()
    };
    for missing in [true, false] {
        for (characters, expected) in [(2547, false), (2548, true), (2550, true)] {
            assert_eq!(
                cutoff(
                    if missing { None } else { Some(0) },
                    if missing { None } else { Some(0) },
                    None,
                    1,
                    characters - 1,
                    1000
                ),
                json!(expected)
            );
        }
    }
    for (input, output) in [(84073, 36544), (88000, 40000)] {
        for reasoning in [None, Some(39900)] {
            assert_eq!(
                cutoff(Some(input), Some(output), reasoning, 100, 1, 150000),
                json!(false)
            );
        }
    }
    for reasoning in [None, Some(2000)] {
        assert_eq!(
            cutoff(Some(80000), Some(50000), reasoning, 144000, 1, 150000),
            json!(true)
        );
    }
    for (visible, expected) in [(47499, false), (47500, true)] {
        assert_eq!(
            cutoff(
                Some(80000),
                Some(50000),
                Some(50000 - visible),
                2,
                1,
                150000
            ),
            json!(expected)
        );
    }
}

#[test]
fn fit_and_remaining_boundary_goldens() {
    use tl3_controller::budget::decision;
    for (limit, expected) in [(66, true), (65, false), (64, false)] {
        assert_eq!(decision::fits(65, limit), expected);
    }
    for used in 0..=4 {
        assert_eq!(decision::remaining(used, 4), 4 - used);
    }
}

proptest! {
    #![proptest_config(proptest::test_runner::Config {
        rng_seed: proptest::test_runner::RngSeed::Fixed(7807),
        .. proptest::test_runner::Config::default()
    })]
    /// #780: cold-price selection uses decoded integers over all finite binary64 weights.
    #[test]
    fn decoded_price_comparison_matches_finite_binary64(left in 0u64..=0x7fef_ffff_ffff_ffff, right in 0u64..=0x7fef_ffff_ffff_ffff) {
        fn decoded(bits:u64) -> tl3_controller::budget::decision::Price {
            let power=((bits>>52)&0x7ff) as i16;
            tl3_controller::budget::decision::Price {
                significand:(bits&((1<<52)-1)) | if power==0 {0} else {1<<52},
                exponent:if power==0 {-1074} else {power-1023-52},
            }
        }
        prop_assert_eq!(decoded(left).greater(decoded(right)), f64::from_bits(left)>f64::from_bits(right));
    }
}
