//! In-memory + on-disk storage for transfer history and known devices.
//!
//! LANdrop deliberately avoids a database dependency: history is a small,
//! append-mostly list, so a JSON file guarded by a `Mutex` (flushed after
//! every mutation) is simpler, has zero schema-migration surface, and is
//! trivial to inspect/back up by hand. If history grows large this is an
//! easy spot to swap in SQLite later without touching callers, since all
//! access goes through [`Store`].

use crate::models::{Device, TransferRecord};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use tracing::warn;
use uuid::Uuid;

const HISTORY_FILE: &str = "history.json";
const MAX_HISTORY_ENTRIES: usize = 500;

pub struct Store {
    data_dir: Option<PathBuf>,
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    /// Active + historical transfers, most recent last.
    transfers: Vec<TransferRecord>,
    devices: HashMap<String, Device>,
}

impl Store {
    /// Create a new store. If `data_dir` is `Some`, history is persisted
    /// to `<data_dir>/history.json` and loaded back on startup.
    pub fn new(data_dir: Option<PathBuf>) -> Self {
        let mut inner = Inner::default();

        if let Some(dir) = &data_dir {
            if let Err(e) = std::fs::create_dir_all(dir) {
                warn!("could not create data directory {:?}: {}", dir, e);
            }
            if let Some(loaded) = load_history(dir) {
                inner.transfers = loaded;
            }
        }

        Self {
            data_dir,
            inner: Mutex::new(inner),
        }
    }

    // ---------------------------------------------------------------
    // Transfers
    // ---------------------------------------------------------------

    pub fn insert_transfer(&self, record: TransferRecord) {
        let mut inner = self.inner.lock().expect("store lock poisoned");
        inner.transfers.push(record);
        if inner.transfers.len() > MAX_HISTORY_ENTRIES {
            let excess = inner.transfers.len() - MAX_HISTORY_ENTRIES;
            inner.transfers.drain(0..excess);
        }
        self.flush_locked(&inner);
    }

    pub fn update_transfer<F>(&self, id: Uuid, f: F) -> Option<TransferRecord>
    where
        F: FnOnce(&mut TransferRecord),
    {
        let mut inner = self.inner.lock().expect("store lock poisoned");
        let record = inner.transfers.iter_mut().find(|t| t.id == id)?;
        f(record);
        record.updated_at = chrono::Utc::now();
        let updated = record.clone();
        self.flush_locked(&inner);
        Some(updated)
    }

    pub fn get_transfer(&self, id: Uuid) -> Option<TransferRecord> {
        let inner = self.inner.lock().expect("store lock poisoned");
        inner.transfers.iter().find(|t| t.id == id).cloned()
    }

    pub fn list_history(&self) -> Vec<TransferRecord> {
        let inner = self.inner.lock().expect("store lock poisoned");
        let mut list = inner.transfers.clone();
        list.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        list
    }

    pub fn delete_transfer(&self, id: Uuid) -> bool {
        let mut inner = self.inner.lock().expect("store lock poisoned");
        let before = inner.transfers.len();
        inner.transfers.retain(|t| t.id != id);
        let removed = inner.transfers.len() != before;
        if removed {
            self.flush_locked(&inner);
        }
        removed
    }

    // ---------------------------------------------------------------
    // Devices
    // ---------------------------------------------------------------

    pub fn upsert_device(&self, device: Device) {
        let mut inner = self.inner.lock().expect("store lock poisoned");
        inner.devices.insert(device.id.clone(), device);
    }

    pub fn mark_device_disconnected(&self, id: &str) {
        let mut inner = self.inner.lock().expect("store lock poisoned");
        if let Some(d) = inner.devices.get_mut(id) {
            d.connected = false;
        }
    }

    pub fn list_devices(&self) -> Vec<Device> {
        let inner = self.inner.lock().expect("store lock poisoned");
        let mut list: Vec<Device> = inner.devices.values().cloned().collect();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        list
    }

    pub fn connected_device_count(&self) -> usize {
        let inner = self.inner.lock().expect("store lock poisoned");
        inner.devices.values().filter(|d| d.connected).count()
    }

    // ---------------------------------------------------------------
    // Internal
    // ---------------------------------------------------------------

    fn flush_locked(&self, inner: &Inner) {
        let Some(dir) = &self.data_dir else {
            return;
        };
        let path = dir.join(HISTORY_FILE);
        match serde_json::to_vec_pretty(&inner.transfers) {
            Ok(bytes) => {
                if let Err(e) = std::fs::write(&path, bytes) {
                    warn!("failed to persist transfer history to {:?}: {}", path, e);
                }
            }
            Err(e) => warn!("failed to serialize transfer history: {}", e),
        }
    }
}

fn load_history(dir: &PathBuf) -> Option<Vec<TransferRecord>> {
    let path = dir.join(HISTORY_FILE);
    let bytes = std::fs::read(&path).ok()?;
    match serde_json::from_slice(&bytes) {
        Ok(records) => Some(records),
        Err(e) => {
            warn!("history file {:?} is corrupt, starting fresh: {}", path, e);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Direction, TransferStatus};
    use chrono::Utc;

    fn sample_record() -> TransferRecord {
        TransferRecord {
            id: Uuid::new_v4(),
            filename: "test.txt".into(),
            size_bytes: 100,
            bytes_transferred: 0,
            sender: "A".into(),
            receiver: "B".into(),
            direction: Direction::Upload,
            status: TransferStatus::Pending,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            error: None,
        }
    }

    #[test]
    fn insert_and_list_transfer() {
        let store = Store::new(None);
        let record = sample_record();
        let id = record.id;
        store.insert_transfer(record);
        let history = store.list_history();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].id, id);
    }

    #[test]
    fn update_transfer_changes_status() {
        let store = Store::new(None);
        let record = sample_record();
        let id = record.id;
        store.insert_transfer(record);

        let updated = store
            .update_transfer(id, |r| {
                r.status = TransferStatus::Completed;
                r.bytes_transferred = 100;
            })
            .unwrap();

        assert_eq!(updated.status, TransferStatus::Completed);
        assert_eq!(updated.progress_percent(), 100.0);
    }

    #[test]
    fn delete_transfer_removes_it() {
        let store = Store::new(None);
        let record = sample_record();
        let id = record.id;
        store.insert_transfer(record);
        assert!(store.delete_transfer(id));
        assert!(store.get_transfer(id).is_none());
    }

    #[test]
    fn history_persists_across_store_instances() {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path().to_path_buf();

        {
            let store = Store::new(Some(data_dir.clone()));
            store.insert_transfer(sample_record());
        }

        let reloaded = Store::new(Some(data_dir));
        assert_eq!(reloaded.list_history().len(), 1);
    }
}
