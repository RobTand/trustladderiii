use std::io::{self, BufRead};
use tl3_github::message::{Policy, check_prose};

fn main() {
    let policy = Policy {
        own_repository: Some("RobTand/tl3-sandbox".into()),
        own_issue: Some(12),
        own_pull_request: Some(34),
        ..Policy::default()
    };
    for line in io::stdin().lock().lines() {
        let text: String = serde_json::from_str(&line.unwrap()).unwrap();
        let result = check_prose(&text, &policy);
        println!(
            "{}",
            serde_json::json!({
                "accepted": result.is_ok(),
                "violations": result.err().map(|errors| format!("{errors:?}")),
            })
        );
    }
}
