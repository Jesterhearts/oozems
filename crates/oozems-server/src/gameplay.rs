use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;
use thiserror::Error;

#[derive(Clone, Copy, Debug)]
pub struct GameplayConfig {
    pub item_drop_despawn: Duration,
    pub initial_skill_points: u32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GameplayFile {
    items: ItemRulesFile,
    skills: SkillRulesFile,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ItemRulesFile {
    drop_despawn: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SkillRulesFile {
    initial_points: u32,
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

        Ok(Self {
            item_drop_despawn,
            initial_skill_points: file.skills.initial_points,
        })
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
            concat!(
                "# See README.md for configuration reference.\n",
                "[items]\n",
                "drop_despawn = \"10m\"\n",
                "[skills]\n",
                "initial_points = 3\n",
            ),
        )
        .expect("write configuration");

        let config = GameplayConfig::load(&path).expect("valid configuration");

        assert_eq!(config.item_drop_despawn, Duration::from_secs(600));
        assert_eq!(config.initial_skill_points, 3);
    }

    #[test]
    fn rejects_zero_drop_duration() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("gameplay.toml");
        fs::write(
            &path,
            "[items]\ndrop_despawn = \"0s\"\n[skills]\ninitial_points = 3\n",
        )
        .expect("write configuration");

        assert!(GameplayConfig::load(&path).is_err());
    }
}
