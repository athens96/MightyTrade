pub mod json_store;
pub mod sqlite_store;

use serde_json::Value;
use std::path::Path;
use std::sync::Arc;

pub type Record = Value;

/// RAII cross-process + cross-thread named lock guard.
pub trait LockGuard: Send {}

pub trait Store: Send + Sync {
    fn get(&self, collection: &str, id: &str) -> Option<Record>;
    fn put(&self, collection: &str, id: &str, record: Record) -> Record;
    fn delete(&self, collection: &str, id: &str) -> bool;
    fn find_all(&self, collection: &str) -> Vec<Record>;
    fn lock(&self, name: &str) -> Box<dyn LockGuard>;

    fn find_where<F>(&self, collection: &str, predicate: F) -> Vec<Record>
    where
        F: Fn(&Record) -> bool,
        Self: Sized,
    {
        self.find_all(collection)
            .into_iter()
            .filter(|r| predicate(r))
            .collect()
    }
}

pub fn find_where<F>(store: &dyn Store, collection: &str, predicate: F) -> Vec<Record>
where
    F: Fn(&Record) -> bool,
{
    store
        .find_all(collection)
        .into_iter()
        .filter(|r| predicate(r))
        .collect()
}

pub fn find_one<F>(store: &dyn Store, collection: &str, predicate: F) -> Option<Record>
where
    F: Fn(&Record) -> bool,
{
    store.find_all(collection).into_iter().find(|r| predicate(r))
}

pub fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(format!("invalid name: {name}"));
    }
    Ok(())
}

pub fn init_storage(backend: &str, data_dir: &Path) -> Result<Arc<dyn Store>, String> {
    match backend {
        "json" => Ok(Arc::new(json_store::JsonFileStore::new(data_dir)?)),
        "sqlite" => Ok(Arc::new(sqlite_store::SqliteStore::new(data_dir, "mighty_trade.db")?)),
        "supabase" => Err("Supabase store is not implemented yet".to_string()),
        other => Err(format!("Unknown STORAGE_BACKEND: {other}")),
    }
}
