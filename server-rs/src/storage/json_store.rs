use fs2::FileExt;
use parking_lot::{Mutex, MutexGuard};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tempfile::NamedTempFile;

use super::{validate_name, LockGuard, Record, Store};

struct FileLockGuard {
    file: File,
    _mem: Arc<Mutex<()>>,
    // Hold the lock via a raw box or keep lock inside struct safely:
    _guard: Option<MutexGuard<'static, ()>>,
}

// SAFETY: MutexGuard is dropped before Arc; lock is only released on Drop.
unsafe impl Send for FileLockGuard {}

impl Drop for FileLockGuard {
    fn drop(&mut self) {
        let _ = self.file.unlock();
        self._guard.take();
    }
}

impl LockGuard for FileLockGuard {}

pub struct JsonFileStore {
    dir: PathBuf,
    locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
}

impl JsonFileStore {
    pub fn new(data_dir: &Path) -> Result<Self, String> {
        fs::create_dir_all(data_dir).map_err(|e| format!("create_dir_all failed: {e}"))?;
        Ok(Self {
            dir: data_dir.to_path_buf(),
            locks: Mutex::new(HashMap::new()),
        })
    }

    fn path(&self, collection: &str) -> PathBuf {
        validate_name(collection).expect("valid collection name");
        self.dir.join(format!("{collection}.json"))
    }

    fn acquire(&self, key: &str) -> FileLockGuard {
        validate_name(key).expect("valid lock key");
        let arc = {
            let mut map = self.locks.lock();
            map.entry(key.to_string())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        // Acquire in-process mutex before OS flock so same-process threads serialize too.
        let guard_ref: MutexGuard<'static, ()> = unsafe {
            let ptr: *const Mutex<()> = Arc::as_ptr(&arc);
            (*ptr).lock()
        };
        let lock_path = self.dir.join(format!(".{key}.lock"));
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&lock_path)
            .expect("open lock file");
        file.lock_exclusive().expect("flock exclusive");
        FileLockGuard {
            file,
            _mem: arc,
            _guard: Some(guard_ref),
        }
    }

    fn read_map(&self, collection: &str) -> Map<String, Value> {
        let path = self.path(collection);
        if !path.exists() {
            return Map::new();
        }
        let raw = fs::read_to_string(&path).unwrap_or_default();
        match serde_json::from_str::<Value>(&raw) {
            Ok(Value::Object(m)) => m,
            _ => Map::new(),
        }
    }

    fn write_map(&self, collection: &str, data: &Map<String, Value>) {
        let path = self.path(collection);
        let mut tmp = NamedTempFile::new_in(&self.dir).expect("create temp json file");
        let body = serde_json::to_string_pretty(data).expect("serialize collection json");
        tmp.write_all(body.as_bytes()).expect("write temp json");
        tmp.persist(&path).expect("atomic persist json");
    }
}

impl Store for JsonFileStore {
    fn get(&self, collection: &str, id: &str) -> Option<Record> {
        let _g = self.acquire(collection);
        self.read_map(collection).get(id).cloned()
    }

    fn put(&self, collection: &str, id: &str, mut record: Record) -> Record {
        if let Some(obj) = record.as_object_mut() {
            obj.insert("id".to_string(), Value::String(id.to_string()));
        }
        let _g = self.acquire(collection);
        let mut map = self.read_map(collection);
        map.insert(id.to_string(), record.clone());
        self.write_map(collection, &map);
        record
    }

    fn delete(&self, collection: &str, id: &str) -> bool {
        let _g = self.acquire(collection);
        let mut map = self.read_map(collection);
        let existed = map.remove(id).is_some();
        if existed {
            self.write_map(collection, &map);
        }
        existed
    }

    fn find_all(&self, collection: &str) -> Vec<Record> {
        let _g = self.acquire(collection);
        self.read_map(collection).into_values().collect()
    }

    fn lock(&self, name: &str) -> Box<dyn LockGuard> {
        validate_name(name).expect("valid lock name");
        Box::new(self.acquire(&format!("lock_{name}")))
    }
}
