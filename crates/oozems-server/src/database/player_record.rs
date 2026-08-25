use std::collections::BTreeSet;

use oozems_proto::v1::CharacterAppearance;
use oozems_proto::v1::CharacterGender;
use oozems_proto::v1::CharacterStats;
use oozems_proto::v1::EquipmentSlot;
use oozems_proto::v1::EquippedItem;
use oozems_proto::v1::InventoryItemStack;
use oozems_proto::v1::InventoryState;
use oozems_proto::v1::KeyBinding;
use oozems_proto::v1::LearnedSkill;
use oozems_proto::v1::MonsterBookCard;
use oozems_proto::v1::PlayerQuest;
use oozems_proto::v1::PlayerState;
use oozems_proto::v1::QuestMobProgress;
use oozems_proto::v1::QuestRecord;
use oozems_proto::v1::QuestRecordEntry;
use oozems_proto::v1::QuestStatus;
use oozems_proto::v1::Vec2;
use surrealdb::types::SurrealValue;
use surrealdb::types::Value;

use super::CharacterName;
use super::PlayerId;

const CORRUPT_RECORD: &str = "persisted player is corrupt";
const INVALID_PLAYER: &str = "invalid player state";

#[derive(Clone, Debug, SurrealValue)]
pub(super) struct PlayerRecord {
    revision: i64,
    name: String,
    level: i64,
    map_id: i64,
    position: PositionRecord,
    appearance: AppearanceRecord,
    stats: StatsRecord,
    inventory: InventoryRecord,
    key_bindings: Vec<KeyBindingRecord>,
    skill_points: i64,
    learned_skills: Vec<LearnedSkillRecord>,
    mesos: i64,
    cash_points: i64,
    quests: Vec<QuestRecordData>,
    quest_records: Vec<PersistedQuestRecord>,
    monster_book_cards: Vec<MonsterBookCardRecord>,
}

#[derive(Clone, Debug, SurrealValue)]
struct PositionRecord {
    x: f64,
    y: f64,
}

#[derive(Clone, Debug, SurrealValue)]
struct AppearanceRecord {
    gender: i64,
    skin_id: i64,
    face_id: i64,
    hair_id: i64,
}

#[derive(Clone, Debug, SurrealValue)]
struct StatsRecord {
    job_id: i64,
    hp: i64,
    max_hp: i64,
    mp: i64,
    max_mp: i64,
    experience: i64,
    experience_required: i64,
    fame: i64,
    ability_points: i64,
    strength: i64,
    dexterity: i64,
    intelligence: i64,
    luck: i64,
}

#[derive(Clone, Debug, SurrealValue)]
struct InventoryRecord {
    capacity: i64,
    stacks: Vec<InventoryStackRecord>,
    equipment: Vec<EquippedItemRecord>,
}

#[derive(Clone, Debug, SurrealValue)]
struct InventoryStackRecord {
    item_id: i64,
    quantity: i64,
    expires_at_unix_ms: i64,
}

#[derive(Clone, Debug, SurrealValue)]
struct EquippedItemRecord {
    slot: i64,
    item_id: i64,
    expires_at_unix_ms: i64,
}

#[derive(Clone, Debug, SurrealValue)]
struct KeyBindingRecord {
    code: String,
    action: i64,
    skill_id: i64,
}

#[derive(Clone, Debug, SurrealValue)]
struct LearnedSkillRecord {
    skill_id: i64,
    level: i64,
    master_level: i64,
}

#[derive(Clone, Debug, SurrealValue)]
struct QuestRecordData {
    quest_id: i64,
    status: i64,
    mob_progress: Vec<QuestMobProgressRecord>,
    accepted_at_unix_ms: i64,
    completed_at_unix_ms: i64,
    dialogue_step: i64,
    completion_quiz_passed: bool,
}

#[derive(Clone, Debug, SurrealValue)]
struct QuestMobProgressRecord {
    mob_id: i64,
    count: i64,
}

#[derive(Clone, Debug, SurrealValue)]
struct PersistedQuestRecord {
    quest_id: i64,
    entries: Vec<QuestRecordEntryRecord>,
}

#[derive(Clone, Debug, SurrealValue)]
struct QuestRecordEntryRecord {
    index: i64,
    value: String,
}

#[derive(Clone, Debug, SurrealValue)]
struct MonsterBookCardRecord {
    card_item_id: i64,
    count: i64,
}

#[derive(Clone, Debug, SurrealValue)]
pub(super) struct PlayerPositionData {
    map_id: i64,
    position: PositionRecord,
}

pub(super) fn invalid_player_error(error: impl std::fmt::Display) -> surrealdb::Error {
    data_error(INVALID_PLAYER, error)
}

pub(super) fn decode_player_record(value: Value) -> surrealdb::Result<PlayerRecord> {
    let Value::Object(mut object) = value else {
        return Err(data_error(CORRUPT_RECORD, "record is not an object"));
    };
    object.remove("id");
    PlayerRecord::from_value(Value::Object(object))
        .map_err(|error| data_error(CORRUPT_RECORD, error))
}

pub(super) fn record_revision(record: PlayerRecord) -> surrealdb::Result<u64> {
    persisted_u64(record.revision, "revision")
}

pub(super) fn player_from_record(
    player_id: &PlayerId,
    record: PlayerRecord,
) -> surrealdb::Result<PlayerState> {
    CharacterName::parse(&record.name).map_err(|error| data_error(CORRUPT_RECORD, error))?;
    let level = persisted_u32(record.level, "level")?;
    if level == 0 {
        return Err(data_error(CORRUPT_RECORD, "level must be positive"));
    }
    let stats = stats_from_record(record.stats)?;
    validate_stats(&stats, CORRUPT_RECORD)?;
    let inventory = inventory_from_record(record.inventory)?;
    validate_inventory(&inventory, CORRUPT_RECORD)?;
    let key_bindings = key_bindings_from_records(record.key_bindings)?;
    validate_key_bindings(&key_bindings, CORRUPT_RECORD)?;
    let learned_skills = canonicalize_learned_skills(
        learned_skills_from_records(record.learned_skills)?,
        CORRUPT_RECORD,
    )?;
    let quests = canonicalize_quests(quests_from_records(record.quests)?, CORRUPT_RECORD)?;
    let quest_records =
        crate::quest_records::canonicalize(quest_records_from_records(record.quest_records)?)
            .map_err(|error| data_error(CORRUPT_RECORD, error))?;
    let monster_book_cards = crate::monster_book::canonicalize(monster_book_cards_from_records(
        record.monster_book_cards,
    )?)
    .map_err(|error| data_error(CORRUPT_RECORD, error))?;

    Ok(PlayerState {
        id: player_id.as_str().to_owned(),
        name: record.name,
        level,
        map_id: persisted_u32(record.map_id, "map_id")?,
        position: Some(position_from_record(record.position)?),
        appearance: Some(appearance_from_record(record.appearance)?),
        stats: Some(stats),
        inventory: Some(inventory),
        key_bindings,
        skill_points: persisted_u32(record.skill_points, "skill_points")?,
        learned_skills,
        mesos: persisted_u64(record.mesos, "mesos")?,
        quests,
        revision: persisted_u64(record.revision, "revision")?,
        quest_records,
        monster_book_cards,
        cash_points: persisted_u64(record.cash_points, "cash_points")?,
    })
}

pub(super) fn record_from_player(
    player: &PlayerState,
    revision: u64,
) -> surrealdb::Result<PlayerRecord> {
    PlayerId::parse(&player.id).map_err(|error| data_error(INVALID_PLAYER, error))?;
    CharacterName::parse(&player.name).map_err(|error| data_error(INVALID_PLAYER, error))?;
    if player.level == 0 {
        return Err(data_error(INVALID_PLAYER, "level must be positive"));
    }
    let position = player
        .position
        .as_ref()
        .ok_or_else(|| data_error(INVALID_PLAYER, "position is required"))?;
    let appearance = player
        .appearance
        .as_ref()
        .ok_or_else(|| data_error(INVALID_PLAYER, "appearance is required"))?;
    let stats = player
        .stats
        .as_ref()
        .ok_or_else(|| data_error(INVALID_PLAYER, "stats are required"))?;
    let inventory = player
        .inventory
        .as_ref()
        .ok_or_else(|| data_error(INVALID_PLAYER, "inventory is required"))?;
    validate_appearance(appearance, INVALID_PLAYER)?;
    validate_stats(stats, INVALID_PLAYER)?;
    validate_inventory(inventory, INVALID_PLAYER)?;
    validate_key_bindings(&player.key_bindings, INVALID_PLAYER)?;

    let learned_skills =
        canonicalize_learned_skills(player.learned_skills.clone(), INVALID_PLAYER)?;
    let quests = canonicalize_quests(player.quests.clone(), INVALID_PLAYER)?;
    let quest_records = crate::quest_records::canonicalize(player.quest_records.clone())
        .map_err(|error| data_error(INVALID_PLAYER, error))?;
    let monster_book_cards = crate::monster_book::canonicalize(player.monster_book_cards.clone())
        .map_err(|error| data_error(INVALID_PLAYER, error))?;

    Ok(PlayerRecord {
        revision: persisted_i64(revision, "revision")?,
        name: player.name.clone(),
        level: i64::from(player.level),
        map_id: i64::from(player.map_id),
        position: position_record(position, INVALID_PLAYER)?,
        appearance: AppearanceRecord {
            gender: i64::from(appearance.gender),
            skin_id: i64::from(appearance.skin_id),
            face_id: i64::from(appearance.face_id),
            hair_id: i64::from(appearance.hair_id),
        },
        stats: stats_record(stats)?,
        inventory: inventory_record(inventory)?,
        key_bindings: player.key_bindings.iter().map(key_binding_record).collect(),
        skill_points: i64::from(player.skill_points),
        learned_skills: learned_skills.iter().map(learned_skill_record).collect(),
        mesos: persisted_i64(player.mesos, "mesos")?,
        cash_points: persisted_i64(player.cash_points, "cash_points")?,
        quests: quests
            .iter()
            .map(quest_record_data)
            .collect::<Result<_, _>>()?,
        quest_records: quest_records.iter().map(persisted_quest_record).collect(),
        monster_book_cards: monster_book_cards
            .iter()
            .map(monster_book_card_record)
            .collect(),
    })
}

pub(super) fn position_data_from_player(
    player: &PlayerState
) -> surrealdb::Result<PlayerPositionData> {
    PlayerId::parse(&player.id).map_err(|error| data_error(INVALID_PLAYER, error))?;
    let position = player
        .position
        .as_ref()
        .ok_or_else(|| data_error(INVALID_PLAYER, "position is required"))?;
    Ok(PlayerPositionData {
        map_id: i64::from(player.map_id),
        position: position_record(position, INVALID_PLAYER)?,
    })
}

fn validate_appearance(
    appearance: &CharacterAppearance,
    context: &str,
) -> surrealdb::Result<()> {
    let gender = CharacterGender::try_from(appearance.gender)
        .map_err(|_| data_error(context, "appearance gender is invalid"))?;
    if gender == CharacterGender::Unspecified {
        return Err(data_error(context, "appearance gender is unspecified"));
    }
    Ok(())
}

fn validate_stats(
    stats: &CharacterStats,
    context: &str,
) -> surrealdb::Result<()> {
    if stats.max_hp == 0 || stats.hp > stats.max_hp {
        return Err(data_error(context, "HP values are invalid"));
    }
    if stats.max_mp == 0 || stats.mp > stats.max_mp {
        return Err(data_error(context, "MP values are invalid"));
    }
    if stats.experience_required == 0 {
        return Err(data_error(context, "experience_required must be positive"));
    }
    Ok(())
}

fn validate_inventory(
    inventory: &InventoryState,
    context: &str,
) -> surrealdb::Result<()> {
    if inventory.capacity == 0 || inventory.stacks.len() > inventory.capacity as usize {
        return Err(data_error(context, "inventory capacity is invalid"));
    }
    for stack in &inventory.stacks {
        if stack.item_id == 0 || stack.quantity == 0 {
            return Err(data_error(
                context,
                "inventory stacks require positive item IDs and quantities",
            ));
        }
    }
    let mut slots = BTreeSet::new();
    for equipped in &inventory.equipment {
        let slot = EquipmentSlot::try_from(equipped.slot)
            .map_err(|_| data_error(context, "equipment slot is invalid"))?;
        if slot == EquipmentSlot::Unspecified || equipped.item_id == 0 {
            return Err(data_error(context, "equipped item is invalid"));
        }
        if !slots.insert(slot as i32) {
            return Err(data_error(context, "equipment slot appears more than once"));
        }
    }
    Ok(())
}

fn validate_key_bindings(
    bindings: &[KeyBinding],
    context: &str,
) -> surrealdb::Result<()> {
    crate::keymap::validate_bindings(bindings).map_err(|error| data_error(context, error))
}

fn canonicalize_learned_skills(
    mut skills: Vec<LearnedSkill>,
    context: &str,
) -> surrealdb::Result<Vec<LearnedSkill>> {
    skills.sort_unstable_by_key(|skill| skill.skill_id);
    let mut previous = None;
    for skill in &skills {
        if skill.skill_id == 0 || (skill.level == 0 && skill.master_level == 0) {
            return Err(data_error(context, "learned skill is invalid"));
        }
        if previous == Some(skill.skill_id) {
            return Err(data_error(
                context,
                format!("learned skill {} appears more than once", skill.skill_id),
            ));
        }
        previous = Some(skill.skill_id);
    }
    Ok(skills)
}

fn canonicalize_quests(
    mut quests: Vec<PlayerQuest>,
    context: &str,
) -> surrealdb::Result<Vec<PlayerQuest>> {
    for quest in &mut quests {
        if quest.quest_id == 0 {
            return Err(data_error(context, "quest ID must be positive"));
        }
        let status = QuestStatus::try_from(quest.status)
            .map_err(|_| data_error(context, "quest status is invalid"))?;
        if status == QuestStatus::Unspecified && quest.dialogue_step == 0 {
            return Err(data_error(
                context,
                "an unspecified quest requires pending dialogue",
            ));
        }
        quest
            .mob_progress
            .sort_unstable_by_key(|entry| entry.mob_id);
        let mut previous_mob = None;
        for progress in &quest.mob_progress {
            if progress.mob_id == 0 {
                return Err(data_error(context, "quest mob ID must be positive"));
            }
            if previous_mob == Some(progress.mob_id) {
                return Err(data_error(
                    context,
                    format!("quest mob {} appears more than once", progress.mob_id),
                ));
            }
            previous_mob = Some(progress.mob_id);
        }
    }
    quests.sort_unstable_by_key(|quest| quest.quest_id);
    if let Some(duplicates) = quests
        .windows(2)
        .find(|quests| quests[0].quest_id == quests[1].quest_id)
    {
        return Err(data_error(
            context,
            format!("quest {} appears more than once", duplicates[0].quest_id),
        ));
    }
    Ok(quests)
}

fn position_from_record(record: PositionRecord) -> surrealdb::Result<Vec2> {
    if !record.x.is_finite()
        || !record.y.is_finite()
        || record.x.abs() > f64::from(f32::MAX)
        || record.y.abs() > f64::from(f32::MAX)
    {
        return Err(data_error(CORRUPT_RECORD, "position is not finite"));
    }
    Ok(Vec2 {
        x: record.x as f32,
        y: record.y as f32,
    })
}

fn position_record(
    position: &Vec2,
    context: &str,
) -> surrealdb::Result<PositionRecord> {
    if !position.x.is_finite() || !position.y.is_finite() {
        return Err(data_error(context, "position is not finite"));
    }
    Ok(PositionRecord {
        x: f64::from(position.x),
        y: f64::from(position.y),
    })
}

fn appearance_from_record(record: AppearanceRecord) -> surrealdb::Result<CharacterAppearance> {
    let appearance = CharacterAppearance {
        gender: persisted_i32(record.gender, "appearance.gender")?,
        skin_id: persisted_u32(record.skin_id, "appearance.skin_id")?,
        face_id: persisted_u32(record.face_id, "appearance.face_id")?,
        hair_id: persisted_u32(record.hair_id, "appearance.hair_id")?,
    };
    validate_appearance(&appearance, CORRUPT_RECORD)?;
    Ok(appearance)
}

fn stats_from_record(record: StatsRecord) -> surrealdb::Result<CharacterStats> {
    Ok(CharacterStats {
        job_id: persisted_u32(record.job_id, "stats.job_id")?,
        hp: persisted_u32(record.hp, "stats.hp")?,
        max_hp: persisted_u32(record.max_hp, "stats.max_hp")?,
        mp: persisted_u32(record.mp, "stats.mp")?,
        max_mp: persisted_u32(record.max_mp, "stats.max_mp")?,
        experience: persisted_u64(record.experience, "stats.experience")?,
        fame: persisted_i32(record.fame, "stats.fame")?,
        ability_points: persisted_u32(record.ability_points, "stats.ability_points")?,
        strength: persisted_u32(record.strength, "stats.strength")?,
        dexterity: persisted_u32(record.dexterity, "stats.dexterity")?,
        intelligence: persisted_u32(record.intelligence, "stats.intelligence")?,
        luck: persisted_u32(record.luck, "stats.luck")?,
        experience_required: persisted_u64(
            record.experience_required,
            "stats.experience_required",
        )?,
    })
}

fn stats_record(stats: &CharacterStats) -> surrealdb::Result<StatsRecord> {
    Ok(StatsRecord {
        job_id: i64::from(stats.job_id),
        hp: i64::from(stats.hp),
        max_hp: i64::from(stats.max_hp),
        mp: i64::from(stats.mp),
        max_mp: i64::from(stats.max_mp),
        experience: persisted_i64(stats.experience, "stats.experience")?,
        experience_required: persisted_i64(stats.experience_required, "stats.experience_required")?,
        fame: i64::from(stats.fame),
        ability_points: i64::from(stats.ability_points),
        strength: i64::from(stats.strength),
        dexterity: i64::from(stats.dexterity),
        intelligence: i64::from(stats.intelligence),
        luck: i64::from(stats.luck),
    })
}

fn inventory_from_record(record: InventoryRecord) -> surrealdb::Result<InventoryState> {
    let stacks = record
        .stacks
        .into_iter()
        .map(|stack| {
            Ok(InventoryItemStack {
                item_id: persisted_u32(stack.item_id, "inventory.stacks.item_id")?,
                quantity: persisted_u32(stack.quantity, "inventory.stacks.quantity")?,
                expires_at_unix_ms: persisted_u64(
                    stack.expires_at_unix_ms,
                    "inventory.stacks.expires_at_unix_ms",
                )?,
            })
        })
        .collect::<surrealdb::Result<_>>()?;
    let equipment = record
        .equipment
        .into_iter()
        .map(|equipped| {
            Ok(EquippedItem {
                slot: persisted_i32(equipped.slot, "inventory.equipment.slot")?,
                item_id: persisted_u32(equipped.item_id, "inventory.equipment.item_id")?,
                expires_at_unix_ms: persisted_u64(
                    equipped.expires_at_unix_ms,
                    "inventory.equipment.expires_at_unix_ms",
                )?,
            })
        })
        .collect::<surrealdb::Result<_>>()?;
    Ok(InventoryState {
        equipment,
        capacity: persisted_u32(record.capacity, "inventory.capacity")?,
        stacks,
    })
}

fn inventory_record(inventory: &InventoryState) -> surrealdb::Result<InventoryRecord> {
    Ok(InventoryRecord {
        capacity: i64::from(inventory.capacity),
        stacks: inventory
            .stacks
            .iter()
            .map(|stack| {
                Ok(InventoryStackRecord {
                    item_id: i64::from(stack.item_id),
                    quantity: i64::from(stack.quantity),
                    expires_at_unix_ms: persisted_i64(
                        stack.expires_at_unix_ms,
                        "inventory.stacks.expires_at_unix_ms",
                    )?,
                })
            })
            .collect::<surrealdb::Result<_>>()?,
        equipment: inventory
            .equipment
            .iter()
            .map(|equipped| {
                Ok(EquippedItemRecord {
                    slot: i64::from(equipped.slot),
                    item_id: i64::from(equipped.item_id),
                    expires_at_unix_ms: persisted_i64(
                        equipped.expires_at_unix_ms,
                        "inventory.equipment.expires_at_unix_ms",
                    )?,
                })
            })
            .collect::<surrealdb::Result<_>>()?,
    })
}

fn key_bindings_from_records(records: Vec<KeyBindingRecord>) -> surrealdb::Result<Vec<KeyBinding>> {
    records
        .into_iter()
        .map(|record| {
            Ok(KeyBinding {
                code: record.code,
                action: persisted_i32(record.action, "key_bindings.action")?,
                skill_id: persisted_u32(record.skill_id, "key_bindings.skill_id")?,
            })
        })
        .collect()
}

fn key_binding_record(binding: &KeyBinding) -> KeyBindingRecord {
    KeyBindingRecord {
        code: binding.code.clone(),
        action: i64::from(binding.action),
        skill_id: i64::from(binding.skill_id),
    }
}

fn learned_skills_from_records(
    records: Vec<LearnedSkillRecord>
) -> surrealdb::Result<Vec<LearnedSkill>> {
    records
        .into_iter()
        .map(|record| {
            Ok(LearnedSkill {
                skill_id: persisted_u32(record.skill_id, "learned_skills.skill_id")?,
                level: persisted_u32(record.level, "learned_skills.level")?,
                master_level: persisted_u32(record.master_level, "learned_skills.master_level")?,
            })
        })
        .collect()
}

fn learned_skill_record(skill: &LearnedSkill) -> LearnedSkillRecord {
    LearnedSkillRecord {
        skill_id: i64::from(skill.skill_id),
        level: i64::from(skill.level),
        master_level: i64::from(skill.master_level),
    }
}

fn quests_from_records(records: Vec<QuestRecordData>) -> surrealdb::Result<Vec<PlayerQuest>> {
    records
        .into_iter()
        .map(|record| {
            let mob_progress = record
                .mob_progress
                .into_iter()
                .map(|progress| {
                    Ok(QuestMobProgress {
                        mob_id: persisted_u32(progress.mob_id, "quests.mob_progress.mob_id")?,
                        count: persisted_u32(progress.count, "quests.mob_progress.count")?,
                    })
                })
                .collect::<surrealdb::Result<_>>()?;
            Ok(PlayerQuest {
                quest_id: persisted_u32(record.quest_id, "quests.quest_id")?,
                status: persisted_i32(record.status, "quests.status")?,
                mob_progress,
                accepted_at_unix_ms: persisted_u64(
                    record.accepted_at_unix_ms,
                    "quests.accepted_at_unix_ms",
                )?,
                completed_at_unix_ms: persisted_u64(
                    record.completed_at_unix_ms,
                    "quests.completed_at_unix_ms",
                )?,
                dialogue_step: persisted_u32(record.dialogue_step, "quests.dialogue_step")?,
                completion_quiz_passed: record.completion_quiz_passed,
            })
        })
        .collect()
}

fn quest_record_data(quest: &PlayerQuest) -> surrealdb::Result<QuestRecordData> {
    Ok(QuestRecordData {
        quest_id: i64::from(quest.quest_id),
        status: i64::from(quest.status),
        mob_progress: quest
            .mob_progress
            .iter()
            .map(|progress| QuestMobProgressRecord {
                mob_id: i64::from(progress.mob_id),
                count: i64::from(progress.count),
            })
            .collect(),
        accepted_at_unix_ms: persisted_i64(
            quest.accepted_at_unix_ms,
            "quests.accepted_at_unix_ms",
        )?,
        completed_at_unix_ms: persisted_i64(
            quest.completed_at_unix_ms,
            "quests.completed_at_unix_ms",
        )?,
        dialogue_step: i64::from(quest.dialogue_step),
        completion_quiz_passed: quest.completion_quiz_passed,
    })
}

fn quest_records_from_records(
    records: Vec<PersistedQuestRecord>
) -> surrealdb::Result<Vec<QuestRecord>> {
    records
        .into_iter()
        .map(|record| {
            let entries = record
                .entries
                .into_iter()
                .map(|entry| {
                    Ok(QuestRecordEntry {
                        index: persisted_u32(entry.index, "quest_records.entries.index")?,
                        value: entry.value,
                    })
                })
                .collect::<surrealdb::Result<_>>()?;
            Ok(QuestRecord {
                quest_id: persisted_u32(record.quest_id, "quest_records.quest_id")?,
                entries,
            })
        })
        .collect()
}

fn persisted_quest_record(record: &QuestRecord) -> PersistedQuestRecord {
    PersistedQuestRecord {
        quest_id: i64::from(record.quest_id),
        entries: record
            .entries
            .iter()
            .map(|entry| QuestRecordEntryRecord {
                index: i64::from(entry.index),
                value: entry.value.clone(),
            })
            .collect(),
    }
}

fn monster_book_cards_from_records(
    records: Vec<MonsterBookCardRecord>
) -> surrealdb::Result<Vec<MonsterBookCard>> {
    records
        .into_iter()
        .map(|record| {
            Ok(MonsterBookCard {
                card_item_id: persisted_u32(
                    record.card_item_id,
                    "monster_book_cards.card_item_id",
                )?,
                count: persisted_u32(record.count, "monster_book_cards.count")?,
            })
        })
        .collect()
}

fn monster_book_card_record(card: &MonsterBookCard) -> MonsterBookCardRecord {
    MonsterBookCardRecord {
        card_item_id: i64::from(card.card_item_id),
        count: i64::from(card.count),
    }
}

fn persisted_u32(
    value: i64,
    field: &str,
) -> surrealdb::Result<u32> {
    u32::try_from(value).map_err(|_| {
        data_error(
            CORRUPT_RECORD,
            format!("{field} is outside the supported range"),
        )
    })
}

fn persisted_i32(
    value: i64,
    field: &str,
) -> surrealdb::Result<i32> {
    i32::try_from(value).map_err(|_| {
        data_error(
            CORRUPT_RECORD,
            format!("{field} is outside the supported range"),
        )
    })
}

fn persisted_u64(
    value: i64,
    field: &str,
) -> surrealdb::Result<u64> {
    u64::try_from(value)
        .map_err(|_| data_error(CORRUPT_RECORD, format!("{field} cannot be negative")))
}

fn persisted_i64(
    value: u64,
    field: &str,
) -> surrealdb::Result<i64> {
    i64::try_from(value).map_err(|_| {
        data_error(
            INVALID_PLAYER,
            format!("{field} is outside the supported range"),
        )
    })
}

fn data_error(
    context: &str,
    error: impl std::fmt::Display,
) -> surrealdb::Error {
    surrealdb::Error::internal(format!("{context}: {error}"))
}

#[cfg(test)]
pub(super) fn set_first_inventory_stack_quantity(
    record: &mut PlayerRecord,
    quantity: i64,
) {
    record.inventory.stacks[0].quantity = quantity;
}
