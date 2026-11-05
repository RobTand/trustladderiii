use super::*;
fn arms(source: &str) -> &str {
    let start = source
        .find("            Observation::Progress { key, checkpoint } => {")
        .unwrap();
    let end = start
        + source[start..]
            .find("            Observation::Classified {")
            .unwrap_or_else(|| {
                source[start..]
                    .find("            _ => unreachable!")
                    .unwrap()
            });
    &source[start..end]
}
fn method<'a>(source: &'a str, name: &str) -> &'a str {
    let token = format!("fn {name}(");
    let token = source.find(&token).unwrap();
    let start = source[..token].rfind('\n').unwrap() + 1;
    &source[start..start + source[start..].find("\n    }\n").unwrap() + 7]
}
#[test]
fn pinned_real_sources_and_unchanged_arms() {
    let pins = include_str!("pins.txt");
    for (name, source) in [
        ("arms", include_str!("base-arms.rs.txt")),
        ("delegates", include_str!("base-delegates.rs.txt")),
        ("state", include_str!("base-state.rs.txt")),
    ] {
        assert!(
            pins.lines()
                .any(|line| line == format!("{name} {:x}", Sha256::digest(source.as_bytes())))
        );
    }
    let old = include_str!("reference.rs");
    let new = include_str!("current.rs");
    assert_eq!(arms(old), include_str!("base-arms.rs.txt"));
    assert_eq!(arms(new), arms(include_str!("../../src/lib.rs")));
    assert_eq!(
        arms(old),
        arms(new),
        "G2d has no residue and must leave both arms unchanged"
    );
    for name in [
        "progress_allowance",
        "quota_allowance",
        "retire_wait",
        "allowance",
        "now_ms",
    ] {
        assert_eq!(
            method(old, name),
            method(include_str!("base-delegates.rs.txt"), name)
        );
        assert_eq!(
            method(new, name),
            method(include_str!("../../src/looks.rs"), name)
        );
        assert_eq!(method(old, name), method(new, name));
    }
    let state = include_str!("types.rs");
    assert_eq!(state, include_str!("base-state.rs.txt"));
    assert!(include_str!("../../src/looks.rs").contains(state));
}
