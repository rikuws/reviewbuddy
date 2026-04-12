use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{de::DeserializeOwned, Serialize};

#[derive(Debug, Clone)]
pub struct CacheStore {
    path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct CachedDocument<T> {
    pub value: T,
    pub fetched_at_ms: i64,
}

impl CacheStore {
    pub fn new(path: PathBuf) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("Failed to create cache directory: {error}"))?;
        }

        let store = Self { path };
        store.initialize()?;
        Ok(store)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn get<T>(&self, key: &str) -> Result<Option<CachedDocument<T>>, String>
    where
        T: DeserializeOwned,
    {
        let connection = self.connection()?;
        let row = connection
            .query_row(
                "SELECT json, fetched_at_ms FROM documents WHERE key = ?1",
                params![key],
                |row| {
                    let json: String = row.get(0)?;
                    let fetched_at_ms: i64 = row.get(1)?;
                    Ok((json, fetched_at_ms))
                },
            )
            .optional()
            .map_err(|error| format!("Failed to read cache document '{key}': {error}"))?;

        match row {
            Some((json, fetched_at_ms)) => {
                let value = serde_json::from_str::<T>(&json).map_err(|error| {
                    format!("Failed to deserialize cache document '{key}': {error}")
                })?;

                Ok(Some(CachedDocument {
                    value,
                    fetched_at_ms,
                }))
            }
            None => Ok(None),
        }
    }

    pub fn put<T>(&self, key: &str, value: &T, fetched_at_ms: i64) -> Result<(), String>
    where
        T: Serialize,
    {
        let json = serde_json::to_string(value)
            .map_err(|error| format!("Failed to serialize cache document '{key}': {error}"))?;
        let connection = self.connection()?;

        connection
            .execute(
                "INSERT INTO documents (key, json, fetched_at_ms)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(key) DO UPDATE SET
                   json = excluded.json,
                   fetched_at_ms = excluded.fetched_at_ms",
                params![key, json, fetched_at_ms],
            )
            .map_err(|error| format!("Failed to write cache document '{key}': {error}"))?;

        Ok(())
    }

    pub fn delete(&self, key: &str) -> Result<(), String> {
        let connection = self.connection()?;

        connection
            .execute("DELETE FROM documents WHERE key = ?1", params![key])
            .map_err(|error| format!("Failed to delete cache document '{key}': {error}"))?;

        Ok(())
    }

    fn initialize(&self) -> Result<(), String> {
        let connection = self.connection()?;

        connection
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS documents (
                    key TEXT PRIMARY KEY,
                    json TEXT NOT NULL,
                    fetched_at_ms INTEGER NOT NULL
                );",
            )
            .map_err(|error| format!("Failed to initialize cache schema: {error}"))?;

        Ok(())
    }

    fn connection(&self) -> Result<Connection, String> {
        Connection::open(&self.path).map_err(|error| {
            format!(
                "Failed to open cache database '{}': {error}",
                self.path.display()
            )
        })
    }
}
