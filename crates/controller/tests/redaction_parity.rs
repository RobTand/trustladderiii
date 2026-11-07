mod redaction_support;
use redaction_support::shared_inputs;

#[test]
fn every_retained_matrix_input_reaches_the_shared_engine() {
    let inputs = shared_inputs();
    assert!(inputs.len() > 700);
    for input in inputs {
        let output = tl3_controller::redaction::text(&input).unwrap();
        assert!(!output.contains('\0') || input.contains('\0'));
    }
}

#[test]
fn scheme_folding_and_literal_hyphens_protect_controller_output() {
    for scheme in ["Basic", "Basıc", "Basİc"] {
        for suffix in ["", "-tail"] {
            let secret = format!("TL3FAKEzE9p-R4cV7x{suffix}");
            let input = format!("{scheme} {secret}");
            assert_eq!(
                tl3_controller::redaction::text(&input).unwrap(),
                format!("{scheme} [REDACTED]")
            );
        }
    }
}
