//! #806: test-only pre-rewrite iterator reference, retained for a later follow-up.
use tl3_kernel::*;

// Retained verbatim until a follow-up removes this equivalence reference.
struct ReferenceWorld {
    nodes: Vec<Node>,
    dependencies: Vec<Dependency>,
}

impl ReferenceWorld {
    pub fn measure(&self) -> u64 {
        let mut total = 0;
        for n in &self.nodes {
            total += n.distance();
        }
        total += self
            .dependencies
            .iter()
            .filter(|d| {
                d.status != DependencyStatus::Delivered
                    && !self.nodes[d.node.0 as usize].phase.is_closed()
            })
            .count() as u64;
        total
    }
}

pub(super) fn compare(w: &World) {
    let reference = ReferenceWorld {
        nodes: w.nodes.clone(),
        dependencies: w.dependencies().to_vec(),
    };
    assert_eq!(w.measure(), reference.measure(), "world: {w:#?}");
}
