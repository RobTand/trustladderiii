#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Character,
    Star,
    NonemptyStar,
    Recursive,
    Directories,
}

/// #820 protected paths: the parsed AST's NFA recurrence. Star never consumes `/`.
/// `older` is the union of preceding-token states at least two characters back,
/// so a nonterminal `**/` consumes either nothing or `.+/`, as CPython 3.14 does.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out == match kind {
    Step::Character => diagonal && accepts,
    Step::Star => same || left && !separator,
    Step::NonemptyStar => (diagonal || left) && !separator,
    Step::Recursive => same || left,
    Step::Directories => same || older && separator,
}))]
pub fn transition(
    kind: Step,
    same: bool,
    diagonal: bool,
    left: bool,
    older: bool,
    separator: bool,
    accepts: bool,
) -> bool {
    match kind {
        Step::Character => diagonal && accepts,
        Step::Star => same || left,
        Step::NonemptyStar => (diagonal || left) && !separator,
        Step::Recursive => same || left,
        Step::Directories => same || older && separator,
    }
}

#[cfg(verus_keep_ghost)]
use vstd::prelude::*;
#[cfg(not(verus_keep_ghost))]
macro_rules! checked_while { (($condition:expr) [$($contract:tt)*] $body:block) => { while $condition $body }; }
#[cfg(verus_keep_ghost)]
macro_rules! checked_while { (($condition:expr) [$($contract:tt)*] $body:block) => { vstd::prelude::verus_exec_expr! { while $condition $($contract)* $body } }; }

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
pub struct Atom {
    pub kind: Step,
    pub accepts: Vec<bool>,
}

#[cfg(verus_keep_ghost)]
vstd::prelude::verus! {
    /// #820 denotation of the parsed pattern AST. Lengths are unbounded.
    pub open spec fn cell(atoms: Seq<Atom>, separators: Seq<bool>, t: nat, p: nat) -> bool
        decreases t, p,
    {
        if t == 0 { p == 0 }
        else if p == 0 {
            match atoms[t as int-1].kind {
                Step::Star | Step::Recursive | Step::Directories => cell(atoms,separators,(t-1) as nat,0),
                _ => false,
            }
        } else {
            match atoms[t as int-1].kind {
                Step::Character => cell(atoms,separators,(t-1) as nat,(p-1) as nat) && atoms[t as int-1].accepts@[p as int-1],
                Step::Star => cell(atoms,separators,(t-1) as nat,p) || cell(atoms,separators,t,(p-1) as nat) && !separators[p as int-1],
                Step::NonemptyStar => (cell(atoms,separators,(t-1) as nat,(p-1) as nat) || cell(atoms,separators,t,(p-1) as nat)) && !separators[p as int-1],
                Step::Recursive => cell(atoms,separators,(t-1) as nat,p) || cell(atoms,separators,t,(p-1) as nat),
                Step::Directories => cell(atoms,separators,(t-1) as nat,p) || separators[p as int-1] && earlier(atoms,separators,(t-1) as nat,(p-1) as nat),
            }
        }
    }
    pub open spec fn earlier(atoms: Seq<Atom>, separators: Seq<bool>, t: nat, count: nat) -> bool
        decreases t, count,
    {
        count > 0 && (cell(atoms,separators,t,(count-1) as nat) || earlier(atoms,separators,t,(count-1) as nat))
    }

}

/// #820 protected paths: evaluate the whole typed AST against the path.
/// Parser and character-class recognition supply the acceptance bits; the verified
/// decision computes the full NFA table for arbitrary finite pattern/path lengths.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out =>
    requires separators.len()<usize::MAX,
        forall|t:int| 0<=t<atoms.len() ==> atoms[t].accepts.len()==separators.len(),
    ensures out==cell(atoms@,separators@,atoms.len() as nat,separators.len() as nat),
))]
pub fn evaluate(atoms: &[Atom], separators: &[bool]) -> bool {
    let n = separators.len();
    let mut previous = Vec::new();
    previous.push(true);
    let mut i = 0;
    checked_while! { (i<n) [
        invariant i<=n, previous.len()==i+1,
            previous[0], forall|j:int| 1<=j<previous.len() ==> !previous[j],
        decreases n-i,
    ] { previous.push(false); i+=1; }}
    let mut t = 0;
    checked_while! { (t<atoms.len()) [
        invariant n==separators.len(), n<usize::MAX, t<=atoms.len(), previous.len()==n+1,
            forall|k:int| 0<=k<atoms.len() ==> atoms[k].accepts.len()==n,
            forall|j:int| 0<=j<=n ==> previous[j]==cell(atoms@,separators@,t as nat,j as nat),
        decreases atoms.len()-t,
    ] {
        let kind=atoms[t].kind;
        let mut row=Vec::new();
        row.push(matches!(kind,Step::Star|Step::Recursive|Step::Directories) && previous[0]);
        let mut older=false;
        let mut p=1;
        checked_while! { (p<=n) [
            invariant n==separators.len(), n<usize::MAX, t<atoms.len(), previous.len()==n+1,
                atoms[t as int].accepts.len()==n, kind==atoms[t as int].kind,
                1<=p<=n+1, row.len()==p,
                forall|j:int| 0<=j<=n ==> previous[j]==cell(atoms@,separators@,t as nat,j as nat),
                forall|j:int| 0<=j<p ==> row[j]==cell(atoms@,separators@,(t+1) as nat,j as nat),
                older == (if p<2 { false } else { earlier(atoms@,separators@,t as nat,(p-2) as nat) }),
            decreases n+1-p,
        ] {
            if p>=2 {older |= previous[p-2];}
            let next=transition(kind,previous[p],previous[p-1],row[p-1],older,separators[p-1],atoms[t].accepts[p-1]);
            #[cfg(verus_keep_ghost)]
            vstd::prelude::verus_exec_expr! { proof {
                reveal_with_fuel(earlier,2);
                assert(older == earlier(atoms@,separators@,t as nat,(p-1) as nat));
                reveal_with_fuel(cell,2);
                assert(next == cell(atoms@,separators@,(t+1) as nat,p as nat));
            }};
            row.push(next);
            p+=1;
        }}
        previous=row; t+=1;
    }}
    previous[n]
}
