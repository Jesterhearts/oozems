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
    global_drops: Vec<LootEntry>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LootEntry {
    content: LootContent,
    chance_per_million: u32,
    quest: Option<QuestLootGate>,
    required_skill_id: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LootContent {
    Item {
        item_id: u32,
        minimum_quantity: u32,
        maximum_quantity: u32,
    },
    Mesos {
        minimum: u64,
        maximum: u64,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RolledLoot {
    Item { item_id: u32, quantity: u32 },
    Mesos { amount: u64 },
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
    global_drops: Vec<LootEntryFile>,
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
#[serde(untagged)]
enum LootEntryFile {
    Item(ItemLootEntryFile),
    Mesos(MesoLootEntryFile),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ItemLootEntryFile {
    item_id: u32,
    chance_per_million: u32,
    minimum_quantity: Option<u32>,
    maximum_quantity: Option<u32>,
    quest_id: Option<u32>,
    required_skill_id: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MesoLootEntryFile {
    chance_per_million: u32,
    minimum_mesos: u64,
    maximum_mesos: u64,
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
        self.mob_tables.len()
            + self.reactor_tables.len()
            + usize::from(!self.global_drops.is_empty())
    }

    pub fn item_reference_ids(&self) -> impl Iterator<Item = u32> + '_ {
        self.mob_tables
            .values()
            .chain(self.reactor_tables.values())
            .flatten()
            .chain(&self.global_drops)
            .filter_map(|entry| match entry.content {
                LootContent::Item { item_id, .. } => Some(item_id),
                LootContent::Mesos { .. } => None,
            })
    }
}

pub fn roll_mob_loot(
    catalog: &LootCatalog,
    mob_id: u32,
    player: &PlayerState,
    source_skill_id: Option<u32>,
    random_state: &mut u64,
) -> Vec<RolledLoot> {
    let mut rolled = roll_table(
        catalog.mob_tables.get(&mob_id),
        player,
        source_skill_id,
        random_state,
    );
    rolled.extend(roll_entries(
        &catalog.global_drops,
        player,
        source_skill_id,
        random_state,
    ));
    rolled
}

pub fn roll_reactor_loot(
    catalog: &LootCatalog,
    reactor_id: u32,
    player: &PlayerState,
    random_state: &mut u64,
) -> Vec<RolledLoot> {
    roll_table(
        catalog.reactor_tables.get(&reactor_id),
        player,
        None,
        random_state,
    )
}

fn roll_table(
    entries: Option<&Vec<LootEntry>>,
    player: &PlayerState,
    source_skill_id: Option<u32>,
    random_state: &mut u64,
) -> Vec<RolledLoot> {
    entries.map_or_else(Vec::new, |entries| {
        roll_entries(entries, player, source_skill_id, random_state)
    })
}

fn roll_entries<'a>(
    entries: impl IntoIterator<Item = &'a LootEntry>,
    player: &PlayerState,
    source_skill_id: Option<u32>,
    random_state: &mut u64,
) -> Vec<RolledLoot> {
    entries
        .into_iter()
        .filter(|entry| quest_gate_allows(entry, player))
        .filter(|entry| {
            entry
                .required_skill_id
                .is_none_or(|required| source_skill_id == Some(required))
        })
        .filter_map(|entry| {
            let random = crate::random::next_u64(random_state);
            if !roll_succeeds(random, entry.chance_per_million) {
                return None;
            }
            Some(match entry.content {
                LootContent::Item {
                    item_id,
                    minimum_quantity,
                    maximum_quantity,
                } => RolledLoot::Item {
                    item_id,
                    quantity: u32::try_from(roll_inclusive(
                        u64::from(minimum_quantity),
                        u64::from(maximum_quantity),
                        random_state,
                    ))
                    .expect("an item quantity range contains u32 values"),
                },
                LootContent::Mesos { minimum, maximum } => RolledLoot::Mesos {
                    amount: roll_inclusive(minimum, maximum, random_state),
                },
            })
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
    let global_drops = build_entries(
        path,
        item_definitions,
        &quest_definitions,
        "global drop",
        None,
        file.global_drops,
    )?;
    Ok(LootCatalog {
        mob_tables,
        reactor_tables,
        global_drops,
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
        let entries = build_entries(
            path,
            item_definitions,
            quest_definitions,
            source_kind,
            Some(source_id),
            drops,
        )?;
        if tables.insert(source_id, entries).is_some() {
            return invalid(
                path,
                format!("{source_kind} {source_id} has duplicate loot tables"),
            );
        }
    }
    Ok(tables)
}

fn build_entries(
    path: &Path,
    item_definitions: &(impl ItemDefinitionLookup + ?Sized),
    quest_definitions: &HashMap<u32, &QuestDefinition>,
    source_kind: &str,
    source_id: Option<u32>,
    drops: Vec<LootEntryFile>,
) -> Result<Vec<LootEntry>, LootConfigError> {
    let source = source_label(source_kind, source_id);
    let mut entries = Vec::with_capacity(drops.len());
    for drop in drops {
        let (content, chance_per_million, quest, required_skill_id, item_detail) = match drop {
            LootEntryFile::Item(drop) => {
                validate_chance(path, &source, drop.chance_per_million)?;
                let (minimum_quantity, maximum_quantity) =
                    item_quantity_range(path, &source, &drop)?;
                validate_item(path, &source, drop.item_id, item_definitions)?;
                let quest = drop
                    .quest_id
                    .map(|quest_id| {
                        build_quest_gate(path, &source, drop.item_id, quest_id, quest_definitions)
                    })
                    .transpose()?;
                if drop.required_skill_id == Some(0) {
                    return invalid(
                        path,
                        format!(
                            "{source} item {} required_skill_id must be positive",
                            drop.item_id
                        ),
                    );
                }
                if source_kind == "reactor" && drop.required_skill_id.is_some() {
                    return invalid(
                        path,
                        format!(
                            "{source} item {} cannot require a killing skill",
                            drop.item_id
                        ),
                    );
                }
                (
                    LootContent::Item {
                        item_id: drop.item_id,
                        minimum_quantity,
                        maximum_quantity,
                    },
                    drop.chance_per_million,
                    quest,
                    drop.required_skill_id,
                    Some((drop.item_id, drop.quest_id, drop.required_skill_id)),
                )
            }
            LootEntryFile::Mesos(drop) => {
                validate_chance(path, &source, drop.chance_per_million)?;
                if drop.minimum_mesos == 0 || drop.minimum_mesos > drop.maximum_mesos {
                    return invalid(
                        path,
                        format!(
                            "{source} meso range must be positive and ordered, got {}..={}",
                            drop.minimum_mesos, drop.maximum_mesos
                        ),
                    );
                }
                if drop.maximum_mesos > i64::MAX as u64 {
                    return invalid(
                        path,
                        format!("{source} meso range exceeds the supported player balance"),
                    );
                }
                (
                    LootContent::Mesos {
                        minimum: drop.minimum_mesos,
                        maximum: drop.maximum_mesos,
                    },
                    drop.chance_per_million,
                    None,
                    None,
                    None,
                )
            }
        };
        let duplicate = item_detail.map_or_else(
            || entries.iter().any(|entry: &LootEntry| entry.content == content),
            |(item_id, quest_id, skill_id)| {
                entries.iter().any(|entry| {
                    matches!(entry.content, LootContent::Item { item_id: existing, .. } if existing == item_id)
                        && entry.quest.map(|gate| gate.quest_id) == quest_id
                        && entry.required_skill_id == skill_id
                })
            },
        );
        if duplicate {
            let detail = item_detail.map_or_else(
                || String::from("meso range"),
                |(item_id, quest_id, skill_id)| {
                    format!("item {item_id} quest {quest_id:?} skill {skill_id:?}")
                },
            );
            return invalid(path, format!("{source} {detail} is duplicated"));
        }
        entries.push(LootEntry {
            content,
            chance_per_million,
            quest,
            required_skill_id,
        });
    }
    Ok(entries)
}

fn source_label(
    source_kind: &str,
    source_id: Option<u32>,
) -> String {
    source_id.map_or_else(
        || source_kind.to_owned(),
        |source_id| format!("{source_kind} {source_id}"),
    )
}

fn validate_chance(
    path: &Path,
    source: &str,
    chance_per_million: u32,
) -> Result<(), LootConfigError> {
    if chance_per_million == 0 || chance_per_million > CHANCE_SCALE {
        return invalid(
            path,
            format!(
                "{source} chance_per_million must be between 1 and {CHANCE_SCALE}, got \
                 {chance_per_million}"
            ),
        );
    }
    Ok(())
}

fn item_quantity_range(
    path: &Path,
    source: &str,
    drop: &ItemLootEntryFile,
) -> Result<(u32, u32), LootConfigError> {
    let range = match (drop.minimum_quantity, drop.maximum_quantity) {
        (None, None) => (1, 1),
        (Some(minimum), Some(maximum)) => (minimum, maximum),
        _ => {
            return invalid(
                path,
                format!(
                    "{source} item {} must specify both quantity bounds or neither",
                    drop.item_id
                ),
            );
        }
    };
    if range.0 == 0 || range.0 > range.1 {
        return invalid(
            path,
            format!(
                "{source} item {} quantity range must be positive and ordered, got {}..={}",
                drop.item_id, range.0, range.1
            ),
        );
    }
    Ok(range)
}

fn build_quest_gate(
    path: &Path,
    source: &str,
    item_id: u32,
    quest_id: u32,
    quest_definitions: &HashMap<u32, &QuestDefinition>,
) -> Result<QuestLootGate, LootConfigError> {
    let Some(quest) = quest_definitions.get(&quest_id) else {
        return invalid(
            path,
            format!("{source} item {item_id} references unknown quest {quest_id}"),
        );
    };
    if quest.completion.items.iter().any(|requirement| {
        requirement.item_id == item_id && requirement.condition == QuestItemCondition::Absent
    }) {
        return invalid(
            path,
            format!(
                "{source} item {item_id} cannot drop for quest {quest_id} because completion \
                 requires it to be absent"
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
    quest.required_quantity.is_none_or(|required| {
        let LootContent::Item { item_id, .. } = entry.content else {
            return false;
        };
        player_item_quantity(player, item_id) < u64::from(required)
    })
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
    source: &str,
    item_id: u32,
    item_definitions: &(impl ItemDefinitionLookup + ?Sized),
) -> Result<(), LootConfigError> {
    match item_definitions.item_definition(item_id) {
        Ok(Some(_)) => Ok(()),
        Ok(None) => invalid(
            path,
            format!("{source} item {item_id} is not in the item catalog"),
        ),
        Err(error) => invalid(
            path,
            format!("{source} item {item_id} metadata could not be loaded: {error}"),
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

fn roll_inclusive(
    minimum: u64,
    maximum: u64,
    random_state: &mut u64,
) -> u64 {
    if minimum == maximum {
        return minimum;
    }
    let width = maximum - minimum + 1;
    let random = crate::random::next_u64(random_state);
    let offset = ((u128::from(random) * u128::from(width)) >> 64) as u64;
    minimum + offset
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
    use std::path::Path;

    use oozems_proto::v1::EquippedItem;
    use oozems_proto::v1::InventoryItemStack;
    use oozems_proto::v1::InventoryState;
    use oozems_proto::v1::ItemDefinition;
    use oozems_proto::v1::PlayerQuest;
    use oozems_proto::v1::PlayerState;
    use oozems_proto::v1::QuestStatus;

    use super::LootCatalog;
    use super::RolledLoot;
    use super::roll_mob_loot;
    use super::roll_reactor_loot;
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

    struct PermissiveItemDefinitions(ItemDefinition);

    impl ItemDefinitionLookup for PermissiveItemDefinitions {
        fn item_definition(
            &self,
            _item_id: u32,
        ) -> Result<Option<&ItemDefinition>, ItemRuleError> {
            Ok(Some(&self.0))
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
        assert_eq!(
            roll_mob_loot(&catalog, 100, &player, None, &mut 1),
            vec![item(1)]
        );
        assert_eq!(
            roll_reactor_loot(&catalog, 100, &player, &mut 1),
            vec![item(2)]
        );
        assert!(roll_mob_loot(&catalog, 101, &player, None, &mut 1).is_empty());
        assert!(roll_reactor_loot(&catalog, 101, &player, &mut 1).is_empty());
    }

    #[test]
    fn mob_loot_combines_specific_and_global_items_and_mesos() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("loot.toml");
        fs::write(
            &path,
            "[[global_drops]]\nitem_id = 2\nchance_per_million = 1000000\nminimum_quantity = \
             2\nmaximum_quantity = 4\n[[global_drops]]\nchance_per_million = \
             1000000\nminimum_mesos = 10\nmaximum_mesos = 12\n[[mobs]]\nmob_id = \
             100\n[[mobs.drops]]\nitem_id = 1\nchance_per_million = \
             1000000\n[[reactors]]\nreactor_id = 100\n[[reactors.drops]]\nitem_id = \
             3\nchance_per_million = 1000000\n",
        )
        .expect("write loot configuration");
        let definitions = vec![definition(1), definition(2), definition(3)];
        let catalog =
            LootCatalog::load(&path, &definitions, std::iter::empty()).expect("loot catalog");
        let player = PlayerState::default();

        let mob_loot = roll_mob_loot(&catalog, 100, &player, None, &mut 1);
        assert_eq!(mob_loot[0], item(1));
        assert!(matches!(
            mob_loot[1],
            RolledLoot::Item {
                item_id: 2,
                quantity: 2..=4
            }
        ));
        assert!(matches!(mob_loot[2], RolledLoot::Mesos { amount: 10..=12 }));
        assert_eq!(
            roll_reactor_loot(&catalog, 100, &player, &mut 1),
            vec![item(3)]
        );
    }

    #[test]
    fn skill_gated_loot_requires_the_killing_skill() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("loot.toml");
        fs::write(
            &path,
            "[[mobs]]\nmob_id = 100\n[[mobs.drops]]\nitem_id = 1\nchance_per_million = \
             1000000\nrequired_skill_id = 5001001\n",
        )
        .expect("write loot configuration");
        let catalog = LootCatalog::load(&path, &[definition(1)], std::iter::empty())
            .expect("skill-gated loot catalog");
        let player = PlayerState::default();

        assert!(roll_mob_loot(&catalog, 100, &player, None, &mut 1).is_empty());
        assert!(roll_mob_loot(&catalog, 100, &player, Some(5_001_003), &mut 1).is_empty());
        assert_eq!(
            roll_mob_loot(&catalog, 100, &player, Some(5_001_001), &mut 1),
            vec![item(1)]
        );
    }

    #[test]
    fn generated_v83_catalog_matches_the_runtime_schema() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/v83/loot.toml");
        let definitions = PermissiveItemDefinitions(definition(1));
        let quest = quest_definition(1_035, 4_031_802, 1);

        let catalog =
            LootCatalog::load(&path, &definitions, [&quest]).expect("generated v83 loot catalog");

        assert_eq!(catalog.len(), 353);
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
    fn invalid_quantity_and_meso_rows_are_rejected_at_the_configuration_boundary() {
        for (source, expected) in [
            (
                "[[mobs]]\nmob_id = 100\n[[mobs.drops]]\nitem_id = 1\nchance_per_million = \
                 1\nminimum_quantity = 1\n",
                "must specify both quantity bounds or neither",
            ),
            (
                "[[mobs]]\nmob_id = 100\n[[mobs.drops]]\nitem_id = 1\nchance_per_million = \
                 1\nminimum_quantity = 0\nmaximum_quantity = 1\n",
                "quantity range must be positive and ordered",
            ),
            (
                "[[mobs]]\nmob_id = 100\n[[mobs.drops]]\nitem_id = 1\nchance_per_million = \
                 1\nrequired_skill_id = 0\n",
                "required_skill_id must be positive",
            ),
            (
                "[[reactors]]\nreactor_id = 100\n[[reactors.drops]]\nitem_id = \
                 1\nchance_per_million = 1\nrequired_skill_id = 5001001\n",
                "cannot require a killing skill",
            ),
            (
                "[[mobs]]\nmob_id = 100\n[[mobs.drops]]\nitem_id = 1\nchance_per_million = \
                 1\nminimum_quantity = 1\nmaximum_quantity = 1\n[[mobs.drops]]\nitem_id = \
                 1\nchance_per_million = 2\nminimum_quantity = 1\nmaximum_quantity = 2\n",
                "item 1 quest None skill None is duplicated",
            ),
            (
                "[[mobs]]\nmob_id = 100\n[[mobs.drops]]\nchance_per_million = 1\nminimum_mesos = \
                 0\nmaximum_mesos = 1\n",
                "meso range must be positive and ordered",
            ),
            (
                "[[mobs]]\nmob_id = 100\n[[mobs.drops]]\nchance_per_million = 1\nminimum_mesos = \
                 1\nmaximum_mesos = 2\n[[mobs.drops]]\nchance_per_million = 2\nminimum_mesos = \
                 1\nmaximum_mesos = 2\n",
                "meso range is duplicated",
            ),
        ] {
            let directory = tempfile::tempdir().expect("temporary directory");
            let path = directory.path().join("loot.toml");
            fs::write(&path, source).expect("write invalid loot configuration");

            let error = LootCatalog::load(&path, &[definition(1)], std::iter::empty())
                .expect_err("invalid loot row must fail");

            assert!(error.to_string().contains(expected), "{error}");
        }
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

        assert_eq!(
            roll_mob_loot(&catalog, 100, &player, None, &mut 1),
            vec![item(1)]
        );
        let mut random_state = 1;
        assert!(roll_mob_loot(&catalog, 101, &player, None, &mut random_state).is_empty());
        assert_eq!(random_state, 1);

        player.quests.push(PlayerQuest {
            quest_id: 7,
            status: QuestStatus::Started.into(),
            ..PlayerQuest::default()
        });
        assert_eq!(
            roll_mob_loot(&catalog, 100, &player, None, &mut 1),
            vec![item(1), item(1)]
        );
        assert_eq!(
            roll_mob_loot(&catalog, 101, &player, None, &mut 1),
            vec![item(1)]
        );

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
        assert_eq!(
            roll_mob_loot(&catalog, 100, &player, None, &mut 1),
            vec![item(1)]
        );
        let mut random_state = 1;
        assert!(roll_mob_loot(&catalog, 101, &player, None, &mut random_state).is_empty());
        assert_eq!(random_state, 1);

        player.quests[0].status = QuestStatus::Completed.into();
        assert_eq!(
            roll_mob_loot(&catalog, 100, &player, None, &mut 1),
            vec![item(1)]
        );
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

    fn item(item_id: u32) -> RolledLoot {
        RolledLoot::Item {
            item_id,
            quantity: 1,
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
