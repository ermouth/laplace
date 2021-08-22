use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::Permission;

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct ApplicationSettings {
    pub title: String,
    pub enabled: bool,
}

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct PermissionsSettings {
    pub required: Vec<Permission>,
    pub allowed: Vec<Permission>,
}

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct DatabaseSettings {
    pub path: PathBuf,
}

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct NetworkSettings {
    pub gossipsub: GossipsubSettings,
}

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct GossipsubSettings {
    pub addr: String,
    pub dial_ports: Vec<u16>,
}

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct DapSettings {
    pub application: ApplicationSettings,
    pub permissions: PermissionsSettings,
    pub database: DatabaseSettings,
    pub network: NetworkSettings,
}
