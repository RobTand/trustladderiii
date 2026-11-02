//! A request's priority, from its issue (#416, #417).

/// P0–P3 as `Some(0..=3)`, or `None` when the issue is unranked.
/// A `P0`–`P3` label wins over a `[P0]`–`[P3]` title prefix; with several
/// such labels, the most urgent wins. Both are case-insensitive.
pub fn issue_priority<S: AsRef<str>>(title: &str, labels: &[S]) -> Option<u8> {
    labels
        .iter()
        .filter_map(|label| level(label.as_ref()))
        .min()
        .or_else(|| {
            let rest = title.trim_start().strip_prefix('[')?;
            let (level_text, _) = rest.split_once(']')?;
            level(level_text)
        })
}

fn level(text: &str) -> Option<u8> {
    match text.as_bytes() {
        [b'P' | b'p', digit @ b'0'..=b'3'] => Some(digit - b'0'),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: [&str; 0] = [];

    #[test]
    fn title_prefix_names_the_priority() {
        for (title, expected) in [
            ("[P0] Fix the pool", Some(0)),
            ("[P1] Fix the pool", Some(1)),
            ("[p2]Fix the pool", Some(2)),
            ("  [P3] Fix the pool", Some(3)),
            ("Fix the pool [P1]", None),
            ("[P4] Fix the pool", None),
            ("[P1x] Fix the pool", None),
            ("P1 Fix the pool", None),
            ("", None),
        ] {
            assert_eq!(issue_priority(title, &NONE), expected, "{title:?}");
        }
    }

    #[test]
    fn labels_name_the_priority_and_win_over_the_title() {
        assert_eq!(issue_priority("Fix", &["P2"]), Some(2));
        assert_eq!(issue_priority("Fix", &["tl3", "p1"]), Some(1));
        assert_eq!(issue_priority("[P3] Fix", &["P0"]), Some(0));
        assert_eq!(issue_priority("[P0] Fix", &["P3"]), Some(3));
        // Several levels: the most urgent wins.
        assert_eq!(issue_priority("Fix", &["P3", "P1", "P2"]), Some(1));
        assert_eq!(
            issue_priority("[P2] Fix", &["P5", "priority", "P1-ish"]),
            Some(2)
        );
        assert_eq!(issue_priority("Fix", &["tl3"]), None);
    }
}
