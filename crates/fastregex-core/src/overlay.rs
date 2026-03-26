use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};

use dashmap::DashMap;

#[derive(Debug, Clone)]
pub struct OverlayFile {
    pub text: String,
    pub gram_hashes: HashSet<u64>,
}

#[derive(Debug, Clone)]
pub enum OverlayEntry {
    Modified(OverlayFile),
    Deleted,
}

#[derive(Debug, Clone)]
pub struct OverlaySnapshot {
    pub generation: u64,
    pub files: HashMap<String, OverlayEntry>,
}

#[derive(Debug)]
pub struct OverlayStore {
    files: DashMap<String, OverlayEntry>,
    generation: AtomicU64,
}

impl Default for OverlayStore {
    fn default() -> Self {
        Self {
            files: DashMap::new(),
            generation: AtomicU64::new(0),
        }
    }
}

impl OverlayStore {
    pub fn upsert_modified(&self, path: String, text: String, gram_hashes: HashSet<u64>) {
        self.files.insert(
            path,
            OverlayEntry::Modified(OverlayFile { text, gram_hashes }),
        );
        self.generation.fetch_add(1, Ordering::Relaxed);
    }

    pub fn upsert_deleted(&self, path: String) {
        self.files.insert(path, OverlayEntry::Deleted);
        self.generation.fetch_add(1, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> OverlaySnapshot {
        let mut files = HashMap::with_capacity(self.files.len());
        for entry in self.files.iter() {
            files.insert(entry.key().clone(), entry.value().clone());
        }

        OverlaySnapshot {
            generation: self.generation.load(Ordering::Relaxed),
            files,
        }
    }

    pub fn dirty_files(&self) -> usize {
        self.files.len()
    }
}
