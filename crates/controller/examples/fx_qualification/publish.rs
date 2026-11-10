use super::*;

#[derive(Deserialize,Serialize)]
#[serde(deny_unknown_fields)]
struct Publication {
    schema:String,
    preimages:BTreeMap<String,Option<String>>,
    outputs:BTreeMap<String,String>,
}

pub fn verify_private(inputs:&Inputs)->Result<()> {
    prepare::require_admitted()?;
    let private=inputs.out.join("private");
    let generated=d8::current::generate(&private.join("qualification/current.json"))?;
    if durable::digest(&generated)!=tl3_controller::adapters::flux_checks::activation::qualification_sha256() {
        return Err("rebuilt production pin differs from D8 inputs".into());
    }
    let generator:Value=serde_json::from_slice(&durable::read(&inputs.out.join("generator-prepared/result.json"))?)?;
    evidence::verify_references(&generator)?;
    // Re-run the production activation boundary with the exact prepared options,
    // bundle set and private record. No service registration or deployment.
    let profile_path=inputs.out.join("generator-prepared/attempt/execution.json");
    tl3_controller::adapters::workers::execution::execute(tl3_controller::adapters::workers::execution::Command::Activation {
        profile:profile_path,image:inputs.generator_image.clone(),
    })?;
    durable::atomic(&inputs.out.join("private-verification.json"),&serde_json::to_vec_pretty(&json!({
        "status":"PASS","d8_byte_identical":true,"production_activation_load":true,
        "qualification_sha256":durable::digest(&generated),"deployment":generator["activation"]}))?)?;
    Ok(())
}

pub fn publish(inputs:&Inputs,outputs:&BTreeMap<String,Vec<u8>>,canaries:&[evidence::CanaryEvidence])->Result<Value> {
    let decisions:Vec<_>=canaries.iter().zip(policy::ORDER).map(|(c,kind)|(kind,if c.status=="PASS"{policy::Status::Pass}else{policy::Status::Fail})).collect();
    if policy::verdict(&decisions,true)!=policy::Status::Pass {return Err("publication requires all four accepted canaries".into());}
    for canary in canaries {evidence::verify_references(&serde_json::to_value(canary)?)?;}
    let verified:Value=serde_json::from_slice(&durable::read(&inputs.out.join("private-verification.json"))?)?;
    let pin=outputs.get(policy::PIN).ok_or("generated pin missing")?;
    if verified["status"]!="PASS" || verified["qualification_sha256"]!=durable::digest(pin) {
        return Err("final admitted verification missing or changed".into());
    }
    let paths=policy::publication_order(&outputs.keys().cloned().collect::<Vec<_>>())?;
    let journal=inputs.out.join("publication.json");
    let hashes:BTreeMap<_,_>=outputs.iter().map(|(path,bytes)|(path.clone(),durable::digest(bytes))).collect();
    let publication=if journal.exists() {
        let publication:Publication=serde_json::from_slice(&durable::read(&journal)?)?;
        if publication.schema!="tl3.fx_pin_publication.v1" || publication.outputs!=hashes {return Err("publication inputs changed".into());}
        publication
    } else {
        let mut preimages=BTreeMap::new();
        for path in &paths {
            let target=inputs.repo.join(path);
            if target.ancestors().any(Path::is_symlink) {return Err("publication symlink refused".into());}
            preimages.insert(path.clone(),if target.exists(){Some(durable::digest(&durable::read(&target)?))}else{None});
        }
        let publication=Publication {schema:"tl3.fx_pin_publication.v1".into(),preimages,outputs:hashes};
        write(&journal,&publication)?;
        publication
    };
    for path in &paths {
        let target=inputs.repo.join(path);
        let observed=if target.exists(){Some(durable::digest(&durable::read(&target)?))}else{None};
        if observed.as_ref()!=publication.preimages[path].as_ref() && observed.as_ref()!=Some(&publication.outputs[path]) {
            return Err("publication target differs from both retained preimage and output".into());
        }
    }
    let mut installed=vec![];
    for path in paths {
        let target=inputs.repo.join(&path);fs::create_dir_all(target.parent().ok_or("publication parent")?)?;
        durable::atomic(&target,&outputs[&path])?;
        let mut value=identity(&target)?;value["relative_path"]=path.into();installed.push(value);
    }
    let regenerated=d8::generate_checkout(&inputs.repo.join("crates/controller"))?;
    if regenerated!=*pin || durable::read(&inputs.repo.join(policy::PIN))?!=*pin {return Err("published pin differs from regeneration".into());}
    Ok(json!({"path":inputs.repo.join(policy::PIN),"sha256":durable::digest(pin),"bytes":pin.len(),
        "inputs_index":identity(&inputs.repo.join(policy::INDEX))?,"check":"byte_identical",
        "rebuilt_tree":serde_json::from_slice::<Value>(&durable::read(&inputs.out.join("qualified-production/runtime-build.json"))?)?["tree"],
        "activation_record":verified["deployment"],"installed_outputs":installed}))
}
