use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use uuid::Uuid;

use crate::models::{normalize_name, CreateRecord, Record, RecordType};

#[derive(Debug, Clone, Default)]
pub struct RecordStore {
    records: HashMap<String, Record>,
}

impl RecordStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, req: CreateRecord) -> Record {
        let record = Record {
            id: Uuid::new_v4().to_string(),
            name: normalize_name(&req.name),
            record_type: req.record_type,
            ttl: req.ttl,
            data: req.data,
        };
        self.records.insert(record.id.clone(), record.clone());
        record
    }

    pub fn get(&self, id: &str) -> Option<Record> {
        self.records.get(id).cloned()
    }

    pub fn list_filtered(&self, name: Option<&str>, rtype: Option<RecordType>) -> Vec<Record> {
        self.records
            .values()
            .filter(|r| name.is_none_or(|n| r.name == normalize_name(n)))
            .filter(|r| rtype.is_none_or(|t| r.record_type == t))
            .cloned()
            .collect()
    }

    /// Look up DNS records matching a canonical name + type.
    pub fn lookup(&self, name: &str, rtype: RecordType) -> Vec<Record> {
        let canonical = normalize_name(name);
        self.records
            .values()
            .filter(|r| r.name == canonical && r.record_type == rtype)
            .cloned()
            .collect()
    }

    pub fn delete(&mut self, id: &str) -> bool {
        self.records.remove(id).is_some()
    }

    pub fn delete_all(&mut self) {
        self.records.clear();
    }

    pub fn delete_filtered(&mut self, name: Option<&str>, rtype: Option<RecordType>) {
        let to_remove: Vec<String> = self
            .records
            .values()
            .filter(|r| name.is_none_or(|n| r.name == normalize_name(n)))
            .filter(|r| rtype.is_none_or(|t| r.record_type == t))
            .map(|r| r.id.clone())
            .collect();
        for id in to_remove {
            self.records.remove(&id);
        }
    }
}

/// Shared, thread-safe wrapper around `RecordStore`.
#[derive(Clone)]
pub struct SharedStore {
    inner: Arc<RwLock<RecordStore>>,
}

impl SharedStore {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(RecordStore::new())),
        }
    }

    pub fn add(&self, req: CreateRecord) -> Record {
        self.inner.write().unwrap().add(req)
    }

    pub fn get(&self, id: &str) -> Option<Record> {
        self.inner.read().unwrap().get(id)
    }

    pub fn list_filtered(&self, name: Option<&str>, rtype: Option<RecordType>) -> Vec<Record> {
        self.inner.read().unwrap().list_filtered(name, rtype)
    }

    pub fn lookup(&self, name: &str, rtype: RecordType) -> Vec<Record> {
        self.inner.read().unwrap().lookup(name, rtype)
    }

    pub fn delete(&self, id: &str) -> bool {
        self.inner.write().unwrap().delete(id)
    }

    pub fn delete_all(&self) {
        self.inner.write().unwrap().delete_all()
    }

    pub fn delete_filtered(&self, name: Option<&str>, rtype: Option<RecordType>) {
        self.inner.write().unwrap().delete_filtered(name, rtype)
    }
}
