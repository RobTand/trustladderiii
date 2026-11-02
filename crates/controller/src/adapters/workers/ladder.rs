//! Model choice for one worker step (Q22, Q44, Q53).
use crate::{Error, Result};
use tl3_config::{Model, Models};

/// The step's models in fallback order, its configured model first. A reviewer
/// only gets models from a different family than the maker that made the head.
pub(crate) fn candidates<'a>(
    models: &'a Models,
    role: &str,
    rung: usize,
    review_restarted: bool,
    maker: Option<&str>,
) -> Result<Vec<&'a Model>> {
    let ladder: &[Model] = match role {
        "classifier" => &models.comment_classifier,
        "ingest" => &models.ingest,
        "audit" => &models.audit,
        "maker" => &models.maker,
        "planner" => &models.planner,
        "reviewer" => &models.reviewer,
        "postmortem" => &models.postmortem,
        _ => return Err(Error::Invalid("unsupported worker role")),
    };
    if ladder.is_empty() {
        return Err(Error::Invalid("worker role has no configured model"));
    }
    if role != "reviewer" {
        return Ok(ordered(ladder, rung.min(ladder.len() - 1)));
    }
    let configured = &models.maker[rung.min(models.maker.len() - 1)];
    // A maker model no longer configured can't name its family; use the attempt's own.
    let maker = maker
        .and_then(|name| models.maker.iter().find(|model| model.name == name))
        .unwrap_or(configured);
    let different = |model: &Model| !model.family.eq_ignore_ascii_case(&maker.family);
    let first = ladder
        .iter()
        .position(different)
        .ok_or(Error::Invalid("reviewer needs a different model family"))?;
    let start = (first + usize::from(review_restarted)).min(ladder.len() - 1);
    let mut models = ordered(ladder, start);
    let configured = models[0];
    models.retain(|model| std::ptr::eq(*model, configured) || different(model));
    Ok(models)
}

/// The ladder from `start` onward, then the models before it, each name once.
fn ordered(ladder: &[Model], start: usize) -> Vec<&Model> {
    let mut models: Vec<&Model> = Vec::new();
    for model in ladder[start..].iter().chain(&ladder[..start]) {
        if !models.iter().any(|seen| seen.name == model.name) {
            models.push(model);
        }
    }
    models
}

/// Q53: the first model that hasn't reported quota, else the configured one.
pub(super) fn choose<'a>(candidates: &[&'a Model], limited: impl Fn(&str) -> bool) -> &'a Model {
    candidates
        .iter()
        .find(|model| !limited(&model.name))
        .unwrap_or(&candidates[0])
}

/// The model a step runs now: its first candidate whose announced quota
/// reset hasn't passed. The worker adapter and the console's board share it.
pub(crate) fn running<'a>(
    candidates: &[&'a Model],
    resets: &std::collections::BTreeMap<String, u64>,
    now_ms: u64,
) -> &'a Model {
    choose(candidates, |name| {
        resets.get(name).is_some_and(|reset| *reset > now_ms)
    })
}

/// Models that reported quota, by name, with their announced resets.
pub(crate) fn quota_resets(
    snapshot: &tl3_store::Snapshot,
) -> std::collections::BTreeMap<String, u64> {
    snapshot
        .rows
        .iter()
        .filter(|row| row.table == "model_quotas")
        .filter_map(|row| Some((row.id.clone(), row.value["reset_ms"].as_u64()?)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn models() -> Models {
        let mut config =
            tl3_config::Config::parse(include_str!("../../../../config/example.toml")).unwrap();
        config.models.reviewer = vec![
            config.models.maker[0].clone(),
            config.models.maker[2].clone(),
        ];
        config.models
    }

    fn names(models: &[&Model]) -> Vec<String> {
        models
            .iter()
            .map(|m| format!("{}/{}", m.name, m.budget_tokens))
            .collect()
    }

    #[test]
    fn planner_falls_back_to_the_next_model() {
        let models = models();
        let all = candidates(&models, "planner", 0, false, None).unwrap();
        assert_eq!(
            names(&all),
            ["default-model/32000", "alternate-model/32000"]
        );
        assert_eq!(
            choose(&all, |name| name == "default-model").name,
            "alternate-model"
        );
        assert_eq!(choose(&all, |_| false).name, "default-model");
        // Every model out: the configured model, which waits for its reset.
        assert_eq!(choose(&all, |_| true).name, "default-model");
        let retry = candidates(&models, "planner", 1, false, None).unwrap();
        assert_eq!(
            names(&retry),
            ["alternate-model/32000", "default-model/32000"]
        );
    }

    #[test]
    fn maker_attempt_one_under_quota_moves_to_the_next_eligible_model() {
        let models = models();
        // Attempts 1 and 2 share a model, so the shared model is tried once.
        let first = candidates(&models, "maker", 0, false, None).unwrap();
        assert_eq!(
            names(&first),
            ["default-model/32000", "alternate-model/64000"]
        );
        let chosen = choose(&first, |name| name == "default-model");
        assert_eq!(chosen.name, "alternate-model");
        let second = candidates(&models, "maker", 1, false, None).unwrap();
        assert_eq!(
            names(&second),
            ["default-model/64000", "alternate-model/64000"]
        );
    }

    #[test]
    fn reviewer_stays_a_different_family_from_the_maker_that_ran() {
        let models = models();
        let configured = candidates(&models, "reviewer", 0, false, None).unwrap();
        assert_eq!(names(&configured), ["alternate-model/64000"]);
        // The maker fell back to the alternate family, so review moves to default.
        let after_fallback =
            candidates(&models, "reviewer", 0, false, Some("alternate-model")).unwrap();
        assert_eq!(names(&after_fallback), ["default-model/32000"]);
        assert_eq!(
            choose(&after_fallback, |name| name == "default-model").name,
            "default-model",
            "no same-family reviewer even when the different family is out"
        );
        // A maker model no longer configured falls back to the attempt's configured maker.
        let unknown = candidates(&models, "reviewer", 0, false, Some("retired-model")).unwrap();
        assert_eq!(names(&unknown), ["alternate-model/64000"]);
    }
}
