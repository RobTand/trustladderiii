use serde_json::{Value, json};
use wiremock::{Request, ResponseTemplate};

pub fn access_token(path: &str, installation: u64, token: &str) -> Option<ResponseTemplate> {
    (path == format!("/app/installations/{installation}/access_tokens")).then(|| {
        ResponseTemplate::new(201).set_body_json(json!({
            "token": token, "expires_at": "2099-01-01T00:00:00Z"
        }))
    })
}

pub trait Routes {
    fn pulls(&self) -> Value;
    fn checks(&self, sha: &str) -> Value;
    fn suites(&self) -> Value {
        json!([])
    }
    fn checks_mut(&mut self) -> Option<&mut Vec<Value>> {
        None
    }
    fn check_posted(&mut self) {}
    fn pull_mut(&mut self) -> Option<&mut Value> {
        None
    }
    fn pull_patched(&mut self, _body: &Value) {}
    fn created_status(&self) -> u16 {
        201
    }
}

// Scenarios supply evidence and side effects; shared routes own URL and response shapes.
pub fn respond(
    request: &Request,
    path: &str,
    repository: &str,
    state: &mut impl Routes,
) -> Option<ResponseTemplate> {
    let path = path.strip_prefix(repository)?;
    let method = request.method.as_str();
    let value = match (method, path) {
        ("GET", "/pulls") => state.pulls(),
        ("GET", path) if path.starts_with("/commits/") && path.ends_with("/check-runs") => {
            let sha = path
                .strip_prefix("/commits/")?
                .strip_suffix("/check-runs")?;
            json!({"check_runs": state.checks(sha)})
        }
        ("GET", path) if path.starts_with("/commits/") && path.ends_with("/check-suites") => {
            json!({"check_suites": state.suites()})
        }
        ("POST", "/check-runs") => {
            let mut check: Value = serde_json::from_slice(&request.body).unwrap();
            let checks = state.checks_mut()?;
            check["id"] = json!(checks.len() + 1);
            check["app"] = json!({"id": 123, "slug": "factory"});
            checks.push(check.clone());
            state.check_posted();
            return Some(ResponseTemplate::new(state.created_status()).set_body_json(check));
        }
        ("PATCH", path) if path.starts_with("/check-runs/") => {
            let id: u64 = path.strip_prefix("/check-runs/")?.parse().ok()?;
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            let checks = state.checks_mut()?;
            let check = checks.iter_mut().find(|check| check["id"] == id)?;
            for (key, value) in body.as_object().unwrap() {
                check[key] = value.clone();
            }
            check.clone()
        }
        ("PATCH", "/pulls/100") => {
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            let pull = state.pull_mut()?;
            for field in ["state", "body"] {
                if let Some(value) = body.get(field) {
                    pull[field] = value.clone();
                }
            }
            let response = pull.clone();
            state.pull_patched(&body);
            response
        }
        _ => return None,
    };
    Some(ResponseTemplate::new(200).set_body_json(value))
}
