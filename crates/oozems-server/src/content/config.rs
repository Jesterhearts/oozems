use std::collections::BTreeSet;
use std::fs;
use std::io::ErrorKind;
use std::path::Path;
use std::path::PathBuf;

use serde::Deserialize;
use thiserror::Error;

#[derive(Clone, Debug, Default)]
pub struct ContentConfig {
    pub(super) npcs: NpcFilter,
    pub(super) quest_ids: Option<BTreeSet<u32>>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct NpcFilter {
    allowed_limited_names: Option<BTreeSet<String>>,
    allowed_ids: Option<BTreeSet<u32>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ContentFile {
    npcs: NpcFilterFile,
    quests: QuestFilterFile,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct NpcFilterFile {
    allowed_limited_names: Option<Vec<String>>,
    allowed_ids: Option<Vec<u32>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct QuestFilterFile {
    allowed_ids: Option<Vec<u32>>,
}

#[derive(Debug, Error)]
pub enum ContentConfigError {
    #[error("failed to read content configuration {path}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse content configuration {path}")]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
}

impl ContentConfig {
    pub fn load(path: &Path) -> Result<Self, ContentConfigError> {
        let source = match fs::read_to_string(path) {
            Ok(source) => source,
            Err(source) if source.kind() == ErrorKind::NotFound => return Ok(Self::default()),
            Err(source) => {
                return Err(ContentConfigError::Read {
                    path: path.to_owned(),
                    source,
                });
            }
        };
        let file =
            toml::from_str::<ContentFile>(&source).map_err(|source| ContentConfigError::Parse {
                path: path.to_owned(),
                source,
            })?;
        Ok(Self {
            npcs: NpcFilter {
                allowed_limited_names: file
                    .npcs
                    .allowed_limited_names
                    .map(|names| names.into_iter().collect()),
                allowed_ids: file.npcs.allowed_ids.map(|ids| ids.into_iter().collect()),
            },
            quest_ids: file.quests.allowed_ids.map(|ids| ids.into_iter().collect()),
        })
    }
}

impl NpcFilter {
    pub(super) fn allows(
        &self,
        npc_id: u32,
        limited_name: Option<&str>,
    ) -> bool {
        if let (Some(limited_name), Some(allowed_names)) =
            (limited_name, &self.allowed_limited_names)
            && !allowed_names.contains(limited_name)
        {
            return false;
        }
        self.allowed_ids
            .as_ref()
            .is_none_or(|ids| ids.contains(&npc_id))
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::ContentConfig;

    #[test]
    fn missing_configuration_preserves_unrestricted_loading() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let config = ContentConfig::load(&directory.path().join("missing.toml"))
            .expect("default configuration");

        assert!(config.npcs.allows(100, None));
        assert!(config.npcs.allows(100, Some("summer")));
        assert!(config.quest_ids.is_none());
    }

    #[test]
    fn loads_limited_name_and_id_allowlists() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("content.toml");
        fs::write(
            &path,
            concat!(
                "[npcs]\n",
                "allowed_limited_names = [\"summer\", \"anniversary\"]\n",
                "allowed_ids = [100, 200, 100]\n",
                "[quests]\n",
                "allowed_ids = [1009, 1009]\n",
            ),
        )
        .expect("write configuration");

        let config = ContentConfig::load(&path).expect("valid configuration");

        assert!(config.npcs.allows(100, None));
        assert!(config.npcs.allows(100, Some("summer")));
        assert!(!config.npcs.allows(100, Some("winter")));
        assert!(!config.npcs.allows(300, None));
        assert_eq!(
            config
                .quest_ids
                .expect("quest allowlist")
                .into_iter()
                .collect::<Vec<_>>(),
            vec![1009]
        );
    }

    #[test]
    fn an_empty_allowlist_disables_all_npcs() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("content.toml");
        fs::write(&path, "[npcs]\nallowed_ids = []\n").expect("write configuration");

        let config = ContentConfig::load(&path).expect("valid configuration");

        assert!(!config.npcs.allows(100, None));
    }

    #[test]
    fn an_empty_limited_name_allowlist_excludes_only_limited_npcs() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("content.toml");
        fs::write(&path, "[npcs]\nallowed_limited_names = []\n").expect("write configuration");

        let config = ContentConfig::load(&path).expect("valid configuration");

        assert!(config.npcs.allows(100, None));
        assert!(!config.npcs.allows(100, Some("summer")));
    }

    #[test]
    fn rejects_unknown_settings() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("content.toml");
        fs::write(&path, "[npcs]\nunknown = true\n").expect("write configuration");

        assert!(ContentConfig::load(&path).is_err());
    }
}
