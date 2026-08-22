use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;
use thiserror::Error;

#[derive(Clone, Copy, Debug)]
pub struct GameplayConfig {
    pub item_drop_despawn: Duration,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GameplayFile {
    items: ItemRulesFile,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ItemRulesFile {
    drop_despawn: String,
}

#[derive(Debug, Error)]
pub enum GameplayConfigError {
    #[error("failed to read gameplay configuration {path}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse gameplay configuration {path}")]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("items.drop_despawn in {path} is invalid")]
    InvalidDropDespawn {
        path: PathBuf,
        #[source]
        source: humantime::DurationError,
    },
    #[error("items.drop_despawn in {path} must be greater than zero")]
    EmptyDropDespawn { path: PathBuf },
}

impl GameplayConfig {
    pub fn load(path: &Path) -> Result<Self, GameplayConfigError> {
        let source = fs::read_to_string(path).map_err(|source| GameplayConfigError::Read {
            path: path.to_owned(),
            source,
        })?;
        let file = toml::from_str::<GameplayFile>(&source).map_err(|source| {
            GameplayConfigError::Parse {
                path: path.to_owned(),
                source,
            }
        })?;
        let item_drop_despawn =
            humantime::parse_duration(&file.items.drop_despawn).map_err(|source| {
                GameplayConfigError::InvalidDropDespawn {
                    path: path.to_owned(),
                    source,
                }
            })?;
        if item_drop_despawn.is_zero() {
            return Err(GameplayConfigError::EmptyDropDespawn {
                path: path.to_owned(),
            });
        }

        Ok(Self { item_drop_despawn })
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::Duration;

    use super::GameplayConfig;

    #[test]
    fn loads_human_readable_drop_duration() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("gameplay.toml");
        fs::write(
            &path,
            "# See README.md for configuration reference.\n[items]\ndrop_despawn = \"10m\"\n",
        )
        .expect("write configuration");

        let config = GameplayConfig::load(&path).expect("valid configuration");

        assert_eq!(config.item_drop_despawn, Duration::from_secs(600));
    }

    #[test]
    fn rejects_zero_drop_duration() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("gameplay.toml");
        fs::write(&path, "[items]\ndrop_despawn = \"0s\"\n").expect("write configuration");

        assert!(GameplayConfig::load(&path).is_err());
    }
}
