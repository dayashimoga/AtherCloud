use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub hmac_secret: String,
    pub scratch_dir: PathBuf,
    pub mounts: HashMap<String, MountConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MountConfig {
    pub path: PathBuf,
    pub read_only: bool,
    pub description: Option<String>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        let mut mounts = HashMap::new();
        let default_media = std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join("media_storage");

        mounts.insert(
            "default".to_string(),
            MountConfig {
                path: default_media,
                read_only: false,
                description: Some("Default local media storage".to_string()),
            },
        );

        Self {
            host: "0.0.0.0".to_string(),
            port: 8080,
            hmac_secret: "aetherstream_default_development_secret_key_change_in_prod".to_string(),
            scratch_dir: PathBuf::from("./scratch"),
            mounts,
        }
    }
}
