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
    completion: Completion,
    #[serde(default)]
    note: Option<String>,
}

pub(super) struct Records(PathBuf);
impl Records {
    pub fn new(root: &Path) -> Self {
        Self(root.to_owned())
    }
    fn path(&self, key: &str) -> PathBuf {
        let encoded: String = key.bytes().map(|b| format!("{b:02x}")).collect();
        self.0.join(format!("{encoded}.json"))
    }
    pub fn reconcile(&self, delivery: &Delivery) -> Result<Reconciliation> {
        match fs::read(self.path(&delivery.key)) {
            Ok(bytes) => Ok(Reconciliation::Found(
                serde_json::from_slice::<Record>(&bytes)?.completion,
            )),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Reconciliation::Absent),
            Err(e) => Err(e.into()),
        }
    }
    pub fn deliver(&self, delivery: &Delivery, completion: Completion) -> Result<Completion> {
        let path = self.path(&delivery.key);
        let mut record = match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice::<Record>(&bytes)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Record {
                key: delivery.key.clone(),
                deliveries: 0,
                completion,
                note: delivery.note.clone(),
            },
            Err(e) => return Err(e.into()),
        };
        record.deliveries += 1;
        let temporary = path.with_extension("tmp");
        let mut file = File::create(&temporary)?;
        file.write_all(&serde_json::to_vec(&record)?)?;
        file.sync_all()?;
        fs::rename(temporary, path)?;
        File::open(&self.0)?.sync_all()?;
        Ok(record.completion)
    }
}

pub fn delivery_counts(root: &Path) -> Result<BTreeMap<String, u64>> {
    let mut counts = BTreeMap::new();
    for entry in fs::read_dir(root)? {
        let path = entry?.path();
        if path.extension().is_some_and(|x| x == "json") {
            let record: Record = serde_json::from_slice(&fs::read(path)?)?;
            counts.insert(record.key, record.deliveries);
        }
    }
    Ok(counts)
}

pub fn delivered_notes(root: &Path) -> Result<Vec<String>> {
    let mut notes = vec![];
    for entry in fs::read_dir(root)? {
        let path = entry?.path();
        if path.extension().is_some_and(|x| x == "json") {
            let record: Record = serde_json::from_slice(&fs::read(path)?)?;
            if let Some(note) = record.note {
                notes.push(note);
            }
        }
    }
    Ok(notes)
}
