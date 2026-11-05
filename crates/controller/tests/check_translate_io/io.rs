use super::*;
use std::{cell::RefCell, panic::{AssertUnwindSafe, catch_unwind}};
use tl3_controller::adapters::prismabuild::CheckRun;
use tl3_kernel::{FileId, Node, Phase, RepoId};
use tl3_store::{Epoch, InternKind, Interned, Revision, WorldRow};
pub(super) const SHA: &str = "1111111111111111111111111111111111111111";
pub(super) const OTHER: &str = "2222222222222222222222222222222222222222";
pub(super) type World = tl3_kernel::World;
type Dictionary = BTreeMap<(String,String,String),u64>;
#[derive(Clone, Debug)]
pub(super) struct Step {
    pub arm: u8,
    pub present: bool,
    pub node: u64,
    pub adopted: bool,
    pub same_head: bool,
    pub snapshot_head: Option<String>,
    pub rebasing: bool,
    pub current: u64,
    pub observed: u64,
    pub job: Option<u64>,
    pub sha: String,
    pub base: String,
    pub run: CheckRun,
    pub changed: Vec<String>,
    pub failed: Vec<findings::FailedTest>,
    pub results: Vec<Value>,
    pub pass: bool,
    pub footprint: Vec<u64>,
    pub invalid_footprint: bool,
    pub request: Option<Value>,
    pub prior: Vec<(String,Value)>,
}
pub(super) fn check_run() -> CheckRun {
    serde_json::from_value(json!({"repository":"/served","base_sha":OTHER,
        "plan":[{"name":"check-1","kind":"regression","tests":["tests/a.py"],"goal_ids":["goal-1"]}]})).unwrap()
}
impl Default for Step {
    fn default() -> Self {
        Self { arm:0,present:true,node:0,adopted:true,same_head:true,snapshot_head:Some(SHA.into()),
            rebasing:true,current:7,observed:7,job:Some(7),sha:SHA.into(),base:OTHER.into(),run:check_run(),
            changed:vec!["src/a.rs".into(),"src/b.rs".into()],failed:vec![findings::FailedTest{test:"tests/a.py".into(),message:"assertion".into()}],
            results:vec![json!({"name":"Existing tests that import the change","kind":"regression","verdict":"fail","tests":[{"test":"tests/a.py","base":"passed","head":"failed","passed":false}]})],pass:true,footprint:vec![3],invalid_footprint:false,
            request:Some(json!({"repo":{"owner":"Owner","name":"Repo","installation":1},"issue":1,"issue_id":1})),
            prior:vec![(format!("0:{OTHER}"),json!({"selected_failures":["tests/old.py"]}))] }
    }
}
pub(super) type Read = std::result::Result<Option<Value>, &'static str>;
#[derive(Clone,Debug)]
pub(super) struct Schedule {
    pub steps: Vec<Step>,
    pub reads: Vec<Read>,
    pub interned: Dictionary,
    pub initial_rows: BTreeMap<(String,String),Value>,
    pub failures: Vec<bool>,
    pub select_failures: Vec<bool>,
}
impl Default for Schedule {
    fn default() -> Self {
        Self{steps:vec![Step::default()],reads:vec![Ok(Some(json!({"check_run":check_run()})))],
            interned:BTreeMap::new(), initial_rows:BTreeMap::new(),failures:vec![],select_failures:vec![]}
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Run {
    pub calls: Vec<Value>, pub results: Vec<Value>,pub staged_rows:Vec<Value>,
    pub persisted_rows:BTreeMap<(String,String),Value>,pub pending_intern:Dictionary,
    pub persisted_intern:Dictionary,pub writes:usize,
}
#[derive(Default)]
pub(super) struct Feedback {
    pub outside_paths_total:usize,pub outside_paths:Vec<String>,pub failed_tests:Vec<findings::FailedTest>,
}
impl Feedback {
    fn value(&self)->Value { json!({"total":self.outside_paths_total,"outside":self.outside_paths,"failed":self.failed_tests}) }
}
#[derive(Clone,Copy,Debug)]
struct Cut{write:usize,after:bool}
#[derive(Debug)]
struct Crash;
struct Recording{run:Run,cut:Option<Cut>,reads:Vec<Read>,read_index:usize,failures:Vec<bool>,select_failures:Vec<bool>,select_index:usize}
thread_local!{static ACTIVE:RefCell<Option<Recording>>=const{RefCell::new(None)};}
fn with<T>(f:impl FnOnce(&mut Recording)->T)->T{ACTIVE.with(|r|f(r.borrow_mut().as_mut().unwrap()))}
fn call(op:&str,args:Value){with(|r|r.run.calls.push(json!({"op":op,"args":args})));}
fn begin_write(op:&str,args:Value)->(usize,bool){with(|r|{
    let index=r.run.writes;r.run.writes+=1;r.run.calls.push(json!({"op":op,"args":args}));
    if r.cut.is_some_and(|c|c.write==index&&!c.after){std::panic::panic_any(Crash);}
    (index,r.failures.get(index).copied().unwrap_or(false))
})}
fn end_write(index:usize){with(|r|{if r.cut.is_some_and(|c|c.write==index&&c.after){std::panic::panic_any(Crash);}});}
pub(super) struct Controller{pub store:Store}
pub(super) struct Store;
pub(super) struct InternId(i64);
impl InternId{pub fn get(&self)->i64{self.0}}
impl Store {
    fn read(&mut self,op:&str,id:&str,table:Option<&str>)->Result<Option<Value>>{
        let response=with(|r|{let v=r.reads.get(r.read_index).cloned().unwrap_or(Ok(None));r.read_index+=1;v});
        call(op,json!({"id":id,"table":table,"result":response}));
        response.map_err(|_|Error::Store(tl3_store::Error::Invalid("injected read failure")))
    }
    pub fn worker_materials(&mut self,id:&str)->Result<Option<Value>>{self.read("worker_materials",id,None)}
    pub fn worker_document(&mut self,id:&str,table:&str)->Result<Option<Value>>{self.read("worker_document",id,Some(table))}
    pub fn intern_repo(&mut self,owner:&str,name:&str)->Result<InternId>{self.intern("Repo",&owner.to_ascii_lowercase(),&name.to_ascii_lowercase())}
    pub fn intern_commit(&mut self,node:&str,sha:&str)->Result<InternId>{
        tl3_worktrees::CommitId::new(sha).map_err(|_|Error::Store(tl3_store::Error::Invalid("commit SHA must be 40 or 64 lowercase hexadecimal digits")))?;
        self.intern("Commit",node,sha)
    }
    pub fn intern_path(&mut self,repo:InternId,path:&str)->Result<InternId>{self.intern("Path",&repo.get().to_string(),path)}
    fn intern(&mut self,kind:&str,scope:&str,value:&str)->Result<InternId>{
        if scope.is_empty()||value.is_empty(){return Err(Error::Store(tl3_store::Error::Invalid("intern scope and value must be nonempty")));}
        let key=(kind.into(),scope.into(),value.into());
        let failed=with(|r|{let v=r.select_failures.get(r.select_index).copied().unwrap_or(false);r.select_index+=1;v});
        if failed{call("intern.select.error",json!(key));return Err(Error::Store(tl3_store::Error::Invalid("injected intern select failure")));}
        let existing=with(|r|r.run.pending_intern.get(&key).copied());
        call("intern.select",json!({"key":key,"result":existing}));
        if let Some(id)=existing{return Ok(InternId(id as i64));}
        let(index,failed)=begin_write("intern.insert",json!(key));
        let result=if failed{None}else{Some(with(|r|{let id=r.run.pending_intern.values().copied().max().unwrap_or(0)+1;r.run.pending_intern.insert(key,id);id}))};
        call("intern.result",json!(result));end_write(index);
        result.map(|id|InternId(id as i64)).ok_or(Error::Store(tl3_store::Error::Invalid("injected intern insert failure")))
    }
}
pub(super) struct Commit{pub rows:Rows}
pub(super) struct Rows;
impl Rows {pub fn push(&mut self,row:RowChange){
    let value=match row{RowChange::Put{table,id,value}=>json!({"put":{"table":table,"id":id,"value":value}}),RowChange::Delete{table,id}=>json!({"delete":{"table":table,"id":id}})};
    call("row.push",value.clone());with(|r|r.run.staged_rows.push(value));
}}
pub(super) fn put(table:&str,id:String,value:Value)->RowChange{RowChange::Put{table:table.into(),id,value}}
fn persist()->Result<()>{
    let(index,failed)=begin_write("transaction.commit",Value::Null);
    if !failed{with(|r|{r.run.persisted_intern=r.run.pending_intern.clone();for row in &r.run.staged_rows{
        if let Some(p)=row.get("put"){r.run.persisted_rows.insert((p["table"].as_str().unwrap().into(),p["id"].as_str().unwrap().into()),p["value"].clone());}
        else{let d=&row["delete"];r.run.persisted_rows.remove(&(d["table"].as_str().unwrap().into(),d["id"].as_str().unwrap().into()));}
    }});}
    end_write(index);if failed{Err(Error::Store(tl3_store::Error::Invalid("injected commit failure")))}else{Ok(())}
}
fn row(table:&str,id:String,value:Value)->WorldRow{WorldRow{table:table.into(),id,value,version:Revision(0),arrival:0}}
fn snapshot(step:&Step)->Snapshot{
    let mut rows=vec![];if let Some(v)=&step.request{rows.push(row("requests",step.node.to_string(),v.clone()));}
    rows.extend(step.prior.iter().map(|(id,v)|row("check_reports",id.clone(),v.clone())));
    Snapshot{revision:Revision(0),epoch:Epoch(0),rows,pending_effects:vec![],credit:vec![],
        interned:step.snapshot_head.as_ref().map(|s|vec![Interned{id:2,kind:InternKind::Commit,scope:"0".into(),value:s.clone()}]).unwrap_or_default()}
}
fn run(s:&Schedule,new:bool,cut:Option<Cut>)->Run{
    // Catch both injected crashes and the old CheckPlan malformed-findings panic.
    // The panic payload is compared, not converted into a successful result.
    ACTIVE.with(|r|*r.borrow_mut()=Some(Recording{run:Run{persisted_intern:s.interned.clone(),pending_intern:s.interned.clone(),persisted_rows:s.initial_rows.clone(),..Run::default()},cut,reads:s.reads.clone(),read_index:0,failures:s.failures.clone(),select_failures:s.select_failures.clone(),select_index:0}));
    let result=catch_unwind(AssertUnwindSafe(||{
        let mut controller=Controller{store:Store};
        for step in &s.steps{
            with(|r|{r.run.pending_intern=r.run.persisted_intern.clone();r.run.staged_rows.clear();});
            let mut world=World::new(&[],1);
            let footprint=if step.invalid_footprint{
                let mut foreign=World::new(&[],1);foreign.intern_footprint([FileId(100)])
            }else{world.intern_footprint(step.footprint.iter().copied().map(FileId))};
            if step.present{let mut n=Node::new(RepoId(0));n.adopted=step.adopted;n.phase=if step.rebasing{Phase::Rebasing}else{Phase::Checking};n.generation=step.current;n.job=step.job;n.head=Some(HeadId(if step.same_head{2}else{99}));n.footprint=footprint;world.nodes.push(n);}
            let snapshot=snapshot(step);
            let observation=match step.arm{
                0=>Observation::WorktreePrepared{node:NodeId(step.node),repository:"/new-repository".into(),base_sha:step.base.clone(),sha:step.sha.clone()},
                1=>Observation::CheckPlan{node:NodeId(step.node),sha:step.sha.clone(),run:step.run.clone()},
                _=>Observation::Checks{node:NodeId(step.node),generation:step.observed,sha:step.sha.clone(),changed_paths:step.changed.clone(),failed_tests:step.failed.clone(),report:"check report".into(),pass:step.pass,results:step.results.clone(),base_sha:step.base.clone(),action_key:Some("action".into())},
            };
            let mut commit=Commit{rows:Rows};let mut feedback=Feedback::default();
            let translate=match(new,step.arm){
                (false,0)=>prepared_reference::translate,(true,0)=>prepared_current::translate,
                (false,1)=>plan_reference::translate,(true,1)=>plan_current::translate,
                (false,_)=>checks_reference::translate,(true,_)=>checks_current::translate,
            };
            match translate(&mut controller,&snapshot,&world,observation,&mut commit,&mut feedback){
                Ok(event)=>{call("translation.result",json!({"event":event,"feedback":feedback.value()}));match persist(){Ok(())=>with(|r|r.run.results.push(json!({"event":event,"feedback":feedback.value()}))),Err(e)=>{with(|r|r.run.results.push(json!({"error":e.to_string()})));break;}}},
                Err(e)=>{with(|r|r.run.results.push(json!({"error":e.to_string()})));break;}
            }
        }
    }));
    if let Err(e)=result{
        let value=if e.is::<Crash>(){json!({"crash":true})}else if let Some(s)=e.downcast_ref::<String>(){json!({"panic":s})}else if let Some(s)=e.downcast_ref::<&str>(){json!({"panic":s})}else{std::panic::resume_unwind(e)};
        with(|r|r.run.results.push(value));
    }
    ACTIVE.with(|r|r.borrow_mut().take().unwrap().run)
}
pub(super) fn compare(s:&Schedule)->Run{
    let old=run(s,false,None);assert_eq!(old,run(s,true,None),"schedule {s:?}");
    for write in 0..old.writes{for after in [false,true]{let cut=Some(Cut{write,after});let old_cut=run(s,false,cut);assert_eq!(old_cut,run(s,true,cut),"cut {cut:?}; schedule {s:?}");assert_eq!(old_cut.results.last().unwrap(),&json!({"crash":true}));}}
    old
}
