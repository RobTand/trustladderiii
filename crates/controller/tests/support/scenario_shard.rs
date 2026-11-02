use std::{collections::HashSet, env};

macro_rules! worker_groups {
    ($($group:ident: $markers:expr),* $(,)?) => {
        #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
        pub enum WorkerGroup { $($group),* }

        pub const WORKER_GROUPS: &[WorkerGroup] = &[$(WorkerGroup::$group),*];

        impl WorkerGroup {
            pub const fn markers(self) -> usize {
                match self { $(Self::$group => $markers),* }
            }
        }
    };
}

worker_groups! {
    Router: 2,
    Waits: 6,
    IngestAudit: 6,
    Launch: 4,
    Restart: 2,
    Reply: 9,
    Memory: 8,
    Summaries: 3,
    Sessions: 4,
    Retirement: 5,
    Planning: 7,
    MakerRepositoryImages: 1,
    MakerAdoptedFix: 1,
    MakerReproducingFix: 1,
    MakerDeclaredWait: 1,
    MakerReviewFeedback: 1,
    MakerLegacySession: 1,
    MakerOutsidePaths: 1,
    MakerRecordedFailure: 1,
    MakerAcceptance: 1,
    MakerControllerRestart: 1,
    MakerRebaseUnchanged: 1,
    MakerRebaseEdited: 1,
    MakerReplanRestart: 1,
    MakerProcess: 1,
    MakerNormal: 1,
    MakerKilledAfterCommit: 1,
    MakerSessionSaveFails: 1,
    MakerTwiceKilled: 1,
    MakerFixKilled: 1,
    MakerSelfCheckMismatch: 1,
    MakerChecksFail: 1,
    MakerOutside: 1,
    MakerCantMeet: 1,
    MakerUnmet: 1,
    MakerOutOfRoom: 1,
    MakerNeedsFiles: 1,
    MakerCiFailure: 1,
    MakerCiFact: 1,
    MakerCleanup: 1,
}

/// Assign each declared scenario group to exactly one Bazel shard.
/// Groups keep their original fixture lifetime and scenario timing markers.
pub struct Shard {
    index: usize,
    loads: Vec<usize>,
    planned: HashSet<WorkerGroup>,
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
            planned: HashSet::new(),
            selected: 0,
        }
    }

    pub fn unsharded() -> Self {
        Self::new(0, 1)
    }

    pub fn run(&mut self, group: WorkerGroup, scenario: impl FnOnce()) {
        assert!(
            self.planned.insert(group),
            "duplicate scenario group: {group:?}"
        );
        let markers = group.markers();
        let owner = self
            .loads
            .iter()
            .enumerate()
            .min_by_key(|(_, load)| *load)
            .unwrap()
            .0;
        self.loads[owner] += markers;
        if owner == self.index {
            scenario();
            self.selected += markers;
        }
    }

    pub fn finish(self, label: &str) {
        for group in WORKER_GROUPS {
            assert!(
                self.planned.contains(group),
                "missing scenario group: {group:?}"
            );
        }
        assert!(self.selected > 0, "empty scenario shard");
        println!("{} {label} scenarios passed; no skips", self.selected);
    }
}
