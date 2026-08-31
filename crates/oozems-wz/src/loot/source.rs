use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::path::Path;

use anyhow::Context;
use anyhow::Result;
use anyhow::bail;

use super::model::Diagnostics;
use super::model::ItemFact;
use super::model::ItemKind;
use super::model::MobFact;
use super::model::WzFacts;
use super::policy::GlobalDrop;
use super::policy::LootPolicy;
use super::policy::PolicyMobDrop;
use crate::Archive;
use crate::NodeSummary;
use crate::NodeTree;
use crate::OpenOptions;
use crate::archive_info;
use crate::list;
use crate::open_archive;
use crate::tree;

const MONSTER_BOOK_NODE_LIMIT: usize = 300_000;
const STRING_MOB_NODE_LIMIT: usize = 300_000;
const QUEST_NODE_LIMIT: usize = 500_000;
const ITEM_NODE_LIMIT: usize = 10_000;
const MOB_NODE_LIMIT: usize = 1_000;
const MAP_LIFE_NODE_LIMIT: usize = 10_000;
const SKILL_NODE_LIMIT: usize = 10_000;
const LIST_PAGE_SIZE: usize = 1_000;

pub(crate) struct SourceLoad {
    pub facts: WzFacts,
    pub versions: BTreeMap<String, i16>,
    pub diagnostics: Diagnostics,
}

pub(crate) fn load(
    directory: &Path,
    options: OpenOptions,
    policy: &LootPolicy,
) -> Result<SourceLoad> {
    if !directory.is_dir() {
        bail!(
            "WZ data directory does not exist or is not a directory: {}",
            directory.display()
        );
    }
    let strings = open_required(directory, "String.wz", options)?;
    let items = open_required(directory, "Item.wz", options)?;
    let mobs = open_required(directory, "Mob.wz", options)?;
    let quests = open_required(directory, "Quest.wz", options)?;
    let mut versions = BTreeMap::from([
        version_entry("String.wz", &strings),
        version_entry("Item.wz", &items),
        version_entry("Mob.wz", &mobs),
        version_entry("Quest.wz", &quests),
    ]);
    let source_map_ids = policy
        .mob_drops
        .iter()
        .filter_map(|drop| drop.source_map_id)
        .collect::<BTreeSet<_>>();
    let required_skill_ids = policy
        .mob_drops
        .iter()
        .filter_map(|drop| drop.required_skill_id)
        .collect::<BTreeSet<_>>();
    let maps = (!source_map_ids.is_empty())
        .then(|| open_required(directory, "Map.wz", options))
        .transpose()?;
    let skills = (!required_skill_ids.is_empty())
        .then(|| open_required(directory, "Skill.wz", options))
        .transpose()?;
    if let Some(maps) = &maps {
        versions.insert("Map.wz".to_owned(), archive_info(maps).version);
    }
    if let Some(skills) = &skills {
        versions.insert("Skill.wz".to_owned(), archive_info(skills).version);
    }
    let mut diagnostics = Diagnostics::default();

    let monster_book = tree(&strings, "/MonsterBook.img", MONSTER_BOOK_NODE_LIMIT)
        .context("failed to read String.wz/MonsterBook.img")?;
    let associations = extract_monster_book(&monster_book, &mut diagnostics);

    let card_tree = tree(&items, "/Consume/0238.img", MONSTER_BOOK_NODE_LIMIT)
        .context("failed to read Item.wz/Consume/0238.img")?;
    let card_sources = extract_card_sources(&card_tree, &mut diagnostics);

    let candidate_mob_ids = associations
        .keys()
        .copied()
        .chain(card_sources.keys().copied())
        .chain(policy.mob_drops.iter().map(|drop| drop.mob_id))
        .collect::<BTreeSet<_>>();
    let names = match tree(&strings, "/Mob.img", STRING_MOB_NODE_LIMIT) {
        Ok(tree) => extract_mob_names(&tree, &candidate_mob_ids, &mut diagnostics),
        Err(error) => {
            diagnostics.warn(format!(
                "String.wz/Mob.img could not be read; generated mob names are omitted: {error:#}"
            ));
            BTreeMap::new()
        }
    };
    let mob_image_paths = numeric_image_paths(&mobs, "/")?;
    let mob_facts = load_mob_facts(
        &mobs,
        &candidate_mob_ids,
        &mob_image_paths,
        &names,
        &mut diagnostics,
    );

    let valid_mob_ids = mob_facts.keys().copied().collect::<BTreeSet<_>>();
    let associations = associations
        .into_iter()
        .filter(|(mob_id, _)| valid_mob_ids.contains(mob_id))
        .collect::<BTreeMap<_, _>>();
    let card_sources = card_sources
        .into_iter()
        .filter(|(mob_id, _)| {
            if valid_mob_ids.contains(mob_id) {
                true
            } else {
                diagnostics.omit(
                    "card_mob_unavailable",
                    format!("Monster Book card source mob {mob_id} has no valid Mob.wz source"),
                );
                false
            }
        })
        .collect::<BTreeMap<_, _>>();

    let quest_item_ids = associations
        .values()
        .flatten()
        .copied()
        .filter(|item_id| *item_id / 10_000 == 403)
        .chain(
            policy
                .mob_drops
                .iter()
                .filter(|drop| drop.quest_id.is_some())
                .map(|drop| drop.item_id),
        )
        .collect::<BTreeSet<_>>();
    let quest_ids = policy
        .mob_drops
        .iter()
        .filter_map(|drop| drop.quest_id)
        .collect::<BTreeSet<_>>();
    let quest_tree = tree(&quests, "/Check.img", QUEST_NODE_LIMIT)
        .context("failed to read Quest.wz/Check.img")?;
    let completion_quests =
        extract_completion_quests(&quest_tree, &quest_item_ids, &mut diagnostics);
    let completion_mobs = extract_completion_mobs(&quest_tree, &quest_ids, &mut diagnostics);

    let global_item_ids = policy.global_drops.iter().filter_map(|drop| match drop {
        GlobalDrop::Item(item) => Some(item.item_id),
        GlobalDrop::Mesos(_) => None,
    });
    let candidate_item_ids = associations
        .values()
        .flatten()
        .copied()
        .chain(card_sources.values().map(|card| card.item_id))
        .chain(policy.mob_drops.iter().map(|drop| drop.item_id))
        .chain(global_item_ids)
        .collect::<BTreeSet<_>>();
    let equipment_needed = candidate_item_ids
        .iter()
        .any(|item_id| *item_id / 1_000_000 == 1);
    let character = if equipment_needed {
        open_optional(directory, "Character.wz", options, &mut diagnostics)?
    } else {
        None
    };
    if let Some(character) = &character {
        versions.insert("Character.wz".to_owned(), archive_info(character).version);
    }
    let equipment_paths = character
        .as_ref()
        .map(|archive| equipment_image_paths(archive, &candidate_item_ids))
        .transpose()?
        .unwrap_or_default();
    let item_facts = load_item_facts(
        &items,
        character.as_ref(),
        &candidate_item_ids,
        &card_sources,
        &equipment_paths,
        &mut diagnostics,
    );
    let source_map_mobs = maps
        .as_ref()
        .map(|archive| load_source_map_mobs(archive, &source_map_ids))
        .transpose()?
        .unwrap_or_default();
    let validated_skill_ids = skills
        .as_ref()
        .map(|archive| load_required_skill_ids(archive, &required_skill_ids))
        .transpose()?
        .unwrap_or_default();
    validate_policy_mob_drop_sources(
        &policy.mob_drops,
        &mob_facts,
        &item_facts,
        &completion_quests,
        &completion_mobs,
        &source_map_mobs,
        &validated_skill_ids,
    )?;
    let cards = card_sources
        .into_iter()
        .filter_map(|(mob_id, card)| {
            item_facts
                .contains_key(&card.item_id)
                .then_some((mob_id, card.item_id))
        })
        .collect();

    Ok(SourceLoad {
        facts: WzFacts {
            associations,
            cards,
            mobs: mob_facts,
            items: item_facts,
            completion_quests,
        },
        versions,
        diagnostics,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CardSource {
    item_id: u32,
    slot_max: Option<u32>,
}

fn open_required(
    directory: &Path,
    name: &str,
    options: OpenOptions,
) -> Result<Archive> {
    let path = directory.join(name);
    if !path.is_file() {
        bail!("required WZ archive is absent: {}", path.display());
    }
    open_archive(&path, options)
}

fn open_optional(
    directory: &Path,
    name: &str,
    options: OpenOptions,
    diagnostics: &mut Diagnostics,
) -> Result<Option<Archive>> {
    let path = directory.join(name);
    if !path
        .try_exists()
        .with_context(|| format!("failed to inspect optional archive {}", path.display()))?
    {
        diagnostics.warn(format!(
            "{name} is absent; equipment rewards cannot be validated and are omitted"
        ));
        return Ok(None);
    }
    open_archive(&path, options).map(Some)
}

fn version_entry(
    name: &str,
    archive: &Archive,
) -> (String, i16) {
    (name.to_owned(), archive_info(archive).version)
}

pub(crate) fn extract_monster_book(
    root: &NodeTree,
    diagnostics: &mut Diagnostics,
) -> BTreeMap<u32, BTreeSet<u32>> {
    let mut output = BTreeMap::<u32, BTreeSet<u32>>::new();
    for entry in &root.children {
        let Some(mob_id) = entry.name.parse::<u32>().ok().filter(|id| *id > 0) else {
            diagnostics.omit(
                "malformed_monster_book_entry",
                format!(
                    "MonsterBook.img entry {:?} is not a positive mob ID",
                    entry.name
                ),
            );
            continue;
        };
        let Some(rewards) = child(entry, "reward") else {
            diagnostics.omit(
                "malformed_monster_book_entry",
                format!("MonsterBook.img mob {mob_id} has no reward node"),
            );
            continue;
        };
        let item_ids = positive_leaf_integers(rewards);
        if item_ids.is_empty() {
            diagnostics.omit(
                "malformed_monster_book_entry",
                format!("MonsterBook.img mob {mob_id} has no positive reward IDs"),
            );
            continue;
        }
        let rewards = output.entry(mob_id).or_default();
        for item_id in item_ids {
            if !rewards.insert(item_id) {
                diagnostics.omit(
                    "duplicate_monster_book_association",
                    format!(
                        "MonsterBook.img repeats mob {mob_id} item {item_id}; one row is retained"
                    ),
                );
            }
        }
    }
    output
}

fn extract_card_sources(
    root: &NodeTree,
    diagnostics: &mut Diagnostics,
) -> BTreeMap<u32, CardSource> {
    let mut output = BTreeMap::new();
    for item in &root.children {
        let Some(item_id) = item.name.parse::<u32>().ok().filter(|id| *id > 0) else {
            continue;
        };
        if item_id / 10_000 != 238 || item.name != format!("{item_id:08}") {
            diagnostics.omit(
                "malformed_card_source",
                format!(
                    "Consume/0238.img entry {:?} is not an exact eight-digit card item source",
                    item.name
                ),
            );
            continue;
        }
        let Some(info) = child(item, "info") else {
            diagnostics.omit(
                "malformed_card_source",
                format!("Monster Book card {item_id} has no info node"),
            );
            continue;
        };
        let Some(spec) = child(item, "spec") else {
            diagnostics.omit(
                "malformed_card_source",
                format!("Monster Book card {item_id} has no spec node"),
            );
            continue;
        };
        let valid_marker = child(info, "monsterBook").and_then(exact_wz_int) == Some(1);
        let source_mob_id = child(info, "mob")
            .and_then(exact_wz_int)
            .and_then(|value| u32::try_from(value).ok())
            .filter(|value| *value > 0);
        let consume_on_pickup = child(spec, "consumeOnPickup").and_then(exact_wz_int) == Some(1);
        let Some(source_mob_id) = source_mob_id else {
            diagnostics.omit(
                "malformed_card_source",
                format!("Monster Book card {item_id} has no positive exact int info/mob"),
            );
            continue;
        };
        if !valid_marker || !consume_on_pickup {
            diagnostics.omit(
                "malformed_card_source",
                format!(
                    "Monster Book card {item_id} must have exact int monsterBook=1 and \
                     consumeOnPickup=1"
                ),
            );
            continue;
        }
        let source = CardSource {
            item_id,
            slot_max: child(info, "slotMax").and_then(positive_integer),
        };
        if let Some(existing) = output.insert(source_mob_id, source) {
            let retained = existing.item_id.min(item_id);
            output.insert(
                source_mob_id,
                if retained == existing.item_id {
                    existing
                } else {
                    source
                },
            );
            diagnostics.omit(
                "duplicate_card_mapping",
                format!(
                    "mob {source_mob_id} maps to card items {} and {item_id}; lower item ID \
                     {retained} is retained",
                    existing.item_id
                ),
            );
        }
    }
    output
}

fn extract_mob_names(
    root: &NodeTree,
    wanted: &BTreeSet<u32>,
    diagnostics: &mut Diagnostics,
) -> BTreeMap<u32, String> {
    let mut output = BTreeMap::new();
    for entry in &root.children {
        let Some(mob_id) = entry.name.parse::<u32>().ok() else {
            continue;
        };
        if !wanted.contains(&mob_id) {
            continue;
        }
        let Some(name) = child(entry, "name")
            .and_then(|node| node.value.as_ref())
            .and_then(serde_json::Value::as_str)
            .filter(|name| !name.trim().is_empty())
        else {
            diagnostics.warn(format!(
                "String.wz/Mob.img has no usable name for mob {mob_id}"
            ));
            continue;
        };
        output.insert(mob_id, name.to_owned());
    }
    output
}

fn load_mob_facts(
    archive: &Archive,
    wanted: &BTreeSet<u32>,
    image_paths: &BTreeMap<u32, String>,
    names: &BTreeMap<u32, String>,
    diagnostics: &mut Diagnostics,
) -> BTreeMap<u32, MobFact> {
    wanted
        .iter()
        .filter_map(|mob_id| {
            let Some(image_path) = image_paths.get(mob_id) else {
                diagnostics.omit(
                    "missing_mob_source",
                    format!("mob {mob_id} is absent from Mob.wz"),
                );
                return None;
            };
            let info_path = format!("{image_path}/info");
            let info = match tree(archive, &info_path, MOB_NODE_LIMIT) {
                Ok(info) => info,
                Err(error) => {
                    diagnostics.omit(
                        "malformed_mob_source",
                        format!("mob {mob_id} metadata cannot be read at {info_path}: {error:#}"),
                    );
                    return None;
                }
            };
            match read_mob_fact(*mob_id, names.get(mob_id).cloned(), &info) {
                Some(fact) => Some((*mob_id, fact)),
                None => {
                    diagnostics.omit(
                        "malformed_mob_source",
                        format!(
                            "mob {mob_id} must have a positive integer level, nonnegative integer \
                             exp, and integer boss flag when present"
                        ),
                    );
                    None
                }
            }
        })
        .collect()
}

pub(crate) fn read_mob_fact(
    mob_id: u32,
    name: Option<String>,
    info: &NodeTree,
) -> Option<MobFact> {
    let level = child(info, "level").and_then(positive_integer)?;
    let experience = match child(info, "exp") {
        Some(value) => nonnegative_u64(value)?,
        None => 0,
    };
    let boss = match child(info, "boss") {
        Some(value) => integer_value(value)? != 0,
        None => false,
    };
    Some(MobFact {
        mob_id,
        name,
        level,
        experience,
        boss,
    })
}

pub(crate) fn extract_completion_quests(
    root: &NodeTree,
    wanted_items: &BTreeSet<u32>,
    diagnostics: &mut Diagnostics,
) -> BTreeMap<u32, BTreeSet<u32>> {
    let mut output = BTreeMap::<u32, BTreeSet<u32>>::new();
    for quest in &root.children {
        let Some(quest_id) = quest.name.parse::<u32>().ok().filter(|id| *id > 0) else {
            continue;
        };
        let Some(items) = child(quest, "1").and_then(|completion| child(completion, "item")) else {
            continue;
        };
        for entry in &items.children {
            let item_id = child(entry, "id").and_then(positive_integer);
            let Some(item_id) = item_id.filter(|item_id| wanted_items.contains(item_id)) else {
                continue;
            };
            let count = match child(entry, "count") {
                Some(count) => {
                    let Some(count) = integer_value(count) else {
                        diagnostics.omit(
                            "malformed_quest_requirement",
                            format!(
                                "quest {quest_id} completion item {item_id} has a noninteger count"
                            ),
                        );
                        continue;
                    };
                    count
                }
                None => 1,
            };
            if count <= 0 {
                diagnostics.omit(
                    "nonpositive_quest_requirement",
                    format!(
                        "quest {quest_id} completion item {item_id} has nonpositive count {count}"
                    ),
                );
                continue;
            }
            output.entry(item_id).or_default().insert(quest_id);
        }
    }
    output
}

pub(crate) fn extract_completion_mobs(
    root: &NodeTree,
    wanted_quests: &BTreeSet<u32>,
    diagnostics: &mut Diagnostics,
) -> BTreeMap<u32, BTreeSet<u32>> {
    let mut output = BTreeMap::<u32, BTreeSet<u32>>::new();
    for quest in &root.children {
        let Some(quest_id) = quest.name.parse::<u32>().ok().filter(|id| *id > 0) else {
            continue;
        };
        if !wanted_quests.contains(&quest_id) {
            continue;
        }
        let Some(mobs) = child(quest, "1").and_then(|completion| child(completion, "mob")) else {
            continue;
        };
        for entry in &mobs.children {
            let Some(mob_id) = child(entry, "id").and_then(positive_integer) else {
                continue;
            };
            let count = match child(entry, "count") {
                Some(count) => {
                    let Some(count) = integer_value(count) else {
                        diagnostics.omit(
                            "malformed_quest_requirement",
                            format!(
                                "quest {quest_id} completion mob {mob_id} has a noninteger count"
                            ),
                        );
                        continue;
                    };
                    count
                }
                None => 1,
            };
            if count <= 0 {
                diagnostics.omit(
                    "nonpositive_quest_requirement",
                    format!(
                        "quest {quest_id} completion mob {mob_id} has nonpositive count {count}"
                    ),
                );
                continue;
            }
            output.entry(quest_id).or_default().insert(mob_id);
        }
    }
    output
}

fn validate_policy_mob_drop_sources(
    drops: &[PolicyMobDrop],
    mobs: &BTreeMap<u32, MobFact>,
    items: &BTreeMap<u32, ItemFact>,
    completion_quests: &BTreeMap<u32, BTreeSet<u32>>,
    completion_mobs: &BTreeMap<u32, BTreeSet<u32>>,
    source_map_mobs: &BTreeMap<u32, BTreeSet<u32>>,
    validated_skill_ids: &BTreeSet<u32>,
) -> Result<()> {
    for (index, drop) in drops.iter().enumerate() {
        if !mobs.contains_key(&drop.mob_id) {
            bail!(
                "mob_drops[{index}] mob {} has no valid Mob.wz source",
                drop.mob_id
            );
        }
        let Some(item) = items.get(&drop.item_id) else {
            bail!(
                "mob_drops[{index}] item {} has no valid supported item source",
                drop.item_id
            );
        };
        if let Some(slot_max) = item.slot_max
            && drop.maximum_quantity > slot_max
        {
            bail!(
                "mob_drops[{index}] maximum quantity {} exceeds item {} slotMax {}",
                drop.maximum_quantity,
                drop.item_id,
                slot_max
            );
        }
        if let Some(quest_id) = drop.quest_id {
            if !completion_quests
                .get(&drop.item_id)
                .is_some_and(|quest_ids| quest_ids.contains(&quest_id))
            {
                bail!(
                    "mob_drops[{index}] Quest.wz quest {quest_id} does not positively require \
                     item {} at completion",
                    drop.item_id
                );
            }
            if !completion_mobs
                .get(&quest_id)
                .is_some_and(|mob_ids| mob_ids.contains(&drop.mob_id))
            {
                bail!(
                    "mob_drops[{index}] Quest.wz quest {quest_id} does not positively require mob \
                     {} at completion",
                    drop.mob_id
                );
            }
        }
        if let Some(map_id) = drop.source_map_id
            && !source_map_mobs
                .get(&map_id)
                .is_some_and(|mob_ids| mob_ids.contains(&drop.mob_id))
        {
            bail!(
                "mob_drops[{index}] Map.wz map {map_id} does not contain mob {}",
                drop.mob_id
            );
        }
        if let Some(skill_id) = drop.required_skill_id
            && !validated_skill_ids.contains(&skill_id)
        {
            bail!("mob_drops[{index}] required skill {skill_id} has no valid Skill.wz source");
        }
    }
    Ok(())
}

fn load_source_map_mobs(
    archive: &Archive,
    map_ids: &BTreeSet<u32>,
) -> Result<BTreeMap<u32, BTreeSet<u32>>> {
    map_ids
        .iter()
        .map(|map_id| {
            let directory = map_id / 100_000_000;
            let path = format!("/Map/Map{directory}/{map_id:09}.img/life");
            let life = tree(archive, &path, MAP_LIFE_NODE_LIMIT)
                .with_context(|| format!("failed to read Map.wz source map {map_id}"))?;
            let mob_ids = life
                .children
                .iter()
                .filter(|spawn| child(spawn, "type").and_then(string_value) == Some("m"))
                .filter_map(|spawn| child(spawn, "id").and_then(numeric_id))
                .collect::<BTreeSet<_>>();
            Ok((*map_id, mob_ids))
        })
        .collect()
}

fn load_required_skill_ids(
    archive: &Archive,
    skill_ids: &BTreeSet<u32>,
) -> Result<BTreeSet<u32>> {
    skill_ids
        .iter()
        .map(|skill_id| {
            let job_id = skill_id / 10_000;
            let path = format!("/{job_id}.img/skill/{skill_id}");
            tree(archive, &path, SKILL_NODE_LIMIT)
                .with_context(|| format!("failed to read Skill.wz skill {skill_id}"))?;
            Ok(*skill_id)
        })
        .collect()
}

fn load_item_facts(
    item_archive: &Archive,
    character_archive: Option<&Archive>,
    wanted: &BTreeSet<u32>,
    cards: &BTreeMap<u32, CardSource>,
    equipment_paths: &BTreeMap<u32, String>,
    diagnostics: &mut Diagnostics,
) -> BTreeMap<u32, ItemFact> {
    let card_sources = cards
        .values()
        .map(|card| (card.item_id, *card))
        .collect::<BTreeMap<_, _>>();
    let mut output = BTreeMap::new();
    for item_id in wanted {
        if let Some(card) = card_sources.get(item_id) {
            output.insert(
                *item_id,
                ItemFact {
                    item_id: *item_id,
                    kind: ItemKind::MonsterBookCard,
                    slot_max: card.slot_max,
                },
            );
            continue;
        }
        if *item_id / 10_000 == 238 {
            diagnostics.omit(
                "unvalidated_card_reward",
                format!(
                    "MonsterBook.img references card-like item {item_id} without a validated \
                     Consume/0238.img mapping"
                ),
            );
            continue;
        }
        let (archive, path, kind) = match *item_id / 1_000_000 {
            1 => {
                let Some(archive) = character_archive else {
                    diagnostics.omit(
                        "missing_equipment_source",
                        format!(
                            "equipment reward {item_id} cannot be validated without Character.wz"
                        ),
                    );
                    continue;
                };
                let Some(path) = equipment_paths.get(item_id) else {
                    diagnostics.omit(
                        "missing_equipment_source",
                        format!("equipment reward {item_id} is absent from Character.wz"),
                    );
                    continue;
                };
                (archive, path.clone(), ItemKind::Equipment)
            }
            2 => (
                item_archive,
                ordinary_item_path("Consume", *item_id),
                ItemKind::Consume,
            ),
            4 => (
                item_archive,
                ordinary_item_path("Etc", *item_id),
                ItemKind::Etc,
            ),
            3 | 5 => {
                diagnostics.omit(
                    "unsupported_item_category",
                    format!(
                        "item {item_id} is an install, cash, or pet reward and is not generated"
                    ),
                );
                continue;
            }
            _ => {
                diagnostics.omit(
                    "unsupported_item_category",
                    format!("item {item_id} does not belong to a supported inventory category"),
                );
                continue;
            }
        };
        let info = match tree(archive, &path, ITEM_NODE_LIMIT) {
            Ok(info) => info,
            Err(error) => {
                diagnostics.omit(
                    "missing_or_malformed_item_source",
                    format!("item {item_id} source cannot be read at {path}: {error:#}"),
                );
                continue;
            }
        };
        let slot_max = match child(&info, "slotMax") {
            Some(value) => match positive_integer(value) {
                Some(value) => Some(value),
                None => {
                    diagnostics.omit(
                        "malformed_item_source",
                        format!("item {item_id} has a nonpositive or noninteger info/slotMax"),
                    );
                    continue;
                }
            },
            None => None,
        };
        output.insert(
            *item_id,
            ItemFact {
                item_id: *item_id,
                kind,
                slot_max: if kind == ItemKind::Equipment {
                    Some(1)
                } else {
                    slot_max
                },
            },
        );
    }
    output
}

fn ordinary_item_path(
    directory: &str,
    item_id: u32,
) -> String {
    format!("/{directory}/{:04}.img/{item_id:08}/info", item_id / 10_000)
}

fn numeric_image_paths(
    archive: &Archive,
    path: &str,
) -> Result<BTreeMap<u32, String>> {
    Ok(list_all(archive, path)?
        .into_iter()
        .filter(|node| node.kind == "image")
        .filter_map(|node| numeric_image_id(&node.name).map(|item_id| (item_id, node.path)))
        .collect())
}

fn equipment_image_paths(
    archive: &Archive,
    wanted: &BTreeSet<u32>,
) -> Result<BTreeMap<u32, String>> {
    let mut output = BTreeMap::new();
    for directory in list_all(archive, "/")?
        .into_iter()
        .filter(|node| node.kind == "directory")
    {
        for image in list_all(archive, &directory.path)?
            .into_iter()
            .filter(|node| node.kind == "image")
        {
            let Some(item_id) = numeric_image_id(&image.name) else {
                continue;
            };
            if wanted.contains(&item_id) {
                output.insert(item_id, format!("{}/info", image.path));
            }
        }
    }
    Ok(output)
}

fn numeric_image_id(name: &str) -> Option<u32> {
    name.strip_suffix(".img")?.parse().ok()
}

fn list_all(
    archive: &Archive,
    path: &str,
) -> Result<Vec<NodeSummary>> {
    let mut offset = 0;
    let mut output = Vec::new();
    loop {
        let page = list(archive, path, offset, LIST_PAGE_SIZE)?;
        output.extend(page.nodes);
        let Some(next) = page.next_offset else {
            break;
        };
        offset = next;
    }
    Ok(output)
}

fn child<'a>(
    node: &'a NodeTree,
    name: &str,
) -> Option<&'a NodeTree> {
    node.children.iter().find(|child| child.name == name)
}

fn positive_leaf_integers(node: &NodeTree) -> Vec<u32> {
    let mut output = Vec::new();
    collect_positive_leaf_integers(node, &mut output);
    output
}

fn collect_positive_leaf_integers(
    node: &NodeTree,
    output: &mut Vec<u32>,
) {
    if node.children.is_empty() {
        if let Some(value) = positive_integer(node) {
            output.push(value);
        }
        return;
    }
    for child in &node.children {
        collect_positive_leaf_integers(child, output);
    }
}

fn exact_wz_int(node: &NodeTree) -> Option<i64> {
    (node.kind == "int").then(|| integer_value(node)).flatten()
}

fn positive_integer(node: &NodeTree) -> Option<u32> {
    integer_value(node)
        .and_then(|value| u32::try_from(value).ok())
        .filter(|value| *value > 0)
}

fn nonnegative_u64(node: &NodeTree) -> Option<u64> {
    integer_value(node).and_then(|value| u64::try_from(value).ok())
}

fn integer_value(node: &NodeTree) -> Option<i64> {
    let value = node.value.as_ref()?;
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok()))
}

fn string_value(node: &NodeTree) -> Option<&str> {
    node.value.as_ref()?.as_str()
}

fn numeric_id(node: &NodeTree) -> Option<u32> {
    positive_integer(node).or_else(|| string_value(node)?.parse().ok())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn monster_book_associations_use_entry_mob_ids_and_deduplicate_rewards() {
        let root = branch(
            "MonsterBook.img",
            vec![
                branch(
                    "100",
                    vec![branch(
                        "reward",
                        vec![scalar("0", json!(4_000_000)), scalar("1", json!(4_000_000))],
                    )],
                ),
                branch(
                    "101",
                    vec![branch("reward", vec![scalar("0", json!(2_000_000))])],
                ),
            ],
        );
        let mut diagnostics = Diagnostics::default();

        let associations = extract_monster_book(&root, &mut diagnostics);

        assert_eq!(associations[&100], BTreeSet::from([4_000_000]));
        assert_eq!(associations[&101], BTreeSet::from([2_000_000]));
        assert_eq!(
            diagnostics.omissions["duplicate_monster_book_association"],
            1
        );
    }

    #[test]
    fn card_sources_require_exact_markers_and_add_the_source_mob_mapping() {
        let root = branch(
            "0238.img",
            vec![branch(
                "02380000",
                vec![
                    branch(
                        "info",
                        vec![scalar("monsterBook", json!(1)), scalar("mob", json!(100))],
                    ),
                    branch("spec", vec![scalar("consumeOnPickup", json!(1))]),
                ],
            )],
        );
        let mut diagnostics = Diagnostics::default();

        let cards = extract_card_sources(&root, &mut diagnostics);

        assert_eq!(cards[&100].item_id, 2_380_000);
        assert!(diagnostics.omissions.is_empty());
    }

    #[test]
    fn completion_requirements_only_keep_positive_wanted_items() {
        let root = branch(
            "Check.img",
            vec![
                quest(1000, 4_030_000, 3),
                quest(1001, 4_030_000, 0),
                quest(1002, 4_030_001, 2),
            ],
        );
        let mut diagnostics = Diagnostics::default();

        let requirements =
            extract_completion_quests(&root, &BTreeSet::from([4_030_000]), &mut diagnostics);

        assert_eq!(requirements[&4_030_000], BTreeSet::from([1000]));
        assert_eq!(diagnostics.omissions["nonpositive_quest_requirement"], 1);
    }

    #[test]
    fn completion_mob_requirements_only_keep_positive_wanted_quests() {
        let root = branch(
            "Check.img",
            vec![
                quest_with_mob(1000, 100, 1),
                quest_with_mob(1001, 101, 0),
                quest_with_mob(1002, 102, 1),
            ],
        );
        let mut diagnostics = Diagnostics::default();

        let requirements =
            extract_completion_mobs(&root, &BTreeSet::from([1000, 1001]), &mut diagnostics);

        assert_eq!(requirements[&1000], BTreeSet::from([100]));
        assert!(!requirements.contains_key(&1002));
        assert_eq!(diagnostics.omissions["nonpositive_quest_requirement"], 1);
    }

    #[test]
    fn explicit_quest_mob_drop_requires_matching_wz_requirements() {
        let drop = PolicyMobDrop {
            mob_id: 100,
            item_id: 4_030_000,
            quest_id: Some(1000),
            source_map_id: None,
            required_skill_id: None,
            chance_per_million: 1_000_000,
            minimum_quantity: 1,
            maximum_quantity: 1,
        };
        let mobs = BTreeMap::from([(
            100,
            MobFact {
                mob_id: 100,
                name: None,
                level: 1,
                experience: 1,
                boss: false,
            },
        )]);
        let items = BTreeMap::from([(
            4_030_000,
            ItemFact {
                item_id: 4_030_000,
                kind: ItemKind::Etc,
                slot_max: Some(100),
            },
        )]);
        let item_requirements = BTreeMap::from([(4_030_000, BTreeSet::from([1000]))]);
        let mob_requirements = BTreeMap::from([(1000, BTreeSet::from([101]))]);

        let error = validate_policy_mob_drop_sources(
            &[drop],
            &mobs,
            &items,
            &item_requirements,
            &mob_requirements,
            &BTreeMap::new(),
            &BTreeSet::new(),
        )
        .expect_err("mismatched mob requirement");

        assert!(
            error
                .to_string()
                .contains("does not positively require mob 100")
        );
    }

    #[test]
    fn explicit_quest_mob_drop_must_fit_the_item_stack() {
        let drop = PolicyMobDrop {
            mob_id: 100,
            item_id: 4_030_000,
            quest_id: Some(1000),
            source_map_id: None,
            required_skill_id: None,
            chance_per_million: 1_000_000,
            minimum_quantity: 1,
            maximum_quantity: 2,
        };
        let mobs = BTreeMap::from([(
            100,
            MobFact {
                mob_id: 100,
                name: None,
                level: 1,
                experience: 1,
                boss: false,
            },
        )]);
        let items = BTreeMap::from([(
            4_030_000,
            ItemFact {
                item_id: 4_030_000,
                kind: ItemKind::Etc,
                slot_max: Some(1),
            },
        )]);
        let item_requirements = BTreeMap::from([(4_030_000, BTreeSet::from([1000]))]);
        let mob_requirements = BTreeMap::from([(1000, BTreeSet::from([100]))]);

        let error = validate_policy_mob_drop_sources(
            &[drop],
            &mobs,
            &items,
            &item_requirements,
            &mob_requirements,
            &BTreeMap::new(),
            &BTreeSet::new(),
        )
        .expect_err("quantity exceeds slotMax");

        assert!(error.to_string().contains("exceeds item 4030000 slotMax 1"));
    }

    #[test]
    fn policy_mob_drop_validates_its_source_map_and_required_skill() {
        let drop = PolicyMobDrop {
            mob_id: 100,
            item_id: 4_030_000,
            quest_id: None,
            source_map_id: Some(200),
            required_skill_id: Some(500),
            chance_per_million: 1_000_000,
            minimum_quantity: 1,
            maximum_quantity: 1,
        };
        let mobs = BTreeMap::from([(
            100,
            MobFact {
                mob_id: 100,
                name: None,
                level: 1,
                experience: 1,
                boss: false,
            },
        )]);
        let items = BTreeMap::from([(
            4_030_000,
            ItemFact {
                item_id: 4_030_000,
                kind: ItemKind::Etc,
                slot_max: Some(100),
            },
        )]);
        let source_maps = BTreeMap::from([(200, BTreeSet::from([100]))]);

        validate_policy_mob_drop_sources(
            &[drop],
            &mobs,
            &items,
            &BTreeMap::new(),
            &BTreeMap::new(),
            &source_maps,
            &BTreeSet::from([500]),
        )
        .expect("matching map and skill sources");

        let error = validate_policy_mob_drop_sources(
            &[drop],
            &mobs,
            &items,
            &BTreeMap::new(),
            &BTreeMap::new(),
            &BTreeMap::from([(200, BTreeSet::from([101]))]),
            &BTreeSet::from([500]),
        )
        .expect_err("mismatched source map");
        assert!(
            error
                .to_string()
                .contains("map 200 does not contain mob 100")
        );
    }

    #[test]
    fn mob_metadata_requires_level_and_accepts_absent_exp_and_boss() {
        let info = branch("info", vec![scalar("level", json!(12))]);
        assert_eq!(
            read_mob_fact(100, Some("Slime".to_owned()), &info),
            Some(MobFact {
                mob_id: 100,
                name: Some("Slime".to_owned()),
                level: 12,
                experience: 0,
                boss: false,
            })
        );
        assert!(read_mob_fact(100, None, &branch("info", Vec::new())).is_none());
    }

    fn quest(
        quest_id: u32,
        item_id: u32,
        count: i64,
    ) -> NodeTree {
        branch(
            &quest_id.to_string(),
            vec![branch(
                "1",
                vec![branch(
                    "item",
                    vec![branch(
                        "0",
                        vec![scalar("id", json!(item_id)), scalar("count", json!(count))],
                    )],
                )],
            )],
        )
    }

    fn quest_with_mob(
        quest_id: u32,
        mob_id: u32,
        count: i64,
    ) -> NodeTree {
        branch(
            &quest_id.to_string(),
            vec![branch(
                "1",
                vec![branch(
                    "mob",
                    vec![branch(
                        "0",
                        vec![scalar("id", json!(mob_id)), scalar("count", json!(count))],
                    )],
                )],
            )],
        )
    }

    fn branch(
        name: &str,
        children: Vec<NodeTree>,
    ) -> NodeTree {
        NodeTree {
            name: name.to_owned(),
            kind: "property",
            value: None,
            details: None,
            children,
        }
    }

    fn scalar(
        name: &str,
        value: serde_json::Value,
    ) -> NodeTree {
        NodeTree {
            name: name.to_owned(),
            kind: "int",
            value: Some(value),
            details: None,
            children: Vec::new(),
        }
    }
}
