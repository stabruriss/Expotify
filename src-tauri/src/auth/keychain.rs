use anyhow::{Context, Result};
use serde::{de::DeserializeOwned, Serialize};
use std::fs;
use std::path::PathBuf;

const APP_DIR: &str = "com.expotify.app";

pub struct KeychainStorage;

impl KeychainStorage {
    fn file_path(key: &str) -> Result<PathBuf> {
        let dir = dirs::data_local_dir()
            .context("Could not determine local data directory")?
            .join(APP_DIR);
        fs::create_dir_all(&dir)?;
        Ok(dir.join(format!("{}.json", key)))
    }

    pub fn store<T: Serialize>(key: &str, value: &T) -> Result<()> {
        let path = Self::file_path(key)?;
        let json = serde_json::to_string(value)?;
        fs::write(&path, json)?;
        Ok(())
    }

    pub fn get<T: DeserializeOwned>(key: &str) -> Result<Option<T>> {
        let path = Self::file_path(key)?;
        if !path.exists() {
            return Ok(None);
        }
        let json = fs::read_to_string(&path)?;
        let value = serde_json::from_str(&json).context("Failed to deserialize stored value")?;
        Ok(Some(value))
    }

    pub fn delete(key: &str) -> Result<()> {
        let path = Self::file_path(key)?;
        if path.exists() {
            fs::remove_file(&path)?;
        }
        Ok(())
    }
}
