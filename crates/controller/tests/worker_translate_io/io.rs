use super::*;
use std::{cell::RefCell, panic::{catch_unwind, AssertUnwindSafe}};
use tl3_controller::adapters::prismabuild::CheckRun;
use tl3_kernel::{Node, Phase, RepoId};
use tl3_store::{Epoch, Revision, WorldRow, Interned, InternKind};

pub(super) const OLD_HEAD:&str="1111111111111111111111111111111111111111";
pub(super) const NEW_HEAD:&str="2222222222222222222222222222222222222222";
#[derive(Clone, Debug)]
pub(super) enum Read { Value(Option<Value>), Store, Json }
#[derive(Clone, Debug)]
pub(super) struct Schedule {
    pub observation:Observation,
    pub phase:Phase,
    pub run:Option<u64>,
    pub generation:u64,
    pub missing:bool,
    pub head:Option<u64>,
    pub round:u8,
    pub snapshot:Snapshot,
    pub documents:Vec<Read>,
    pub materials:Vec<Read>,
    pub plans:Vec<Read>,
    pub protected:Vec<u8>,
    pub prerequisites:Vec<u8>,
    pub intern_failures:Vec<bool>,
}
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Run {
    pub calls:Vec<Value>,
    pub rows:Vec<Value>,
    pub result:Value,
    pub pending:BTreeMap<String,i64>,
    pub persisted:BTreeMap<String,i64>,
    pub committed:Vec<Value>,
    pub world:Value,
    pub writes:usize,
}
#[derive(Clone, Copy)]
struct Cut { write:usize, after:bool }
#[derive(Debug)] struct Crash;
struct Recording {
    run:Run, schedule:Schedule, cut:Option<Cut>,
    documents:usize, materials:usize, plans:usize, protected:usize, prerequisites:usize,
}
thread_local! {static ACTIVE:RefCell<Option<Recording>> = const {RefCell::new(None)};}
fn with<T>(f:impl FnOnce(&mut Recording)->T)->T {ACTIVE.with(|r|f(r.borrow_mut().as_mut().unwrap()))}
fn call(op:&str,args:Value){with(|r|r.run.calls.push(json!({"op":op,"args":args})));}
fn read(op:&str,args:Value)->Result<Option<Value>> {
    let item=with(|r|{
        let (tape,index)=match op {
            "worker_document"=>(&r.schedule.documents,&mut r.documents),
            "worker_materials"=>(&r.schedule.materials,&mut r.materials),
            "check_run"=>(&r.schedule.plans,&mut r.plans),
            _=>unreachable!(),
        };
        let result=tape.get(*index).cloned().unwrap_or(Read::Value(None)); *index+=1;result
    });
    call(op,json!({"args":args,"result":format!("{item:?}")}));
    match item {
        Read::Value(v)=>Ok(v),
        Read::Store=>Err(Error::Store(tl3_store::Error::Invalid("injected read failure"))),
        Read::Json=>Err(Error::StoredJson(serde_json::from_str::<Value>("{").unwrap_err())),
    }
}
pub(super) struct Store;
#[derive(Clone, Copy, Serialize)]
pub(super) struct Id(i64);
impl Id {pub fn get(self)->i64 {self.0}}
impl Store {
    pub fn worker_document(&mut self,node:&str,kind:&str)->Result<Option<Value>> {read("worker_document",json!([node,kind]))}
    pub fn worker_materials(&mut self,node:&str)->Result<Option<Value>> {read("worker_materials",json!([node]))}
    fn intern(&mut self,kind:&str,scope:&str,value:&str)->std::result::Result<Id,tl3_store::Error> {
        let key=format!("{kind}:{scope}:{value}");
        let existing=with(|r|r.run.pending.get(&key).copied());
        call("intern.select",json!({"kind":kind,"scope":scope,"value":value,"result":existing}));
        if let Some(id)=existing {return Ok(Id(id));}
        with(|r|{
            let write=r.run.writes; r.run.writes+=1;
            r.run.calls.push(json!({"op":"intern.insert","kind":kind,"scope":scope,"value":value}));
            if r.cut.is_some_and(|c|c.write==write&&!c.after){std::panic::panic_any(Crash);}
            let id=if r.schedule.intern_failures.get(write).copied().unwrap_or(false) {None} else {
                let id=r.run.pending.len() as i64+100; r.run.pending.insert(key,id);Some(id)
            };
            r.run.calls.push(json!({"op":"intern.result","id":id}));
            if r.cut.is_some_and(|c|c.write==write&&c.after){std::panic::panic_any(Crash);}
            id.map(Id).ok_or(tl3_store::Error::Invalid("injected intern failure"))
        })
    }
    pub fn intern_commit(&mut self,scope:&str,sha:&str)->std::result::Result<Id,tl3_store::Error>{self.intern("commit",scope,sha)}
    pub fn intern_repo(&mut self,owner:&str,name:&str)->std::result::Result<Id,tl3_store::Error>{self.intern("repo",owner,name)}
    pub fn intern_path(&mut self,repo:Id,path:&str)->std::result::Result<Id,tl3_store::Error>{self.intern("path",&repo.get().to_string(),path)}
}
pub(super) struct Controller {pub store:Store}
pub(super) type World=tl3_kernel::World;
pub(super) struct Commit {pub rows:Rows}
pub(super) struct Rows;
impl Rows {pub fn push(&mut self,row:RowChange){let value=serde_json::to_value(row).unwrap();call("row.push",value.clone());with(|r|r.run.rows.push(value));}}
pub(super) fn put(table:&str,id:String,value:Value)->RowChange {RowChange::Put{table:table.into(),id,value}}
pub(super) fn check_run(_: &Snapshot,node:NodeId,head:Option<&str>)->Result<Option<CheckRun>> {
    read("check_run",json!([node,head]))?.map(serde_json::from_value).transpose().map_err(Error::StoredJson)
}
impl Controller {
    pub fn protected_reply(&mut self,_:&Snapshot,commit:&mut Commit,node:NodeId,role:&str,reply:&Value)->Result<Option<Reply>> {
        let result=with(|r|{let v=r.schedule.protected.get(r.protected).copied().unwrap_or(0);r.protected+=1;v});
        call("protected_reply",json!({"node":node,"role":role,"reply":reply,"result":result}));
        match result {0=>Ok(None),1=>{commit.rows.push(put("close_reasons",node.0.to_string(),json!({"reason":"protected_paths","source":role})));Ok(Some(Reply::ProtectedPaths))},_=>Err(Error::Invalid("injected protection failure"))}
    }
    pub fn prepare_merge_dependencies(&mut self,_:&Snapshot,_:&World,node:NodeId,reply:&Value,commit:&mut Commit)->Result<()> {
        let result=with(|r|{let v=r.schedule.prerequisites.get(r.prerequisites).copied().unwrap_or(0);r.prerequisites+=1;v});
        call("prepare_merge_dependencies",json!({"node":node,"reply":reply,"result":result}));
        if result==1{return Err(Error::Invalid("injected prerequisite failure"));}
        commit.rows.push(put("merge_prerequisites",node.0.to_string(),reply.get("merge_prerequisites").cloned().unwrap_or(json!([]))));Ok(())
    }
}
fn execute(schedule:&Schedule,current:bool,cut:Option<Cut>)->Run {
    let mut world=World::new(&[4],4);
    let mut node=Node::new(RepoId(0)); node.phase=schedule.phase; node.run=schedule.run;
    node.generation=schedule.generation;node.head=schedule.head.map(HeadId);node.round=schedule.round;
    node.footprint=world.intern_footprint(vec![tl3_kernel::FileId(1)]);
    if !schedule.missing {world.nodes.push(node);}
    let pending=schedule.snapshot.interned.iter().map(|i|(format!("{:?}:{}:{}",i.kind,i.scope,i.value).to_lowercase(),i.id)).collect();
    ACTIVE.with(|r|*r.borrow_mut()=Some(Recording{run:Run{calls:vec![],rows:vec![],result:Value::Null,pending,persisted:BTreeMap::new(),committed:vec![],world:Value::Null,writes:0},schedule:schedule.clone(),cut,documents:0,materials:0,plans:0,protected:0,prerequisites:0}));
    let result=catch_unwind(AssertUnwindSafe(||{
        let mut controller=Controller{store:Store};let mut commit=Commit{rows:Rows};
        if current {current::translate(&mut controller,&schedule.snapshot,&mut world,schedule.observation.clone(),&mut commit)}
        else {reference::translate(&mut controller,&schedule.snapshot,&mut world,schedule.observation.clone(),&mut commit)}
    }));
    let mut recording=ACTIVE.with(|r|r.borrow_mut().take().unwrap());
    recording.run.world=json!(world);
    recording.run.result=match result {
        Ok(Ok((event,detail)))=>{recording.run.persisted=recording.run.pending.clone();recording.run.committed=recording.run.rows.clone();json!({"event":event,"detailed_review":detail})},
        Ok(Err(error))=>json!({"error":format!("{error:?}")}),
        Err(error) if error.is::<Crash>()=>json!({"crash":true}),
        Err(error)=>std::panic::resume_unwind(error),
    };
    recording.run
}
pub(super) fn compare(s:&Schedule)->Run {
    let old=execute(s,false,None);let new=execute(s,true,None);assert_eq!(old,new,"schedule {s:?}");
    for write in 0..old.writes {for after in [false,true] {
        let cut=Some(Cut{write,after});assert_eq!(execute(s,false,cut),execute(s,true,cut),"write {write}, after {after}, schedule {s:?}");
    }}old
}
pub(super) fn schedule(role:&str,reply:Value)->Schedule {
    let phase=match role {"ingest"=>Phase::Ingesting,"audit"=>Phase::Auditing,"planner"=>Phase::Planning{must_split:false},"maker"=>Phase::Making,_=>Phase::Reviewing};
    let rows=vec![WorldRow{version:Revision::ZERO,arrival:0,table:"requests".into(),id:"0".into(),value:json!({"repo":{"owner":"safe","name":"repo","installation":1},"issue":1,"issue_id":1})},
        WorldRow{version:Revision::ZERO,arrival:0,table:"goals".into(),id:"0".into(),value:json!([{"id":"goal-1","description":"Do work","verification":"test"}])},
        WorldRow{version:Revision::ZERO,arrival:0,table:"model_quotas".into(),id:"model".into(),value:json!({"reset_ms":7})}];
    Schedule{observation:Observation::WorkerReply{node:NodeId(0),generation:7,role:role.into(),reply,model:Some("model".into())},phase,run:Some(7),generation:7,missing:false,head:Some(1),round:0,
        snapshot:Snapshot{revision:Revision::ZERO,epoch:Epoch::ZERO,rows,pending_effects:vec![],credit:vec![],interned:vec![Interned{kind:InternKind::Commit,id:1,scope:"0".into(),value:OLD_HEAD.into()}]},
        documents:vec![Read::Value(None)],materials:vec![Read::Value(Some(json!({"goals":[{"id":"goal-1"}],"standing_rules":[]})))],plans:vec![],protected:vec![],prerequisites:vec![],intern_failures:vec![]}
}
