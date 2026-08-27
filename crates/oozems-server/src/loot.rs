use std::collections::HashMap;
use std::fs;
use std::io::ErrorKind;
use std::path::Path;
use std::path::PathBuf;

use oozems_proto::v1::PlayerState;
use serde::Deserialize;
use thiserror::Error;

use crate::content::QuestDefinition;
use crate::content::QuestItemCondition;
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
    quest: Option<QuestLootGate>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct QuestLootGate {
    quest_id: u32,
    required_quantity: Option<u32>,
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
    quest_id: Option<u32>,
}

impl LootCatalog {
    pub(crate) fn load<'a>(
        path: &Path,
        item_definitions: &(impl ItemDefinitionLookup + ?Sized),
        quest_definitions: impl IntoIterator<Item = &'a QuestDefinition>,
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
        build_catalog(path, item_definitions, quest_definitions, file)
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
    player: &PlayerState,
    random_state: &mut u64,
) -> Vec<u32> {
    roll_items(&catalog.mob_tables, mob_id, player, random_state)
}

pub fn roll_reactor_items(
    catalog: &LootCatalog,
    reactor_id: u32,
    player: &PlayerState,
    random_state: &mut u64,
) -> Vec<u32> {
    roll_items(&catalog.reactor_tables, reactor_id, player, random_state)
}

fn roll_items(
    tables: &HashMap<u32, Vec<LootEntry>>,
    source_id: u32,
    player: &PlayerState,
    random_state: &mut u64,
) -> Vec<u32> {
    tables
        .get(&source_id)
        .into_iter()
        .flatten()
        .filter(|entry| quest_gate_allows(entry, player))
        .filter_map(|entry| {
            let random = crate::random::next_u64(random_state);
            roll_succeeds(random, entry.chance_per_million).then_some(entry.item_id)
        })
        .collect()
}

fn build_catalog<'a>(
    path: &Path,
    item_definitions: &(impl ItemDefinitionLookup + ?Sized),
    quest_definitions: impl IntoIterator<Item = &'a QuestDefinition>,
    file: LootFile,
) -> Result<LootCatalog, LootConfigError> {
    let quest_definitions = quest_definitions
        .into_iter()
        .map(|quest| (quest.id, quest))
        .collect::<HashMap<_, _>>();
    let mob_tables = build_tables(
        path,
        item_definitions,
        &quest_definitions,
        "mob",
        file.mobs
            .into_iter()
            .map(|table| (table.mob_id, table.drops)),
    )?;
    let reactor_tables = build_tables(
        path,
        item_definitions,
        &quest_definitions,
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
    quest_definitions: &HashMap<u32, &QuestDefinition>,
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
            let quest = drop
                .quest_id
                .map(|quest_id| {
                    build_quest_gate(
                        path,
                        source_kind,
                        source_id,
                        drop.item_id,
                        quest_id,
                        quest_definitions,
                    )
                })
                .transpose()?;
            if entries
                .iter()
                .any(|entry: &LootEntry| entry.item_id == drop.item_id && entry.quest == quest)
            {
                return invalid(
                    path,
                    format!(
                        "{source_kind} {source_id} item {} quest {:?} is duplicated",
                        drop.item_id, drop.quest_id
                    ),
                );
            }
            entries.push(LootEntry {
                item_id: drop.item_id,
                chance_per_million: drop.chance_per_million,
                quest,
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

fn build_quest_gate(
    path: &Path,
    source_kind: &str,
    source_id: u32,
    item_id: u32,
    quest_id: u32,
    quest_definitions: &HashMap<u32, &QuestDefinition>,
) -> Result<QuestLootGate, LootConfigError> {
    let Some(quest) = quest_definitions.get(&quest_id) else {
        return invalid(
            path,
            format!("{source_kind} {source_id} item {item_id} references unknown quest {quest_id}"),
        );
    };
    if quest.completion.items.iter().any(|requirement| {
        requirement.item_id == item_id && requirement.condition == QuestItemCondition::Absent
    }) {
        return invalid(
            path,
            format!(
                "{source_kind} {source_id} item {item_id} cannot drop for quest {quest_id} \
                 because completion requires it to be absent"
            ),
        );
    }
    let required_quantity = quest
        .completion
        .items
        .iter()
        .filter(|requirement| requirement.item_id == item_id)
        .filter_map(|requirement| match requirement.condition {
            QuestItemCondition::AtLeast(quantity) => Some(quantity.get()),
            QuestItemCondition::Absent => None,
        })
        .max();
    Ok(QuestLootGate {
        quest_id,
        required_quantity,
    })
}

fn quest_gate_allows(
    entry: &LootEntry,
    player: &PlayerState,
) -> bool {
    let Some(quest) = entry.quest else {
        return true;
    };
    if crate::quests::progress(player, quest.quest_id) != crate::quests::QuestProgress::Started {
        return false;
    }
    quest
        .required_quantity
        .is_none_or(|required| player_item_quantity(player, entry.item_id) < u64::from(required))
}

fn player_item_quantity(
    player: &PlayerState,
    item_id: u32,
) -> u64 {
    let Some(inventory) = &player.inventory else {
        return 0;
    };
    let stack_quantity = inventory
        .stacks
        .iter()
        .filter(|stack| stack.item_id == item_id)
        .fold(0_u64, |total, stack| {
            total.saturating_add(u64::from(stack.quantity))
        });
    inventory
        .equipment
        .iter()
        .filter(|item| item.item_id == item_id)
        .fold(stack_quantity, |total, _| total.saturating_add(1))
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
    use std::num::NonZeroU32;

    use oozems_proto::v1::EquippedItem;
    use oozems_proto::v1::InventoryItemStack;
    use oozems_proto::v1::InventoryState;
    use oozems_proto::v1::ItemDefinition;
    use oozems_proto::v1::PlayerQuest;
    use oozems_proto::v1::PlayerState;
    use oozems_proto::v1::QuestStatus;

    use super::LootCatalog;
    use super::roll_mob_items;
    use super::roll_reactor_items;
    use crate::content::QuestActions;
    use crate::content::QuestCompletionRequirements;
    use crate::content::QuestDefinition;
    use crate::content::QuestDialogue;
    use crate::content::QuestInfo;
    use crate::content::QuestItemCondition;
    use crate::content::QuestItemRequirement;
    use crate::content::QuestStartRequirements;
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
        let catalog =
            LootCatalog::load(&path, &definitions, std::iter::empty()).expect("loot catalog");
        let player = PlayerState::default();

        assert_eq!(catalog.len(), 2);
        assert_eq!(roll_mob_items(&catalog, 100, &player, &mut 1), vec![1]);
        assert_eq!(roll_reactor_items(&catalog, 100, &player, &mut 1), vec![2]);
        assert!(roll_mob_items(&catalog, 101, &player, &mut 1).is_empty());
        assert!(roll_reactor_items(&catalog, 101, &player, &mut 1).is_empty());
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

        let error = LootCatalog::load(&path, &[definition(1)], std::iter::empty())
            .expect_err("unknown loot item must fail");

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

        let catalog = LootCatalog::load(&path, &definitions, std::iter::empty())
            .expect("lazy loot item should load");

        assert_eq!(catalog.len(), 1);
        assert_eq!(definitions.lookups.get(), 1);
    }

    #[test]
    fn quest_drops_require_an_active_quest_below_its_item_target() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("loot.toml");
        fs::write(
            &path,
            "[[mobs]]\nmob_id = 100\n[[mobs.drops]]\nitem_id = 1\nchance_per_million = \
             1000000\n[[mobs.drops]]\nitem_id = 1\nchance_per_million = 1000000\nquest_id = \
             7\n[[mobs]]\nmob_id = 101\n[[mobs.drops]]\nitem_id = 1\nchance_per_million = \
             1000000\nquest_id = 7\n",
        )
        .expect("write loot configuration");
        let quest = quest_definition(7, 1, 2);
        let catalog =
            LootCatalog::load(&path, &[definition(1)], [&quest]).expect("quest loot catalog");
        let mut player = PlayerState::default();

        assert_eq!(roll_mob_items(&catalog, 100, &player, &mut 1), vec![1]);
        let mut random_state = 1;
        assert!(roll_mob_items(&catalog, 101, &player, &mut random_state).is_empty());
        assert_eq!(random_state, 1);

        player.quests.push(PlayerQuest {
            quest_id: 7,
            status: QuestStatus::Started.into(),
            ..PlayerQuest::default()
        });
        assert_eq!(roll_mob_items(&catalog, 100, &player, &mut 1), vec![1, 1]);
        assert_eq!(roll_mob_items(&catalog, 101, &player, &mut 1), vec![1]);

        player.inventory = Some(InventoryState {
            equipment: vec![EquippedItem {
                item_id: 1,
                ..EquippedItem::default()
            }],
            stacks: vec![InventoryItemStack {
                item_id: 1,
                quantity: 1,
                ..InventoryItemStack::default()
            }],
            ..InventoryState::default()
        });
        assert_eq!(roll_mob_items(&catalog, 100, &player, &mut 1), vec![1]);
        let mut random_state = 1;
        assert!(roll_mob_items(&catalog, 101, &player, &mut random_state).is_empty());
        assert_eq!(random_state, 1);

        player.quests[0].status = QuestStatus::Completed.into();
        assert_eq!(roll_mob_items(&catalog, 100, &player, &mut 1), vec![1]);
    }

    #[test]
    fn unknown_quest_drops_are_rejected_at_the_configuration_boundary() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("loot.toml");
        fs::write(
            &path,
            "[[mobs]]\nmob_id = 100\n[[mobs.drops]]\nitem_id = 1\nchance_per_million = \
             1\nquest_id = 7\n",
        )
        .expect("write loot configuration");

        let error = LootCatalog::load(&path, &[definition(1)], std::iter::empty())
            .expect_err("unknown quest must fail");

        assert!(error.to_string().contains("references unknown quest 7"));
    }

    #[test]
    fn quest_drops_reject_an_absent_completion_requirement() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("loot.toml");
        fs::write(
            &path,
            "[[mobs]]\nmob_id = 100\n[[mobs.drops]]\nitem_id = 1\nchance_per_million = \
             1\nquest_id = 7\n",
        )
        .expect("write loot configuration");
        let mut quest = quest_definition(7, 1, 1);
        quest.completion.items[0].condition = QuestItemCondition::Absent;

        let error = LootCatalog::load(&path, &[definition(1)], [&quest])
            .expect_err("absent quest item drop must fail");

        assert!(
            error
                .to_string()
                .contains("completion requires it to be absent")
        );
    }

    fn definition(item_id: u32) -> ItemDefinition {
        ItemDefinition {
            item_id,
            ..ItemDefinition::default()
        }
    }

    fn quest_definition(
        quest_id: u32,
        item_id: u32,
        required_quantity: u32,
    ) -> QuestDefinition {
        QuestDefinition {
            id: quest_id,
            name: format!("Quest {quest_id}"),
            start: QuestStartRequirements::default(),
            completion: QuestCompletionRequirements {
                items: vec![QuestItemRequirement {
                    item_id,
                    condition: QuestItemCondition::AtLeast(
                        NonZeroU32::new(required_quantity).expect("nonzero item requirement"),
                    ),
                }],
                ..QuestCompletionRequirements::default()
            },
            start_actions: QuestActions::default(),
            completion_actions: QuestActions::default(),
            dialogue: QuestDialogue::default(),
            info: QuestInfo::default(),
        }
    }
}
