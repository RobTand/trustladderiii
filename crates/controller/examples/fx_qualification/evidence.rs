use super::*;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CanaryEvidence {
    pub kind: String,
    pub job: Option<Value>,
    pub status: String,
    pub ending: Option<api::Ending>,
    pub terminal_receipt: Option<api::BlobRef>,
    pub cleanup: Option<api::CleanupResult>,
    pub executions: Vec<Value>,
    pub artifacts: Vec<Value>,
    pub checks: Vec<Check>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Check {
    pub name: String,
    pub pass: bool,
    pub evidence: Vec<api::BlobRef>,
}

pub fn verify_references(value: &Value) -> Result<()> {
    match value {
        Value::Object(fields) => {
            if let (Some(path), Some(hash), Some(bytes)) = (fields.get("path").and_then(Value::as_str),
                fields.get("sha256").and_then(Value::as_str),fields.get("bytes").and_then(Value::as_u64)) {
                let path = Path::new(path);
                if !api::absolute(path) || path.ancestors().any(Path::is_symlink) {
                    return Err("evidence path must be absolute and nonsymlink".into());
                }
                let observed = identity(path)?;
                if observed["sha256"] != hash || observed["bytes"] != bytes {
                    return Err("referenced evidence hash/size changed".into());
                }
                if let Some(mode) = fields.get("mode") {
                    if observed["mode"] != *mode { return Err("referenced artifact mode changed".into()); }
                }
            }
            for child in fields.values() { verify_references(child)?; }
        }
        Value::Array(children) => { for child in children { verify_references(child)?; } }
        _ => {}
    }
    Ok(())
}

pub fn pathset(repo: &Path, head: &str) -> Result<BTreeMap<String, String>> {
    if !api::hex(head,40) { return Err("full Git commit required".into()); }
    let output = std::process::Command::new("/usr/bin/git").current_dir(repo)
        .env_clear().env("PATH","/usr/bin:/bin").args(["ls-tree","-rz","--full-tree",head]).output()?;
    if !output.status.success() { return Err("Git tree inspection failed".into()); }
    let mut entries = BTreeMap::new();
    for line in output.stdout.split(|c| *c == 0).filter(|line| !line.is_empty()) {
        let line = std::str::from_utf8(line)?;
        let (metadata,path) = line.split_once('\t').ok_or("invalid Git tree entry")?;
        if policy::relevant(path) { entries.insert(path.into(),metadata.into()); }
    }
    if entries.is_empty() { return Err("empty executor pathset refused".into()); }
    Ok(entries)
}

pub fn carry(args: &[std::ffi::OsString]) -> Result<()> {
    let mut fields = BTreeMap::new();
    for pair in args.chunks(2) {
        if pair.len() != 2 { return Err("carry requires --repo --verdict --head".into()); }
        let name = pair[0].to_str().ok_or("UTF-8 option required")?;
        if !["--repo","--verdict","--head"].contains(&name) || fields.insert(name,pair[1].to_str().ok_or("UTF-8 input required")?).is_some() {
            return Err("invalid carry option".into());
        }
    }
    let get = |name| fields.get(name).copied().ok_or("missing carry option");
    let repo = Path::new(get("--repo")?);
    let path = Path::new(get("--verdict")?);
    let head = get("--head")?;
    let mut reasons = Vec::new();
    let mut changed = Vec::new();
    let result = (|| -> Result<()> {
        let verdict: Value = serde_json::from_slice(&durable::read(path)?)?;
        if verdict["schema"] != "tl3.fx_executor_qualification.v1" || verdict["status"] != "PASS"
            || verdict["complete"] != true || verdict["executor_pathset"]["version"] != policy::PATHSET_VERSION
        { return Err("complete PASS at the identical pathset version required".into()); }
        verify_references(&verdict)?;
        let qualified = verdict["qualified_head"].as_str().ok_or("qualified head absent")?;
        let previous = pathset(repo,qualified)?;
        let current = pathset(repo,head)?;
        let retained: BTreeMap<String,String> = serde_json::from_value(verdict["executor_pathset"]["entries"].clone())?;
        if previous != retained || verdict["executor_pathset"]["sha256"] != durable::digest(&serde_json::to_vec(&previous)?) {
            return Err("qualified pathset receipt changed".into());
        }
        let paths: std::collections::BTreeSet<_> = previous.keys().chain(current.keys()).collect();
        changed = paths.into_iter().filter(|path| previous.get(*path) != current.get(*path)).cloned().collect();
        if !changed.is_empty() { return Err("executor source paths changed".into()); }
        if git(repo,&["rev-parse","HEAD"])? != head || !git(repo,&["status","--porcelain","--untracked-files=all"])?.is_empty() {
            return Err("carry requires a clean checkout at the requested head".into());
        }
        let generated = d8::generate_checkout(&repo.join("crates/controller"))?;
        let installed = durable::read(&repo.join(policy::PIN))?;
        if generated != installed || verdict["pin"]["sha256"] != durable::digest(&installed) {
            return Err("D8 regeneration or installed pin differs".into());
        }
        let outputs = verdict["pin"]["installed_outputs"].as_array().ok_or("derived inventory absent")?;
        let expected: std::collections::BTreeSet<String> = outputs.iter().map(|output| output["relative_path"].as_str().map(String::from).ok_or("relative output missing")).collect::<std::result::Result<_,_>>()?;
        let index: d8::current::Index = serde_json::from_slice(&durable::read(&repo.join(policy::INDEX))?)?;
        let mut observed: std::collections::BTreeSet<String> = d8::current::installed_paths(&index).iter()
            .map(|p| format!("crates/controller/{}",p.display())).collect();
        observed.extend([policy::PIN.into(),policy::INDEX.into()]);
        if expected != observed { return Err("derived outputs have changed closed inventory".into()); }
        for output in outputs {
            let relative = output["relative_path"].as_str().ok_or("relative output missing")?;
            if !policy::derived(relative) || identity(&repo.join(relative))?["sha256"] != output["sha256"] {
                return Err("derived output differs from qualification".into());
            }
        }
        Ok(())
    })();
    if let Err(error) = result { reasons.push(error.to_string()); }
    println!("{}",json!({"decision":if reasons.is_empty(){"carry"}else{"rerun"},"head":head,"pathset_version":policy::PATHSET_VERSION,"changed_paths":changed,"reasons":reasons}));
    Ok(())
}
