use std::env;
use std::net::SocketAddr;
use std::path::PathBuf;

use thiserror::Error;

const DEFAULT_BIND: &str = "127.0.0.1:3000";

#[derive(Debug)]
pub struct Config {
    pub bind: SocketAddr,
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub public_dir: PathBuf,
    pub wz_dir: PathBuf,
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("OOZEMS_BIND must be a socket address, got {value:?}")]
    InvalidBind {
        value: String,
        #[source]
        source: std::net::AddrParseError,
    },
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let bind_value = env::var("OOZEMS_BIND").unwrap_or_else(|_| DEFAULT_BIND.to_owned());
        let bind = bind_value
            .parse()
            .map_err(|source| ConfigError::InvalidBind {
                value: bind_value,
                source,
            })?;

        Ok(Self {
            bind,
            config_dir: env_path("OOZEMS_CONFIG_DIR", PathBuf::from("config")),
            data_dir: env_path("OOZEMS_DATA_DIR", PathBuf::from("data")),
            public_dir: env_path("OOZEMS_PUBLIC_DIR", manifest_dir.join("public")),
            wz_dir: env_path("OOZEMS_WZ_DIR", PathBuf::from("data")),
        })
    }
}

fn env_path(
    name: &str,
    default: PathBuf,
) -> PathBuf {
    env::var_os(name).map(PathBuf::from).unwrap_or(default)
}
