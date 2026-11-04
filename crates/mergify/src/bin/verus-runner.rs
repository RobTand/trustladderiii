//! Offline Verus process boundary for #770.

use std::path::Path;
use std::process::Command;
use std::time::Instant;

fn check_summary(output: &str, expected: u64) -> Result<u64, String> {
    let summaries: Vec<_> = output
        .lines()
        .filter_map(|line| line.strip_prefix("verification results:: "))
        .collect();
    let [summary] = summaries.as_slice() else {
        return Err("Verus must emit exactly one verifier summary".into());
    };
    let count = summary
        .strip_suffix(" verified, 0 errors")
        .and_then(|count| count.parse::<u64>().ok())
        .ok_or("Verus summary is malformed or reports verification errors")?;
    if expected == 0 || count == 0 || count != expected {
        return Err(format!(
            "Verus verified {count} obligations; expected exactly {expected} positive obligations"
        ));
    }
    Ok(count)
}

fn check_result(status: Option<i32>, output: &str, expected: u64) -> Result<u64, String> {
    if status != Some(0) {
        return Err(format!("Verus failed or timed out: exit {status:?}"));
    }
    check_summary(output, expected)
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let [verifier, z3, rustc, source, expected, label] = args.as_slice() else {
        return Err("usage: verus-runner VERUS Z3 RUSTC SOURCE EXPECTED LABEL".into());
    };
    let expected = expected.parse::<u64>()?;
    if expected == 0 {
        return Err("expected obligations must be positive".into());
    }
    let verifier = Path::new(verifier).canonicalize()?;
    let z3 = Path::new(z3).canonicalize()?;
    let rustc = Path::new(rustc).canonicalize()?;
    let sysroot = rustc
        .parent()
        .and_then(Path::parent)
        .ok_or("missing private sysroot")?;
    let started = Instant::now();
    let result = Command::new("/usr/bin/timeout")
        .args(["--kill-after=5s", "45s"])
        .arg(verifier)
        .args(["--num-threads", "1", "--rlimit", "30", "--sysroot"])
        .arg(sysroot)
        .arg(source)
        .env_clear()
        .env("PATH", format!("{}/bin:/usr/bin:/bin", sysroot.display()))
        .env("LD_LIBRARY_PATH", sysroot.join("lib"))
        .env("VERUS_USE_RUSTUP", "0")
        .env("VERUS_Z3_PATH", z3)
        .env("OMP_NUM_THREADS", "1")
        .env("RAYON_NUM_THREADS", "1")
        .output()?;
    let output = format!(
        "{}{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    print!("{output}");
    let verified = check_result(result.status.code(), &output, expected)?;
    let receipt = serde_json::json!({"proofs": {label: {
        "kind": "verus", "expected_obligations": expected, "verified": verified,
        "seconds": started.elapsed().as_secs_f64(),
    }}});
    if let Some(directory) = std::env::var_os("TEST_UNDECLARED_OUTPUTS_DIR") {
        std::fs::write(
            Path::new(&directory).join("inventory.json"),
            format!("{receipt}\n"),
        )?;
    }
    println!("VERUS_PROOF {receipt}");
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::check_summary;

    #[test]
    fn accepts_exact_positive_obligation_count() {
        assert_eq!(
            check_summary("verification results:: 2 verified, 0 errors\n", 2),
            Ok(2)
        );
    }

    #[test]
    fn rejects_zero_missing_duplicate_and_wrong_counts() {
        for (output, expected) in [
            ("verification results:: 0 verified, 0 errors\n", 1),
            ("nothing ran", 1),
            ("verification results:: 2 verified, 0 errors\n", 1),
            ("verification results:: 2 verified, 1 errors\n", 2),
            ("verification results:: 2 verified, 0 errors\n", 0),
            (
                "verification results:: 2 verified, 0 errors\nverification results:: 2 verified, 0 errors\n",
                2,
            ),
        ] {
            assert!(check_summary(output, expected).is_err(), "{output}");
        }
    }
}

#[cfg(test)]
mod process_tests {
    use super::check_result;

    #[test]
    fn rejects_timeout_nonzero_and_signal_even_with_a_success_summary() {
        for status in [Some(124), Some(1), None] {
            assert!(
                check_result(status, "verification results:: 2 verified, 0 errors\n", 2).is_err()
            );
        }
    }
}
