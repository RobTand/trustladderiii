//! The kernel's footprint set operations, checked against `BTreeSet`.
//! A set of ids below 64 is an inline bitset, and any larger id makes it an
//! interned list. The proofs only see small inline sets, so these tests cover
//! the rest: unions of every mix, file locks between inline and interned sets
//! in both orders, and the 63/64 boundary.

use std::collections::BTreeSet;
use tl3_kernel::*;

/// Subsets of these ids give inline sets, interned sets and every mix.
const IDS: [u64; 7] = [0, 1, 63, 64, 65, 1 << 40, u64::MAX];

fn subset(bits: u32) -> BTreeSet<FileId> {
    (0..IDS.len())
        .filter(|i| bits & (1 << i) != 0)
        .map(|i| FileId(IDS[i]))
        .collect()
}

fn subsets() -> impl Iterator<Item = BTreeSet<FileId>> {
    (1..1u32 << IDS.len()).map(subset)
}

/// Confirm the node's effects, then send it a reply.
fn reply(w: &mut World, node: NodeId, reply: Reply) {
    let i = node.index().unwrap();
    while let Some(e) = w.nodes[i].effect {
        *w = step(
            w.clone(),
            Event::Ack {
                node,
                seq: e.seq,
                ack: Ack::Confirmed,
            },
        )
        .0;
    }
    *w = step(
        w.clone(),
        Event::Reply {
            node,
            generation: w.nodes[i].generation,
            reply,
        },
    )
    .0;
    assert!(w.valid(), "invalid world after {reply:?}:\n{w:#?}");
}

/// Add a request in repo 0 and plan it as one leaf over `files`.
fn plan(w: &mut World, files: &BTreeSet<FileId>) -> NodeId {
    *w = step(w.clone(), Event::NewRequest { repo: RepoId(0) }).0;
    let node = NodeId(w.nodes.len() as u64 - 1);
    let footprint = w.intern_footprint(files.iter().copied());
    reply(w, node, Reply::IngestDone { size: 1 });
    reply(w, node, Reply::AuditAgrees);
    reply(
        w,
        node,
        Reply::PlanLeaf {
            footprint,
            fits: true,
        },
    );
    node
}

fn files(w: &World, node: NodeId) -> BTreeSet<FileId> {
    let footprint = w.nodes[node.index().unwrap()].footprint;
    w.files(footprint).expect("a known footprint").collect()
}

#[test]
fn extending_a_footprint_makes_the_union() {
    for a in subsets() {
        let mut base = World::new(&[1], 1);
        let node = plan(&mut base, &a);
        assert_eq!(files(&base, node), a);
        for b in subsets() {
            let mut w = base.clone();
            let extra = w.intern_footprint(b.iter().copied());
            reply(
                &mut w,
                node,
                Reply::MakerNeedsFiles {
                    files: extra,
                    fits: true,
                },
            );
            let union: BTreeSet<FileId> = a.union(&b).copied().collect();
            assert_eq!(files(&w, node), union, "{a:?} with {b:?}");
            // Equal sets share one handle, so the union is in canonical form.
            let footprint = w.nodes[0].footprint;
            assert_eq!(w.intern_footprint(union.iter().copied()), footprint);
        }
    }
}

#[test]
fn a_second_leaf_starts_only_when_its_files_are_free() {
    for a in subsets() {
        let mut base = World::new(&[2], 2);
        let first = plan(&mut base, &a);
        assert_eq!(base.nodes[0].phase, Phase::Making);
        for b in subsets() {
            let mut w = base.clone();
            let second = plan(&mut w, &b);
            let free = a.is_disjoint(&b);
            let n = w.nodes[1];
            assert_eq!(n.phase == Phase::Making && n.holds, free, "{a:?} and {b:?}");
            assert_eq!(n.phase == Phase::Ready(Resume::Making), !free);
            assert_eq!(files(&w, first), a);
            assert_eq!(files(&w, second), b);
        }
    }
}

#[test]
fn two_large_disjoint_footprints_hold_at_once() {
    let wide = |from: u64| (0..300).map(move |i| FileId((1 << 40) + from + i));
    let mut w = World::new(&[2], 2);
    plan(&mut w, &wide(0).collect());
    plan(&mut w, &wide(300).collect());
    assert!(w.nodes.iter().all(|n| n.phase == Phase::Making && n.holds));
    // One shared file is enough to wait.
    let mut w = World::new(&[2], 2);
    plan(&mut w, &wide(0).collect());
    plan(&mut w, &wide(299).collect());
    assert_eq!(w.nodes[1].phase, Phase::Ready(Resume::Making));
}
