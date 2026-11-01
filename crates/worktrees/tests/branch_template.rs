use tl3_worktrees::branch_template::BranchTemplate;

#[test]
fn repo_conventions_branches_round_trip() {
    for template in ["tl3/issue-{issue}-node-{node}", "prismabuild-{issue}-tl3-{node}", "node-{node}/issue-{issue}-done", "v2{issue}-3{node}7"] {
        let template = BranchTemplate::new(template).unwrap();
        for issue in [1, 42, u64::MAX] {
            for node in [0, 1, 123, u64::MAX] {
                let name = template.format(issue, node).unwrap();
                assert_eq!(template.parse(name.as_str()), Some((issue, node)));
            }
        }
        for other in ["topic", "tl3/issue-042-node-0", "tl3/issue-0-node-0", "prismabuild-42-tl3-00", "prismabuild-18446744073709551616-tl3-0"] {
            assert_eq!(template.parse(other), None, "{other}");
        }
    }
}

#[test]
fn repo_conventions_invalid_templates_are_rejected() {
    for template in ["topic-{issue}", "topic-{node}", "topic-{issue}{node}", "topic-{issue}0{node}", "topic-{issue}-{node}-{issue}", "topic-{issue}-{node}-{unknown}", "bad..{issue}-{node}"] {
        assert!(BranchTemplate::new(template).is_err(), "{template}");
    }
}
