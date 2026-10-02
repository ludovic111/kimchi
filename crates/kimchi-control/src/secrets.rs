//! API keys live in the OS keychain (macOS Keychain, Windows Credential
//! Manager, Secret Service on Linux), never in plain files.

use std::collections::HashMap;

use kimchi_gen::SecretStore;
use parking_lot::Mutex;

const SERVICE: &str = "app.kimchi.editor";

/// Keychain-backed store with an in-memory cache, so the OS is asked at most
/// once per provider per launch.
#[derive(Default)]
pub struct KeychainSecrets {
    cache: Mutex<HashMap<String, Option<String>>>,
}

impl SecretStore for KeychainSecrets {
    fn get(&self, provider: &str) -> Option<String> {
        if let Some(v) = self.cache.lock().get(provider) {
            return v.clone();
        }
        let v = keyring::Entry::new(SERVICE, provider).ok().and_then(|e| e.get_password().ok());
        self.cache.lock().insert(provider.to_string(), v.clone());
        v
    }

    fn set(&self, provider: &str, key: &str) -> Result<(), String> {
        keyring::Entry::new(SERVICE, provider)
            .and_then(|e| e.set_password(key))
            .map_err(|e| format!("Couldn't save the key to the keychain: {e}"))?;
        self.cache.lock().insert(provider.to_string(), Some(key.to_string()));
        Ok(())
    }

    fn delete(&self, provider: &str) -> Result<(), String> {
        match keyring::Entry::new(SERVICE, provider).and_then(|e| e.delete_credential()) {
            Ok(()) | Err(keyring::Error::NoEntry) => {}
            Err(e) => return Err(format!("Couldn't remove the key: {e}")),
        }
        self.cache.lock().insert(provider.to_string(), None);
        Ok(())
    }
}
