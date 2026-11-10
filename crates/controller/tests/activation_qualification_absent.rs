//! Hard gate for the canary-qualification staging bypass (lead decision FX-REQUAL-43 OQ-1 (a)).
//!
//! The `activation-qualification` feature lets the `fx-stage-packet` example stage a canary
//! packet before an activation pin exists. Deployed binaries must never contain that path:
//! - the feature is not default and only the example requires it (Cargo and generated Bazel);
//! - no workspace crate or deploy build enables it;
//! - the skip constructor and packet marker exist in exactly one cfg-gated source location;
//! - the gate-built deployed binaries contain neither, while the example does (positive control).
use std::path::{Path, PathBuf};

const FEATURE: &str = "activation-qualification";
const SKIP_SYMBOL: &str = "for_canary_qualification";
const MARKER: &str = "tl3.fx_canary_qualification_staging.v1";
const STAGING: &str = "src/adapters/flux_checks/staging.rs";

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn checkout() -> PathBuf {
    manifest_dir().join("../..")
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// Cargo.toml as (header, body lines). Our manifests use plain one-line keys.
fn sections(text: &str) -> Vec<(String, Vec<String>)> {
    let mut sections = vec![(String::new(), Vec::new())];
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            sections.push((line.into(), Vec::new()));
        } else if !line.is_empty() && !line.starts_with('#') {
            sections.last_mut().unwrap().1.push(line.into());
        }
    }
    sections
}

fn mentions(lines: &[String]) -> usize {
    lines.iter().filter(|line| line.contains(FEATURE)).count()
}

#[test]
fn feature_is_off_by_default_and_required_only_by_the_canary_example() {
    let text = read(&manifest_dir().join("Cargo.toml"));
    assert_eq!(
        text.matches(FEATURE).count(),
        2,
        "the feature is declared once and required once"
    );
    let sections = sections(&text);
    let features: Vec<_> = sections
        .iter()
        .filter(|(header, _)| header == "[features]")
        .collect();
    assert_eq!(features.len(), 1, "one [features] table");
    let features = &features[0].1;
    assert!(
        features
            .iter()
            .any(|line| line == "activation-qualification = []"),
        "the feature enables nothing else"
    );
    assert!(
        !features
            .iter()
            .any(|line| line.starts_with("default") && line.contains(FEATURE)),
        "the feature must never be default"
    );
    assert_eq!(mentions(features), 1);
    let mut required = Vec::new();
    for (header, lines) in &sections {
        if mentions(lines) == 0 || header == "[features]" {
            continue;
        }
        assert_eq!(
            header, "[[example]]",
            "only an example may require the feature"
        );
        required.push(lines.clone());
    }
    assert_eq!(required.len(), 1);
    assert!(required[0].contains(&"name = \"fx-stage-packet\"".to_owned()));
    assert!(required[0].contains(&"required-features = [\"activation-qualification\"]".to_owned()));
}

#[test]
fn no_workspace_crate_generated_build_or_deploy_build_enables_the_feature() {
    let checkout = checkout();
    assert!(!read(&checkout.join("Cargo.toml")).contains(FEATURE));
    let crates = checkout.join("crates");
    let mut scanned = 0;
    for entry in std::fs::read_dir(&crates).unwrap() {
        let directory = entry.unwrap().path();
        let manifest = directory.join("Cargo.toml");
        if directory.ends_with("controller") || !manifest.is_file() {
            continue;
        }
        scanned += 1;
        assert!(
            !read(&manifest).contains(FEATURE),
            "{} enables the canary-only feature",
            manifest.display()
        );
    }
    assert!(scanned > 0, "workspace manifests were not scanned");
    // The generated `features` field is unified from every workspace dependent
    // (tools/bazel/rust_builds.py), and the gate refuses a stale BUILD file.
    let build = read(&manifest_dir().join("BUILD.bazel"));
    assert_eq!(
        build.matches(FEATURE).count(),
        1,
        "only the example's features may name it"
    );
    assert!(
        build.contains(r#"example_features = {"fx-stage-packet": ["activation-qualification"]}"#)
    );
    assert!(
        !build
            .lines()
            .any(|line| line.trim_start().starts_with("features =")),
        "deployed controller binaries share the library's crate features"
    );
    for script in [
        "deploy/dl380g10/build-release.sh",
        "deploy/dl380g10/verify.sh",
    ] {
        let text = read(&checkout.join(script));
        for line in text.lines().filter(|line| line.contains("cargo ")) {
            assert!(
                !line.contains("--features") && !line.contains("--all-features"),
                "{script} builds with extra features: {line}"
            );
            assert!(
                !line.contains(FEATURE),
                "{script} builds with the feature: {line}"
            );
        }
    }
    // The release build refuses from Cargo's own artifact report, not only from bytes.
    let release = read(&checkout.join("deploy/dl380g10/build-release.sh"));
    assert!(release.contains("--message-format=json-render-diagnostics"));
    assert!(release.contains(&format!("FORBIDDEN = '{FEATURE}'")));
    assert!(!read(&checkout.join("deploy/dl380g10/verify.sh")).contains(FEATURE));
}

fn rust_sources(directory: &Path, found: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_sources(&path, found);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            found.push(path);
        }
    }
}

#[test]
fn skip_and_marker_live_only_in_cfg_gated_staging_code() {
    let root = manifest_dir();
    let mut sources = Vec::new();
    rust_sources(&root.join("src"), &mut sources);
    assert!(sources.iter().any(|path| path.ends_with(STAGING)));
    for path in &sources {
        let text = read(path);
        let staging = path.ends_with(STAGING);
        for needle in [MARKER, SKIP_SYMBOL, "CanaryQualification"] {
            if !staging {
                // Production, workers and activation never construct, call or recognise it.
                assert!(
                    !text.contains(needle) || path.ends_with("activation/tests.rs"),
                    "{} references {needle}",
                    path.display()
                );
            }
        }
        if !staging {
            continue;
        }
        assert_eq!(text.matches(MARKER).count(), 1, "one marker definition");
        let lines: Vec<_> = text.lines().collect();
        for (index, line) in lines.iter().enumerate() {
            let declares = line.contains(MARKER)
                || line.contains(&format!("fn {SKIP_SYMBOL}("))
                || line.trim_start().starts_with("CanaryQualification,")
                || line.contains("fn canary_qualification_marker(");
            if !declares {
                continue;
            }
            // The declaration's own attribute block must carry the feature gate.
            let gated = lines[..index]
                .iter()
                .rev()
                .take_while(|line| {
                    let line = line.trim_start();
                    line.starts_with("#[") || line.starts_with("///")
                })
                .any(|line| line.contains(r#"#[cfg(feature = "activation-qualification")]"#));
            assert!(gated, "{STAGING}:{} is not feature-gated", index + 1);
        }
        assert!(text.contains(&format!("pub fn {SKIP_SYMBOL}(")));
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    let (first, rest) = needle.split_first().unwrap();
    let mut offset = 0;
    while let Some(position) = haystack[offset..].iter().position(|byte| byte == first) {
        let start = offset + position;
        if haystack[start + 1..].starts_with(rest) {
            return true;
        }
        offset = start + 1;
    }
    false
}

/// A feature-enabled test build is never a deploy build; the gate builds this without it.
#[cfg(not(feature = "activation-qualification"))]
mod deployed {
    use super::*;

    /// tools/bazel/rust.bzl points CARGO_MANIFEST_DIR at this fixed runfiles path.
    const BAZEL_MANIFEST_DIR: &str = "/tmp/tl3-workspace/crates/controller";

    fn bazel() -> bool {
        env!("CARGO_MANIFEST_DIR") == BAZEL_MANIFEST_DIR
    }

    fn binary(name: &str, path: Option<&'static str>) -> Option<(String, Vec<u8>)> {
        let path = path?;
        let bytes = std::fs::read(path).unwrap_or_else(|error| panic!("{name} at {path}: {error}"));
        assert!(bytes.starts_with(b"\x7fELF"), "{name} is not an ELF binary");
        Some((name.into(), bytes))
    }

    #[test]
    fn deployed_binaries_lack_the_skip_symbol_and_marker_while_the_example_has_it() {
        let mut deployed = vec![
            binary("tl3", option_env!("CARGO_BIN_EXE_tl3")),
            binary("tl3d", option_env!("CARGO_BIN_EXE_tl3d")),
            binary(
                "tl3-run-backend",
                option_env!("CARGO_BIN_EXE_tl3-run-backend"),
            ),
            binary(
                "tl3-worker-supervisor",
                option_env!("CARGO_BIN_EXE_tl3-worker-supervisor"),
            ),
        ];
        let deploy_helper = binary(
            "example_tl3-deploy",
            option_env!("CARGO_BIN_EXE_example_tl3-deploy"),
        );
        let example = binary(
            "example_fx-stage-packet",
            option_env!("CARGO_BIN_EXE_example_fx-stage-packet"),
        );
        if bazel() {
            assert!(
                deploy_helper.is_some() && example.is_some(),
                "the gate must scan the deploy helper and the positive control"
            );
        }
        deployed.push(deploy_helper);
        let mut scanned = Vec::new();
        for (name, bytes) in deployed.into_iter().flatten() {
            for needle in [SKIP_SYMBOL, MARKER] {
                assert!(
                    !contains(&bytes, needle.as_bytes()),
                    "deployed binary {name} contains {needle}"
                );
            }
            scanned.push(name);
        }
        assert!(scanned.len() >= 4, "deployed binaries missing: {scanned:?}");
        if let Some((name, bytes)) = example {
            assert!(
                contains(&bytes, MARKER.as_bytes()),
                "positive control {name} lacks the marker; the absence scan would be vacuous"
            );
            println!(
                "positive control {name}: marker present, symbol {}",
                if contains(&bytes, SKIP_SYMBOL.as_bytes()) {
                    "present"
                } else {
                    "inlined or stripped"
                }
            );
        }
        println!("scanned deployed binaries: {scanned:?}");
    }
}

#[test]
fn byte_scan_finds_needles_anywhere() {
    assert!(contains(
        b"xxfor_canary_qualificationyy",
        SKIP_SYMBOL.as_bytes()
    ));
    assert!(contains(MARKER.as_bytes(), MARKER.as_bytes()));
    assert!(!contains(
        b"for_canary_qualificatio",
        SKIP_SYMBOL.as_bytes()
    ));
    assert!(!contains(b"ffor_canary", b"fz"));
}
