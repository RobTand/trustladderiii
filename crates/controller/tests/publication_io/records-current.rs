use crate::{
    Result,
    executor::{Completion, Delivery, Reconciliation},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Serialize, Deserialize)]
struct Record {
    key: String,
    deliveries: u64,
    #[serde(default)]
    epochs: Vec<i64>,
    completion: Completion,
    #[serde(default)]
    note: Option<String>,
}

pub(super) struct Records(PathBuf);
impl Records {
    pub fn new(root: &Path) -> Self {
        Self(root.to_owned())
    }
    pub(super) fn path(&self, key: &str) -> PathBuf {
        let encoded: String = key.bytes().map(|b| format!("{b:02x}")).collect();
        self.0.join(format!("{encoded}.json"))
    }
    pub fn reconcile(&self, delivery: &Delivery) -> Result<Reconciliation> {
        match fs::read(self.path(&delivery.key)) {
            Ok(bytes) => Ok(Reconciliation::Found(Box::new(
                serde_json::from_slice::<Record>(&bytes)?.completion,
            ))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Reconciliation::Absent),
            Err(e) => Err(e.into()),
        }
    }
    pub fn contract_evidence(
        &self,
        delivery: &Delivery,
    ) -> Result<crate::dispatch_liveness::decision::ContractEvidence> {
        use crate::dispatch_liveness::decision::ContractEvidence;
        Ok(match self.reconcile(delivery)? {
            Reconciliation::Found(_) => ContractEvidence::Ending,
            Reconciliation::Absent | Reconciliation::Pending => ContractEvidence::Absent,
        })
    }
    pub fn fence(&self, incoming: i64) -> Result<()> {
        let fence = self.0.join("epoch.fence");
        let epoch = match fs::read(&fence) {
            Ok(bytes) => serde_json::from_slice::<i64>(&bytes)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => 0,
            Err(e) => return Err(e.into()),
        };
        let order = match incoming.cmp(&epoch) {
            std::cmp::Ordering::Less => tl3_delivery::EpochOrder::Older,
            std::cmp::Ordering::Equal => tl3_delivery::EpochOrder::Current,
            std::cmp::Ordering::Greater => tl3_delivery::EpochOrder::Newer,
        };
        if !tl3_delivery::accepts_fence(order) {
            return Err(crate::Error::Adapter(Box::new(std::io::Error::other(
                "upstream refused an older controller epoch",
            ))));
        }
        if tl3_delivery::advances_fence(order) || !fence.exists() {
            self.write(&fence, &serde_json::to_vec(&incoming)?)?;
        }
        Ok(())
    }
    pub fn deliver(&self, delivery: &Delivery, completion: Completion) -> Result<Completion> {
        self.fence(delivery.epoch)?;
        self.remember(delivery, completion)
    }

    pub fn remember(&self, delivery: &Delivery, completion: Completion) -> Result<Completion> {
        let path = self.path(&delivery.key);
        let mut record = match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice::<Record>(&bytes)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Record {
                key: delivery.key.clone(),
                deliveries: 0,
                epochs: Vec::new(),
                completion,
                note: delivery.note.clone(),
            },
            Err(e) => return Err(e.into()),
        };
        record.deliveries += 1;
        record.epochs.push(delivery.epoch);
        self.write(&path, &serde_json::to_vec(&record)?)?;
        Ok(record.completion)
    }

    pub(super) fn write(&self, path: &Path, bytes: &[u8]) -> Result<()> {
        let temporary = path.with_extension("tmp");
        let mut file = File::create(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(temporary, path)?;
        File::open(&self.0)?.sync_all()?;
        Ok(())
    }
}
