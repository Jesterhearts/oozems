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
    mob_tables: HashMap<u32, Vec<LootEntry>>,
    reactor_tables: HashMap<u32, Vec<LootEntry>>,
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
    reactors: Vec<ReactorLootFile>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MobLootFile {
    mob_id: u32,
    drops: Vec<LootEntryFile>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReactorLootFile {
    reactor_id: u32,
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
        self.mob_tables.len() + self.reactor_tables.len()
    }

    pub fn item_reference_ids(&self) -> impl Iterator<Item = u32> + '_ {
        self.mob_tables
            .values()
            .chain(self.reactor_tables.values())
            .flatten()
            .map(|entry| entry.item_id)
    }
}

pub fn roll_mob_items(
    catalog: &LootCatalog,
    mob_id: u32,
    random_state: &mut u64,
) -> Vec<u32> {
    roll_items(&catalog.mob_tables, mob_id, random_state)
}

pub fn roll_reactor_items(
    catalog: &LootCatalog,
    reactor_id: u32,
    random_state: &mut u64,
) -> Vec<u32> {
    roll_items(&catalog.reactor_tables, reactor_id, random_state)
}

fn roll_items(
    tables: &HashMap<u32, Vec<LootEntry>>,
    source_id: u32,
    random_state: &mut u64,
) -> Vec<u32> {
    tables
        .get(&source_id)
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
    let mob_tables = build_tables(
        path,
        item_definitions,
        "mob",
        file.mobs
            .into_iter()
            .map(|table| (table.mob_id, table.drops)),
    )?;
    let reactor_tables = build_tables(
        path,
        item_definitions,
        "reactor",
        file.reactors
            .into_iter()
            .map(|table| (table.reactor_id, table.drops)),
    )?;
    Ok(LootCatalog {
        mob_tables,
        reactor_tables,
    })
}

fn build_tables(
    path: &Path,
    item_definitions: &(impl ItemDefinitionLookup + ?Sized),
    source_kind: &str,
    source_tables: impl IntoIterator<Item = (u32, Vec<LootEntryFile>)>,
) -> Result<HashMap<u32, Vec<LootEntry>>, LootConfigError> {
    let mut tables = HashMap::new();
    for (source_id, drops) in source_tables {
        if drops.is_empty() {
            return invalid(path, format!("{source_kind} {source_id} has no drops"));
        }
        let mut entries = Vec::with_capacity(drops.len());
        for drop in drops {
            if drop.chance_per_million == 0 || drop.chance_per_million > CHANCE_SCALE {
                return invalid(
                    path,
                    format!(
                        "{source_kind} {source_id} item {} chance_per_million must be between 1 \
                         and {CHANCE_SCALE}",
                        drop.item_id
                    ),
                );
            }
            validate_item(path, source_kind, source_id, drop.item_id, item_definitions)?;
            if entries
                .iter()
                .any(|entry: &LootEntry| entry.item_id == drop.item_id)
            {
                return invalid(
                    path,
                    format!(
                        "{source_kind} {source_id} item {} is duplicated",
                        drop.item_id
                    ),
                );
            }
            entries.push(LootEntry {
                item_id: drop.item_id,
                chance_per_million: drop.chance_per_million,
            });
        }
        if tables.insert(source_id, entries).is_some() {
            return invalid(
                path,
                format!("{source_kind} {source_id} has duplicate loot tables"),
            );
        }
    }
    Ok(tables)
}

fn validate_item(
    path: &Path,
    source_kind: &str,
    source_id: u32,
    item_id: u32,
    item_definitions: &(impl ItemDefinitionLookup + ?Sized),
) -> Result<(), LootConfigError> {
    match item_definitions.item_definition(item_id) {
        Ok(Some(_)) => Ok(()),
        Ok(None) => invalid(
            path,
            format!("{source_kind} {source_id} item {item_id} is not in the item catalog"),
        ),
        Err(error) => invalid(
            path,
            format!(
                "{source_kind} {source_id} item {item_id} metadata could not be loaded: {error}"
            ),
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
    use std::cell::Cell;
    use std::fs;

    use oozems_proto::v1::ItemDefinition;

    use super::LootCatalog;
    use super::roll_mob_items;
    use super::roll_reactor_items;
    use crate::items::ItemDefinitionLookup;
    use crate::items::ItemRuleError;

    struct LazyItemDefinitions {
        definition: ItemDefinition,
        lookups: Cell<usize>,
    }

    impl ItemDefinitionLookup for LazyItemDefinitions {
        fn item_definition(
            &self,
            item_id: u32,
        ) -> Result<Option<&ItemDefinition>, ItemRuleError> {
            self.lookups.set(self.lookups.get() + 1);
            Ok((self.definition.item_id == item_id).then_some(&self.definition))
        }
    }

    #[test]
    fn mob_and_reactor_ids_use_separate_tables() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("loot.toml");
        fs::write(
            &path,
            "[[mobs]]\nmob_id = 100\n[[mobs.drops]]\nitem_id = 1\nchance_per_million = \
             1000000\n[[reactors]]\nreactor_id = 100\n[[reactors.drops]]\nitem_id = \
             2\nchance_per_million = 1000000\n",
        )
        .expect("write loot configuration");
        let definitions = vec![definition(1), definition(2)];
        let catalog = LootCatalog::load(&path, &definitions).expect("loot catalog");

        assert_eq!(catalog.len(), 2);
        assert_eq!(roll_mob_items(&catalog, 100, &mut 1), vec![1]);
        assert_eq!(roll_reactor_items(&catalog, 100, &mut 1), vec![2]);
        assert!(roll_mob_items(&catalog, 101, &mut 1).is_empty());
        assert!(roll_reactor_items(&catalog, 101, &mut 1).is_empty());
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
    fn lazy_lookup_items_are_valid_loot_without_eager_projection() {
        let item_id = 4_000_001;
        let definitions = LazyItemDefinitions {
            definition: definition(item_id),
            lookups: Cell::new(0),
        };
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

        let catalog = LootCatalog::load(&path, &definitions).expect("lazy loot item should load");

        assert_eq!(catalog.len(), 1);
        assert_eq!(definitions.lookups.get(), 1);
    }

    fn definition(item_id: u32) -> ItemDefinition {
        ItemDefinition {
            item_id,
            ..ItemDefinition::default()
        }
    }
}
