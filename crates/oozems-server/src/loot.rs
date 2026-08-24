use std::collections::HashMap;
use std::fs;
use std::io::ErrorKind;
use std::path::Path;
use std::path::PathBuf;

use serde::Deserialize;
use thiserror::Error;

use crate::items::ItemDefinitionLookup;

const CHANCE_SCALE: u32 = 1_000_000;

#[derive(Clone, Debug, Default)]
pub struct LootCatalog {
    tables: HashMap<u32, Vec<LootEntry>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LootEntry {
    item_id: u32,
    chance_per_million: u32,
}

#[derive(Debug, Error)]
pub enum LootConfigError {
    #[error("failed to read loot configuration {path}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse loot configuration {path}")]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("loot configuration {path} is invalid: {message}")]
    Invalid { path: PathBuf, message: String },
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct LootFile {
    mobs: Vec<MobLootFile>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MobLootFile {
    mob_id: u32,
    drops: Vec<LootEntryFile>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LootEntryFile {
    item_id: u32,
    chance_per_million: u32,
}

impl LootCatalog {
    pub fn load(
        path: &Path,
        item_definitions: &(impl ItemDefinitionLookup + ?Sized),
    ) -> Result<Self, LootConfigError> {
        let source = match fs::read_to_string(path) {
            Ok(source) => source,
            Err(source) if source.kind() == ErrorKind::NotFound => String::new(),
            Err(source) => {
                return Err(LootConfigError::Read {
                    path: path.to_owned(),
                    source,
                });
            }
        };
        let file =
            toml::from_str::<LootFile>(&source).map_err(|source| LootConfigError::Parse {
                path: path.to_owned(),
                source,
            })?;
        build_catalog(path, item_definitions, file)
    }

    pub fn len(&self) -> usize {
        self.tables.len()
    }

    pub fn item_reference_ids(&self) -> impl Iterator<Item = u32> + '_ {
        self.tables.values().flatten().map(|entry| entry.item_id)
    }
}

pub fn roll_items(
    catalog: &LootCatalog,
    mob_id: u32,
    random_state: &mut u64,
) -> Vec<u32> {
    catalog
        .tables
        .get(&mob_id)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let random = crate::random::next_u64(random_state);
            roll_succeeds(random, entry.chance_per_million).then_some(entry.item_id)
        })
        .collect()
}

fn build_catalog(
    path: &Path,
    item_definitions: &(impl ItemDefinitionLookup + ?Sized),
    file: LootFile,
) -> Result<LootCatalog, LootConfigError> {
    let mut tables = HashMap::new();
    for table in file.mobs {
        if table.drops.is_empty() {
            return invalid(path, format!("mob {} has no drops", table.mob_id));
        }
        let mut entries = Vec::with_capacity(table.drops.len());
        for drop in table.drops {
            if drop.chance_per_million == 0 || drop.chance_per_million > CHANCE_SCALE {
                return invalid(
                    path,
                    format!(
                        "mob {} item {} chance_per_million must be between 1 and {CHANCE_SCALE}",
                        table.mob_id, drop.item_id
                    ),
                );
            }
            validate_item(path, table.mob_id, drop.item_id, item_definitions)?;
            if entries
                .iter()
                .any(|entry: &LootEntry| entry.item_id == drop.item_id)
            {
                return invalid(
                    path,
                    format!("mob {} item {} is duplicated", table.mob_id, drop.item_id),
                );
            }
            entries.push(LootEntry {
                item_id: drop.item_id,
                chance_per_million: drop.chance_per_million,
            });
        }
        if tables.insert(table.mob_id, entries).is_some() {
            return invalid(
                path,
                format!("mob {} has duplicate loot tables", table.mob_id),
            );
        }
    }
    Ok(LootCatalog { tables })
}

fn validate_item(
    path: &Path,
    mob_id: u32,
    item_id: u32,
    item_definitions: &(impl ItemDefinitionLookup + ?Sized),
) -> Result<(), LootConfigError> {
    match item_definitions.item_definition(item_id) {
        Ok(Some(_)) => Ok(()),
        Ok(None) => invalid(
            path,
            format!("mob {mob_id} item {item_id} is not in the item catalog"),
        ),
        Err(error) => invalid(
            path,
            format!("mob {mob_id} item {item_id} metadata could not be loaded: {error}"),
        ),
    }
}

fn roll_succeeds(
    random: u64,
    chance_per_million: u32,
) -> bool {
    let scaled = (u128::from(random) * u128::from(CHANCE_SCALE)) >> 64;
    scaled < u128::from(chance_per_million)
}

fn invalid<T>(
    path: &Path,
    message: impl Into<String>,
) -> Result<T, LootConfigError> {
    Err(LootConfigError::Invalid {
        path: path.to_owned(),
        message: message.into(),
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use oozems_proto::v1::ItemDefinition;

    use super::LootCatalog;
    use super::roll_items;
    use crate::content::ContentCatalog;
    use crate::content::ContentConfig;

    #[test]
    fn guaranteed_entries_roll_independently() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("loot.toml");
        fs::write(
            &path,
            "[[mobs]]\nmob_id = 100\n[[mobs.drops]]\nitem_id = 1\nchance_per_million = \
             1000000\n[[mobs.drops]]\nitem_id = 2\nchance_per_million = 1000000\n",
        )
        .expect("write loot configuration");
        let definitions = vec![definition(1), definition(2)];
        let catalog = LootCatalog::load(&path, &definitions).expect("loot catalog");

        assert_eq!(roll_items(&catalog, 100, &mut 1), vec![1, 2]);
        assert!(roll_items(&catalog, 101, &mut 1).is_empty());
    }

    #[test]
    fn unknown_items_are_rejected_at_the_configuration_boundary() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("loot.toml");
        fs::write(
            &path,
            "[[mobs]]\nmob_id = 100\n[[mobs.drops]]\nitem_id = 9\nchance_per_million = 1\n",
        )
        .expect("write loot configuration");

        let error =
            LootCatalog::load(&path, &[definition(1)]).expect_err("unknown loot item must fail");

        assert!(
            error
                .to_string()
                .contains("item 9 is not in the item catalog")
        );
    }

    #[test]
    fn indexed_non_eager_items_are_valid_loot_without_eager_projection() {
        let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let wz_dir = manifest_dir.join("../../data");
        if !["Map.wz", "Character.wz", "Item.wz"]
            .iter()
            .all(|name| wz_dir.join(name).exists())
        {
            return;
        }
        let content = ContentCatalog::load(
            &wz_dir,
            &ContentConfig::load(&manifest_dir.join("../../config/content.toml"))
                .expect("content configuration"),
        )
        .expect("content catalog");
        let item_id = content
            .indexed_item_ids()
            .find(|item_id| {
                !content
                    .item_definition_slice()
                    .iter()
                    .any(|definition| definition.item_id == *item_id)
            })
            .expect("item source index should contain a non-eager item");
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("loot.toml");
        fs::write(
            &path,
            format!(
                "[[mobs]]\nmob_id = 100\n[[mobs.drops]]\nitem_id = {item_id}\nchance_per_million \
                 = 1\n"
            ),
        )
        .expect("write loot configuration");

        let catalog = LootCatalog::load(&path, &content).expect("indexed loot item should load");

        assert_eq!(catalog.len(), 1);
        assert!(
            !content
                .item_definition_slice()
                .iter()
                .any(|definition| definition.item_id == item_id)
        );

        fs::write(
            &path,
            "[[mobs]]\nmob_id = 100\n[[mobs.drops]]\nitem_id = 4294967295\nchance_per_million = \
             1\n",
        )
        .expect("write unknown loot item");
        let error =
            LootCatalog::load(&path, &content).expect_err("unknown loot item should be rejected");
        assert!(
            error
                .to_string()
                .contains("item 4294967295 is not in the item catalog")
        );
    }

    #[test]
    fn bundled_loot_references_available_items() {
        let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let wz_dir = manifest_dir.join("../../data");
        if !["Map.wz", "Character.wz"]
            .iter()
            .all(|name| wz_dir.join(name).exists())
        {
            return;
        }
        let content = ContentCatalog::load(
            &wz_dir,
            &ContentConfig::load(&manifest_dir.join("../../config/content.toml"))
                .expect("content configuration"),
        )
        .expect("content catalog");

        let loot = LootCatalog::load(
            &manifest_dir.join("../../config/loot.toml"),
            content.item_definition_slice(),
        )
        .expect("loot catalog");

        assert_eq!(loot.len(), 7);
    }

    fn definition(item_id: u32) -> ItemDefinition {
        ItemDefinition {
            item_id,
            ..ItemDefinition::default()
        }
    }
}
