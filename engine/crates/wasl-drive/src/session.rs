//! Persistence of resumable-upload session URIs, so an interrupted upload continues where it
//! stopped instead of starting over. The SQLite-backed store lives in `wasl-store`.

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;

use crate::Result;

/// Key → resumable session URI. Keys are chosen by the caller (e.g. `pair|path|localHash`) so a
/// changed file never resumes an old session.
#[async_trait]
pub trait SessionStore: Send + Sync {
    async fn get(&self, key: &str) -> Result<Option<String>>;
    async fn put(&self, key: &str, uri: &str) -> Result<()>;
    async fn remove(&self, key: &str) -> Result<()>;
}

/// A [`SessionStore`] that lives as long as the process.
#[derive(Debug, Default)]
pub struct MemorySessionStore {
    map: Mutex<HashMap<String, String>>,
}

impl MemorySessionStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn map(&self) -> std::sync::MutexGuard<'_, HashMap<String, String>> {
        self.map.lock().unwrap_or_else(|p| p.into_inner())
    }
}

#[async_trait]
impl SessionStore for MemorySessionStore {
    async fn get(&self, key: &str) -> Result<Option<String>> {
        Ok(self.map().get(key).cloned())
    }

    async fn put(&self, key: &str, uri: &str) -> Result<()> {
        self.map().insert(key.to_string(), uri.to_string());
        Ok(())
    }

    async fn remove(&self, key: &str) -> Result<()> {
        self.map().remove(key);
        Ok(())
    }
}
