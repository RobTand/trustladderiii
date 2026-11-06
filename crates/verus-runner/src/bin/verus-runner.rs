//! Offline Verus process boundary for #770.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

fn check_summary(output: &str, expected: u64, partial: bool) -> Result<u64, String> {
    let summaries: Vec<_> = output
        .lines()
        .filter_map(|line| line.strip_prefix("verification results:: "))
        .collect();
    let [summary] = summaries.as_slice() else {
        return Err("Verus must emit exactly one verifier summary".into());
    };
    let summary = if partial {
        summary
            .strip_suffix(" (partial verification with `--verify-*`)")
            .ok_or("missing partial-verification marker")?
    } else {
        summary
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

fn check_result(
    status: Option<i32>,
    output: &str,
    expected: u64,
    partial: bool,
) -> Result<u64, String> {
    if status != Some(0) {
        return Err(format!("Verus failed or timed out: exit {status:?}"));
    }
    check_summary(output, expected, partial)
}

mod measurement {
    use serde_json::{Value, json};

    pub fn check_result(
        status: Option<i32>,
        output: &str,
        expected: u64,
        partial: bool,
    ) -> Result<Value, String> {
        if status != Some(0) {
            return Err(format!("Verus failed or timed out: exit {status:?}"));
        }
        let mut objects = Vec::new();
        let mut offset = 0;
        for line in output.split_inclusive('\n') {
            if line.trim() == "{"
                && let Some(Ok(value)) = serde_json::Deserializer::from_str(&output[offset..])
                    .into_iter::<Value>()
                    .next()
                && value.get("verification-results").is_some()
            {
                objects.push(value);
            }
            offset += line.len();
        }
        let [stats] = objects.as_slice() else {
            return Err("Verus must emit exactly one JSON verification result".into());
        };
        let verdict = &stats["verification-results"];
        if expected == 0
            || verdict["verified"].as_u64() != Some(expected)
            || verdict["errors"].as_u64() != Some(0)
            || verdict["success"].as_bool() != Some(true)
            || verdict["encountered-error"].as_bool() != Some(false)
            || verdict["encountered-vir-error"].as_bool() != Some(false)
            || verdict["is-verifying-entire-crate"].as_bool() != Some(!partial)
        {
            return Err(format!(
                "Verus JSON verdict failed, has the wrong selection, or differs from {expected} positive obligations: {verdict}"
            ));
        }
        Ok(stats.clone())
    }

    pub fn receipt(stats: &Value, label: &str, seconds: f64) -> Result<Value, String> {
        if seconds > 20.0 {
            return Err(format!(
                "{label}: group exceeds the hard 20s ceiling ({seconds:.3}s)"
            ));
        }
        let smt = &stats["times-ms"]["smt"];
        let max_function = smt["smt-run-module-times"]
            .as_array()
            .ok_or("missing SMT module measurements")?
            .iter()
            .map(|module| {
                module["function-breakdown"]
                    .as_array()
                    .ok_or("missing SMT function measurements")?
                    .iter()
                    .map(|f| f["rlimit"].as_u64().ok_or("missing function rlimit"))
                    .collect::<Result<Vec<_>, _>>()
            })
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .flatten()
            .max()
            .ok_or("empty SMT function measurements")?;
        Ok(json!({
            "label": label, "seconds": seconds,
            "verified": stats["verification-results"]["verified"],
            "rlimit_budget": 30,
            "rlimit_init": smt["rlimit-init"].as_u64().ok_or("missing init rlimit")?,
            "rlimit_run": smt["rlimit-run"].as_u64().ok_or("missing run rlimit")?,
            "max_function_rlimit": max_function,
        }))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn verdict() -> Value {
            json!({"verification-results": {
                "success": true, "verified": 2, "errors": 0,
                "encountered-error": false, "encountered-vir-error": false,
                "is-verifying-entire-crate": true,
            }})
        }

        #[test]
        fn measured_verdict_requires_exact_count_selection_and_success() {
            let text = serde_json::to_string_pretty(&verdict()).unwrap();
            assert!(check_result(Some(0), &text, 2, false).is_ok());
            for (status, expected, partial) in [
                (Some(124), 2, false),
                (None, 2, false),
                (Some(1), 2, false),
                (Some(0), 0, false),
                (Some(0), 3, false),
                (Some(0), 2, true),
            ] {
                assert!(check_result(status, &text, expected, partial).is_err());
            }
            assert!(check_result(Some(0), &format!("{text}\n{text}"), 2, false).is_err());
            assert!(check_result(Some(0), "nothing ran", 2, false).is_err());
            for field in [
                "errors",
                "success",
                "encountered-error",
                "encountered-vir-error",
            ] {
                let mut value = verdict();
                value["verification-results"][field] = match field {
                    "errors" => json!(1),
                    "success" => json!(false),
                    _ => json!(true),
                };
                assert!(
                    check_result(
                        Some(0),
                        &serde_json::to_string_pretty(&value).unwrap(),
                        2,
                        false
                    )
                    .is_err()
                );
            }
        }
    }
}
mod mutation {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    pub struct Scratch(PathBuf);

    impl Scratch {
        pub fn new() -> std::io::Result<Self> {
            let path = std::env::temp_dir().join(format!(
                "verus-mutant-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir(&path)?;
            Ok(Self(path))
        }

        pub fn copy_crate(&self, root: &Path) -> std::io::Result<PathBuf> {
            let target = self.0.join("crate");
            copy_tree(root, &target)?;
            Ok(target)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
        fs::create_dir(to)?;
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            let target = to.join(entry.file_name());
            if entry.path().is_dir() {
                copy_tree(&entry.path(), &target)?;
            } else {
                fs::copy(entry.path(), target)?;
            }
        }
        Ok(())
    }

    pub fn seed(source: &str, function: &str, before: &str, after: &str) -> Result<String, String> {
        let needle = format!("fn {function}");
        let starts: Vec<_> = source
            .match_indices(&needle)
            .filter(|(at, _)| {
                source
                    .as_bytes()
                    .get(at + needle.len())
                    .is_some_and(|c| matches!(*c, b'(' | b'<'))
            })
            .map(|(at, _)| at)
            .collect();
        let [start] = starts.as_slice() else {
            return Err(format!(
                "expected exactly one executable function {function}"
            ));
        };
        let end = source[*start..]
            .find("\n#[cfg_attr")
            .map_or(source.len(), |end| start + end);
        let body = &source[*start..end];
        if before.is_empty() || body.matches(before).count() != 1 {
            return Err(format!("seed must match exactly once inside {function}"));
        }
        Ok(format!(
            "{}{}{}",
            &source[..*start],
            body.replacen(before, after, 1),
            &source[end..]
        ))
    }

    pub fn check_rejection(status: Option<i32>, output: &str) -> Result<(), String> {
        let summaries: Vec<_> = output
            .lines()
            .filter_map(|line| line.strip_prefix("verification results:: "))
            .collect();
        let [summary] = summaries.as_slice() else {
            return Err("mutant must emit exactly one verifier summary".into());
        };
        let valid = summary.strip_suffix(" (partial verification with `--verify-*`)");
        let errors = valid
            .and_then(|s| s.split_once(" verified, "))
            .and_then(|(verified, rest)| verified.parse::<u64>().ok().map(|_| rest))
            .and_then(|rest| rest.strip_suffix(" errors"))
            .and_then(|count| count.parse::<u64>().ok());
        if status != Some(1)
            || errors.is_none_or(|n| n == 0)
            || ![
                "postcondition not satisfied",
                "assertion failed",
                "precondition not satisfied",
            ]
            .iter()
            .any(|message| output.contains(message))
            || output.contains("error[E")
            || output.contains("unrecognized option")
        {
            return Err(
                "expected an SMT obligation failure, never a frontend error or timeout".into(),
            );
        }
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn seed_changes_only_the_named_executable_function() {
            let source = "#[cfg_attr(test, before)]\nfn send() { before }\n#[cfg_attr(test, before)]\nfn other() { before }";
            assert_eq!(
                seed(source, "send", "before", "after").unwrap(),
                source.replacen("{ before }", "{ after }", 1)
            );
            assert!(seed(source, "missing", "before", "after").is_err());
            assert!(seed(source, "send", "missing", "after").is_err());
        }

        #[test]
        fn only_obligation_failure_qualifies() {
            let failure = "error: postcondition not satisfied\nverification results:: 0 verified, 1 errors (partial verification with `--verify-*`)\nerror: aborting due to 1 previous error\n";
            assert!(check_rejection(Some(1), failure).is_ok());
            for status in [Some(0), Some(124), Some(137), None] {
                assert!(check_rejection(status, failure).is_err());
            }
            assert!(check_rejection(Some(1), &format!("error[E0308]\n{failure}")).is_err());
            assert!(check_rejection(Some(1), &failure.replace("1 errors", "0 errors")).is_err());
            assert!(
                check_rejection(
                    Some(1),
                    &failure.replace("postcondition not satisfied", "parse failed")
                )
                .is_err()
            );
        }
    }
}

struct Toolchain {
    verifier: PathBuf,
    z3: PathBuf,
    sysroot: PathBuf,
}

struct Verification {
    status: Option<i32>,
    output: String,
    seconds: f64,
}

impl Toolchain {
    fn verify(&self, source: &Path, module: &str, measured: bool) -> std::io::Result<Verification> {
        let started = Instant::now();
        let mut process = Command::new("/usr/bin/timeout");
        process
            .args(["--kill-after=5s", "45s"])
            .arg(&self.verifier)
            .args([
                "--num-threads",
                "1",
                "--rlimit",
                "30",
                "--multiple-errors",
                "0",
                "-V",
                "spinoff-all",
                "--sysroot",
            ])
            .arg(&self.sysroot)
            .arg(source)
            .args(["--edition", "2024"]);
        let (selected_module, function) = module
            .split_once('#')
            .map_or((module, None), |(owner, function)| (owner, Some(function)));
        if selected_module == "root" {
            process.arg("--verify-root");
        } else if !selected_module.is_empty() {
            process.args(["--verify-only-module", selected_module]);
        }
        if let Some(function) = function {
            process.args(["--verify-function", function]);
        }
        if measured {
            process.args(["--time", "--output-json"]);
        }
        let result = process
            .env_clear()
            .env(
                "PATH",
                format!("{}/bin:/usr/bin:/bin", self.sysroot.display()),
            )
            .env("LD_LIBRARY_PATH", self.sysroot.join("lib"))
            .env("VERUS_USE_RUSTUP", "0")
            .env("VERUS_Z3_PATH", &self.z3)
            .env("OMP_NUM_THREADS", "1")
            .env("RAYON_NUM_THREADS", "1")
            .output()?;
        Ok(Verification {
            status: result.status.code(),
            output: format!(
                "{}{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            ),
            seconds: started.elapsed().as_secs_f64(),
        })
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let [
        verifier,
        z3,
        rustc,
        source,
        expected,
        label,
        module,
        options @ ..,
    ] = args.as_slice()
    else {
        return Err("usage: verus-runner VERUS Z3 RUSTC SOURCE EXPECTED LABEL MODULE [--measure [--mutant SOURCE MODULE#FUNCTION BEFORE AFTER]]".into());
    };
    let (measured, mutant) = match options {
        [] => (false, None),
        [flag] if flag == "--measure" => (true, None),
        [measure, flag, path, function, before, after]
            if measure == "--measure" && flag == "--mutant" && module.is_empty() =>
        {
            (true, Some((path, function, before, after)))
        }
        _ => return Err("invalid measurement or mutation options".into()),
    };
    let expected = expected.parse::<u64>()?;
    if expected == 0 {
        return Err("expected obligations must be positive".into());
    }
    let rustc = Path::new(rustc).canonicalize()?;
    let toolchain = Toolchain {
        verifier: Path::new(verifier).canonicalize()?,
        z3: Path::new(z3).canonicalize()?,
        sysroot: rustc
            .parent()
            .and_then(Path::parent)
            .ok_or("missing private sysroot")?
            .to_owned(),
    };
    let source = Path::new(source);
    let result = toolchain.verify(source, module, measured)?;
    let (verified, mut measurements, stats) = if measured {
        let stats =
            measurement::check_result(result.status, &result.output, expected, !module.is_empty())
                .inspect_err(|_| print!("{}", result.output))?;
        let receipt = measurement::receipt(&stats, label, result.seconds)?;
        (expected, Some(receipt), Some(stats))
    } else {
        print!("{}", result.output);
        (
            check_result(result.status, &result.output, expected, !module.is_empty())?,
            None,
            None,
        )
    };
    if let Some((path, selection, before, after)) = mutant {
        let (_, function) = selection
            .split_once('#')
            .ok_or("mutation requires MODULE#FUNCTION")?;
        let root = source
            .parent()
            .and_then(Path::parent)
            .ok_or("proof must be inside a crate directory")?;
        let relative = Path::new(path).strip_prefix(root)?;
        let scratch = mutation::Scratch::new()?;
        let copied = scratch.copy_crate(root)?;
        let path = copied.join(relative);
        let seeded = mutation::seed(&std::fs::read_to_string(&path)?, function, before, after)?;
        std::fs::write(&path, seeded)?;
        let changed =
            toolchain.verify(&copied.join(source.strip_prefix(root)?), selection, false)?;
        print!("{}", changed.output);
        mutation::check_rejection(changed.status, &changed.output)?;
        measurements.as_mut().ok_or("mutation must be measured")?["mutant"] = serde_json::json!({
            "obligation": selection.replace('#', "::"), "source": relative,
            "rejection": "verification obligation", "seconds": changed.seconds,
        });
    }
    let receipt = serde_json::json!({"proofs": {label: {
        "kind": "verus", "expected_obligations": expected, "verified": verified,
        "module": module, "threads": 1, "seconds": result.seconds,
    }}});
    println!("VERUS_PROOF {receipt}");
    if let Some(measurements) = &measurements {
        println!("VERUS_MEASUREMENT {measurements}");
    }
    if let Some(directory) = std::env::var_os("TEST_UNDECLARED_OUTPUTS_DIR") {
        let directory = Path::new(&directory);
        std::fs::write(directory.join("inventory.json"), format!("{receipt}\n"))?;
        if let Some(measurements) = measurements {
            std::fs::write(
                directory.join("measurement.json"),
                format!("{measurements}\n"),
            )?;
        }
        if let Some(stats) = stats {
            std::fs::write(directory.join("verifier.json"), format!("{stats}\n"))?;
        }
    }
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
            check_summary("verification results:: 2 verified, 0 errors\n", 2, false),
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
            assert!(check_summary(output, expected, false).is_err(), "{output}");
        }
    }

    #[test]
    fn partial_verification_requires_its_marker_and_exact_count() {
        let partial = "verification results:: 2 verified, 0 errors (partial verification with `--verify-*`)\n";
        assert_eq!(check_summary(partial, 2, true), Ok(2));
        assert!(check_summary(partial, 2, false).is_err());
        assert!(check_summary(partial, 1, true).is_err());
        assert!(check_summary("verification results:: 2 verified, 0 errors\n", 2, true).is_err());
    }
}

#[cfg(test)]
mod process_tests {
    use super::check_result;

    #[test]
    fn rejects_timeout_nonzero_and_signal_even_with_a_success_summary() {
        for status in [Some(124), Some(1), None] {
            assert!(
                check_result(
                    status,
                    "verification results:: 2 verified, 0 errors\n",
                    2,
                    false
                )
                .is_err()
            );
        }
    }
}
