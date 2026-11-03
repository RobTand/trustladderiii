use std::{
    cmp::Reverse,
    collections::{HashMap, HashSet},
    env,
};

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
    Memory: 10,
    Summaries: 3,
    Sessions: 4,
    Retirement: 5,
    Planning: 12,
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

impl WorkerGroup {
    fn estimated_seconds(self) -> usize {
        // BuildBuddy child 705afa47-9d28-4941-aacf-19edf4b6503f, rounded seconds.
        // The other seven 11-marker shards averaged 7 s per marker.
        match self {
            Self::MakerChecksFail => 242,
            Self::Restart => 187,
            Self::MakerCiFailure => 84,
            _ => 7 * self.markers(),
        }
    }
}

/// Assign each declared scenario group to exactly one Bazel shard.
/// Groups keep their original fixture lifetime and scenario timing markers.
pub struct Shard {
    index: usize,
    owners: HashMap<WorkerGroup, usize>,
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
        let mut groups = WORKER_GROUPS.to_vec();
        groups.sort_by_key(|group| Reverse(group.estimated_seconds()));
        let mut loads = vec![0; total];
        let mut owners = HashMap::new();
        for group in groups {
            let owner = loads
                .iter()
                .enumerate()
                .min_by_key(|(_, load)| *load)
                .unwrap()
                .0;
            loads[owner] += group.estimated_seconds();
            owners.insert(group, owner);
        }
        Self {
            index,
            owners,
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
        let owner = self.owners[&group];
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
