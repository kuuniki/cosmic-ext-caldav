use std::path::PathBuf;
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

const KEYRING_SERVICE: &str = "cosmic-caldav";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    pub accounts: Vec<Account>,
}

// Stored in JSON - no password here
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountMeta {
    pub id: String,
    pub display_name: String,
    pub url: String,
    pub username: String,
}

// In-memory account with password loaded from keyring
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    pub id: String,
    pub display_name: String,
    pub url: String,
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct StoredConfig {
    pub accounts: Vec<AccountMeta>,
}

#[allow(dead_code)]
impl Config {
    fn path() -> PathBuf {
        let mut p = dirs::config_dir().unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from("/tmp")).join(".config"));
        p.push("cosmic-caldav");
        p.push("config.json");
        p
    }

    pub fn load() -> Self {
        let path = Self::path();
        let stored: StoredConfig = if path.exists() {
            std::fs::read_to_string(&path)
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default()
        } else {
            StoredConfig::default()
        };

        // Accounts whose password cannot be retrieved from the keyring are skipped.
        // This prevents silent authentication failures later and avoids an empty
        // password being passed to the CalDAV server.
        let accounts = stored.accounts.into_iter().filter_map(|meta| {
            match keyring::Entry::new(KEYRING_SERVICE, &meta.id)
                .and_then(|e| e.get_password())
            {
                Ok(password) => Some(Account {
                    id: meta.id,
                    display_name: meta.display_name,
                    url: meta.url,
                    username: meta.username,
                    password,
                }),
                Err(e) => {
                    eprintln!(
                        "Warning: skipping account '{}' — could not retrieve password from keyring: {:?}",
                        meta.display_name, e
                    );
                    None
                }
            }
        }).collect();

        Config { accounts }
    }

    pub fn save(&self) {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let stored = StoredConfig {
            accounts: self.accounts.iter().map(|a| AccountMeta {
                id: a.id.clone(),
                display_name: a.display_name.clone(),
                url: a.url.clone(),
                username: a.username.clone(),
            }).collect(),
        };
        if let Ok(json) = serde_json::to_string_pretty(&stored) {
            let _ = std::fs::write(path, json);
        }
    }

    /// Adds a new account, storing its password securely in the OS keyring.
    ///
    /// Returns `Err` if the keyring operation fails — the account is **not** added
    /// in that case, preventing silent use of an unsecured credential store.
    pub fn add_account(&mut self, url: String, username: String, password: String) -> Result<(), String> {
        let id = format!("{}-{}", username, url.replace("://", "_").replace('/', "_"));
        let short_name = username.split('@').next().unwrap_or(&username).to_string();
        let provider_label = if url.contains("google.com") {
            " (Google)"
        } else if url.contains("outlook.office365.com") {
            " (Outlook)"
        } else if url.contains("nextcloud") || url.contains("remote.php") {
            " (Nextcloud)"
        } else {
            " (CalDAV)"
        };
        let display_name = format!("{}{}", short_name, provider_label);

        // Store password in keyring — fail loudly if this is not possible.
        keyring::Entry::new(KEYRING_SERVICE, &id)
            .map_err(|e| format!("Could not access the system keyring: {:?}", e))?
            .set_password(&password)
            .map_err(|e| format!("Could not save your password securely: {:?}", e))?;

        self.accounts.retain(|a| a.id != id);
        self.accounts.push(Account { id, display_name, url, username, password });
        self.save();
        Ok(())
    }

    pub fn remove_account(&mut self, id: &str) {
        // Delete password from keyring
        if let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE, id) {
            // Zero the stored credential before deleting to reduce window
            // where another process could read it from the keyring backend.
            let _ = entry.delete_credential();
        }
        // Zero in-memory password for the removed account before dropping
        if let Some(account) = self.accounts.iter_mut().find(|a| a.id == id) {
            account.password.zeroize();
        }
        self.accounts.retain(|a| a.id != id);
        self.save();
    }
}
