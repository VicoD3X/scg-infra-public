use std::{
    env,
    error::Error,
    fs,
    net::SocketAddr,
    path::{Path, PathBuf},
    str::FromStr,
};

use scg_core::RealmId;
use serde::Deserialize;

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    bind: Option<String>,
    data_dir: Option<PathBuf>,
    realm: Option<String>,
    event_retention: Option<usize>,
    snapshot_retention: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct NodeConfig {
    pub bind: SocketAddr,
    pub database_path: PathBuf,
    pub realm: RealmId,
    pub event_retention: usize,
    pub snapshot_retention: usize,
}

impl NodeConfig {
    pub fn load(path: Option<&Path>) -> Result<Self, Box<dyn Error>> {
        let file = match path {
            Some(path) => toml::from_str::<FileConfig>(&fs::read_to_string(path)?)?,
            None => FileConfig::default(),
        };

        let bind = env::var("SCG_BIND")
            .ok()
            .or(file.bind)
            .unwrap_or_else(|| "127.0.0.1:8080".to_owned())
            .parse::<SocketAddr>()?;
        let data_dir = env::var_os("SCG_DATA_DIR")
            .map(PathBuf::from)
            .or(file.data_dir)
            .unwrap_or_else(|| PathBuf::from("./var/scg"));
        let realm = RealmId::from_str(
            &env::var("SCG_REALM")
                .ok()
                .or(file.realm)
                .unwrap_or_else(|| "live".to_owned()),
        )?;
        let database_name = format!("{}.db", realm.storage_key().replace(':', "-"));

        Ok(Self {
            bind,
            database_path: data_dir.join(database_name),
            realm,
            event_retention: env_usize("SCG_EVENT_RETENTION")
                .or(file.event_retention)
                .unwrap_or(1000)
                .max(1),
            snapshot_retention: env_usize("SCG_SNAPSHOT_RETENTION")
                .or(file.snapshot_retention)
                .unwrap_or(20)
                .max(1),
        })
    }
}

fn env_usize(name: &str) -> Option<usize> {
    env::var(name).ok()?.parse().ok()
}

