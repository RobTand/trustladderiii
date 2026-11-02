use std::env;

/// Assign each declared scenario group to exactly one Bazel shard.
/// Groups keep their original fixture lifetime and scenario timing markers.
pub struct Shard {
    index: usize,
    total: usize,
    next: usize,
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
        assert!(
            total > 0 && index < total,
            "invalid Bazel shard index/count"
        );
        Self::new(index, total)
    }

    pub fn new(index: usize, total: usize) -> Self {
        Self {
            index,
            total,
            next: 0,
            planned: 0,
            selected: 0,
        }
    }

    pub fn unsharded() -> Self {
        Self {
            index: 0,
            total: 1,
            next: 0,
            planned: 0,
            selected: 0,
        }
    }

    pub fn run(&mut self, markers: usize, scenario: impl FnOnce()) {
        let selected = self.next % self.total == self.index;
        self.next += 1;
        self.planned += markers;
        if selected {
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
