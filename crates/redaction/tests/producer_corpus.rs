//! Actual producer inputs and assertions transferred from the deleted Python policy tests.
use serde_json::{Value, json};
use tl3_redaction::structured::{PAYLOAD_BYTES, Redactor};
fn data() -> &'static Value {
    static DATA: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    DATA.get_or_init(|| {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let output = std::process::Command::new("python3")
            .args(["-I"])
            .arg(root.join("../../workers/tests/tracing_policy_cases.py"))
            .arg(root.join("tests/fixtures/generator-inputs.json"))
            .output().unwrap();
        assert!(output.status.success(), "fixture generator failed: {}", String::from_utf8_lossy(&output.stderr));
        serde_json::from_slice(&output.stdout).unwrap()
    })
}
#[test]
fn committed_corpus_has_no_secret_values_even_after_json_decoding() {
    fn inspect(value: &Value) {
        match value {
            Value::String(text) => {
                assert_eq!(tl3_redaction::text(text).unwrap(), *text, "committed corpus contains a secret pattern");
                if let Ok(decoded) = serde_json::from_str::<Value>(text) { inspect(&decoded); }
            }
            Value::Array(items) => items.iter().for_each(inspect),
            Value::Object(fields) => { for (key, value) in fields { inspect(&json!(key)); inspect(value); } }
            _ => (),
        }
    }
    let wire = include_str!("fixtures/generator-inputs.json");
    assert_eq!(tl3_redaction::text(wire).unwrap(), wire);
    inspect(&serde_json::from_str::<Value>(wire).unwrap());
}

fn no_fragments(output: &str, secrets: &[Value], label: &str) {
    for secret in secrets {
        let secret = secret.as_str().unwrap();
        let chars: Vec<_> = secret.chars().collect();
        let width = chars.len().min(8);
        for fragment in chars.windows(width) {
            let fragment: String = fragment.iter().collect();
            assert!(
                !output.contains(&fragment),
                "producer secret fragment leaked in {label}"
            );
        }
    }
}
fn payload(value: Value) -> String {
    Redactor::default()
        .payload(&value.to_string(), PAYLOAD_BYTES)
        .unwrap()
}
#[test]
fn every_planted_form_is_safe_through_each_production_carrier() {
    let data = data();
    let forms = data["forms"].as_array().unwrap();
    assert!(forms.len() > 400);
    let mut executed = 0;
    for case in forms {
        let label = case[0].as_str().unwrap();
        let text = case[1].as_str().unwrap();
        let secrets = case[2].as_array().unwrap();
        no_fragments(
            &Redactor::default().text(text, 8192).unwrap(),
            secrets,
            label,
        );
        executed += 1;
        let encoded = json!({"text":text}).to_string();
        for input in [
            json!(text),
            json!(format!("lead line\n{text}\ntrailer line that follows")),
            json!(format!("I looked at {text} and then moved on.")),
            json!({"note":text}),
            json!(encoded.clone()),
            json!(json!(encoded.clone()).to_string()),
            json!(json!(json!(encoded).to_string()).to_string()),
            json!({"tool_calls":[{"name":"bash","args":{"command":text}}],"messages":[{"role":"user","content":text}]}),
            json!([{"role":"tool","content":json!({"stdout":text}).to_string()}]),
            json!({"tool_calls":[{"function":{"name":"bash","arguments":json!({"command":text}).to_string()}}]}),
        ] {
            no_fragments(&payload(input), secrets, label);
            executed += 1;
        }
    }
    println!(
        "producer forms={} carrier assertions={executed}; documented unsupported forms excluded by fixture inventory",
        forms.len()
    );
}
#[test]
fn ordinary_text_usage_and_source_files_keep_their_content() {
    let data = data();
    for text in data["ordinary"].as_array().unwrap() {
        let source = text.as_str().unwrap();
        assert_eq!(Redactor::default().text(source, 8192).unwrap(), source);
        assert_eq!(
            serde_json::from_str::<Value>(&payload(text.clone())).unwrap(),
            *text
        );
    }
    let usage = json!({"input_tokens":5,"output_tokens":6,"total_tokens":11,"cache_read_input_tokens":3,
        "cache_creation_input_tokens":2,"max_tokens":4096,"budget_tokens":32000,"token_count":12,
        "tokenizer":"gpt2","key":"name","primary_key":"id",
        "input_token_details":{"cache_read":8,"cache_creation":2},
        "usage_metadata":{"input_tokens":9,"output_token_details":{"reasoning":4}}});
    assert_eq!(
        serde_json::from_str::<Value>(&payload(usage.clone())).unwrap(),
        usage
    );
    for path in [
        "docs/secret-scanning.md",
        "tests/test_secret_redaction.py",
        "src/credentials.rs",
        "src/password.py",
    ] {
        let input = json!({"matches":[{"path":path,"line":1,"text":"ordinary-line"}]});
        assert!(payload(input).contains("ordinary-line"));
    }
}
#[test]
fn sensitive_files_fields_and_quoted_json_use_structural_policy() {
    let data = data();
    for path in data["sensitive_paths"].as_array().unwrap() {
        let safe = payload(json!({"path":path,"content":"visible-env-line"}));
        assert!(!safe.contains("visible-env-line"));
        assert!(safe.contains("sensitive file"));
    }
    for key in [
        "OPENAI_API_KEY",
        "x-api-key",
        "github_token",
        "webhook_secret",
        "secret",
        "token",
        "passwd_hash",
        "private_key_id",
        "credential_id",
        "Cookie",
        "Set-Cookie",
        "AUTHORIZATION",
    ] {
        let safe: Value =
            serde_json::from_str(&payload(json!({key:"opaque-sensitive-value-296"}))).unwrap();
        assert_eq!(safe[key], "[REDACTED]");
    }
    for input in [
        json!({"env":[{"name":"DB_PASSWORD","value":"k8s-json-env-296"}]}),
        json!({"secret":123456789012_u64}),
        json!({"auth":"Ym9iOmh1bnRlcjI5Ng=="}),
        json!({"k":json!({"password":"ab\"cd-nested-296"}).to_string(),"note":"kept"}),
        json!({"token":["list-secret-296","other"],"note":"kept"}),
        json!(json!({"headers":{"private_key":12345296},"note":"kept"}).to_string()),
    ] {
        let safe = payload(input);
        for secret in [
            "k8s-json-env-296",
            "123456789012",
            "Ym9iOmh1bnRlcjI5Ng==",
            "cd-nested-296",
            "list-secret-296",
            "12345296",
        ] {
            assert!(!safe.contains(secret));
        }
    }
}
#[test]
fn oversized_producer_payloads_keep_recent_turns_and_both_argument_ends() {
    let messages: Vec<_> = (0..8).map(|index|json!({"role":"user","content":format!("turn-{index}\n{}newest-{index}","old\n".repeat(10000))})).collect();
    let safe = payload(json!({"messages":messages}));
    assert!(serde_json::from_str::<Value>(&safe).unwrap()["messages"].is_array());
    assert!(safe.contains("newest-7"));
    assert!(safe.contains("truncated"));
    let safe = payload(
        json!({"path":"file.txt","argument":format!("{}argument-tail","old\n".repeat(50000)),"result":format!("{}result-tail","old\n".repeat(50000))}),
    );
    assert!(safe.contains("argument-tail") && safe.contains("result-tail"));
    assert!(safe.len() <= PAYLOAD_BYTES);
    for (head, tail, padding) in [
        ("HEAD_MARK ", " FINAL_ONE_LINE_MARK", 20000),
        ("START_ARG ", " ENDARG", 30000),
    ] {
        let safe: Value = serde_json::from_str(&payload(json!(format!(
            "{head}{}{tail}",
            "z".repeat(padding)
        ))))
        .unwrap();
        let safe = safe.as_str().unwrap();
        assert!(safe.starts_with(head) && safe.ends_with(tail));
        assert!(safe.contains("truncated"));
    }
    let secret = format!("sk-{}", "A".repeat(100000));
    let safe = payload(
        json!({"content":format!("{}{secret}\nlatest-visible-line","old\n".repeat(3000000))}),
    );
    assert!(safe.contains("latest-visible-line"));
    assert!(!safe.contains(&"A".repeat(32)));
    let private = format!(
        "-----BEGIN PRIVATE KEY-----\n{}-----END PRIVATE KEY-----\nvisible-tail",
        "A\n".repeat(100000)
    );
    assert!(!payload(json!(private)).contains("A\\n"));
}
#[test]
fn cuts_never_publish_multiline_or_explicit_producer_secret_fragments() {
    let mut redactor = Redactor::default();
    let secret = "producer-secret-Q7r9T3x6";
    redactor.secrets.insert(secret.into());
    for offset in (0..9000).step_by(5) {
        let source = format!(
            "{} password=zE9pR4cV7xB2nM8aQ6tK x {secret} y {}",
            "A".repeat(offset),
            "B".repeat(9000 - offset)
        );
        let safe = redactor.text(&source, 8192).unwrap();
        no_fragments(
            &safe,
            &[json!(secret), json!("zE9pR4cV7xB2nM8aQ6tK")],
            "swept cut",
        );
    }
    for (begin, end) in [
        (
            "-----BEGIN RSA PRIVATE KEY-----",
            "-----END RSA PRIVATE KEY-----",
        ),
        (
            "-----BEGIN OPENSSH PRIVATE KEY-----",
            "-----END OPENSSH PRIVATE KEY-----",
        ),
        (
            "-----BEGIN PGP PRIVATE KEY BLOCK-----\nVersion: x\n",
            "-----END PGP PRIVATE KEY BLOCK-----",
        ),
        (
            "-----BEGIN ENCRYPTED PRIVATE KEY-----",
            "-----END ENCRYPTED PRIVATE KEY-----",
        ),
        ("api_key: |", "next: 1"),
    ] {
        let indent = if begin.starts_with("api_key") {
            "  "
        } else {
            ""
        };
        let body = (0..40)
            .map(|index| format!("{indent}zE9pR4cV7xB2nM8aQ6tK{index:02}"))
            .collect::<Vec<_>>()
            .join("\n");
        for fill in 60..135 {
            let source = format!(
                "{}\n{begin}\n{body}\n{end}\n{}",
                "ordinary filler\n".repeat(150),
                "filler\n".repeat(fill)
            );
            for safe in [
                redactor.text(&source, 8192).unwrap(),
                redactor
                    .payload(&json!(source).to_string(), PAYLOAD_BYTES)
                    .unwrap(),
            ] {
                assert!(!safe.contains("zE9pR4cV7x"));
            }
        }
    }
}
#[test]
fn adversarial_producer_inputs_stay_bounded() {
    let data = data();
    let mut executed = 0;
    for unit in data["adversarial"].as_array().unwrap() {
        let unit = unit.as_str().unwrap();
        let source = unit.repeat(8192 / unit.len() + 1);
        let source: String = source.chars().take(8192).collect();
        let start = std::time::Instant::now();
        for input in [source.clone(), format!("x\n{source}")] {
            assert!(Redactor::default().text(&input, 8192).unwrap().len() <= PAYLOAD_BYTES);
            executed += 1;
        }
        assert!(
            start.elapsed() < std::time::Duration::from_secs(2),
            "bounded matcher exceeded adversarial guard"
        );
    }
    println!("adversarial assertions={executed}");
}
