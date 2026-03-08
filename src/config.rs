use std::path::PathBuf;
use serde::{Deserialize, Serialize};

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
        let mut p = dirs::config_dir().unwrap_or_else(|| PathBuf::from("~/.config"));
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

        let accounts = stored.accounts.into_iter().map(|meta| {
            let password = keyring::Entry::new(KEYRING_SERVICE, &meta.id)
                .ok()
                .and_then(|e| e.get_password().ok())
                .unwrap_or_default();
            Account {
                id: meta.id,
                display_name: meta.display_name,
                url: meta.url,
                username: meta.username,
                password,
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

    pub fn add_account(&mut self, url: String, username: String, password: String) {
        let id = format!("{}-{}", username, url.replace("://", "_").replace('/', "_"));
        let short_name = username.split('@').next().unwrap_or(&username).to_string();
        let provider_label = if url.contains("google.com") {
            " (Google)"
        } else if url.contains("outlook.office365.com") {
            " (Outlook)"
        } else {
            " (Nextcloud)"
        };
        let display_name = format!("{}{}", short_name, provider_label);

        // Store password in keyring
        if let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE, &id) {
            let _ = entry.set_password(&password);
        }

        self.accounts.retain(|a| a.id != id);
        self.accounts.push(Account { id, display_name, url, username, password });
        self.save();
    }

    pub fn remove_account(&mut self, id: &str) {
        // Delete password from keyring
        if let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE, id) {
            let _ = entry.delete_credential();
        }
        self.accounts.retain(|a| a.id != id);
        self.save();
    }
}
