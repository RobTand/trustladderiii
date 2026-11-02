//! A request's priority, from its issue (#416, #417).

/// P0–P3 as `Some(0..=3)`, or `None` when the issue is unranked.
pub fn issue_priority<S: AsRef<str>>(_title: &str, _labels: &[S]) -> Option<u8> {
    None
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
