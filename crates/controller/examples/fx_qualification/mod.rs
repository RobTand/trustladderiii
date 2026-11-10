//! Rerunnable qualification coordinator. Computation runs only under admission.
mod admission;
mod bb;
mod evidence;
mod generator;
#[cfg(test)]
mod launch_failure_fixture;
mod live;
mod policy;
mod prepare;
#[cfg(test)]
mod producer_fixture;
mod publish;
#[cfg(test)]
mod tests;

use super::{Result, git, identity, write};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
};
use tl3_controller::run_backend::{self as api, durable};

#[path = "../../tools/generate_fx_qualification.rs"]
#[allow(dead_code)]
mod d8;

pub use bb::inventory_build;

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    Ok(serde_json::from_slice(&durable::read(path)?)?)
}

pub fn admitted_command(kind: &str, path: &Path) -> Result<()> {
    let inputs: Inputs = read_json(path)?;
    match kind {
        "admitted-prepare" => prepare::admitted(&inputs),
        "admitted-generator-prepare" => generator::admitted(&inputs),
        "admitted-verify" => publish::verify_private(&inputs),
        _ => Err("unknown admitted operation".into()),
    }
}

const PQ: &str = "dl380g10:5000/tl3/prismaquant@sha256:47273bfe2a3352d7e37e24a3e6c425adc8dc198e7a60002bfcee9957c4902dbc";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Inputs {
    repo: PathBuf,
    head: String,
    test_image: String,
    generator_image: String,
    image_map: PathBuf,
    image_map_sha256: String,
    out: PathBuf,
    readiness_seconds: u64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct State {
    schema: String,
    inputs: Inputs,
    phase: String,
    builds: Vec<bb::Build>,
    canaries: Vec<evidence::CanaryEvidence>,
}

fn lifecycle_canaries(
    canaries: &[evidence::CanaryEvidence],
) -> Result<Vec<evidence::CanaryEvidence>> {
    ["launch_failure", "cancel", "success"]
        .into_iter()
        .map(|kind| {
            let mut matches = canaries.iter().filter(|canary| canary.kind == kind);
            let canary = matches.next().ok_or("missing lifecycle canary kind")?;
            if matches.next().is_some() {
                return Err("duplicate lifecycle canary kind".into());
            }
            Ok(canary.clone())
        })
        .collect()
}

fn retain_canary(canaries: &mut Vec<evidence::CanaryEvidence>, result: evidence::CanaryEvidence) {
    canaries.retain(|canary| canary.kind != result.kind);
    canaries.push(result);
    canaries.sort_by_key(|canary| match canary.kind.as_str() {
        "launch_failure" => 0,
        "cancel" => 1,
        "success" => 2,
        "generator" => 3,
        _ => 4,
    });
}

fn collect_lifecycle(
    inputs: &Inputs,
    state: &mut State,
    mut collect: impl FnMut(&str) -> Result<evidence::CanaryEvidence>,
) -> Result<()> {
    for kind in ["launch_failure", "cancel", "success"] {
        let result = collect(kind)?;
        let pending = result.job.is_some() && result.terminal_receipt.is_none();
        retain_canary(&mut state.canaries, result);
        save(&inputs.out, state)?;
        if pending {
            return Err("lifecycle job not terminal; slot B retained".into());
        }
    }
    if lifecycle_canaries(&state.canaries)?
        .iter()
        .any(|result| result.status != "PASS")
    {
        return Err("lifecycle canaries did not pass; no D8 inputs generated".into());
    }
    Ok(())
}

fn exit_code(verdict: &Value, dry: bool) -> i32 {
    if verdict["status"] == "PASS"
        || dry
            && verdict["errors"].as_array().is_some_and(|errors| {
                errors.len() == 1 && errors[0]["code"] == "dry_run_no_jobs_submitted"
            })
    {
        0
    } else if verdict["status"] == "FAIL" {
        1
    } else {
        2
    }
}

fn save(out: &Path, state: &State) -> Result<()> {
    durable::atomic(&out.join("state.json"), &serde_json::to_vec_pretty(state)?)?;
    Ok(())
}

fn parse(args: &[std::ffi::OsString]) -> Result<(Inputs, bool)> {
    let mut values = BTreeMap::new();
    let mut dry = false;
    let mut cursor = args.iter();
    while let Some(name) = cursor.next() {
        let name = name.to_str().ok_or("UTF-8 option required")?;
        if name == "--dry-run" {
            if dry {
                return Err("duplicate --dry-run".into());
            }
            dry = true;
            continue;
        }
        if ![
            "--repo",
            "--head",
            "--test-image",
            "--generator-image",
            "--image-map",
            "--image-map-sha256",
            "--out",
            "--readiness-seconds",
        ]
        .contains(&name)
        {
            return Err("unknown qualification option".into());
        }
        if values
            .insert(
                name,
                cursor
                    .next()
                    .ok_or("missing option value")?
                    .to_str()
                    .ok_or("UTF-8 input required")?,
            )
            .is_some()
        {
            return Err("duplicate qualification option".into());
        }
    }
    let required = |key| {
        values
            .get(key)
            .copied()
            .ok_or("missing qualification option")
    };
    let inputs = Inputs {
        repo: required("--repo")?.into(),
        head: required("--head")?.into(),
        test_image: required("--test-image")?.into(),
        generator_image: required("--generator-image")?.into(),
        image_map: required("--image-map")?.into(),
        image_map_sha256: required("--image-map-sha256")?.into(),
        out: required("--out")?.into(),
        readiness_seconds: values
            .get("--readiness-seconds")
            .unwrap_or(&"120")
            .parse()?,
    };
    Ok((inputs, dry))
}

fn validate(inputs: &Inputs) -> Result<()> {
    if !api::hex(&inputs.head, 40)
        || !api::hex(&inputs.image_map_sha256, 64)
        || !api::absolute(&inputs.repo)
        || !api::absolute(&inputs.out)
        || !api::absolute(&inputs.image_map)
        || !inputs.out.starts_with("/mnt/shared/flux-out/tl3")
        || inputs.out == Path::new("/mnt/shared/flux-out/tl3")
        || inputs.generator_image != PQ
        || !(1..=600).contains(&inputs.readiness_seconds)
    {
        return Err("qualification input contract refused".into());
    }
    for path in [&inputs.repo, &inputs.out, &inputs.image_map] {
        if path.ancestors().any(Path::is_symlink) {
            return Err("symlink input refused".into());
        }
    }
    let head = git(&inputs.repo, &["rev-parse", "HEAD"])?;
    if head != inputs.head {
        let staged: bb::OverlayCommit = read_json(&inputs.out.join("publication.json"))?;
        if head != staged.commit
            || staged.base_head != inputs.head
            || git(&inputs.repo, &["rev-list", "--parents", "-n1", &head])?
                != format!("{head} {}", inputs.head)
            || git(&inputs.repo, &["rev-parse", "HEAD^{tree}"])? != staged.tree
        {
            return Err("isolated checkout at H or owned published candidate required".into());
        }
    }
    if !git(
        &inputs.repo,
        &["status", "--porcelain", "--untracked-files=all"],
    )?
    .is_empty()
    {
        return Err("clean qualification checkout required".into());
    }
    if inputs.repo == Path::new("/home/rob/.t3/projects/trustladderiii") {
        return Err("shared project checkout refused".into());
    }
    if identity(&inputs.image_map)?["sha256"] != inputs.image_map_sha256 {
        return Err("image map digest mismatch".into());
    }
    let map: Value = read_json(&inputs.image_map)?;
    for (profile, image) in [
        ("sandbox", &inputs.test_image),
        ("prismaquant", &inputs.generator_image),
    ] {
        let matches: Vec<_> = map["profiles"]
            .as_array()
            .ok_or("image map profiles")?
            .iter()
            .filter(|entry| entry["profile"] == profile && entry["registry_ref"] == *image)
            .collect();
        if matches.len() != 1
            || matches[0]["platform"] != "linux/amd64"
            || !matches[0]["rootless_present_hosts"]
                .as_array()
                .is_some_and(|hosts| hosts.iter().any(|host| host == "dl380g10"))
        {
            return Err("immutable provisioned image map entry required".into());
        }
    }
    Ok(())
}

fn plan(inputs: &Inputs) -> Value {
    let jobs: Vec<_> = [
        ("preparation", 600, false), ("launch_failure", 120, false),
        ("cancel", 300, false), ("success", 120, false), ("generator_preparation", 300, false), ("generator", 240, generator::PASS_FLUX_URI),
        ("canonical_verification", 600, false),
    ].into_iter().map(|(kind, duration, pass_flux_uri)| json!({
        "kind":kind,"version":1,
        "resources":[{"type":"slot","count":1,"label":"task","with":[{"type":"core","count":1},{"type":"memory","count":2}]}],
        "tasks":[{"command":["<command-materialized-by-admitted-preparation>"],"slot":"task","count":{"per_slot":1}}],
        "attributes":{"system":{"duration":duration,"environment":{"FLUX_F58_FORCE_ASCII":"1"}}},"gpus":0,"parallelism":1,"native_threads":1,
        "host_dependency":{"host":"dl380g10","reason":"provisioned amd64 image store and host interpreter"},
        "environment_policy":{"start_empty":true,"request_allowlist":["HOME","PATH","XDG_RUNTIME_DIR"],
            "controller_allowlist":["OUT","TL3_RUN_DIAGNOSTIC_DIR","FLUX_SCRATCH","TL3_RUN_BACKEND","TL3_RUN_INSTANCE",
                "TL3_RUN_OWNER","TL3_RUN_CORRELATION","TL3_RUN_REQUEST_DIGEST","TL3_EXECUTION_BACKEND","TL3_RUN_BACKEND_SESSION",
                "TL3_RUN_BACKEND_CLI","TL3_RUN_BACKEND_CONFIG"],
            "native_thread_allowlist":["OMP_NUM_THREADS","MKL_NUM_THREADS","OPENBLAS_NUM_THREADS","TORCH_NUM_THREADS",
                "NUMEXPR_NUM_THREADS","VECLIB_MAXIMUM_THREADS","BLIS_NUM_THREADS"],"pass_flux_uri":pass_flux_uri},
        "rlimit_flags":["--rlimit=-*"],"submission":"NOT_SUBMITTED"
    })).collect();
    json!({"schema":"tl3.fx_qualification_plan.v1","head":inputs.head,"serial":true,
        "flux_job_count":jobs.len(),"jobs":jobs,"readiness_seconds":inputs.readiness_seconds,
        "readiness":"flushed marker contents AND durable qualified container binding; never timer or file existence",
        "cancel_scope":"one owned ready job only", "image_map_sha256":inputs.image_map_sha256})
}

fn dry_result(inputs: &Inputs, builds: Value) -> Result<Value> {
    let planned = plan(inputs);
    durable::atomic(
        &inputs.out.join("plan.json"),
        &serde_json::to_vec_pretty(&planned)?,
    )?;
    println!("{}", serde_json::to_string_pretty(&planned)?);
    Ok(
        json!({"schema":"tl3.fx_executor_qualification.v1","status":"UNKNOWN","complete":false,
        "qualified_head":inputs.head,"qualified_tree":git(&inputs.repo,&["rev-parse",&format!("{}^{{tree}}",inputs.head)])?,
        "builds":builds,"canaries":[],"pin":null,"errors":[{"phase":"planning","code":"dry_run_no_jobs_submitted","evidence":[]}]}),
    )
}

fn failure_verdict(inputs: &Inputs, state: &State, error: &str) -> Value {
    let preparation_jobs: Vec<_> = ["preparation-job", "generator-preparation-job"]
        .into_iter()
        .filter_map(|name| admission::retained(inputs, name))
        .collect();
    let verification_jobs: Vec<_> = admission::retained(inputs, "verification-job")
        .into_iter()
        .collect();
    json!({"schema":"tl3.fx_executor_qualification.v1","status":if state.canaries.iter().any(|c|c.status=="FAIL"){"FAIL"}else{"UNKNOWN"},"complete":false,
            "qualified_head":inputs.head,"qualified_tree":git(&inputs.repo,&["rev-parse",&format!("{}^{{tree}}",inputs.head)]).ok(),
            "executor_pathset":{"version":policy::PATHSET_VERSION,"entries":evidence::pathset(&inputs.repo,&inputs.head).ok()},
            "inputs":{"image_map":identity(&inputs.image_map).ok(),"test_image":inputs.test_image,"generator_image":inputs.generator_image},
            "builds":state.builds,"preparation_jobs":preparation_jobs,
            "verification_jobs":verification_jobs,"canaries":state.canaries,"pin":null,
            "coverage":{"lifecycle":false,"real_image_generator":false,"go_live_profiles":[]},
            "errors":[{"phase":state.phase,"code":error,"evidence":[]}]})
}

pub fn command(args: &[std::ffi::OsString]) -> Result<i32> {
    if args.first().is_some_and(|s| s == "carry") {
        return evidence::carry(&args[1..]);
    }
    let (inputs, dry) = parse(&args[1..])?;
    validate(&inputs)?;
    unsafe {
        libc::umask(0o077);
    }
    if !inputs.out.exists() {
        fs::create_dir(&inputs.out)?;
    }
    let metadata = fs::symlink_metadata(&inputs.out)?;
    if !metadata.is_dir()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.permissions().mode() & 0o077 != 0
    {
        return Err("owner-only nonsymlink output directory required".into());
    }
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(inputs.out.join(".lock"))?;
    use std::os::fd::AsRawFd;
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err("qualification output already owned by another coordinator".into());
    }
    let state_path = inputs.out.join("state.json");
    let mut state: State = if state_path.exists() {
        let state: State = read_json(&state_path)?;
        if state.schema != "tl3.fx_qualification_state.v1" || state.inputs != inputs {
            return Err("retained qualification inputs changed; use a new output directory".into());
        }
        state
    } else {
        State {
            schema: "tl3.fx_qualification_state.v1".into(),
            inputs: inputs.clone(),
            phase: "preparing".into(),
            builds: vec![],
            canaries: vec![],
        }
    };
    if !dry {
        bb::preflight_transport(&inputs)?;
        bb::verify_client()?;
    }
    save(&inputs.out, &state)?;
    let outcome = (|| -> Result<Value> {
        if inputs.out.join("verdict.json").exists() {
            let retained: Value = read_json(&inputs.out.join("verdict.json"))?;
            if retained["complete"] == true || retained["status"] == "FAIL" {
                evidence::verify_references(&retained)?;
                return Ok(retained);
            }
        }
        if dry && std::env::var_os("BUILDBUDDY_ARTIFACTS_DIRECTORY").is_some() {
            let preview = bb::admitted_preview(&inputs)?;
            return dry_result(&inputs, json!([preview]));
        }
        let build = bb::build(&inputs, "production", None)?;
        let rebuilt = inputs.out.join("production/example_fx-stage-packet");
        if identity(&std::env::current_exe()?)?["sha256"] != identity(&rebuilt)?["sha256"] {
            use std::os::unix::process::CommandExt;
            return Err(std::process::Command::new(rebuilt)
                .args(std::env::args_os().skip(1))
                .exec()
                .into());
        }
        state.builds = vec![build];
        save(&inputs.out, &state)?;
        let planned = plan(&inputs);
        durable::atomic(
            &inputs.out.join("plan.json"),
            &serde_json::to_vec_pretty(&planned)?,
        )?;
        if dry {
            return dry_result(&inputs, serde_json::to_value(&state.builds)?);
        }
        let input_path = inputs.out.join("inputs.json");
        if !input_path.exists() {
            write(&input_path, &inputs)?;
        }
        let options = admission::options(&inputs, &inputs.out.join("production"))?;
        let (exit, hosts) = admission::capture(
            &options,
            &["exec".into(), "-rall".into(), "/bin/hostname".into()],
            &inputs.out.join("host-readiness.log"),
        )?;
        let hosts = String::from_utf8(hosts)?;
        if exit != 0
            || ["dl380g10", "sparky", "sparklina"]
                .iter()
                .any(|host| !hosts.lines().any(|line| line.trim() == *host))
        {
            return Err("all three standing Flux hosts must be reachable before admission".into());
        }
        let command = |binary: &str, operation: &str| {
            vec![
                inputs
                    .out
                    .join(binary)
                    .join("example_fx-stage-packet")
                    .to_string_lossy()
                    .into_owned(),
                operation.into(),
                input_path.to_string_lossy().into_owned(),
            ]
        };
        let preparation = admission::run(
            &inputs,
            &options,
            "preparation-job",
            &command("production", "admitted-prepare"),
            600,
        )?;
        state.phase = "lifecycle".into();
        save(&inputs.out, &state)?;
        collect_lifecycle(&inputs, &mut state, |kind| {
            let packet = inputs.out.join(format!("packet-{kind}"));
            let request: api::Request = read_json(&packet.join("request.json"))?;
            let options: api::Options = read_json(&packet.join("held-options.json"))?;
            live::collect(&inputs, request, options, kind)
        })?;
        state.phase = "lifecycle_complete".into();
        save(&inputs.out, &state)?;
        let overlay = prepare::lifecycle_inputs(&inputs, &lifecycle_canaries(&state.canaries)?)?;
        let qualified = bb::build(&inputs, "qualified-production", Some(&overlay))?;
        if state.builds.len() == 1 {
            state.builds.push(qualified.clone());
        }
        state.phase = "pin_built".into();
        save(&inputs.out, &state)?;
        let generator_preparation = admission::run(
            &inputs,
            &options,
            "generator-preparation-job",
            &command("qualified-production", "admitted-generator-prepare"),
            300,
        )?;
        let request: api::Request = read_json(&inputs.out.join("generator-prepared/request.json"))?;
        let gen_options: api::Options =
            read_json(&inputs.out.join("generator-journal/options.json"))?;
        let result = live::collect(&inputs, request, gen_options, "generator")?;
        retain_canary(&mut state.canaries, result.clone());
        save(&inputs.out, &state)?;
        if result.status != "PASS" {
            return Err("generator canary did not pass; checkout pin not published".into());
        }
        state.phase = "generator_complete".into();
        save(&inputs.out, &state)?;
        let verification = admission::run(
            &inputs,
            &options,
            "verification-job",
            &command("qualified-production", "admitted-verify"),
            600,
        );
        let checked = inputs.out.join("generator-checked.json");
        if checked.exists() {
            let result: evidence::CanaryEvidence = read_json(&checked)?;
            evidence::verify_references(&serde_json::to_value(&result)?)?;
            retain_canary(&mut state.canaries, result);
            save(&inputs.out, &state)?;
        }
        let verification = verification?;
        let pin = publish::publish(&inputs, &overlay, &state.canaries, &qualified)?;
        let entries = evidence::pathset(&inputs.repo, &inputs.head)?;
        let decisions = evidence::decisions(&state.canaries)?;
        if policy::verdict(&decisions, true) != policy::Status::Pass {
            return Err("final decision refused incomplete qualification".into());
        }
        state.phase = "complete".into();
        save(&inputs.out, &state)?;
        Ok(
            json!({"schema":"tl3.fx_executor_qualification.v1","status":"PASS","complete":true,
            "qualified_head":inputs.head,"qualified_tree":git(&inputs.repo,&["rev-parse",&format!("{}^{{tree}}",inputs.head)])?,
            "executor_pathset":{"version":policy::PATHSET_VERSION,"sha256":durable::digest(&serde_json::to_vec(&entries)?),"entries":entries},
            "inputs":{"image_map":identity(&inputs.image_map)?,"test_image":inputs.test_image,"generator_image":inputs.generator_image},
            "builds":state.builds,"preparation_jobs":[preparation,generator_preparation],"verification_jobs":[verification],"canaries":state.canaries,
            "candidate_pin_commit":pin["commit"],"candidate_pin_tree":pin["tree"],"pin":pin,"coverage":{"lifecycle":true,"real_image_generator":true,"go_live_profiles":[]},"errors":[]}),
        )
    })();
    if outcome.is_err() {
        for kind in ["launch_failure", "cancel", "success", "generator"] {
            if !state.canaries.iter().any(|c| c.kind == kind) {
                state.canaries.push(evidence::CanaryEvidence {
                    kind: kind.into(),
                    job: None,
                    status: "NOT_RUN".into(),
                    ending: None,
                    terminal_receipt: None,
                    cleanup: None,
                    executions: vec![],
                    artifacts: vec![],
                    checks: vec![],
                });
            }
        }
    }
    let verdict = match outcome {
        Ok(value) => value,
        Err(error) => failure_verdict(&inputs, &state, &error.to_string()),
    };
    durable::atomic(
        &inputs.out.join("verdict.json"),
        &serde_json::to_vec_pretty(&verdict)?,
    )?;
    println!(
        "{}",
        json!({"verdict":inputs.out.join("verdict.json"),"status":verdict["status"]})
    );
    Ok(exit_code(&verdict, dry))
}
