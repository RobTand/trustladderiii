//! Deterministic inputs and counted assumption filters for the copied checks.
use super::*;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind, panic_any, resume_unwind};
use std::sync::Once;
use std::time::Instant;

pub const SEED: u64 = 0x4b35_1110_5eed_b001;

#[derive(Clone, Debug)]
pub enum Draw {
    U64(u64), U8(u8), Bool(bool), Ticket(Option<u64>), Node(Node),
    Phase(Phase), Priority(Priority), Status(DependencyStatus), Files(Footprint),
}

pub struct Prng(pub u64);
impl Prng {
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
}

// Mixed-radix enumeration visits every tuple of the listed finite domains.
pub fn enumerate(radices: &[usize], mut visit: impl FnMut(usize, &[usize])) {
    let count: usize = radices.iter().product();
    for index in 0..count {
        let mut n = index;
        let digits: Vec<_> = radices.iter().map(|&r| { let d = n % r; n /= r; d }).collect();
        visit(index, &digits);
    }
}

struct Context {
    index: usize,
    rng: Prng,
    draws: VecDeque<Draw>,
    world: Option<World>,
    covers: std::collections::BTreeSet<&'static str>,
}
thread_local! { static CONTEXT: RefCell<Option<Context>> = const { RefCell::new(None) }; }
#[derive(Debug)]
struct Filter(String);
static HOOK: Once = Once::new();

#[track_caller]
pub fn assume(condition: bool) {
    if !condition { panic_any(Filter(std::panic::Location::caller().to_string())); }
}

pub fn cover_hit(label: &'static str) {
    CONTEXT.with(|slot| { slot.borrow_mut().as_mut().unwrap().covers.insert(label); });
}
macro_rules! cover {
    ($condition:expr, $label:literal $(,)?) => {
        if $condition { $crate::common::sampling::cover_hit($label); }
    };
}
pub(crate) use cover;

pub trait Sample: Sized {
    fn draw(value: Option<Draw>, index: usize, rng: &mut Prng) -> Self;
}
#[track_caller]
pub fn any<T: Sample>() -> T {
    CONTEXT.with(|slot| {
        let mut slot = slot.borrow_mut(); let c = slot.as_mut().expect("run_case scope required");
        T::draw(c.draws.pop_front(), c.index, &mut c.rng)
    })
}
macro_rules! sample {
    ($ty:ty, $variant:ident, $index:ident, $rng:ident, $fallback:expr) => {
        impl Sample for $ty {
            fn draw(value: Option<Draw>, $index: usize, $rng: &mut Prng) -> Self {
                match value { Some(Draw::$variant(v)) => v,
                    Some(other) => panic!("wrong deterministic draw: {other:?}"), None => $fallback }
            }
        }
    };
}
sample!(u64, U64, index, rng, bounded_counter(index, rng.next()));
sample!(u8, U8, index, rng, (rng.next() % 4) as u8);
sample!(bool, Bool, index, rng, rng.next() & 1 != 0);
sample!(Option<u64>, Ticket, index, rng, if rng.next() % 5 == 0 { None } else { Some(rng.next() % 8) });
sample!(Phase, Phase, index, rng, phases()[index % phases().len()]);
sample!(Priority, Priority, index, rng, priorities()[index % 5]);
sample!(DependencyStatus, Status, index, rng, statuses()[index % 3]);
sample!(Footprint, Files, index, rng, Footprint::inline(rng.next() % 16));
sample!(Node, Node, index, rng, node_fixture(phases()[index % phases().len()], index, rng.next()));

pub mod vec {
    pub fn exact_vec<T: super::Sample, const N: usize>() -> Vec<T> {
        (0..N).map(|_| super::any()).collect()
    }
}

pub fn world(phase: Option<Phase>) -> World {
    CONTEXT.with(|slot| {
        let mut slot = slot.borrow_mut(); let c = slot.as_mut().unwrap();
        c.world.clone().unwrap_or_else(|| world_fixture(phase, c.index, c.rng.next()))
    })
}

#[derive(Default)]
pub struct Stats {
    pub attempted: usize,
    pub passed: usize,
    pub skipped: usize,
    pub filters: std::collections::BTreeMap<String, usize>,
    pub covers: std::collections::BTreeSet<&'static str>,
}
impl Stats {
    pub fn case(&mut self, index: usize, seed: u64, world: Option<World>, draws: Vec<Draw>, check: impl FnOnce()) {
        HOOK.call_once(|| {
            let prior = std::panic::take_hook();
            std::panic::set_hook(Box::new(move |info| {
                if !info.payload().is::<Filter>() { prior(info); }
            }));
        });
        self.attempted += 1;
        CONTEXT.with(|slot| *slot.borrow_mut() = Some(Context {
            index, rng: Prng(seed ^ index as u64), draws: draws.into(), world,
            covers: Default::default(),
        }));
        let result = catch_unwind(AssertUnwindSafe(check));
        let context = CONTEXT.with(|slot| slot.borrow_mut().take().unwrap());
        self.covers.extend(context.covers);
        match result {
            Ok(()) => { assert!(context.draws.is_empty(), "unused deterministic draws"); self.passed += 1; }
            Err(payload) => match payload.downcast::<Filter>() {
                Ok(filter) => { self.skipped += 1; *self.filters.entry(filter.0).or_default() += 1; }
                Err(payload) => { eprintln!("KANI_FAILURE case={index} seed={seed:#x} world={:?}", context.world); resume_unwind(payload); }
            },
        }
    }
    pub fn finish(&self, name: &str, seed: u64, start: Instant) {
        assert!(self.passed > 0, "all inputs filtered: {name}");
        println!("KANI_STATS name={name} seed={seed:#x} attempted={} passed={} skipped={} elapsed_ns={} covers={:?} filters={:?}",
            self.attempted, self.passed, self.skipped, start.elapsed().as_nanos(), self.covers, self.filters);
    }
}
