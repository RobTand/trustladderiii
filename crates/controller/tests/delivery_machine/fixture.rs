struct Controller {
    store:Store,
    adapters:Adapters,
    held_effects:BTreeMap<Upstream,BTreeMap<String,String>>,
}
impl Controller {
    fn observe(&mut self,key:&str,observation:Observation)->Result<()> {
        Recorded::write(&self.store.io,"observe",json!({"key":key,"observation":observation}),true);Ok(())
    }
    fn failure_allowance(&self,_s:&Snapshot,key:&str,class:&str,c:&mut Commit)->Result<()> {
        let sampled=Recorded::call(&self.store.io,"allowance",json!([key,class]));
        c.rows.push(put("allowances",key.into(),json!({"class":class,"sample":sampled})));Ok(())
    }
    fn apply(&mut self,s:&Snapshot,_world:(),event:Event,c:&mut Commit)->Result<()> {
        let sample=Recorded::call(&self.store.io,"kernel.apply",json!(event));
        c.rows.push(put("kernel", "event".into(),json!({"event":event,"sample":sample})));
        if let Event::Ack {ack:Ack::Retryable,..}=event {
            let mut d:Delivery=decode_stored(s.pending_effects[0].payload.clone())?;
            d.key="replacement".into();
            c.effects.push(NewEffect {key:d.key.clone(),kind:"GitHub".into(),payload:json!(d)});
        }
        Ok(())
    }
    fn postmortem_failed(&mut self,_s:&Snapshot,d:&Delivery,class:&str,c:&mut Commit)->Result<()> {
        c.rows.push(put("postmortems",d.key.clone(),json!(class)));Ok(())
    }
    fn failed_notice(&mut self,_s:&Snapshot,d:&Delivery,class:&str,c:&mut Commit)->Result<()> {
        c.rows.push(put("notice_failure",d.key.clone(),json!(class)));Ok(())
    }
    fn record_progress(&mut self,_s:&Snapshot,c:&mut Commit,key:&str,progress:Option<&Value>)->Result<()> {
        c.rows.push(put("progress",key.into(),json!(progress)));Ok(())
    }
    fn queue_observation(&mut self,_s:&Snapshot,c:&mut Commit,key:&str,index:usize,o:&Observation)->Result<()> {
        c.rows.push(put("inbox",format!("{key}:{index}"),json!(o)));Ok(())
    }
    fn finish_maintenance(&mut self,_s:&Snapshot,key:&str,c:&mut Commit)->Result<()> {
        c.rows.push(put("maintenance",key.into(),json!("finished")));Ok(())
    }
    fn maintenance_failure(&mut self,_s:&Snapshot,d:&Delivery,key:&str,class:&str,_error:Error,c:&mut Commit)->Result<()> {
        let value=Recorded::call(&self.store.io,"maintenance_failure",json!([d.key,key,class]));
        c.rows.push(put("maintenance",key.into(),json!(value)));Ok(())
    }
    fn consume_kernel_input(&mut self,s:&Snapshot,row:&WorldRow)->Result<bool> {
        Recorded::write(&self.store.io,"input.begin",json!(null),false);
        self.store.active=true;
        let input:input_errors::PendingKernelInput=decode_stored(row.value.clone())?;
        let attempted=match input {
            input_errors::PendingKernelInput::Completion {key,completion}=>self.prepare_completion(s,&key,completion),
            input_errors::PendingKernelInput::CancelAbsent {key}=>{
                let effect=s.pending_effects.iter().find(|e|e.key==key).ok_or(Error::Invalid("missing cancellation"))?;
                let d=decode_stored(effect.payload.clone())?;
                self.prepare_cancel_absent(s,effect,&d)
            }
        };
        let accepted=match attempted {
            Ok(mut c)=>{c.rows.push(RowChange::Delete {table:row.table.clone(),id:row.id.clone()});self.store.commit(&c)?;true},
            Err(_)=>false,
        };
        let staged=std::mem::take(&mut self.store.staged);
        if accepted {for (key,value) in staged {self.store.io.borrow_mut().durable.insert(key,value);}}
        self.store.active=false;
        Recorded::write(&self.store.io,"input.finish",json!(accepted),accepted);
        Ok(accepted)
    }
    fn reject_kernel_input(&mut self,_s:&Snapshot,_row:&WorldRow,_error:Error)->Result<()> {Ok(())}
}
fn counter_incident(_r:&WorldRow)->bool {false}

pub(super) fn run(io:Io,scenario:u8)->Result<()> {
    let mut c=Controller {store:Store {io:io.clone(),pending:vec![],active:false,staged:vec![]},adapters:Adapters {io},held_effects:BTreeMap::new()};
    match scenario {
        0=>c.complete("original",completion(Recorded::call(&c.store.io,"completion",json!(null)))),
        1=>{let snapshot=c.store.load()?;let effect=&snapshot.pending_effects[0];c.transition(effect,EffectState::Sent)},
        2=>{let snapshot=c.store.load()?;let effect=&snapshot.pending_effects[0];let d=decode_stored(effect.payload.clone())?;c.cancel_absent(effect,&d)},
        3=>{let snapshot=c.store.load()?;let row=WorldRow {table:"retiring_effects".into(),id:"original".into(),value:snapshot.pending_effects[0].payload.clone(),version:snapshot.revision,arrival:0};c.retire_effect(&snapshot,&row)},
        4=>{let snapshot=c.store.load()?;let incident=looks::Incident {signature:looks::Signature {class:approval_delivery::AMBIGUOUS.into()},open:true,effect:Some("original".into())};let mut change=Commit::new("clear",json!(true),&snapshot);c.authorize_approval_retry(&snapshot,&incident,&mut change);c.store.commit(&change)},
        _=>{let snapshot=c.store.load()?;c.reconcile_approvals(&snapshot).map(|_|())}
    }
}

impl Controller {
    pub(crate) fn submit_kernel_input(
        &mut self,
        id: String,
        input: PendingKernelInput,
    ) -> Result<bool> {
        let snapshot = self.store.load()?;
        let mut queued = Commit::new(format!("queue:{id}"), json!(&input), &snapshot);
        queued
            .rows
            .push(put(PENDING_KERNEL, id.clone(), json!(input)));
        self.store.commit(&queued)?;
        let current = self.store.load()?;
        if current.rows.iter().any(counter_incident) {
            return Ok(true);
        }
        // Redelivery may retry the same retained head after a crash. Every
        // independent newer producer must still wait for that head to finish.
        if let Some(row) = current
            .rows
            .iter()
            .filter(|r| r.table == "inbox" || r.table == PENDING_KERNEL)
            .min_by_key(|r| r.arrival)
            && row.table == PENDING_KERNEL
            && row.id == id
            && let Err(error) = self.consume_kernel_input(&current, row)
        {
            self.reject_kernel_input(&current, row, error)?;
            return Ok(false);
        }
        Ok(true)
    }

}
