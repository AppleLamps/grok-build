//! Session-scoped set of files the model has successfully read.
//!
//! Used by `search_replace` to enforce read-before-edit at runtime (not only
//! as a toolset-level "is there a Read tool?" check). Registered as
//! `State<FileReadTracker>` by the tool registry; unit tests that do not
//! insert a tracker value skip the gate.

use crate::types::resources::{SharedResources, State};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Canonical paths successfully returned by a Read tool in this session.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileReadTracker {
    pub paths: BTreeSet<PathBuf>,
}

crate::register_resource!("grok_build", "FileReadTracker", FileReadTracker);

/// Record a successful read (or write) of `path`. No-op when the tracker is
/// not present on `resources` (unit tests without a registry).
pub async fn record_read(resources: &SharedResources, path: PathBuf) {
    let mut res = resources.lock().await;
    if !res.contains::<State<FileReadTracker>>() {
        return;
    }
    res.get_or_default::<State<FileReadTracker>>()
        .0
        .paths
        .insert(path);
}

/// Whether `path` has been successfully read this session. `false` when the
/// tracker is not registered (unit tests without a registry).
pub async fn was_read(resources: &SharedResources, path: &Path) -> bool {
    let res = resources.lock().await;
    res.get::<State<FileReadTracker>>()
        .is_some_and(|state| state.0.paths.contains(path))
}

/// True when the session tracks reads and `path` has not been read yet.
/// Missing tracker is a no-op so unit tests and callers without agent
/// resources still work.
pub async fn requires_prior_read(resources: &SharedResources, path: &Path) -> bool {
    let res = resources.lock().await;
    res.get::<State<FileReadTracker>>()
        .is_some_and(|state| !state.0.paths.contains(path))
}
