use std::{fs, path::Path};
use tl3_config::Config;
use toml::Value;

const EXAMPLE: &str = include_str!("../example.toml");

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
    for (maker, expected) in
        config
            .models
            .maker
            .iter()
            .zip(["alternate-model", "alternate-model", "default-model"])
    {
        let reviewer = config.models.reviewer_for(&maker.family).unwrap();
        assert!(!reviewer.family.eq_ignore_ascii_case(&maker.family));
        assert_eq!(reviewer.name, expected);
    }
}

#[test]
fn omitted_and_empty_reviewer_ladders_default_to_maker_attempts_one_and_three() {
    for source in [
        EXAMPLE.to_string(),
        changed(&["models", "reviewer"], Value::Array(vec![])),
    ] {
        let config = Config::parse(&source).unwrap();
        assert_eq!(config.models.reviewer.len(), 2);
        for (reviewer, maker) in config
            .models
            .reviewer
            .iter()
            .zip([&config.models.maker[0], &config.models.maker[2]])
        {
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
    for (role, maker_index) in [("ingest", 0), ("audit", 2)] {
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
        (
            vec!["limits", "w"],
            Value::Integer(0),
            "limits.w",
            "slice one",
        ),
        (
            vec!["limits", "w"],
            Value::Integer(2),
            "limits.w",
            "slice one",
        ),
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
        (
            "family = \"alternate\"",
            "family = \"DEFAULT\"",
            "models.maker[2].family",
        ),
        (
            "effort = \"xhigh\", budget_tokens = 64000",
            "effort = \"xhigh\", budget_tokens = 16000",
            "models.maker[1]",
        ),
        ("effort = \"xhigh\", ", "", "models.maker[1]"),
        (
            "name = \"default-model\", family = \"default\", effort = \"xhigh\"",
            "name = \"other\", family = \"default\", effort = \"xhigh\"",
            "models.maker[1]",
        ),
    ] {
        let error = Config::parse(&EXAMPLE.replace(from, to))
            .unwrap_err()
            .to_string();
        assert!(error.contains(field), "{error}");
    }
    let mut document: Value = EXAMPLE.parse().unwrap();
    document["models"]["maker"].as_array_mut().unwrap().pop();
    assert!(
        Config::parse(&toml::to_string(&document).unwrap())
            .unwrap_err()
            .to_string()
            .contains("length 3")
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
    for (maker, expected) in
        config
            .models
            .maker
            .iter()
            .zip(["alternate-model", "alternate-model", "default-model"])
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
fn planning_cannot_remove_protection_or_overhead_evidence() {
    let config = Config::parse(EXAMPLE).unwrap();
    assert_eq!(
        config.planning.overhead_tokens.values().sum::<u32>(),
        30_000
    );
    let mut config = config;
    config
        .planning
        .protected_paths
        .retain(|p| p != "**/conftest.py");
    assert!(config.validate().is_err());
    config.planning = tl3_config::Planning::default();
    config.planning.overhead_tokens.clear();
    assert!(config.validate().is_err());
}
