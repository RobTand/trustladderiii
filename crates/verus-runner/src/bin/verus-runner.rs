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

    fn parse_result(status: Option<i32>, output: &str) -> Result<Value, String> {
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
        Ok(stats.clone())
    }

    pub fn check_result(
        status: Option<i32>,
        output: &str,
        expected: u64,
        partial: bool,
    ) -> Result<Value, String> {
        let stats = parse_result(status, output)?;
        validate_verdict(&stats, expected, partial, false)?;
        Ok(stats)
    }

    fn validate_verdict(
        stats: &Value,
        expected: u64,
        partial: bool,
        optional_success: bool,
    ) -> Result<(), String> {
        let verdict = &stats["verification-results"];
        if expected == 0
            || verdict["verified"].as_u64() != Some(expected)
            || verdict["errors"].as_u64() != Some(0)
            || if optional_success {
                verdict
                    .get("success")
                    .is_some_and(|v| v.as_bool() != Some(true))
            } else {
                verdict["success"].as_bool() != Some(true)
            }
            || verdict["encountered-error"].as_bool() != Some(false)
            || verdict["encountered-vir-error"].as_bool() != Some(false)
            || verdict["is-verifying-entire-crate"].as_bool() != Some(!partial)
        {
            return Err(format!(
                "Verus JSON verdict failed, has the wrong selection, or differs from {expected} positive obligations: {verdict}"
            ));
        }
        Ok(())
    }

    pub fn check_mode_result(
        status: Option<i32>,
        output: &str,
        expected: u64,
        partial: bool,
        mode: super::ExecutionMode,
    ) -> Result<Value, String> {
        let stats = parse_result(status, output)?;
        if mode == super::ExecutionMode::FrontendOnly {
            let v = &stats["verification-results"];
            let smt = &stats["times-ms"]["smt"];
            if expected == 0
                || stats["times-ms"]["num-threads"].as_u64() != Some(1)
                || stats["times-ms"]["total"].as_u64().is_none()
                || v["verified"].as_u64() != Some(0)
                || v["errors"].as_u64() != Some(0)
                || v["encountered-error"].as_bool() != Some(false)
                || v["encountered-vir-error"].as_bool() != Some(false)
                || v["is-verifying-entire-crate"].as_bool() != Some(!partial)
                || v.get("success").is_some_and(|v| v.as_bool() != Some(true))
                || ["total", "smt-init", "smt-run", "rlimit-init", "rlimit-run"]
                    .iter()
                    .any(|k| smt[k].as_u64() != Some(0))
                || ["smt-init-module-times", "smt-run-module-times"]
                    .iter()
                    .any(|k| smt[k].as_array().is_none_or(|a| !a.is_empty()))
            {
                return Err("frontend requires explicit zero-query/SMT evidence and a clean zero-proof verdict".into());
            }
        } else {
            validate_verdict(&stats, expected, partial, true)?;
        }
        Ok(stats)
    }

    pub fn expanded_receipt(stats: &Value, label: &str, seconds: f64) -> Result<Value, String> {
        if seconds > 20.0 {
            eprintln!("VERUS_WALL_WARNING {label}: exceeds historical 20s ceiling ({seconds:.3}s)");
        }
        // Reuse extraction, keeping the legacy hard ceiling confined to --measure.
        let mut receipt = receipt(stats, label, seconds.min(20.0))?;
        receipt["seconds"] = json!(seconds);
        let modules = stats["times-ms"]["smt"]["smt-run-module-times"]
            .as_array()
            .ok_or("missing expanded SMT modules")?;
        let mut sum = 0u64;
        let mut count = 0u64;
        for module in modules {
            for function in module["function-breakdown"]
                .as_array()
                .ok_or("missing function breakdown")?
            {
                sum = sum
                    .checked_add(
                        function["rlimit"]
                            .as_u64()
                            .ok_or("missing function rlimit")?,
                    )
                    .ok_or("function sum overflow")?;
                count += 1;
            }
        }
        if count == 0 || receipt["rlimit_run"].as_u64() != Some(sum) {
            return Err("expanded function sum differs from raw run rlimit or is empty".into());
        }
        receipt["function_rlimit_sum"] = json!(sum);
        receipt["function_count"] = json!(count);
        Ok(receipt)
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

        #[test]
        fn frontend_is_never_a_positive_proof_or_resource_measurement() {
            let mut v = verdict();
            v["verification-results"]["verified"] = json!(0);
            v["times-ms"] = json!({"total": 1, "num-threads": 1, "smt": {"total": 0, "smt-init": 0, "smt-run": 0,
                "rlimit-init": 0, "rlimit-run": 0, "smt-init-module-times": [], "smt-run-module-times": []}});
            let text = serde_json::to_string_pretty(&v).unwrap();
            assert!(
                check_mode_result(
                    Some(0),
                    &text,
                    2,
                    false,
                    super::super::ExecutionMode::FrontendOnly
                )
                .is_ok()
            );
            assert!(check_result(Some(0), &text, 2, false).is_err());
            assert!(expanded_receipt(&v, "fake", 1.0).is_err());
            for status in [Some(127), Some(124), None] {
                assert!(
                    check_mode_result(
                        status,
                        &text,
                        2,
                        false,
                        super::super::ExecutionMode::FrontendOnly
                    )
                    .is_err()
                );
            }
            for field in ["rlimit-init", "rlimit-run", "smt-run-module-times"] {
                let mut bad = v.clone();
                bad["times-ms"]["smt"]
                    .as_object_mut()
                    .unwrap()
                    .remove(field);
                assert!(
                    check_mode_result(
                        Some(0),
                        &serde_json::to_string_pretty(&bad).unwrap(),
                        2,
                        false,
                        super::super::ExecutionMode::FrontendOnly
                    )
                    .is_err()
                );
            }
        }

        #[test]
        fn normal_expanded_metrics_exclude_init_and_warn_only_for_wall() {
            let mut v = verdict();
            v["verification-results"]
                .as_object_mut()
                .unwrap()
                .remove("success");
            v["times-ms"] = json!({"smt": {"rlimit-init": 900, "rlimit-run": 7,
                "smt-run-module-times": [{"function-breakdown": [{"rlimit": 3}, {"rlimit": 4}]}]}});
            let text = serde_json::to_string_pretty(&v).unwrap();
            assert!(
                check_mode_result(
                    Some(0),
                    &text,
                    2,
                    false,
                    super::super::ExecutionMode::NormalStats
                )
                .is_ok()
            );
            let r = expanded_receipt(&v, "fake", 25.0).unwrap();
            assert_eq!(r["rlimit_run"], 7);
            assert_eq!(r["rlimit_init"], 900);
            assert_eq!(r["function_rlimit_sum"], 7);
            assert_eq!(r["function_count"], 2);
            assert_eq!(r["seconds"], 25.0);
            v["times-ms"]["smt"]["rlimit-run"] = json!(907);
            assert!(expanded_receipt(&v, "fake", 1.0).is_err());
        }

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

// Capability checked by the measurement driver before any submission.
const MEASUREMENT_PROTOCOL: &str = "tl3.verus-runner.modes.v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExecutionMode {
    Proof,
    LegacyMeasure,
    FrontendOnly,
    NormalStats,
    DiagnosticStats,
}

impl ExecutionMode {
    fn selected(measured: bool) -> Result<Self, String> {
        match std::env::var("TL3_VERUS_MODE").as_deref() {
            Err(std::env::VarError::NotPresent) => Ok(if measured {
                Self::LegacyMeasure
            } else {
                Self::Proof
            }),
            Ok("frontend-only") if !measured => Ok(Self::FrontendOnly),
            Ok("normal") if !measured => Ok(Self::NormalStats),
            Ok("diagnostic") if !measured => Ok(Self::DiagnosticStats),
            _ => Err("unsupported or conflicting TL3_VERUS_MODE".into()),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Proof => "proof",
            Self::LegacyMeasure => "legacy-measure",
            Self::FrontendOnly => "frontend-only",
            Self::NormalStats => "normal",
            Self::DiagnosticStats => "diagnostic",
        }
    }

    fn modern(self) -> bool {
        matches!(
            self,
            Self::FrontendOnly | Self::NormalStats | Self::DiagnosticStats
        )
    }
}

fn mode_log(
    mode: ExecutionMode,
    metrics: &serde_json::Value,
    raw: &serde_json::Value,
    receipt: &serde_json::Value,
) -> String {
    let _ = (raw, receipt);
    format!("VERUS_MODE {} {metrics}", mode.name())
}

struct Toolchain {
    rustc: PathBuf,
    verifier: PathBuf,
    z3: PathBuf,
    sysroot: PathBuf,
}

struct Verification {
    status: Option<i32>,
    output: String,
    seconds: f64,
    execution: serde_json::Value,
}

impl Toolchain {
    fn new(verifier: &Path, z3: &Path, rustc: &Path) -> std::io::Result<Self> {
        let rustc = rustc.canonicalize()?;
        let sysroot = rustc
            .parent()
            .and_then(Path::parent)
            .ok_or_else(|| std::io::Error::other("missing private sysroot"))?
            .to_owned();
        Ok(Self {
            verifier: verifier.canonicalize()?,
            z3: z3.canonicalize()?,
            rustc,
            sysroot,
        })
    }

    fn verify(
        &self,
        source: &Path,
        module: &str,
        mode: ExecutionMode,
    ) -> std::io::Result<Verification> {
        let mut process = Command::new("/usr/bin/timeout");
        process
            .args([
                "--kill-after=5s",
                if mode == ExecutionMode::DiagnosticStats {
                    "120s"
                } else {
                    "45s"
                },
            ])
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
        match mode {
            ExecutionMode::Proof => {}
            ExecutionMode::LegacyMeasure => {
                process.args(["--time", "--output-json"]);
            }
            _ => {
                process.args(["--time-expanded", "--output-json"]);
            }
        }
        if mode == ExecutionMode::FrontendOnly {
            process.arg("--no-verify");
        }
        if mode != ExecutionMode::FrontendOnly
            && mode.modern()
            && std::env::var("TL3_VERUS_RAW_QUERIES").as_deref() == Ok("1")
        {
            let directory = std::env::var_os("TEST_UNDECLARED_OUTPUTS_DIR")
                .ok_or_else(|| std::io::Error::other("raw queries require output directory"))?;
            let logs = Path::new(&directory).join("raw-queries");
            std::fs::create_dir_all(&logs)?;
            process.arg("--log-all").arg("--log-dir").arg(logs);
        }
        let argv: Vec<_> = std::iter::once(process.get_program())
            .chain(process.get_args())
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        let mut tools = serde_json::Map::new();
        if mode.modern() {
            let rust_verify = self.verifier.with_file_name("rust_verify").canonicalize()?;
            for (name, path) in [
                ("verifier", &self.verifier),
                ("z3", &self.z3),
                ("rustc", &self.rustc),
                ("rust_verify", &rust_verify),
            ] {
                let hash = Command::new("/usr/bin/sha256sum").arg(path).output()?;
                if !hash.status.success() {
                    return Err(std::io::Error::other("tool identity hash failed"));
                }
                let text = String::from_utf8_lossy(&hash.stdout);
                let sha = text
                    .split_whitespace()
                    .next()
                    .ok_or_else(|| std::io::Error::other("missing tool hash"))?;
                tools.insert(name.into(), serde_json::json!({"path": path, "sha256": sha, "bytes": path.metadata()?.len()}));
            }
        }
        let pins = if mode.modern() {
            let path = self
                .sysroot
                .parent()
                .ok_or_else(|| std::io::Error::other("missing toolchain root"))?
                .join("pins.json");
            serde_json::from_slice::<serde_json::Value>(&std::fs::read(path)?)?
        } else {
            serde_json::Value::Null
        };
        let started = Instant::now();
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
            execution: serde_json::json!({
                "protocol": MEASUREMENT_PROTOCOL, "mode": mode.name(), "argv": argv,
                "tools": tools, "pins": pins, "sysroot": self.sysroot,
                "environment": {"PATH": format!("{}/bin:/usr/bin:/bin", self.sysroot.display()),
                    "LD_LIBRARY_PATH": self.sysroot.join("lib"), "VERUS_USE_RUSTUP": "0",
                    "VERUS_Z3_PATH": self.z3, "OMP_NUM_THREADS": "1", "RAYON_NUM_THREADS": "1"},
                "status": result.status.code(),
            }),
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
    let toolchain = Toolchain::new(Path::new(verifier), Path::new(z3), Path::new(rustc))?;
    let source = Path::new(source);
    let mode = ExecutionMode::selected(measured)?;
    let result = toolchain.verify(source, module, mode)?;
    if mode.modern() {
        print!("{}", result.output);
        let directory = std::env::var_os("TEST_UNDECLARED_OUTPUTS_DIR")
            .ok_or("measurement mode requires TEST_UNDECLARED_OUTPUTS_DIR")?;
        let directory = Path::new(&directory);
        std::fs::write(
            directory.join("execution.json"),
            format!("{}\n", result.execution),
        )?;
        let stats = measurement::check_mode_result(
            result.status,
            &result.output,
            expected,
            !module.is_empty(),
            mode,
        )?;
        std::fs::write(directory.join("verifier.json"), format!("{stats}\n"))?;
        let frontend = mode == ExecutionMode::FrontendOnly;
        let receipt = serde_json::json!({"proofs": {label: {
            "kind": if frontend { "FRONTEND_ONLY" } else { "verus" },
            "expected_obligations": expected, "verified": if frontend { 0 } else { expected },
            "module": module, "threads": 1, "seconds": result.seconds,
        }}});
        let metrics = if frontend {
            serde_json::json!({"label": label, "mode": "FRONTEND_ONLY", "no_smt": true,
                "proof": "NOT_RUN", "resource_acceptance": "NOT_APPLICABLE"})
        } else {
            measurement::expanded_receipt(&stats, label, result.seconds)?
        };
        std::fs::write(directory.join("inventory.json"), format!("{receipt}\n"))?;
        std::fs::write(directory.join("measurement.json"), format!("{metrics}\n"))?;
        let raw = serde_json::json!({"label": label, "seconds": result.seconds, "stats": stats});
        println!("{}", mode_log(mode, &metrics, &raw, &receipt));
        return Ok(());
    }
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
        let changed = toolchain.verify(
            &copied.join(source.strip_prefix(root)?),
            selection,
            ExecutionMode::Proof,
        )?;
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

#[cfg(test)]
mod toolchain_tests {
    use super::*;

    #[test]
    fn modern_process_receipt_includes_the_actual_verifier_binary() {
        use std::os::unix::fs::PermissionsExt;
        let root =
            std::env::temp_dir().join(format!("toolchain-process-fixture-{}", std::process::id()));
        std::fs::create_dir_all(root.join("rust/bin")).unwrap();
        std::fs::create_dir(root.join("verus")).unwrap();
        for name in ["rust/bin/rustc", "verus/z3", "verus/rust_verify"] {
            std::fs::write(root.join(name), "FAKE tool bytes, never a solver").unwrap();
        }
        std::fs::write(root.join("pins.json"), "{}").unwrap();
        let verifier = root.join("verus/verus");
        std::fs::write(&verifier, "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&verifier, std::fs::Permissions::from_mode(0o700)).unwrap();
        let tools = Toolchain::new(
            &verifier,
            &root.join("verus/z3"),
            &root.join("rust/bin/rustc"),
        )
        .unwrap();
        let result = tools
            .verify(
                Path::new("FAKE.rs"),
                "kernel#fake",
                ExecutionMode::FrontendOnly,
            )
            .unwrap();
        assert_eq!(result.status, Some(0));
        let identities = result.execution["tools"].as_object().unwrap();
        assert_eq!(identities.len(), 4);
        assert_eq!(
            identities["rust_verify"]["path"],
            serde_json::json!(root.join("verus/rust_verify").canonicalize().unwrap())
        );
        assert_eq!(
            identities["rust_verify"]["sha256"].as_str().unwrap().len(),
            64
        );
        let argv = result.execution["argv"].as_array().unwrap();
        assert!(argv.contains(&serde_json::json!("--no-verify")));
        assert!(!argv.contains(&serde_json::json!("--build-sst")));
        assert_eq!(
            result.execution["sysroot"],
            serde_json::json!(root.join("rust").canonicalize().unwrap())
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn launcher_rustc_is_canonicalized_before_deriving_sysroot() {
        use std::os::unix::fs::symlink;
        let root = std::env::temp_dir().join(format!("toolchain-fixture-{}", std::process::id()));
        std::fs::create_dir_all(root.join("real/rust/bin")).unwrap();
        for name in ["real/rust/bin/rustc", "real/verus", "real/z3"] {
            std::fs::write(root.join(name), "FAKE tool identity").unwrap();
        }
        symlink(
            root.join("real/rust/bin/rustc"),
            root.join("launcher-rustc"),
        )
        .unwrap();
        symlink(root.join("real/verus"), root.join("verus")).unwrap();
        let tools = Toolchain::new(
            &root.join("verus"),
            &root.join("real/z3"),
            &root.join("launcher-rustc"),
        )
        .unwrap();
        assert_eq!(
            tools.sysroot,
            root.join("real/rust").canonicalize().unwrap()
        );
        assert_eq!(
            tools.rustc,
            root.join("real/rust/bin/rustc").canonicalize().unwrap()
        );
        assert_eq!(
            tools.verifier,
            root.join("real/verus").canonicalize().unwrap()
        );
        assert!(Toolchain::new(&root.join("missing"), &tools.z3, &tools.rustc).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
mod receipt_tests {
    use super::*;

    #[test]
    fn modern_modes_emit_unique_gate_receipts_without_frontend_proof_claims() {
        let label = "//crates/kernel:verus_fixture";
        let stats = serde_json::json!({
            "verification-results": {"verified": 1, "errors": 0,
                "encountered-error": false, "encountered-vir-error": false,
                "is-verifying-entire-crate": false},
            "times-ms": {"smt": {"rlimit-run": 1234}},
            "verus": {"version": "0.2026.11.02.8bf68ee"}
        });
        let raw = serde_json::json!({"label": label, "seconds": 1.0, "stats": stats});
        let receipt = serde_json::json!({"proofs": {label: {"verified": 1, "expected_obligations": 1}}});
        let metrics = serde_json::json!({"rlimit_run": 1234});
        for mode in [ExecutionMode::NormalStats, ExecutionMode::DiagnosticStats] {
            let output = mode_log(mode, &metrics, &raw, &receipt);
            if mode == ExecutionMode::NormalStats {
                println!("MARKER_FIXTURE_START\n{output}\nMARKER_FIXTURE_END");
            }
            for (prefix, expected) in [
                ("VERUS_RAW_MEASUREMENT ", &raw),
                ("VERUS_PROOF ", &receipt),
            ] {
                let lines: Vec<_> = output.lines().filter_map(|s| s.strip_prefix(prefix)).collect();
                assert_eq!(lines.len(), 1, "missing or duplicate {prefix}");
                assert_eq!(&serde_json::from_str::<serde_json::Value>(lines[0]).unwrap(), expected);
            }
            assert_eq!(output.lines().filter(|s| s.starts_with("VERUS_MODE ")).count(), 1);
        }
        let frontend = mode_log(ExecutionMode::FrontendOnly, &metrics, &raw, &receipt);
        assert!(!frontend.contains("VERUS_RAW_MEASUREMENT"));
        assert!(!frontend.contains("VERUS_PROOF"));
    }
}
