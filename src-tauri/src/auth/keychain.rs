use anyhow::{Context, Result};
use keyring::Entry;
use serde::{de::DeserializeOwned, Serialize};

const SERVICE_NAME: &str = "com.expotify.app";

pub struct KeychainStorage;

impl KeychainStorage {
    /// Store a value in the keychain
    pub fn store<T: Serialize>(key: &str, value: &T) -> Result<()> {
        let entry = Entry::new(SERVICE_NAME, key)?;
        let json = serde_json::to_string(value)?;
        entry.set_password(&json)?;
        Ok(())
    }

    /// Retrieve a value from the keychain
    pub fn get<T: DeserializeOwned>(key: &str) -> Result<Option<T>> {
        let entry = Entry::new(SERVICE_NAME, key)?;
        match entry.get_password() {
            Ok(json) => {
                let value = serde_json::from_str(&json)
                    .context("Failed to deserialize keychain value")?;
                Ok(Some(value))
            }
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Delete a value from the keychain
    pub fn delete(key: &str) -> Result<()> {
        let entry = Entry::new(SERVICE_NAME, key)?;
        match entry.delete_credential() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()), // Already deleted
            Err(e) => Err(e.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_keychain_operations() {
        let key = "test_key";
        let value = "test_value".to_string();

        // Store
        KeychainStorage::store(key, &value).unwrap();

        // Get
        let retrieved: Option<String> = KeychainStorage::get(key).unwrap();
        assert_eq!(retrieved, Some(value));

        // Delete
        KeychainStorage::delete(key).unwrap();

        // Verify deleted
        let retrieved: Option<String> = KeychainStorage::get(key).unwrap();
        assert_eq!(retrieved, None);
    }
}
