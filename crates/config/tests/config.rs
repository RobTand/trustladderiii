use std::{fs, path::Path};
use tl3_config::Config;
use toml::Value;

const EXAMPLE: &str = include_str!("../example.toml");

#[test]
fn writer_defaults_to_ingest_and_rejects_invalid_explicit_budget() {
    let config = Config::parse(EXAMPLE).unwrap();
    assert_eq!(config.models.writer[0].name, config.models.ingest[0].name);
    let mut document: Value = EXAMPLE.parse().unwrap();
    let writer = document["models"]["ingest"].clone();
    document["models"]
        .as_table_mut()
        .unwrap()
        .insert("writer".into(), writer);
    document["models"]["writer"][0]["budget_tokens"] = Value::Integer(0);
    assert!(Config::parse(&toml::to_string(&document).unwrap()).is_err());
}

#[test]
fn status_load_checks_only_store_credential_metadata() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = tempfile::tempdir().unwrap();
    let path = scratch.path().join("config.toml");
    let secret = scratch.path().join("postgres-password");
    fs::write(&secret, "fixture").unwrap();
    fs::set_permissions(&secret, fs::Permissions::from_mode(0o600)).unwrap();
    let text = EXAMPLE
        .replace("/home/rob/.config/tl3/github-app.json", "/missing/app.json")
        .replace(
            "# password_file = \"/run/tl3/postgres-password\"",
            &format!("password_file = {:?}", secret),
        );
    fs::write(&path, text).unwrap();
    Config::load_status(&path).unwrap();
    assert!(Config::load(&path).is_err());
    fs::set_permissions(&secret, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(Config::load_status(&path).is_err());
    fs::remove_file(&secret).unwrap();
    assert!(Config::load_status(&path).is_err());
}

#[test]
fn store_settings_default_and_validate_without_reading_credentials() {
    let config = Config::parse(EXAMPLE).unwrap();
    assert_eq!(config.store.database, "tl3");
    assert!(config.store.password_file.is_none());
    let mut document: Value = EXAMPLE.parse().unwrap();
    document.as_table_mut().unwrap().remove("store");
    assert_eq!(
        Config::parse(&toml::to_string(&document).unwrap())
            .unwrap()
            .store
            .port,
        5432
    );
    for (key, value) in [
        ("port", Value::Integer(0)),
        ("connect_timeout_secs", Value::Integer(0)),
        ("host", Value::String("postgres://credentials@host".into())),
        ("database", Value::String("".into())),
        ("user", Value::String(" ".into())),
        ("password_file", Value::String("relative/secret".into())),
        ("unexpected", Value::Boolean(true)),
    ] {
        assert!(
            Config::parse(&changed(&["store", key], value)).is_err(),
            "{key}"
        );
    }
    let source = changed(
        &["store", "password_file"],
        Value::String("/does-not-exist/fixture".into()),
    );
    assert!(Config::parse(&source).is_ok());
}

#[test]
fn store_connections_require_loopback_or_unix_sockets() {
    for host in ["localhost", "127.0.0.1", "::1", "/var/run/postgresql"] {
        Config::parse(&changed(&["store", "host"], Value::String(host.into()))).unwrap();
    }
    for host in [
        "database.example.com",
        "192.168.1.120",
        "0.0.0.0",
        "::",
        "2001:db8::1",
        "localhost.example.com",
        "127.0.0.1,remote",
        "postgres://localhost/tl3",
        "relative/socket",
    ] {
        let error = Config::parse(&changed(&["store", "host"], Value::String(host.into())))
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("store.host") && error.contains("no TLS"),
            "{host}: {error}"
        );
        let settings = tl3_config::Store {
            host: host.into(),
            ..tl3_config::Store::default()
        };
        assert!(settings.validate().is_err(), "{host}");
    }
}

fn changed(path: &[&str], value: Value) -> String {
    let mut document: Value = EXAMPLE.parse().unwrap();
    let mut target = &mut document;
    for key in &path[..path.len() - 1] {
        target = target.get_mut(key).unwrap();
    }
    target
        .as_table_mut()
        .unwrap()
        .insert(path.last().unwrap().to_string(), value);
    toml::to_string(&document).unwrap()
}

#[test]
fn example_parses_with_sourced_numbers_and_different_reviewer_family() {
    let config = Config::parse(EXAMPLE).unwrap();
    config.validate().unwrap();
    assert!(config.status.listen.ip().is_loopback());
    assert_eq!(config.limits.fit_tokens, 100_000);
    assert_eq!(config.limits.context_tokens, 150_000);
    assert_eq!(config.limits.credit_reserve, 3);
    assert_eq!(config.limits.free_rechecks, 3);
    assert_eq!(config.limits.w, 1);
    assert_eq!(config.github.rob.id, 24_923_402);
    assert_eq!(config.github.rob.login, "RobTand");
    assert_eq!(config.github.repos[0].name, "tl3-sandbox");
    assert_eq!(config.hindsight.endpoint.as_str(), "http://127.0.0.1:8890/");
    assert_eq!(config.hindsight.bank, "tl3");
    for (maker, expected) in config
        .models
        .maker
        .iter()
        .zip(["alternate-model", "default-model"])
    {
        let reviewer = config.models.reviewer_for(&maker.family).unwrap();
        assert!(!reviewer.family.eq_ignore_ascii_case(&maker.family));
        assert_eq!(reviewer.name, expected);
    }
}

#[test]
fn omitted_and_empty_reviewer_ladders_default_to_the_first_and_last_maker_rungs() {
    for source in [
        EXAMPLE.to_string(),
        changed(&["models", "reviewer"], Value::Array(vec![])),
    ] {
        let config = Config::parse(&source).unwrap();
        assert_eq!(config.models.reviewer.len(), 2);
        for (reviewer, maker) in config.models.reviewer.iter().zip([
            config.models.maker.first().unwrap(),
            config.models.maker.last().unwrap(),
        ]) {
            assert_eq!(reviewer.name, maker.name);
            assert_eq!(reviewer.family, maker.family);
            assert_eq!(reviewer.effort, maker.effort);
            assert_eq!(reviewer.budget_tokens, maker.budget_tokens);
        }
    }
}

#[test]
fn reviewer_ladder_must_cover_every_maker_family() {
    let document: Value = EXAMPLE.parse().unwrap();
    for (role, maker_index) in [("ingest", 0), ("audit", 1)] {
        let mut reviewers = document["models"][role].clone();
        reviewers[0]["family"] = Value::String(
            reviewers[0]["family"]
                .as_str()
                .unwrap()
                .to_ascii_uppercase(),
        );
        let source = changed(&["models", "reviewer"], reviewers);
        let error = Config::parse(&source).unwrap_err().to_string();
        assert!(error.contains("models.reviewer"), "{error}");
        assert!(
            error.contains("add a reviewer from a different family"),
            "{error}"
        );
        assert!(
            error.contains(&format!("models.maker[{maker_index}]")),
            "{error}"
        );
    }
}

#[test]
fn planner_ladder_has_a_next_model_for_a_rerun() {
    let document: Value = EXAMPLE.parse().unwrap();
    let mut planner = document["models"]["planner"].clone();
    planner.as_array_mut().unwrap().truncate(1);
    let error = Config::parse(&changed(&["models", "planner"], planner))
        .unwrap_err()
        .to_string();
    assert!(error.contains("models.planner"), "{error}");
    assert!(error.contains("next model"), "{error}");
}

#[test]
fn invalid_values_name_the_setting_and_the_fix() {
    let cases = [
        (
            vec!["limits", "context_tokens"],
            Value::Integer(0),
            "limits.context_tokens",
            "greater than zero",
        ),
        (
            vec!["limits", "fit_tokens"],
            Value::Integer(0),
            "limits.fit_tokens",
            "greater than zero",
        ),
        (
            vec!["limits", "fit_tokens"],
            Value::Integer(150_000),
            "limits.fit_tokens",
            "less than",
        ),
        (vec!["limits", "w"], Value::Integer(0), "limits.w", "1..=3"),
        (
            vec!["github", "rob", "id"],
            Value::Integer(0),
            "github.rob.id",
            "positive",
        ),
        (
            vec!["github", "rob", "login"],
            Value::String("a/b".into()),
            "github.rob.login",
            "without slashes",
        ),
        (
            vec!["github", "repos"],
            Value::Array(vec![]),
            "github.repos",
            "at least one",
        ),
        (
            vec!["status", "listen"],
            Value::String("0.0.0.0:0".into()),
            "status.listen",
            "fixed port",
        ),
        (
            vec!["status", "listen"],
            Value::String("224.0.0.1:8033".into()),
            "status.listen",
            "unicast",
        ),
        (
            vec!["prismabuild", "container_image"],
            Value::String("tl3:latest".into()),
            "prismabuild.container_image",
            "64 lowercase",
        ),
        (
            vec!["prismabuild", "container_image"],
            Value::String(format!("content:sha256:{}", "a".repeat(64))),
            "prismabuild.container_image",
            "harness.Container",
        ),
        (
            vec!["models", "ingest"],
            Value::Array(vec![]),
            "models.ingest",
            "at least one",
        ),
        (
            vec!["models", "base_url"],
            Value::String("ftp://proxy/v1".into()),
            "models.base_url",
            "HTTP or HTTPS",
        ),
        (
            vec!["models", "base_url"],
            Value::String("https://user:secret@proxy/v1".into()),
            "models.base_url",
            "without credentials",
        ),
        (
            vec!["hindsight", "endpoint"],
            Value::String("http://localhost/?token=secret".into()),
            "hindsight.endpoint",
            "without credentials",
        ),
        (
            vec!["hindsight", "endpoint"],
            Value::String("http://localhost/#key".into()),
            "hindsight.endpoint",
            "without credentials",
        ),
    ];
    for (path, value, field, fix) in cases {
        let error = Config::parse(&changed(&path, value))
            .unwrap_err()
            .to_string();
        assert!(error.contains(field) && error.contains(fix), "{error}");
        assert!(!error.contains("user:secret") && !error.contains("token=secret"));
    }
}

#[test]
fn all_paths_must_be_absolute() {
    for path in [
        vec!["worktrees_root"],
        vec!["github", "app_credentials_file"],
        vec!["models", "key_file"],
        vec!["prismabuild", "tools_dir"],
        vec!["workers", "tiktoken_cache_dir"],
    ] {
        for value in [
            "",
            "relative/path",
            "~/private",
            "$TL3_ROOT/key",
            "/tmp/key\n",
        ] {
            let error = Config::parse(&changed(&path, Value::String(value.into())))
                .unwrap_err()
                .to_string();
            assert!(
                error.contains(&path.join(".")) && error.contains("absolute path"),
                "{error}"
            );
        }
    }
}

#[test]
fn every_upstream_requires_two_positive_allowances() {
    for upstream in [
        "agent_worker",
        "prismabuild",
        "github_mergify",
        "model_calls",
        "hindsight",
    ] {
        for kind in ["silence_secs", "no_progress_secs"] {
            let source = changed(&["allowances", upstream, kind], Value::Integer(0));
            let error = Config::parse(&source).unwrap_err().to_string();
            assert!(
                error.contains(&format!("allowances.{upstream}.{kind}")),
                "{error}"
            );
            assert!(error.contains("look, not a work deadline"), "{error}");
        }
    }
}

#[test]
fn unknown_keys_fail_at_every_table_depth() {
    for path in [
        vec!["unexpected"],
        vec!["limits", "unexpected"],
        vec!["github", "unexpected"],
        vec!["github", "rob", "unexpected"],
        vec!["models", "unexpected"],
        vec!["prismabuild", "unexpected"],
        vec!["hindsight", "unexpected"],
        vec!["status", "unexpected"],
        vec!["allowances", "unexpected"],
        vec!["allowances", "agent_worker", "unexpected"],
    ] {
        let error = Config::parse(&changed(&path, Value::Boolean(true)))
            .unwrap_err()
            .to_string();
        assert!(error.contains("unknown field `unexpected`"), "{error}");
    }
    for source in [
        EXAMPLE.replace(
            "installation_id = 1",
            "installation_id = 1\nunexpected = true",
        ),
        EXAMPLE.replace(
            "name = \"default-model\"",
            "unexpected = true, name = \"default-model\"",
        ),
        EXAMPLE.replace(
            "key_file =",
            "api_key = \"fixture-must-not-appear\"\nkey_file =",
        ),
    ] {
        let error = Config::parse(&source).unwrap_err().to_string();
        assert!(error.contains("unknown field"), "{error}");
        assert!(!error.contains("fixture-must-not-appear"), "{error}");
    }
}

#[test]
fn syntax_missing_fields_wrong_types_and_integer_ranges_fail() {
    for source in [
        "[limits".into(),
        EXAMPLE.replace("fit_tokens = 100000", ""),
        changed(&["limits", "credit_reserve"], Value::Integer(-1)),
        changed(&["limits", "free_rechecks"], Value::Integer(i64::MAX)),
        changed(
            &["limits", "context_tokens"],
            Value::String("150000".into()),
        ),
        changed(
            &["status", "listen"],
            Value::String("localhost:8033".into()),
        ),
        changed(&["status", "listen"], Value::String("0.0.0.0:65536".into())),
        changed(&["models", "base_url"], Value::String("no-url".into())),
        format!("worktrees_root = '/tmp'\n{EXAMPLE}"),
    ] {
        let error = Config::parse(&source).unwrap_err().to_string();
        assert!(error.starts_with("config TOML:"), "{error}");
        assert!(!error.contains(" | "), "source lines leaked: {error}");
    }
}

#[test]
fn maker_ladder_obeys_q22_and_budget_bounds() {
    for (from, to, field) in [
        (
            "budget_tokens = 32000",
            "budget_tokens = 0",
            "budget_tokens",
        ),
        (
            "budget_tokens = 32000",
            "budget_tokens = 150001",
            "budget_tokens",
        ),
        ("name = \"default-model\"", "name = \" \"", ".name"),
        ("family = \"default\"", "family = \"\"", ".family"),
        ("effort = \"high\"", "effort = \"\"", ".effort"),
    ] {
        let error = Config::parse(&EXAMPLE.replace(from, to))
            .unwrap_err()
            .to_string();
        assert!(error.contains(field), "{error}");
    }
    // A one-rung ladder never escalates (#416).
    let mut document: Value = EXAMPLE.parse().unwrap();
    document["models"]["maker"].as_array_mut().unwrap().pop();
    assert!(
        Config::parse(&toml::to_string(&document).unwrap())
            .unwrap_err()
            .to_string()
            .contains("different model line")
    );
}

#[test]
fn repositories_need_valid_names_installations_and_unique_full_names() {
    for (from, to, message) in [
        (
            "installation_id = 1",
            "installation_id = 0",
            "installation_id",
        ),
        ("owner = \"RobTand\"", "owner = \"bad/owner\"", ".owner"),
        ("name = \"tl3-sandbox\"", "name = \"..\"", ".name"),
    ] {
        assert!(
            Config::parse(&EXAMPLE.replace(from, to))
                .unwrap_err()
                .to_string()
                .contains(message)
        );
    }
    let source = format!(
        "{EXAMPLE}\n[[github.repos]]\nowner = 'robtand'\nname = 'TL3-SANDBOX'\ninstallation_id = 2\n"
    );
    assert!(
        Config::parse(&source)
            .unwrap_err()
            .to_string()
            .contains("duplicate served repository")
    );
}

#[test]
fn explicit_reviewer_and_proxy_specific_efforts_are_supported() {
    let mut document: Value = EXAMPLE.parse().unwrap();
    let mut later_reviewer = document["models"]["audit"][0].clone();
    later_reviewer["name"] = Value::String("later-reviewer".into());
    let reviewers = Value::Array(vec![
        document["models"]["ingest"][0].clone(),
        document["models"]["audit"][0].clone(),
        later_reviewer,
    ]);
    document["models"]
        .as_table_mut()
        .unwrap()
        .insert("reviewer".into(), reviewers);
    document["models"]["maker"][1]["effort"] = Value::String("proxy-specific-maximum".into());
    document["limits"]["credit_reserve"] = Value::Integer(0);
    document["limits"]["free_rechecks"] = Value::Integer(0);
    let config = Config::parse(&toml::to_string(&document).unwrap()).unwrap();
    for (maker, expected) in config
        .models
        .maker
        .iter()
        .zip(["alternate-model", "default-model"])
    {
        let reviewer = config
            .models
            .reviewer_for(&maker.family.to_ascii_uppercase())
            .unwrap();
        assert!(!reviewer.family.eq_ignore_ascii_case(&maker.family));
        assert_eq!(reviewer.name, expected);
    }
    assert_eq!(config.models.reviewer.len(), 3);
}

#[test]
fn worker_model_matches_protocol_without_family_or_secret_fields() {
    let config = Config::parse(EXAMPLE).unwrap();
    let model = config
        .models
        .worker_model(&config.models.comment_classifier[0]);
    assert_eq!(
        serde_json::to_value(model).unwrap(),
        serde_json::json!({
            "base_url": "http://192.168.1.120:8317/v1",
            "name": "default-model", "effort": null, "budget_tokens": 8000,
        })
    );
}

#[test]
fn harness_digest_formats_accept_only_immutable_images() {
    for image in [
        format!("sha256:{}", "a".repeat(64)),
        format!("registry:5000/tl3_harness@sha256:{}", "1".repeat(64)),
    ] {
        Config::parse(&changed(
            &["prismabuild", "container_image"],
            Value::String(image),
        ))
        .unwrap();
    }
    for image in [
        format!("sha256:{}", "A".repeat(64)),
        format!("sha256:{}", "g".repeat(64)),
        format!("sha256:{}", "a".repeat(63)),
        format!("@sha256:{}", "a".repeat(64)),
    ] {
        assert!(
            Config::parse(&changed(
                &["prismabuild", "container_image"],
                Value::String(image)
            ))
            .is_err()
        );
    }
}

#[cfg(unix)]
fn private_file(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    fs::write(path, [0xff, 0xfe, 0]).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

#[cfg(unix)]
#[test]
fn load_checks_metadata_without_parsing_secret_contents() {
    let directory = tempfile::tempdir().unwrap();
    let app = directory.path().join("app.json");
    let key = directory.path().join("key");
    let password = directory.path().join("password");
    private_file(&app, 0o600);
    private_file(&key, 0o400);
    private_file(&password, 0o400);
    let mut document: Value = EXAMPLE.parse().unwrap();
    document["github"]["app_credentials_file"] = Value::String(app.display().to_string());
    document["models"]["key_file"] = Value::String(key.display().to_string());
    document["store"].as_table_mut().unwrap().insert(
        "password_file".into(),
        Value::String(password.display().to_string()),
    );
    let path = directory.path().join("config.toml");
    fs::write(&path, toml::to_string(&document).unwrap()).unwrap();
    Config::load(&path).unwrap();
    assert!(
        Config::load(directory.path().join("missing.toml"))
            .unwrap_err()
            .to_string()
            .contains("config: cannot inspect")
    );
}

#[cfg(unix)]
#[test]
fn load_checks_password_file_metadata_and_private_modes() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let directory = tempfile::tempdir().unwrap();
    let app = directory.path().join("app");
    let key = directory.path().join("key");
    let password = directory.path().join("password");
    private_file(&app, 0o600);
    private_file(&key, 0o600);
    private_file(&password, 0o600);
    let mut document: Value = EXAMPLE.parse().unwrap();
    document["github"]["app_credentials_file"] = Value::String(app.display().to_string());
    document["models"]["key_file"] = Value::String(key.display().to_string());
    document["store"].as_table_mut().unwrap().insert(
        "password_file".into(),
        Value::String(password.display().to_string()),
    );
    let path = directory.path().join("config.toml");
    fs::write(&path, toml::to_string(&document).unwrap()).unwrap();
    Config::load(&path).unwrap();
    for mode in [0o644, 0o640, 0o606, 0o000] {
        fs::set_permissions(&password, fs::Permissions::from_mode(mode)).unwrap();
        let error = Config::load(&path).unwrap_err().to_string();
        assert!(
            error.contains("store.password_file") && error.contains("chmod 600"),
            "{error}"
        );
    }
    fs::remove_file(&password).unwrap();
    let error = Config::load(&path).unwrap_err().to_string();
    assert!(
        error.contains("store.password_file: cannot inspect"),
        "{error}"
    );
    fs::create_dir(&password).unwrap();
    let error = Config::load(&path).unwrap_err().to_string();
    assert!(
        error.contains("store.password_file") && error.contains("regular secret file"),
        "{error}"
    );
    fs::remove_dir(&password).unwrap();
    let target = directory.path().join("password-target");
    private_file(&target, 0o400);
    symlink(&target, &password).unwrap();
    Config::load(&path).unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o644)).unwrap();
    let error = Config::load(&path).unwrap_err().to_string();
    assert!(
        error.contains("store.password_file") && error.contains("chmod 600"),
        "{error}"
    );
}

#[cfg(unix)]
#[test]
fn secret_targets_reject_missing_files_directories_and_nonprivate_modes() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let directory = tempfile::tempdir().unwrap();
    let app = directory.path().join("app");
    let key = directory.path().join("key");
    private_file(&app, 0o600);
    private_file(&key, 0o600);
    let mut config = Config::parse(EXAMPLE).unwrap();
    config.github.app_credentials_file = app.clone();
    config.models.key_file = key.clone();
    config.validate_secret_files().unwrap();
    for (field, path) in [
        ("github.app_credentials_file", &app),
        ("models.key_file", &key),
    ] {
        for mode in [0o644, 0o640, 0o606, 0o000] {
            fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
            let error = config.validate_secret_files().unwrap_err().to_string();
            assert!(
                error.contains(field) && error.contains("chmod 600"),
                "{error}"
            );
        }
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }
    let link = directory.path().join("link");
    symlink(&key, &link).unwrap();
    config.models.key_file = link;
    config.validate_secret_files().unwrap();
    fs::set_permissions(&key, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(
        config
            .validate_secret_files()
            .unwrap_err()
            .to_string()
            .contains("chmod 600")
    );
    config.models.key_file = directory.path().into();
    assert!(
        config
            .validate_secret_files()
            .unwrap_err()
            .to_string()
            .contains("regular secret file")
    );
    config.models.key_file = directory.path().join("missing");
    assert!(
        config
            .validate_secret_files()
            .unwrap_err()
            .to_string()
            .contains("models.key_file: cannot inspect")
    );
}

#[test]
fn status_listen_requires_a_specific_unicast_address_and_fixed_port() {
    for address in [
        "127.0.0.1:0",
        "224.0.0.1:8033",
        "[ff02::1]:8033",
        "0.0.0.0:8033",
        "[::]:8033",
    ] {
        let mut config = Config::parse(EXAMPLE).unwrap();
        config.status.listen = address.parse().unwrap();
        assert!(
            config
                .validate()
                .unwrap_err()
                .to_string()
                .contains("status.listen"),
            "{address}"
        );
        assert!(
            Config::parse(&EXAMPLE.replace("127.0.0.1:8033", address)).is_err(),
            "{address}"
        );
    }
    for address in ["127.0.0.1:8033", "[::1]:8033", "192.168.1.2:8033"] {
        let mut config = Config::parse(EXAMPLE).unwrap();
        config.status.listen = address.parse().unwrap();
        config.validate().unwrap();
    }
}

#[test]
fn webhooks_require_a_loopback_address_and_fixed_port() {
    for address in [
        "0.0.0.0:8081",
        "[::]:8081",
        "192.168.1.2:8081",
        "127.0.0.1:0",
    ] {
        assert!(
            Config::parse(&changed(
                &["github", "webhook_listen"],
                Value::String(address.into())
            ))
            .is_err(),
            "{address}"
        );
    }
    for address in ["127.0.0.1:8081", "[::1]:8081"] {
        Config::parse(&changed(
            &["github", "webhook_listen"],
            Value::String(address.into()),
        ))
        .unwrap();
    }
}

#[test]
fn planning_cannot_remove_overhead_evidence() {
    let config = Config::parse(EXAMPLE).unwrap();
    assert_eq!(
        config.planning.overhead_tokens.values().sum::<u32>(),
        30_000
    );
    let mut config = config;
    config.planning = tl3_config::Planning::default();
    config.planning.overhead_tokens.clear();
    assert!(config.validate().is_err());
}

#[test]
fn repository_protected_paths_are_optional_additions() {
    let config = Config::parse(EXAMPLE).unwrap();
    assert!(config.github.repos[0].protected_paths.is_empty());
    let mut document: Value = EXAMPLE.parse().unwrap();
    document["github"]["repos"][0]
        .as_table_mut()
        .unwrap()
        .insert(
            "protected_paths".into(),
            Value::Array(vec![Value::String("CHANGELOG.md".into())]),
        );
    let config = Config::parse(&toml::to_string(&document).unwrap()).unwrap();
    assert_eq!(config.github.repos[0].protected_paths, ["CHANGELOG.md"]);
    assert_eq!(
        config.planning.protected_paths,
        tl3_config::Planning::default().protected_paths
    );
}

#[test]
fn global_and_repository_protected_patterns_share_validation() {
    for pattern in [
        "",
        "/absolute",
        "../escape",
        "nested/../escape",
        "[unclosed",
        "bad\\path",
        "bad\npath",
    ] {
        for repository in [false, true] {
            let mut config = Config::parse(EXAMPLE).unwrap();
            let (paths, field) = if repository {
                (
                    &mut config.github.repos[0].protected_paths,
                    "github.repos[0].protected_paths",
                )
            } else {
                (
                    &mut config.planning.protected_paths,
                    "planning.protected_paths",
                )
            };
            paths.push(pattern.into());
            let error = config
                .validate()
                .expect_err(&format!("accepted {field}: {pattern:?}"));
            assert!(
                matches!(error, tl3_config::Error::Invalid { field: actual, .. } if actual == field)
            );
        }
    }
    let mut config = Config::parse(EXAMPLE).unwrap();
    for pattern in [
        "CHANGELOG.md",
        "fleet/**",
        "**/release-?.toml",
        "docs/[ab]*.md",
    ] {
        config.planning.protected_paths.push(pattern.into());
        config.github.repos[0].protected_paths.push(pattern.into());
    }
    config.validate().unwrap();
}

#[test]
fn planning_overhead_must_leave_room_under_the_fit_limit() {
    for (overhead, valid) in [
        (99_999, true),
        (100_000, false),
        (100_001, false),
        (u32::MAX, false),
    ] {
        let mut config = Config::parse(EXAMPLE).unwrap();
        config.planning.overhead_tokens = [("prompt".into(), overhead)].into();
        let result = config.validate();
        assert_eq!(result.is_ok(), valid, "{overhead}");
        if !valid {
            assert!(
                result
                    .unwrap_err()
                    .to_string()
                    .contains("planning.overhead_tokens")
            );
        }
    }
    let mut config = Config::parse(EXAMPLE).unwrap();
    config.planning.overhead_tokens = [("prompt".into(), u32::MAX), ("making".into(), 1)].into();
    assert!(config.validate().is_err(), "sum must not overflow");
}

#[test]
fn gate_width_accepts_approved_limits() {
    for w in [1, 2, 3] {
        let mut document: Value = EXAMPLE.parse().unwrap();
        document["limits"]["w"] = Value::Integer(w);
        let config = Config::parse(&toml::to_string(&document).unwrap())
            .expect("Q29 amended permits gate widths one through three");
        assert_eq!(i64::from(config.limits.w), w);
    }
}

#[test]
fn gate_width_rejects_values_outside_approval() {
    for w in [256, 4, 8, 255, 0, i64::from(u32::MAX)] {
        let mut document: Value = EXAMPLE.parse().unwrap();
        document["limits"]["w"] = Value::Integer(w);
        let error = Config::parse(&toml::to_string(&document).unwrap()).unwrap_err();
        assert!(error.to_string().contains("limits.w"), "{w}: {error}");
        assert!(error.to_string().contains("1..=3"), "{w}: {error}");
    }
}

#[test]
fn repo_conventions_default_and_accept_explicit_values() {
    let config = Config::parse(EXAMPLE).unwrap();
    assert_eq!(
        config.github.repos[0].branch_template,
        "tl3/issue-{issue}-node-{node}"
    );
    assert_eq!(config.github.repos[0].base_branch, "main");
    let source = EXAMPLE.replace("installation_id = 1", "installation_id = 1\nbranch_template = 'prismabuild-{issue}-tl3-{node}'\nbase_branch = 'master'");
    let config = Config::parse(&source).unwrap();
    assert_eq!(
        config.github.repos[0].branch_template,
        "prismabuild-{issue}-tl3-{node}"
    );
    assert_eq!(config.github.repos[0].base_branch, "master");
}

#[test]
fn repo_conventions_reject_ambiguous_templates_and_invalid_refs() {
    for template in [
        "topic-{issue}",
        "topic-{node}",
        "topic-{issue}{node}",
        "topic-{issue}0{node}",
        "topic-{issue}-{node}-{issue}",
        "topic-{issue}-{node}-{unknown}",
        "bad..{issue}-{node}",
        "{issue}-{node}.lock",
    ] {
        let source = EXAMPLE.replace(
            "installation_id = 1",
            &format!("installation_id = 1\nbranch_template = '{template}'"),
        );
        let error = Config::parse(&source).expect_err(template).to_string();
        assert!(error.contains("branch_template"), "{error}");
    }
    for base in ["", "-master", "bad..base", "refs/heads/", "HEAD"] {
        let source = EXAMPLE.replace(
            "installation_id = 1",
            &format!("installation_id = 1\nbase_branch = '{base}'"),
        );
        assert!(
            Config::parse(&source)
                .unwrap_err()
                .to_string()
                .contains("base_branch")
        );
    }
}

#[test]
fn repository_images_accept_immutable_overrides_and_reject_tags() {
    for image in [
        format!("sha256:{}", "b".repeat(64)),
        format!("registry/repo@sha256:{}", "c".repeat(64)),
    ] {
        let text = changed(&["github", "repos"], {
            let mut document: Value = EXAMPLE.parse().unwrap();
            document["github"]["repos"][0]
                .as_table_mut()
                .unwrap()
                .insert("container_image".into(), Value::String(image));
            document["github"]["repos"].clone()
        });
        assert!(
            Config::parse(&text).is_ok(),
            "immutable per-repo image must parse"
        );
    }
    for image in ["", "tl3-harness:t5", "sha256:abc", "content:sha256:abcd"] {
        let mut document: Value = EXAMPLE.parse().unwrap();
        document["github"]["repos"][0]
            .as_table_mut()
            .unwrap()
            .insert("container_image".into(), Value::String(image.into()));
        assert!(
            Config::parse(&toml::to_string(&document).unwrap()).is_err(),
            "reject mutable or invalid image {image}"
        );
    }
}

#[test]
fn repository_exec_scratch_is_an_optional_boolean() {
    assert!(Config::parse(EXAMPLE).is_ok());
    for enabled in [false, true] {
        let mut document: Value = EXAMPLE.parse().unwrap();
        document["github"]["repos"][0]
            .as_table_mut()
            .unwrap()
            .insert("exec_scratch".into(), Value::Boolean(enabled));
        assert!(
            Config::parse(&toml::to_string(&document).unwrap()).is_ok(),
            "repository exec_scratch={enabled} must parse"
        );
    }
    let mut document: Value = EXAMPLE.parse().unwrap();
    document["github"]["repos"][0]
        .as_table_mut()
        .unwrap()
        .insert("exec_scratch".into(), Value::String("true".into()));
    assert!(Config::parse(&toml::to_string(&document).unwrap()).is_err());
}

#[test]
fn repository_selected_tests_is_an_optional_bounded_limit() {
    let with = |value: Value| {
        let mut document: Value = EXAMPLE.parse().unwrap();
        document["github"]["repos"][0]
            .as_table_mut()
            .unwrap()
            .insert("selected_tests".into(), value);
        Config::parse(&toml::to_string(&document).unwrap())
    };
    assert_eq!(
        Config::parse(EXAMPLE).unwrap().github.repos[0].selected_tests,
        None
    );
    for limit in [1, 200, 1000] {
        assert_eq!(
            with(Value::Integer(limit)).unwrap().github.repos[0].selected_tests,
            Some(limit as u32)
        );
    }
    for invalid in [
        Value::Integer(0),
        Value::Integer(1001),
        Value::Boolean(true),
    ] {
        assert!(
            with(invalid.clone()).is_err(),
            "reject selected_tests={invalid}"
        );
    }
}

#[test]
fn console_is_optional_and_needs_a_fixed_unicast_port_and_absolute_assets() {
    let config = Config::parse(EXAMPLE).unwrap();
    let console = config.console.as_ref().unwrap();
    assert_eq!(console.listen, "127.0.0.1:8034".parse().unwrap());
    assert_eq!(console.assets, Path::new("/srv/tl3/console"));
    let start = EXAMPLE.find("[console]").unwrap();
    let end = start + EXAMPLE[start..].find("\n\n").unwrap();
    assert!(
        Config::parse(&format!("{}{}", &EXAMPLE[..start], &EXAMPLE[end..]))
            .unwrap()
            .console
            .is_none()
    );
    for (field, from, to) in [
        ("console.listen", "127.0.0.1:8034", "127.0.0.1:0"),
        ("console.listen", "127.0.0.1:8034", "0.0.0.0:8034"),
        ("console.listen", "127.0.0.1:8034", "224.0.0.1:8034"),
        ("console.assets", "/srv/tl3/console", "relative/console"),
    ] {
        let error = Config::parse(&EXAMPLE.replace(from, to))
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains(field), "{to}: {error}");
    }
}

// `deploy.py model-defaults` writes these ladders into the live config, so a
// default that breaks the Q22 rules must fail here, not on dl380g10.
#[test]
fn deployment_model_defaults_pass_validation() {
    let defaults: Value = include_str!("../../../deploy/dl380g10/models.toml")
        .parse()
        .unwrap();
    let defaults = defaults["models"].as_table().unwrap();
    let mut document: Value = EXAMPLE.parse().unwrap();
    let models = document["models"].as_table_mut().unwrap();
    for role in [
        "ingest",
        "audit",
        "planner",
        "maker",
        "reviewer",
        "rebaser",
        "comment_classifier",
        "writer",
        "postmortem",
    ] {
        let ladder = defaults
            .get(role)
            .unwrap_or_else(|| panic!("models.toml must set {role}"));
        models.insert(role.into(), ladder.clone());
    }
    assert_eq!(defaults.len(), 9, "models.toml sets only the role ladders");
    let config = Config::parse(&toml::to_string(&document).unwrap()).unwrap();
    for maker in &config.models.maker {
        let reviewer = config.models.reviewer_for(&maker.family).unwrap();
        assert_ne!(reviewer.name, maker.name, "Q22: a different model reviews");
    }
    // #416, approved by Rob 2026-11-02: Sonnet 5.5 high, then Opus 5.5 high.
    let ladder: Vec<_> = config
        .models
        .maker
        .iter()
        .map(|model| (model.name.as_str(), model.effort.as_deref()))
        .collect();
    assert_eq!(
        ladder,
        [
            ("claude-sonnet-5-5", Some("high")),
            ("claude-opus-5-5", Some("high"))
        ]
    );
    // #403: Opus writes postmortems and Sonnet grades; never Fable.
    let postmortem: Vec<_> = config
        .models
        .postmortem
        .iter()
        .map(|model| model.name.as_str())
        .collect();
    assert_eq!(postmortem, ["claude-opus-5-5", "claude-sonnet-5-5"]);
}

#[test]
fn postmortem_models_default_to_the_reviewer_and_the_writer() {
    let config = Config::parse(EXAMPLE).unwrap();
    let names: Vec<_> = config
        .models
        .postmortem
        .iter()
        .map(|model| model.name.clone())
        .collect();
    assert_eq!(
        names,
        [
            config.models.reviewer[0].name.clone(),
            config.models.writer[0].name.clone()
        ]
    );
    let three = EXAMPLE.replace(
        "[models]\n",
        "[models]\npostmortem = [{ name = \"a\", family = \"a\", budget_tokens = 1000 }, { name = \"b\", family = \"b\", budget_tokens = 1000 }, { name = \"c\", family = \"c\", budget_tokens = 1000 }]\n",
    );
    assert_ne!(three, EXAMPLE);
    let error = Config::parse(&three).err().unwrap().to_string();
    assert!(error.contains("models.postmortem"), "{error}");
}

fn maker_ladder(rungs: &[(&str, &str, &str)]) -> String {
    changed(
        &["models", "maker"],
        Value::Array(
            rungs
                .iter()
                .map(|(name, family, effort)| {
                    let mut model = toml::map::Map::new();
                    model.insert("name".into(), Value::String((*name).into()));
                    model.insert("family".into(), Value::String((*family).into()));
                    model.insert("effort".into(), Value::String((*effort).into()));
                    model.insert("budget_tokens".into(), Value::Integer(32000));
                    Value::Table(model)
                })
                .collect(),
        ),
    )
}

#[test]
fn maker_ladder_escalates_to_a_different_model_line_by_attempt_two() {
    // #416 (Q22, Rob 2026-11-02): attempt 2 changes the model, not the effort.
    let config = Config::parse(&maker_ladder(&[
        ("default-model", "default", "high"),
        ("alternate-model", "alternate", "high"),
    ]))
    .unwrap();
    assert_eq!(config.models.maker.len(), 2);
    for rungs in [
        vec![
            ("default-model", "default", "high"),
            ("default-model", "default", "max"),
        ],
        vec![("default-model", "default", "high")],
        vec![
            ("default-model", "default", "high"),
            ("other-model", "DEFAULT", "high"),
            ("alternate-model", "alternate", "high"),
        ],
    ] {
        let error = Config::parse(&maker_ladder(&rungs))
            .unwrap_err()
            .to_string();
        assert!(error.contains("models.maker"), "{error}");
        assert!(error.contains("different model line"), "{error}");
    }
}

#[test]
fn priority_credit_reserve_is_off_by_default() {
    let config = Config::parse(EXAMPLE).unwrap();
    assert!(config.limits.priority_credit_reserve.is_empty());
    for priority in [None, Some(0), Some(1), Some(2), Some(3)] {
        assert_eq!(config.limits.credit_reserve_for(priority), None);
    }
}

#[test]
fn priority_credit_reserve_applies_to_configured_priorities() {
    let mut reserve = toml::map::Map::new();
    reserve.insert("P0".into(), Value::Integer(5));
    reserve.insert("P1".into(), Value::Integer(5));
    let config = Config::parse(&changed(
        &["limits", "priority_credit_reserve"],
        Value::Table(reserve),
    ))
    .unwrap();
    assert_eq!(config.limits.credit_reserve_for(Some(0)), Some(5));
    assert_eq!(config.limits.credit_reserve_for(Some(1)), Some(5));
    assert_eq!(config.limits.credit_reserve_for(Some(2)), None);
    assert_eq!(config.limits.credit_reserve_for(None), None);
    for (key, value) in [("P4", 5), ("high", 5), ("P0", 2), ("P1", 101)] {
        let mut reserve = toml::map::Map::new();
        reserve.insert(key.into(), Value::Integer(value));
        let error = Config::parse(&changed(
            &["limits", "priority_credit_reserve"],
            Value::Table(reserve),
        ))
        .unwrap_err()
        .to_string();
        assert!(
            error.contains("limits.priority_credit_reserve"),
            "{key} {value}: {error}"
        );
    }
}

// Rob, 2026-11-02: "Limit to Opus and Sonnet for now, please. In addition to
// Sol 6.1". Sol is out of quota, so the defaults use Opus and Sonnet only.
// The example config only has placeholder names, so it is not checked.
#[test]
fn deployment_model_defaults_use_only_active_models() {
    let defaults: Value = include_str!("../../../deploy/dl380g10/models.toml")
        .parse()
        .unwrap();
    for name in tl3_config::ACTIVE_MODELS {
        assert!(tl3_config::ALLOWED_MODELS.contains(name), "{name}");
    }
    for (role, ladder) in defaults["models"].as_table().unwrap() {
        for model in ladder.as_array().unwrap() {
            let name = model["name"].as_str().unwrap();
            assert!(
                tl3_config::ACTIVE_MODELS.contains(&name),
                "models.toml {role} uses {name}; active: {:?}",
                tl3_config::ACTIVE_MODELS
            );
        }
    }
}

#[test]
fn console_connects_as_its_own_reader_when_configured() {
    let config = Config::parse(EXAMPLE).unwrap();
    let console = config.console.clone().unwrap();
    assert_eq!(console.store(&config.store).user, config.store.user);
    let with_reader = EXAMPLE.replace(
        "assets = \"/srv/tl3/console\"",
        "assets = \"/srv/tl3/console\"\nuser = \"tl3_console\"\npassword_file = \"/run/tl3/console-password\"",
    );
    let config = Config::parse(&with_reader).unwrap();
    let store = config.console.as_ref().unwrap().store(&config.store);
    assert_eq!(store.user, "tl3_console");
    assert_eq!(
        store.password_file.as_deref(),
        Some(Path::new("/run/tl3/console-password"))
    );
    assert_eq!(store.database, config.store.database);
    for (field, fields) in [
        ("console.password_file", "user = \"tl3_console\""),
        (
            "console.user",
            "password_file = \"/run/tl3/console-password\"",
        ),
        (
            "console.user",
            "user = \"tl3 console\"\npassword_file = \"/run/tl3/console-password\"",
        ),
        (
            "console.password_file",
            "user = \"tl3_console\"\npassword_file = \"console-password\"",
        ),
    ] {
        let error = Config::parse(&EXAMPLE.replace(
            "assets = \"/srv/tl3/console\"",
            &format!("assets = \"/srv/tl3/console\"\n{fields}"),
        ))
        .err()
        .map(|error| error.to_string())
        .unwrap_or_default();
        assert!(error.contains(field), "{fields}: {error}");
    }
}

fn without_planning_paths(keep: impl Fn(&str) -> bool) -> String {
    let mut document: Value = EXAMPLE.parse().unwrap();
    let paths = document["planning"]["protected_paths"]
        .as_array_mut()
        .unwrap();
    paths.retain(|path| keep(path.as_str().unwrap()));
    toml::to_string(&document).unwrap()
}

#[test]
fn config_missing_a_newer_default_loads_and_the_default_stays_protected() {
    // #388: a live file written before .pytest.ini became a default.
    let config = Config::parse(&without_planning_paths(|path| path != ".pytest.ini")).unwrap();
    assert!(
        !config
            .planning
            .protected_paths
            .contains(&".pytest.ini".into())
    );
    assert_eq!(config.planning.defaults_not_listed(), [".pytest.ini"]);
    assert!(
        config
            .planning
            .effective_protected_paths()
            .contains(&".pytest.ini".into())
    );
    assert!(
        tl3_config::protected_paths(&config.planning.protected_paths, &[])
            .contains(&".pytest.ini".into())
    );
}

#[test]
fn nothing_removes_a_default_protected_path() {
    let config = Config::parse(&without_planning_paths(|_| false)).unwrap();
    assert!(config.planning.protected_paths.is_empty());
    let effective = config.planning.effective_protected_paths();
    for default in tl3_config::Planning::default().protected_paths {
        assert!(effective.contains(&default), "{default}");
        assert!(
            tl3_config::protected_paths(&[], &[]).contains(&default),
            "{default}"
        );
    }
}

#[test]
fn configured_extra_paths_are_protected() {
    let mut config = Config::parse(EXAMPLE).unwrap();
    config.planning.protected_paths.push("fleet/**".into());
    config.validate().unwrap();
    assert!(
        config
            .planning
            .effective_protected_paths()
            .contains(&"fleet/**".into())
    );
    let paths =
        tl3_config::protected_paths(&config.planning.protected_paths, &["CHANGELOG.md".into()]);
    for expected in ["fleet/**", "CHANGELOG.md", ".pytest.ini", ".github/**"] {
        assert!(paths.contains(&expected.into()), "{expected}");
    }
    // Sorted and deduplicated, so every consumer sees one list.
    let mut sorted = paths.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(paths, sorted);
}

#[test]
fn fail_fast_defaults_off_and_takes_a_boolean() {
    assert!(!Config::parse(EXAMPLE).unwrap().limits.fail_fast);
    let omitted = EXAMPLE.replace("fail_fast = false\n", "");
    assert_ne!(omitted, EXAMPLE);
    assert!(!Config::parse(&omitted).unwrap().limits.fail_fast);
    let on = changed(&["limits", "fail_fast"], Value::Boolean(true));
    assert!(Config::parse(&on).unwrap().limits.fail_fast);
    for wrong in [Value::Integer(1), Value::String("true".into())] {
        let error = Config::parse(&changed(&["limits", "fail_fast"], wrong))
            .unwrap_err()
            .to_string();
        assert!(error.starts_with("config TOML:"), "{error}");
        assert!(error.contains("fail_fast"), "{error}");
    }
}

#[test]
fn pr_body_lines_need_a_label_values_and_an_allowed_default() {
    // #447: PrismaBuild's PR-body policy.
    let rule = |label: &str, values: &[&str], default: &str| {
        let mut table = toml::map::Map::new();
        table.insert("label".into(), Value::String(label.into()));
        table.insert(
            "values".into(),
            Value::Array(values.iter().map(|v| Value::String((*v).into())).collect()),
        );
        table.insert("default".into(), Value::String(default.into()));
        Value::Table(table)
    };
    let with = |rule: Value| {
        let mut document: Value = EXAMPLE.parse().unwrap();
        document["github"]["repos"][0]
            .as_table_mut()
            .unwrap()
            .insert("pr_body_lines".into(), Value::Array(vec![rule]));
        Config::parse(&toml::to_string(&document).unwrap())
    };
    let config = with(rule("Change type", &["chore", "fix", "test"], "chore")).unwrap();
    assert_eq!(config.github.repos[0].pr_body_lines[0].label, "Change type");
    assert!(
        Config::parse(EXAMPLE).unwrap().github.repos[0]
            .pr_body_lines
            .is_empty()
    );
    for bad in [
        rule("", &["chore"], "chore"),
        rule("Change type:", &["chore"], "chore"),
        rule("Change type", &[], "chore"),
        rule("Change type", &["chore", ""], "chore"),
        rule("Change type", &["fix"], "chore"),
    ] {
        let error = with(bad).unwrap_err().to_string();
        assert!(error.contains("pr_body_lines"), "{error}");
    }
}

#[test]
fn heal_rerun_workflows_are_workflow_file_names() {
    // #447 review: an explicit allowlist, empty by default.
    let with = |files: &[&str]| {
        let mut document: Value = EXAMPLE.parse().unwrap();
        document["github"]["repos"][0]
            .as_table_mut()
            .unwrap()
            .insert(
                "heal_rerun_workflows".into(),
                Value::Array(files.iter().map(|f| Value::String((*f).into())).collect()),
            );
        Config::parse(&toml::to_string(&document).unwrap())
    };
    assert!(
        Config::parse(EXAMPLE).unwrap().github.repos[0]
            .heal_rerun_workflows
            .is_empty()
    );
    assert_eq!(
        with(&["gate.yml"]).unwrap().github.repos[0].heal_rerun_workflows,
        ["gate.yml"]
    );
    for bad in [
        &[""][..],
        &[".github/workflows/gate.yml"],
        &["gate yml"],
        &["gate.yml", "gate.yml"],
    ] {
        let error = with(bad).unwrap_err().to_string();
        assert!(error.contains("heal_rerun_workflows"), "{bad:?}: {error}");
    }
}

#[test]
fn repository_generator_contract_is_optional_and_validated() {
    let config = Config::parse(EXAMPLE).unwrap();
    assert!(config.github.repos[0].generated_files.is_empty());
    assert!(config.github.repos[0].generator.is_none());
    let source = EXAMPLE.replace(
        "[[github.repos]]",
        "[[github.repos]]\ngenerated_files = [\"**/BUILD.bazel\"]\ngenerator = \"python3 tools/bazel/build_files.py\"",
    );
    let config = Config::parse(&source).unwrap();
    assert_eq!(config.github.repos[0].generated_files, ["**/BUILD.bazel"]);
    assert_eq!(
        config.github.repos[0].generator.as_deref(),
        Some("python3 tools/bazel/build_files.py")
    );
    for (before, after) in [
        ("**/BUILD.bazel", "../BUILD.bazel"),
        ("python3 tools/bazel/build_files.py", " "),
        ("python3 tools/bazel/build_files.py", "python3\\nbad"),
    ] {
        assert!(Config::parse(&source.replace(before, after)).is_err());
    }
}

#[test]
fn evidence_tools_are_explicit_configuration() {
    let mut document: toml::Value = EXAMPLE.parse().unwrap();
    document["planning"]["tool_capabilities"] = toml::Value::Array(vec![]);
    let source = toml::to_string(&document).unwrap();
    assert!(
        Config::parse(&source).is_ok(),
        "operators must be able to disable evidence tools"
    );
}

#[test]
fn evidence_tools_accept_always_tests_configuration() {
    let mut document: Value = EXAMPLE.parse().unwrap();
    document["planning"]["tool_capabilities"][0]["runner_features"] =
        Value::Array(vec![Value::String("always_tests".into())]);
    let config = Config::parse(&toml::to_string(&document).unwrap())
        .expect("the installed runner supports always_tests");
    assert_eq!(
        config.planning.tool_capabilities[0].runner_features,
        ["always_tests"]
    );
}

#[test]
fn repository_always_tests_defaults_empty_and_accepts_files_and_node_ids() {
    assert!(
        Config::parse(EXAMPLE).unwrap().github.repos[0]
            .always_tests
            .is_empty()
    );
    let mut document: Value = EXAMPLE.parse().unwrap();
    let tests = vec![
        "tests/test_rules.py",
        "tests/test_rules.py::test_rule[../case]",
    ];
    document["github"]["repos"][0]
        .as_table_mut()
        .unwrap()
        .insert(
            "always_tests".into(),
            Value::Array(tests.iter().map(|s| Value::String((*s).into())).collect()),
        );
    assert_eq!(
        Config::parse(&toml::to_string(&document).unwrap())
            .unwrap()
            .github
            .repos[0]
            .always_tests,
        tests
    );
}

#[test]
fn repository_always_tests_rejects_paths_outside_the_repo_and_globs() {
    for invalid in [
        "/tmp/test_rule.py",
        "../test_rule.py",
        "tests/../test_rule.py",
        "..",
        "tests/*.py",
        "tests/**/test_rule.py",
        "tests/test_?.py",
        "tests/[ab].py",
        "C:/test_rule.py",
        "tests\\test_rule.py",
        "-test_rule.py",
        "",
        "tests/test_rule.py::",
        "tests/test_rule.py\0",
    ] {
        let mut document: Value = EXAMPLE.parse().unwrap();
        document["github"]["repos"][0]
            .as_table_mut()
            .unwrap()
            .insert(
                "always_tests".into(),
                Value::Array(vec![Value::String(invalid.into())]),
            );
        let error = Config::parse(&toml::to_string(&document).unwrap()).expect_err(invalid);
        assert!(error.to_string().contains("always_tests"), "{error}");
    }
}

#[test]
fn repository_always_tests_accepts_twenty_entries_and_names_the_limit_error() {
    let with = |count: usize| {
        let mut document: Value = EXAMPLE.parse().unwrap();
        document["github"]["repos"][0]
            .as_table_mut()
            .unwrap()
            .insert(
                "always_tests".into(),
                Value::Array(
                    (0..count)
                        .map(|n| Value::String(format!("tests/test_rule_{n}.py")))
                        .collect(),
                ),
            );
        Config::parse(&toml::to_string(&document).unwrap())
    };
    assert!(with(20).is_ok());
    let error = with(21)
        .expect_err("21 repo-wide rules must be refused")
        .to_string();
    assert!(
        error.contains("always_tests") && error.contains("at most 20"),
        "{error}"
    );
}
