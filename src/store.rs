//! What the plugin keeps: for each server, the address of its web interface
//! and the token. A token is kept for the server it was typed for and given
//! to no other: a repository only says which server it is on.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[cfg(not(test))]
const SERVICE: &str = "ardoise-gitlab";

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Server {
    /// The address of the web interface, when it is not `https://<host>`.
    pub address: String,
    /// The token, only when the system has no keychain.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub token: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Store {
    /// By the host of the remotes.
    pub servers: BTreeMap<String, Server>,
}

fn file() -> Option<PathBuf> {
    Some(crate::fsutil::data_dir()?.join("servers.json"))
}

#[cfg(not(test))]
fn keychain(host: &str) -> Option<keyring::Entry> {
    keyring::Entry::new(SERVICE, host).ok()
}

impl Store {
    pub fn load() -> Store {
        file()
            .and_then(|f| std::fs::read_to_string(f).ok())
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        let path = file().ok_or_else(|| t!("gl.err_store").to_string())?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        // Only its owner reads it: it may hold a token.
        crate::fsutil::write_atomic(&path, json.as_bytes(), true).map_err(|e| e.to_string())
    }

    pub fn address(&self, host: &str) -> String {
        self.servers
            .get(host)
            .map(|s| s.address.trim())
            .filter(|a| !a.is_empty())
            .map_or_else(|| format!("https://{host}"), str::to_string)
    }

    /// The token of a server (empty: none). The keychain may take a while:
    /// not on the thread that talks to the editor.
    pub fn token(&self, host: &str) -> String {
        let Some(server) = self.servers.get(host) else {
            return String::new();
        };
        if !server.token.is_empty() {
            return server.token.clone();
        }
        #[cfg(not(test))]
        if let Some(Ok(token)) = keychain(host).map(|e| e.get_password()) {
            return token;
        }
        String::new()
    }

    /// Keeps a token, in the keychain of the system when there is one; says
    /// whether it went there.
    pub fn set_token(&mut self, host: &str, token: &str) -> bool {
        let server = self.servers.entry(host.to_string()).or_default();
        #[cfg(not(test))]
        if keychain(host).is_some_and(|e| e.set_password(token).is_ok()) {
            server.token.clear();
            return true;
        }
        server.token = token.to_string();
        false
    }

    pub fn forget(&mut self, host: &str) {
        #[cfg(not(test))]
        if let Some(entry) = keychain(host) {
            let _ = entry.delete_credential();
        }
        self.servers.remove(host);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_is_given_to_the_server_it_was_typed_for_only() {
        let mut store = Store::default();
        store.set_token("gitlab.com", "secret");
        assert_eq!(store.token("gitlab.com"), "secret");
        assert_eq!(store.token("evil.example.com"), "");
        assert_eq!(store.address("gitlab.com"), "https://gitlab.com");
        store.servers.get_mut("gitlab.com").unwrap().address = "https://x.y/gitlab".into();
        assert_eq!(store.address("gitlab.com"), "https://x.y/gitlab");
        store.save().unwrap();
        assert_eq!(Store::load(), store);
        store.forget("gitlab.com");
        assert_eq!(store.token("gitlab.com"), "");
    }
}
