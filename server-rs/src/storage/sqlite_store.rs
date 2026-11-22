use fs2::FileExt;
use parking_lot::{Mutex, MutexGuard};
use rusqlite::{params, Connection};
use serde_json::Value;
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use super::{validate_name, LockGuard, Record, Store};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS records (
    collection TEXT NOT NULL,
    id         TEXT NOT NULL,
    data       TEXT NOT NULL,
    PRIMARY KEY (collection, id)
);
CREATE INDEX IF NOT EXISTS idx_records_collection ON records(collection);
";

struct NamedFileLockGuard {
    file: File,
    _mem: Arc<Mutex<()>>,
    _guard: Option<MutexGuard<'static, ()>>,
}

unsafe impl Send for NamedFileLockGuard {}

impl Drop for NamedFileLockGuard {
    fn drop(&mut self) {
        let _ = self.file.unlock();
        self._guard.take();
    }
}

impl LockGuard for NamedFileLockGuard {}

pub struct SqliteStore {
    dir: PathBuf,
    conn: Mutex<Connection>,
    named_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
}

impl SqliteStore {
    pub fn new(data_dir: &Path, filename: &str) -> Result<Self, String> {
        fs::create_dir_all(data_dir).map_err(|e| format!("create_dir_all failed: {e}"))?;
        let db_path = data_dir.join(filename);
        let conn = Connection::open(&db_path).map_err(|e| format!("sqlite open failed: {e}"))?;
        conn.busy_timeout(Duration::from_secs(10))
            .map_err(|e| format!("sqlite busy_timeout failed: {e}"))?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             PRAGMA busy_timeout=10000;",
        )
        .map_err(|e| format!("sqlite pragma failed: {e}"))?;
        conn.execute_batch(SCHEMA)
            .map_err(|e| format!("sqlite schema failed: {e}"))?;
        Ok(Self {
            dir: data_dir.to_path_buf(),
            conn: Mutex::new(conn),
            named_locks: Mutex::new(HashMap::new()),
        })
    }
}

impl Store for SqliteStore {
    fn get(&self, collection: &str, id: &str) -> Option<Record> {
        validate_name(collection).expect("valid collection");
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare_cached("SELECT data FROM records WHERE collection = ?1 AND id = ?2")
            .ok()?;
        let raw: String = stmt.query_row(params![collection, id], |r| r.get(0)).ok()?;
        serde_json::from_str(&raw).ok()
    }

    fn put(&self, collection: &str, id: &str, mut record: Record) -> Record {
        validate_name(collection).expect("valid collection");
        if let Some(obj) = record.as_object_mut() {
            obj.insert("id".to_string(), Value::String(id.to_string()));
        }
        let raw = serde_json::to_string(&record).expect("serialize record");
        let conn = self.conn.lock();
        conn.execute(
            "INSERT OR REPLACE INTO records(collection, id, data) VALUES (?1, ?2, ?3)",
            params![collection, id, raw],
        )
        .expect("sqlite put");
        record
    }

    fn delete(&self, collection: &str, id: &str) -> bool {
        validate_name(collection).expect("valid collection");
        let conn = self.conn.lock();
        let changed = conn
            .execute(
                "DELETE FROM records WHERE collection = ?1 AND id = ?2",
                params![collection, id],
            )
            .expect("sqlite delete");
        changed > 0
    }

    fn find_all(&self, collection: &str) -> Vec<Record> {
        validate_name(collection).expect("valid collection");
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare_cached("SELECT data FROM records WHERE collection = ?1")
            .expect("sqlite prepare find_all");
        let rows = stmt
            .query_map(params![collection], |r| r.get::<_, String>(0))
            .expect("sqlite query find_all");
        rows.filter_map(|r| r.ok())
            .filter_map(|s| serde_json::from_str::<Value>(&s).ok())
            .collect()
    }

    fn lock(&self, name: &str) -> Box<dyn LockGuard> {
        validate_name(name).expect("valid lock name");
        let arc = {
            let mut map = self.named_locks.lock();
            map.entry(name.to_string())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        let guard_ref: MutexGuard<'static, ()> = unsafe {
            let ptr: *const Mutex<()> = Arc::as_ptr(&arc);
            (*ptr).lock()
        };
        let lock_path = self.dir.join(format!(".lock_{name}.lock"));
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&lock_path)
            .expect("open lock file");
        file.lock_exclusive().expect("flock exclusive");
        Box::new(NamedFileLockGuard {
            file,
            _mem: arc,
            _guard: Some(guard_ref),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sqlite_crud_and_lock() {
        let td = tempfile::tempdir().unwrap();
        let store = SqliteStore::new(td.path(), "test.db").unwrap();
        let saved = store.put("users", "u1", json!({"email": "a@b.com"}));
        assert_eq!(saved["id"], "u1");
        assert_eq!(store.get("users", "u1").unwrap()["email"], "a@b.com");
        assert_eq!(store.find_all("users").len(), 1);
        {
            let _g = store.lock("tok_test");
        }
        assert!(store.delete("users", "u1"));
        assert!(!store.delete("users", "u1"));
        assert!(store.get("users", "u1").is_none());
    }
}
