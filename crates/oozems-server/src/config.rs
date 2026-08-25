use std::env;
use std::net::SocketAddr;
use std::path::Path;
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

#[derive(Default)]
struct EnvironmentInput {
    bind: Option<String>,
    config_dir: Option<PathBuf>,
    data_dir: Option<PathBuf>,
    public_dir: Option<PathBuf>,
    wz_dir: Option<PathBuf>,
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        parse_environment(read_environment(), &manifest_dir)
    }
}

fn read_environment() -> EnvironmentInput {
    EnvironmentInput {
        bind: env::var("OOZEMS_BIND").ok(),
        config_dir: env::var_os("OOZEMS_CONFIG_DIR").map(PathBuf::from),
        data_dir: env::var_os("OOZEMS_DATA_DIR").map(PathBuf::from),
        public_dir: env::var_os("OOZEMS_PUBLIC_DIR").map(PathBuf::from),
        wz_dir: env::var_os("OOZEMS_WZ_DIR").map(PathBuf::from),
    }
}

fn parse_environment(
    input: EnvironmentInput,
    manifest_dir: &Path,
) -> Result<Config, ConfigError> {
    let bind_value = input.bind.unwrap_or_else(|| DEFAULT_BIND.to_owned());
    let bind = bind_value
        .parse()
        .map_err(|source| ConfigError::InvalidBind {
            value: bind_value,
            source,
        })?;

    Ok(Config {
        bind,
        config_dir: input.config_dir.unwrap_or_else(|| PathBuf::from("config")),
        data_dir: input.data_dir.unwrap_or_else(|| PathBuf::from("data")),
        public_dir: input
            .public_dir
            .unwrap_or_else(|| manifest_dir.join("public")),
        wz_dir: input.wz_dir.unwrap_or_else(|| PathBuf::from("data")),
    })
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;
    use std::path::Path;
    use std::path::PathBuf;

    use super::ConfigError;
    use super::EnvironmentInput;
    use super::parse_environment;

    #[test]
    fn empty_environment_uses_documented_defaults() {
        let config = parse_environment(EnvironmentInput::default(), Path::new("/server"))
            .expect("default configuration");

        assert_eq!(config.bind, SocketAddr::from(([127, 0, 0, 1], 3_000)));
        assert_eq!(config.config_dir, Path::new("config"));
        assert_eq!(config.data_dir, Path::new("data"));
        assert_eq!(config.public_dir, Path::new("/server/public"));
        assert_eq!(config.wz_dir, Path::new("data"));
    }

    #[test]
    fn environment_overrides_are_parsed_without_global_mutation() {
        let config = parse_environment(
            EnvironmentInput {
                bind: Some("[::1]:8080".to_owned()),
                config_dir: Some(PathBuf::from("custom-config")),
                data_dir: Some(PathBuf::from("custom-data")),
                public_dir: Some(PathBuf::from("custom-public")),
                wz_dir: Some(PathBuf::from("custom-wz")),
            },
            Path::new("/server"),
        )
        .expect("configured environment");

        assert_eq!(config.bind, "[::1]:8080".parse().expect("socket address"));
        assert_eq!(config.config_dir, Path::new("custom-config"));
        assert_eq!(config.data_dir, Path::new("custom-data"));
        assert_eq!(config.public_dir, Path::new("custom-public"));
        assert_eq!(config.wz_dir, Path::new("custom-wz"));
    }

    #[test]
    fn invalid_bind_reports_the_supplied_value() {
        let error = parse_environment(
            EnvironmentInput {
                bind: Some("localhost".to_owned()),
                ..EnvironmentInput::default()
            },
            Path::new("/server"),
        )
        .expect_err("invalid bind");

        assert!(matches!(
            error,
            ConfigError::InvalidBind { value, .. } if value == "localhost"
        ));
    }
}
