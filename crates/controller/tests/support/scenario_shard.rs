use std::env;

/// Assign each declared scenario group to exactly one Bazel shard.
/// Groups keep their original fixture lifetime and scenario timing markers.
pub struct Shard {
    index: usize,
    loads: Vec<usize>,
    planned: usize,
    selected: usize,
}

impl Shard {
    pub fn from_env() -> Self {
        let parse = |name: &str, default| {
            env::var(name).map_or(default, |value| value.parse().expect("invalid Bazel shard"))
        };
        let index = parse("TEST_SHARD_INDEX", 0);
        let total = parse("TEST_TOTAL_SHARDS", 1);
        Self::new(index, total)
    }

    pub fn new(index: usize, total: usize) -> Self {
        assert!(
            total > 0 && index < total,
            "invalid Bazel shard index/count"
        );
        Self {
            index,
            loads: vec![0; total],
            planned: 0,
            selected: 0,
        }
    }

    pub fn unsharded() -> Self {
        Self::new(0, 1)
    }

    pub fn run(&mut self, markers: usize, scenario: impl FnOnce()) {
        let owner = self
            .loads
            .iter()
            .enumerate()
            .min_by_key(|(_, load)| *load)
            .unwrap()
            .0;
        self.loads[owner] += markers;
        self.planned += markers;
        if owner == self.index {
            scenario();
            self.selected += markers;
        }
    }

    pub fn finish(self, expected: usize, label: &str) {
        assert_eq!(
            self.planned, expected,
            "the shard plan must cover the whole suite"
        );
        assert!(self.selected > 0, "empty scenario shard");
        println!("{} {label} scenarios passed; no skips", self.selected);
    }
}
