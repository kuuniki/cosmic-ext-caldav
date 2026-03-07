use std::path::PathBuf;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    pub accounts: Vec<Account>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    pub id: String,
    pub display_name: String,
    pub url: String,
    pub username: String,
    pub password: String,
}

impl Config {
    fn path() -> PathBuf {
        let mut p = dirs::config_dir().unwrap_or_else(|| PathBuf::from("~/.config"));
        p.push("cosmic-caldav");
        p.push("config.json");
        p
    }

    pub fn load() -> Self {
        let path = Self::path();
        if path.exists() {
            std::fs::read_to_string(&path)
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default()
        } else {
            Self::default()
        }
    }

    pub fn save(&self) {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(path, json);
        }
    }

    pub fn add_account(&mut self, url: String, username: String, password: String) {
        let id = format!("{}-{}", username, url.replace("://", "_").replace('/', "_"));
        let display_name = format!("{} @ {}", username, url);
        self.accounts.retain(|a| a.id != id);
        self.accounts.push(Account { id, display_name, url, username, password });
        self.save();
    }

    pub fn remove_account(&mut self, id: &str) {
        self.accounts.retain(|a| a.id != id);
        self.save();
    }
}
