use std::path::Path;

use oozems_proto::v1::CharacterAppearance;
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
use surrealdb::Surreal;
use surrealdb::engine::local::Db;
use surrealdb::engine::local::SurrealKv;
use surrealdb::types::RecordId;
use surrealdb::types::SurrealValue;
use thiserror::Error;

pub type Database = Surreal<Db>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlayerId(String);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CharacterName(String);

#[derive(Debug, Error)]
pub enum PlayerIdError {
    #[error("player ID must contain 1 to 32 ASCII letters, digits, underscores, or hyphens")]
    Invalid,
}

#[derive(Debug, Error)]
pub enum CharacterNameError {
    #[error("character name must contain 3 to 12 ASCII letters, digits, or underscores")]
    Invalid,
}

#[derive(Clone, Debug, SurrealValue)]
struct PlayerData {
    player_id: String,
    revision: Option<i64>,
    name: String,
    level: u32,
    job_id: u32,
    hp: u32,
    max_hp: u32,
    mp: u32,
    max_mp: u32,
    experience: u64,
    experience_required: u64,
    fame: i32,
    ability_points: u32,
    strength: u32,
    dexterity: u32,
    intelligence: u32,
    luck: u32,
    map_id: u32,
    x: f64,
    y: f64,
    gender: Option<i32>,
    skin_id: Option<u32>,
    face_id: Option<u32>,
    hair_id: Option<u32>,
    inventory_item_ids: Vec<u32>,
    inventory_stack_item_ids: Option<Vec<u32>>,
    inventory_stack_quantities: Option<Vec<u32>>,
    inventory_stack_expires_at_unix_ms: Option<Vec<u64>>,
    inventory_capacity: u32,
    equipped_top: Option<u32>,
    equipped_bottom: Option<u32>,
    equipped_shoes: Option<u32>,
    equipped_top_expires_at_unix_ms: Option<u64>,
    equipped_bottom_expires_at_unix_ms: Option<u64>,
    equipped_shoes_expires_at_unix_ms: Option<u64>,
    key_binding_codes: Vec<String>,
    key_binding_actions: Vec<i32>,
    key_binding_skill_ids: Vec<u32>,
    skill_points: u32,
    learned_skill_ids: Vec<u32>,
    learned_skill_levels: Vec<u32>,
    learned_skill_master_levels: Vec<u32>,
    mesos: u64,
    quest_ids: Vec<u32>,
    quest_statuses: Vec<i32>,
    quest_accepted_at_unix_ms: Option<Vec<u64>>,
    quest_completed_at_unix_ms: Option<Vec<u64>>,
    quest_mob_ids: Option<Vec<Vec<u32>>>,
    quest_mob_counts: Option<Vec<Vec<u32>>>,
    quest_dialogue_steps: Option<Vec<u32>>,
    quest_completion_quiz_passed: Option<Vec<bool>>,
    quest_record_ids: Option<Vec<u32>>,
    quest_record_entry_indices: Option<Vec<Vec<u32>>>,
    quest_record_entry_values: Option<Vec<Vec<String>>>,
    monster_book_card_item_ids: Option<Vec<u32>>,
    monster_book_card_counts: Option<Vec<u32>>,
}

#[derive(Clone, Debug, SurrealValue)]
struct PlayerPositionData {
    map_id: u32,
    x: f64,
    y: f64,
}

#[derive(Clone, Debug, SurrealValue)]
struct PlayerSessionData {
    map_id: u32,
    x: f64,
    y: f64,
    key_binding_codes: Vec<String>,
    key_binding_actions: Vec<i32>,
    key_binding_skill_ids: Vec<u32>,
}

#[derive(Clone, Debug, SurrealValue)]
struct PlayerRecord {
    id: RecordId,
    player_id: String,
    revision: Option<i64>,
    name: String,
    level: u32,
    job_id: Option<u32>,
    hp: Option<u32>,
    max_hp: Option<u32>,
    mp: Option<u32>,
    max_mp: Option<u32>,
    experience: Option<u64>,
    experience_required: Option<u64>,
    fame: Option<i32>,
    ability_points: Option<u32>,
    strength: Option<u32>,
    dexterity: Option<u32>,
    intelligence: Option<u32>,
    luck: Option<u32>,
    map_id: u32,
    x: f64,
    y: f64,
    gender: Option<i32>,
    skin_id: Option<u32>,
    face_id: Option<u32>,
    hair_id: Option<u32>,
    inventory_item_ids: Option<Vec<u32>>,
    inventory_stack_item_ids: Option<Vec<i64>>,
    inventory_stack_quantities: Option<Vec<i64>>,
    inventory_stack_expires_at_unix_ms: Option<Vec<i64>>,
    inventory_capacity: Option<u32>,
    equipped_top: Option<u32>,
    equipped_bottom: Option<u32>,
    equipped_shoes: Option<u32>,
    equipped_top_expires_at_unix_ms: Option<i64>,
    equipped_bottom_expires_at_unix_ms: Option<i64>,
    equipped_shoes_expires_at_unix_ms: Option<i64>,
    key_binding_codes: Option<Vec<String>>,
    key_binding_actions: Option<Vec<i32>>,
    key_binding_skill_ids: Option<Vec<u32>>,
    skill_points: Option<u32>,
    learned_skill_ids: Option<Vec<u32>>,
    learned_skill_levels: Option<Vec<u32>>,
    learned_skill_master_levels: Option<Vec<u32>>,
    mesos: Option<u64>,
    quest_ids: Option<Vec<u32>>,
    quest_statuses: Option<Vec<i32>>,
    quest_accepted_at_unix_ms: Option<Vec<i64>>,
    quest_completed_at_unix_ms: Option<Vec<i64>>,
    quest_mob_ids: Option<Vec<Vec<i64>>>,
    quest_mob_counts: Option<Vec<Vec<i64>>>,
    quest_dialogue_steps: Option<Vec<i64>>,
    quest_completion_quiz_passed: Option<Vec<bool>>,
    quest_record_ids: Option<Vec<i64>>,
    quest_record_entry_indices: Option<Vec<Vec<i64>>>,
    quest_record_entry_values: Option<Vec<Vec<String>>>,
    monster_book_card_item_ids: Option<Vec<i64>>,
    monster_book_card_counts: Option<Vec<i64>>,
}

impl PlayerId {
    pub fn parse(value: &str) -> Result<Self, PlayerIdError> {
        let is_valid = !value.is_empty()
            && value.len() <= 32
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'));

        is_valid
            .then(|| Self(value.to_owned()))
            .ok_or(PlayerIdError::Invalid)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl CharacterName {
    pub fn parse(value: &str) -> Result<Self, CharacterNameError> {
        let is_valid = (3..=12).contains(&value.len())
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_');
        is_valid
            .then(|| Self(value.to_owned()))
            .ok_or(CharacterNameError::Invalid)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

pub async fn open_surreal_kv(path: &Path) -> surrealdb::Result<Database> {
    let database = Surreal::new::<SurrealKv>(path).await?;
    database.use_ns("oozems").use_db("game").await?;
    initialize_schema(&database).await?;
    Ok(database)
}

async fn initialize_schema(database: &Database) -> surrealdb::Result<()> {
    database
        .query(
            r#"
            DEFINE TABLE IF NOT EXISTS player SCHEMAFULL;
            DEFINE FIELD IF NOT EXISTS player_id ON TABLE player TYPE string;
            DEFINE FIELD IF NOT EXISTS revision ON TABLE player TYPE option<int>;
            DEFINE FIELD IF NOT EXISTS name ON TABLE player TYPE string;
            DEFINE FIELD IF NOT EXISTS level ON TABLE player TYPE int;
            DEFINE FIELD IF NOT EXISTS job_id ON TABLE player TYPE option<int>;
            DEFINE FIELD IF NOT EXISTS hp ON TABLE player TYPE option<int>;
            DEFINE FIELD IF NOT EXISTS max_hp ON TABLE player TYPE option<int>;
            DEFINE FIELD IF NOT EXISTS mp ON TABLE player TYPE option<int>;
            DEFINE FIELD IF NOT EXISTS max_mp ON TABLE player TYPE option<int>;
            DEFINE FIELD IF NOT EXISTS experience ON TABLE player TYPE option<int>;
            DEFINE FIELD IF NOT EXISTS experience_required ON TABLE player TYPE option<int>;
            DEFINE FIELD IF NOT EXISTS fame ON TABLE player TYPE option<int>;
            DEFINE FIELD IF NOT EXISTS ability_points ON TABLE player TYPE option<int>;
            DEFINE FIELD IF NOT EXISTS strength ON TABLE player TYPE option<int>;
            DEFINE FIELD IF NOT EXISTS dexterity ON TABLE player TYPE option<int>;
            DEFINE FIELD IF NOT EXISTS intelligence ON TABLE player TYPE option<int>;
            DEFINE FIELD IF NOT EXISTS luck ON TABLE player TYPE option<int>;
            DEFINE FIELD IF NOT EXISTS map_id ON TABLE player TYPE int;
            DEFINE FIELD IF NOT EXISTS x ON TABLE player TYPE float;
            DEFINE FIELD IF NOT EXISTS y ON TABLE player TYPE float;
            DEFINE FIELD IF NOT EXISTS gender ON TABLE player TYPE option<int>;
            DEFINE FIELD IF NOT EXISTS skin_id ON TABLE player TYPE option<int>;
            DEFINE FIELD IF NOT EXISTS face_id ON TABLE player TYPE option<int>;
            DEFINE FIELD IF NOT EXISTS hair_id ON TABLE player TYPE option<int>;
            DEFINE FIELD IF NOT EXISTS inventory_item_ids ON TABLE player TYPE option<array<int>>;
            DEFINE FIELD IF NOT EXISTS inventory_stack_item_ids ON TABLE player TYPE option<array<int>>;
            DEFINE FIELD IF NOT EXISTS inventory_stack_quantities ON TABLE player TYPE option<array<int>>;
            DEFINE FIELD IF NOT EXISTS inventory_stack_expires_at_unix_ms ON TABLE player TYPE option<array<int>>;
            DEFINE FIELD IF NOT EXISTS inventory_capacity ON TABLE player TYPE option<int>;
            DEFINE FIELD IF NOT EXISTS equipped_top ON TABLE player TYPE option<int>;
            DEFINE FIELD IF NOT EXISTS equipped_bottom ON TABLE player TYPE option<int>;
            DEFINE FIELD IF NOT EXISTS equipped_shoes ON TABLE player TYPE option<int>;
            DEFINE FIELD IF NOT EXISTS equipped_top_expires_at_unix_ms ON TABLE player TYPE option<int>;
            DEFINE FIELD IF NOT EXISTS equipped_bottom_expires_at_unix_ms ON TABLE player TYPE option<int>;
            DEFINE FIELD IF NOT EXISTS equipped_shoes_expires_at_unix_ms ON TABLE player TYPE option<int>;
            DEFINE FIELD IF NOT EXISTS key_binding_codes ON TABLE player TYPE option<array<string>>;
            DEFINE FIELD IF NOT EXISTS key_binding_actions ON TABLE player TYPE option<array<int>>;
            DEFINE FIELD IF NOT EXISTS key_binding_skill_ids ON TABLE player TYPE option<array<int>>;
            DEFINE FIELD IF NOT EXISTS skill_points ON TABLE player TYPE option<int>;
            DEFINE FIELD IF NOT EXISTS learned_skill_ids ON TABLE player TYPE option<array<int>>;
            DEFINE FIELD IF NOT EXISTS learned_skill_levels ON TABLE player TYPE option<array<int>>;
            DEFINE FIELD IF NOT EXISTS learned_skill_master_levels ON TABLE player TYPE option<array<int>>;
            DEFINE FIELD IF NOT EXISTS mesos ON TABLE player TYPE option<int>;
            DEFINE FIELD IF NOT EXISTS quest_ids ON TABLE player TYPE option<array<int>>;
            DEFINE FIELD IF NOT EXISTS quest_statuses ON TABLE player TYPE option<array<int>>;
            DEFINE FIELD IF NOT EXISTS quest_accepted_at_unix_ms ON TABLE player TYPE option<array<int>>;
            DEFINE FIELD IF NOT EXISTS quest_completed_at_unix_ms ON TABLE player TYPE option<array<int>>;
            DEFINE FIELD IF NOT EXISTS quest_mob_ids ON TABLE player TYPE option<array<array<int>>>;
            DEFINE FIELD IF NOT EXISTS quest_mob_counts ON TABLE player TYPE option<array<array<int>>>;
            DEFINE FIELD IF NOT EXISTS quest_dialogue_steps ON TABLE player TYPE option<array<int>>;
            DEFINE FIELD IF NOT EXISTS quest_completion_quiz_passed ON TABLE player TYPE option<array<bool>>;
            DEFINE FIELD IF NOT EXISTS quest_record_ids ON TABLE player TYPE option<array<int>>;
            DEFINE FIELD IF NOT EXISTS quest_record_entry_indices ON TABLE player TYPE option<array<array<int>>>;
            DEFINE FIELD IF NOT EXISTS quest_record_entry_values ON TABLE player TYPE option<array<array<string>>>;
            DEFINE FIELD IF NOT EXISTS monster_book_card_item_ids ON TABLE player TYPE option<array<int>>;
            DEFINE FIELD IF NOT EXISTS monster_book_card_counts ON TABLE player TYPE option<array<int>>;
            "#,
        )
        .await?
        .check()?;
    Ok(())
}

pub async fn load_player(
    database: &Database,
    player_id: &PlayerId,
    initial_skill_points: u32,
) -> surrealdb::Result<Option<PlayerState>> {
    let record: Option<PlayerRecord> = database.select(("player", player_id.as_str())).await?;
    Ok(record.map(|record| player_from_record(record, initial_skill_points)))
}

pub async fn create_player(
    database: &Database,
    player_id: &PlayerId,
    name: &CharacterName,
    appearance: CharacterAppearance,
    map_id: u32,
    position: Vec2,
    experience_required: u64,
    initial_skill_points: u32,
) -> surrealdb::Result<PlayerState> {
    let mut stats = starter_character_stats();
    stats.experience_required = experience_required;
    let player = PlayerState {
        id: player_id.as_str().to_owned(),
        name: name.as_str().to_owned(),
        level: 1,
        map_id,
        position: Some(position),
        appearance: Some(appearance),
        stats: Some(stats),
        inventory: Some(crate::items::starter_inventory()),
        key_bindings: crate::keymap::default_bindings(),
        skill_points: initial_skill_points,
        learned_skills: Vec::new(),
        mesos: 0,
        quests: Vec::new(),
        revision: 0,
        quest_records: Vec::new(),
        monster_book_cards: Vec::new(),
    };
    save_player(database, &player).await
}

pub async fn save_player(
    database: &Database,
    player: &PlayerState,
) -> surrealdb::Result<PlayerState> {
    let current: Option<PlayerRecord> = database.select(("player", player.id.as_str())).await?;
    let current_revision = current
        .as_ref()
        .and_then(|record| record.revision)
        .and_then(|revision| u64::try_from(revision).ok())
        .unwrap_or_default();
    let revision = next_player_revision(player.revision, current_revision)?;
    let mut canonical_player = player.clone();
    canonical_player.learned_skills = canonicalize_learned_skills(&canonical_player.learned_skills);
    canonical_player.monster_book_cards =
        crate::monster_book::canonicalize(canonical_player.monster_book_cards)
            .map_err(|error| surrealdb::Error::internal(error.to_string()))?;
    canonical_player.quest_records =
        crate::quest_records::canonicalize(canonical_player.quest_records)
            .map_err(|error| surrealdb::Error::internal(error.to_string()))?;
    let mut data = PlayerData::from(&canonical_player);
    data.revision = Some(
        i64::try_from(revision)
            .map_err(|_| surrealdb::Error::internal("player revision overflow".to_owned()))?,
    );
    let record: Option<PlayerRecord> = database
        .upsert(("player", player.id.as_str()))
        .content(data)
        .await?;

    record
        .map(|record| player_from_record(record, player.skill_points))
        .ok_or_else(|| surrealdb::Error::internal("player upsert returned no record".to_owned()))
}

fn next_player_revision(
    input_revision: u64,
    saved_revision: u64,
) -> surrealdb::Result<u64> {
    input_revision
        .max(saved_revision)
        .checked_add(1)
        .filter(|revision| i64::try_from(*revision).is_ok())
        .ok_or_else(|| surrealdb::Error::internal("player revision overflow".to_owned()))
}

pub async fn save_player_position(
    database: &Database,
    player: &PlayerState,
) -> surrealdb::Result<()> {
    let data = PlayerPositionData::from(player);
    let _: Option<PlayerRecord> = database
        .update(("player", player.id.as_str()))
        .merge(data)
        .await?;
    Ok(())
}

pub async fn save_player_session(
    database: &Database,
    player: &PlayerState,
) -> surrealdb::Result<()> {
    let data = PlayerSessionData::from(player);
    let _: Option<PlayerRecord> = database
        .update(("player", player.id.as_str()))
        .merge(data)
        .await?;
    Ok(())
}

pub fn apply_player_preferences(
    mut current: PlayerState,
    requested: &PlayerState,
) -> PlayerState {
    current.key_bindings = requested.key_bindings.clone();
    current
}

fn player_from_record(
    record: PlayerRecord,
    initial_skill_points: u32,
) -> PlayerState {
    let appearance = appearance_from_record(&record);
    let stats = stats_from_record(&record);
    let inventory = inventory_from_record(&record);
    let key_bindings = key_bindings_from_record(&record);
    let learned_skills = learned_skills_from_record(&record);
    let quests = quests_from_record(&record);
    let quest_records = quest_records_from_record(&record);
    let monster_book_cards = monster_book_cards_from_record(&record);
    let _record_id = record.id;
    PlayerState {
        id: record.player_id,
        name: record.name,
        level: record.level,
        map_id: record.map_id,
        position: Some(Vec2 {
            x: record.x as f32,
            y: record.y as f32,
        }),
        appearance,
        stats: Some(stats),
        inventory: Some(inventory),
        key_bindings,
        skill_points: record.skill_points.unwrap_or(initial_skill_points),
        learned_skills,
        mesos: record.mesos.unwrap_or_default(),
        quests,
        revision: record
            .revision
            .and_then(|revision| u64::try_from(revision).ok())
            .unwrap_or_default(),
        quest_records,
        monster_book_cards,
    }
}

fn stats_from_record(record: &PlayerRecord) -> CharacterStats {
    let defaults = starter_character_stats();
    let max_hp = record.max_hp.unwrap_or(defaults.max_hp).max(1);
    let max_mp = record.max_mp.unwrap_or(defaults.max_mp).max(1);
    let experience_required = record
        .experience_required
        .unwrap_or(defaults.experience_required)
        .max(1);
    CharacterStats {
        job_id: record.job_id.unwrap_or(defaults.job_id),
        hp: record.hp.unwrap_or(defaults.hp).min(max_hp),
        max_hp,
        mp: record.mp.unwrap_or(defaults.mp).min(max_mp),
        max_mp,
        experience: record.experience.unwrap_or(defaults.experience),
        fame: record.fame.unwrap_or(defaults.fame),
        ability_points: record.ability_points.unwrap_or(defaults.ability_points),
        strength: record.strength.unwrap_or(defaults.strength),
        dexterity: record.dexterity.unwrap_or(defaults.dexterity),
        intelligence: record.intelligence.unwrap_or(defaults.intelligence),
        luck: record.luck.unwrap_or(defaults.luck),
        experience_required,
    }
}

fn starter_character_stats() -> CharacterStats {
    CharacterStats {
        job_id: 0,
        hp: 50,
        max_hp: 50,
        mp: 5,
        max_mp: 5,
        experience: 0,
        experience_required: 15,
        fame: 0,
        ability_points: 0,
        strength: 12,
        dexterity: 5,
        intelligence: 4,
        luck: 4,
    }
}

fn appearance_from_record(record: &PlayerRecord) -> Option<CharacterAppearance> {
    Some(CharacterAppearance {
        gender: record.gender?,
        skin_id: record.skin_id?,
        face_id: record.face_id?,
        hair_id: record.hair_id?,
    })
}

fn inventory_from_record(record: &PlayerRecord) -> InventoryState {
    if record.inventory_stack_item_ids.is_none()
        && record.inventory_stack_quantities.is_none()
        && record.inventory_item_ids.is_none()
    {
        return crate::items::starter_inventory();
    }
    let stacks = persisted_inventory_stacks(record);
    let capacity = record
        .inventory_capacity
        .unwrap_or(crate::items::INVENTORY_CAPACITY)
        .max(u32::try_from(stacks.len()).unwrap_or(u32::MAX));
    let equipment = [
        (
            EquipmentSlot::Top,
            record.equipped_top,
            record.equipped_top_expires_at_unix_ms,
        ),
        (
            EquipmentSlot::Bottom,
            record.equipped_bottom,
            record.equipped_bottom_expires_at_unix_ms,
        ),
        (
            EquipmentSlot::Shoes,
            record.equipped_shoes,
            record.equipped_shoes_expires_at_unix_ms,
        ),
    ]
    .into_iter()
    .filter_map(|(slot, item_id, expiration)| persisted_equipped_item(slot, item_id, expiration))
    .collect();

    InventoryState {
        item_ids: Vec::new(),
        equipment,
        capacity,
        stacks,
    }
}

fn persisted_equipped_item(
    slot: EquipmentSlot,
    item_id: Option<u32>,
    expires_at_unix_ms: Option<i64>,
) -> Option<EquippedItem> {
    Some(EquippedItem {
        slot: slot as i32,
        item_id: item_id?,
        expires_at_unix_ms: expires_at_unix_ms
            .map_or(Some(0), |value| u64::try_from(value).ok())?,
    })
}

fn persisted_inventory_stacks(record: &PlayerRecord) -> Vec<InventoryItemStack> {
    let aligned_stacks = record
        .inventory_stack_item_ids
        .as_ref()
        .zip(record.inventory_stack_quantities.as_ref())
        .filter(|(item_ids, quantities)| item_ids.len() == quantities.len())
        .filter(|(item_ids, _)| {
            !item_ids.is_empty() || record.inventory_item_ids.as_ref().is_none_or(Vec::is_empty)
        })
        .and_then(|(item_ids, quantities)| {
            let expirations = match record.inventory_stack_expires_at_unix_ms.as_ref() {
                Some(expirations) if expirations.len() == item_ids.len() => expirations
                    .iter()
                    .map(|expiration| u64::try_from(*expiration).ok())
                    .collect::<Option<Vec<_>>>()?,
                Some(_) => return None,
                None => vec![0; item_ids.len()],
            };
            item_ids
                .iter()
                .zip(quantities.iter().zip(expirations))
                .map(|(item_id, (quantity, expires_at_unix_ms))| {
                    let item_id = u32::try_from(*item_id).ok().filter(|value| *value != 0)?;
                    let quantity = u32::try_from(*quantity).ok().filter(|value| *value != 0)?;
                    Some(InventoryItemStack {
                        item_id,
                        quantity,
                        expires_at_unix_ms,
                    })
                })
                .collect::<Option<Vec<_>>>()
        });

    aligned_stacks.unwrap_or_else(|| {
        crate::items::normalize_legacy_item_ids(
            record.inventory_item_ids.as_deref().unwrap_or_default(),
        )
    })
}

fn key_bindings_from_record(record: &PlayerRecord) -> Vec<KeyBinding> {
    let (Some(codes), Some(actions)) = (
        record.key_binding_codes.as_ref(),
        record.key_binding_actions.as_ref(),
    ) else {
        return crate::keymap::default_bindings();
    };
    if codes.len() != actions.len() {
        return crate::keymap::default_bindings();
    }
    let skill_ids = record
        .key_binding_skill_ids
        .clone()
        .unwrap_or_else(|| vec![0; codes.len()]);
    if skill_ids.len() != codes.len() {
        return crate::keymap::default_bindings();
    }
    let bindings = codes
        .iter()
        .zip(actions.iter().zip(skill_ids))
        .map(|(code, (action, skill_id))| KeyBinding {
            code: code.clone(),
            action: *action,
            skill_id,
        })
        .collect::<Vec<_>>();
    if crate::keymap::validate_bindings(&bindings).is_ok() {
        bindings
    } else {
        crate::keymap::default_bindings()
    }
}

fn learned_skills_from_record(record: &PlayerRecord) -> Vec<LearnedSkill> {
    let (Some(skill_ids), Some(levels)) = (
        record.learned_skill_ids.as_ref(),
        record.learned_skill_levels.as_ref(),
    ) else {
        return Vec::new();
    };
    if skill_ids.len() != levels.len() {
        return Vec::new();
    }
    let master_levels = match record.learned_skill_master_levels.as_ref() {
        Some(master_levels) if master_levels.len() == skill_ids.len() => master_levels.clone(),
        Some(_) => return Vec::new(),
        None => vec![0; skill_ids.len()],
    };
    let skills = skill_ids
        .iter()
        .zip(levels.iter().zip(master_levels))
        .filter(|(skill_id, (level, master_level))| {
            **skill_id != 0 && (**level != 0 || *master_level != 0)
        })
        .map(|(skill_id, (level, master_level))| LearnedSkill {
            skill_id: *skill_id,
            level: *level,
            master_level,
        })
        .collect::<Vec<_>>();
    canonicalize_learned_skills(&skills)
}

fn canonicalize_learned_skills(skills: &[LearnedSkill]) -> Vec<LearnedSkill> {
    let mut canonical = std::collections::BTreeMap::<u32, (u32, u32)>::new();
    for skill in skills
        .iter()
        .filter(|skill| skill.skill_id != 0 && (skill.level > 0 || skill.master_level > 0))
    {
        let (level, master_level) = canonical.entry(skill.skill_id).or_default();
        *level = (*level).max(skill.level);
        *master_level = (*master_level).max(skill.master_level);
    }
    canonical
        .into_iter()
        .map(|(skill_id, (level, master_level))| LearnedSkill {
            skill_id,
            level,
            master_level,
        })
        .collect()
}

fn quests_from_record(record: &PlayerRecord) -> Vec<PlayerQuest> {
    let (Some(quest_ids), Some(statuses)) =
        (record.quest_ids.as_ref(), record.quest_statuses.as_ref())
    else {
        return Vec::new();
    };
    if quest_ids.len() != statuses.len() {
        return Vec::new();
    }
    let metadata_is_aligned = record
        .quest_accepted_at_unix_ms
        .as_ref()
        .zip(record.quest_completed_at_unix_ms.as_ref())
        .zip(record.quest_mob_ids.as_ref())
        .zip(record.quest_mob_counts.as_ref())
        .is_some_and(|(((accepted, completed), mob_ids), mob_counts)| {
            accepted.len() == quest_ids.len()
                && completed.len() == quest_ids.len()
                && mob_ids.len() == quest_ids.len()
                && mob_counts.len() == quest_ids.len()
        });
    let dialogue_metadata_is_aligned = record
        .quest_dialogue_steps
        .as_ref()
        .zip(record.quest_completion_quiz_passed.as_ref())
        .is_some_and(|(steps, passed)| {
            steps.len() == quest_ids.len() && passed.len() == quest_ids.len()
        });
    let mut quests = quest_ids
        .iter()
        .zip(statuses)
        .enumerate()
        .filter_map(|(index, (quest_id, status))| {
            let status = QuestStatus::try_from(*status).ok()?;
            let dialogue_step = dialogue_metadata_is_aligned
                .then(|| {
                    record
                        .quest_dialogue_steps
                        .as_ref()
                        .and_then(|steps| steps.get(index))
                        .and_then(|step| u32::try_from(*step).ok())
                })
                .flatten()
                .unwrap_or_default();
            let completion_quiz_passed = dialogue_metadata_is_aligned
                && record
                    .quest_completion_quiz_passed
                    .as_ref()
                    .and_then(|passed| passed.get(index))
                    .copied()
                    .unwrap_or_default();
            let has_pending_start = status == QuestStatus::Unspecified && dialogue_step > 0;
            (*quest_id != 0 && (status != QuestStatus::Unspecified || has_pending_start)).then_some(
                PlayerQuest {
                    quest_id: *quest_id,
                    status: status as i32,
                    mob_progress: metadata_is_aligned
                        .then(|| persisted_mob_progress(record, index))
                        .flatten()
                        .unwrap_or_default(),
                    accepted_at_unix_ms: metadata_is_aligned
                        .then(|| {
                            persisted_timestamp(record.quest_accepted_at_unix_ms.as_ref(), index)
                        })
                        .flatten()
                        .unwrap_or_default(),
                    completed_at_unix_ms: metadata_is_aligned
                        .then(|| {
                            persisted_timestamp(record.quest_completed_at_unix_ms.as_ref(), index)
                        })
                        .flatten()
                        .unwrap_or_default(),
                    dialogue_step: match status {
                        QuestStatus::Unspecified | QuestStatus::Started => dialogue_step,
                        QuestStatus::Completed => 0,
                    },
                    completion_quiz_passed: status == QuestStatus::Started
                        && completion_quiz_passed,
                },
            )
        })
        .collect::<Vec<_>>();
    quests.sort_by_key(|quest| quest.quest_id);
    quests.dedup_by_key(|quest| quest.quest_id);
    quests
}

fn persisted_timestamp(
    values: Option<&Vec<i64>>,
    index: usize,
) -> Option<u64> {
    u64::try_from(*values?.get(index)?).ok()
}

fn persisted_mob_progress(
    record: &PlayerRecord,
    index: usize,
) -> Option<Vec<QuestMobProgress>> {
    let mob_ids = record.quest_mob_ids.as_ref()?.get(index)?;
    let counts = record.quest_mob_counts.as_ref()?.get(index)?;
    if mob_ids.len() != counts.len() {
        return None;
    }
    let mut progress = mob_ids
        .iter()
        .zip(counts)
        .map(|(mob_id, count)| {
            let mob_id = u32::try_from(*mob_id).ok().filter(|mob_id| *mob_id != 0)?;
            let count = u32::try_from(*count).ok()?;
            Some(QuestMobProgress { mob_id, count })
        })
        .collect::<Option<Vec<_>>>()?;
    progress.sort_by_key(|entry| entry.mob_id);
    progress.dedup_by_key(|entry| entry.mob_id);
    Some(progress)
}

fn quest_records_from_record(record: &PlayerRecord) -> Vec<QuestRecord> {
    let (Some(quest_ids), Some(entry_indices), Some(entry_values)) = (
        record.quest_record_ids.as_ref(),
        record.quest_record_entry_indices.as_ref(),
        record.quest_record_entry_values.as_ref(),
    ) else {
        return Vec::new();
    };
    if quest_ids.len() != entry_indices.len() || quest_ids.len() != entry_values.len() {
        return Vec::new();
    }
    let records = quest_ids
        .iter()
        .zip(entry_indices.iter().zip(entry_values))
        .map(|(quest_id, (indices, values))| {
            if indices.len() != values.len() {
                return None;
            }
            let quest_id = u32::try_from(*quest_id).ok()?;
            let entries = indices
                .iter()
                .zip(values)
                .map(|(index, value)| {
                    Some(QuestRecordEntry {
                        index: u32::try_from(*index).ok()?,
                        value: value.clone(),
                    })
                })
                .collect::<Option<Vec<_>>>()?;
            Some(QuestRecord { quest_id, entries })
        })
        .collect::<Option<Vec<_>>>();
    records
        .and_then(|records| crate::quest_records::canonicalize(records).ok())
        .unwrap_or_default()
}

fn monster_book_cards_from_record(record: &PlayerRecord) -> Vec<MonsterBookCard> {
    let (Some(card_item_ids), Some(counts)) = (
        record.monster_book_card_item_ids.as_ref(),
        record.monster_book_card_counts.as_ref(),
    ) else {
        return Vec::new();
    };
    if card_item_ids.len() != counts.len() {
        return Vec::new();
    }
    let cards = card_item_ids
        .iter()
        .zip(counts)
        .map(|(card_item_id, count)| {
            Some(MonsterBookCard {
                card_item_id: u32::try_from(*card_item_id).ok()?,
                count: u32::try_from(*count).ok()?,
            })
        })
        .collect::<Option<Vec<_>>>();
    cards
        .and_then(|cards| crate::monster_book::canonicalize(cards).ok())
        .unwrap_or_default()
}

impl From<&PlayerState> for PlayerData {
    fn from(player: &PlayerState) -> Self {
        let position = player.position.as_ref().cloned().unwrap_or_default();
        let appearance = player.appearance.as_ref();
        let stats = player.stats.unwrap_or_else(starter_character_stats);
        let inventory = player
            .inventory
            .clone()
            .unwrap_or_else(crate::items::starter_inventory);
        let inventory_stacks = if inventory.stacks.is_empty() {
            crate::items::normalize_legacy_item_ids(&inventory.item_ids)
        } else {
            inventory.stacks.clone()
        };
        let inventory_item_ids = legacy_inventory_item_ids(&inventory_stacks);
        let inventory_stack_item_ids = inventory_stacks.iter().map(|stack| stack.item_id).collect();
        let inventory_stack_quantities = inventory_stacks
            .iter()
            .map(|stack| stack.quantity)
            .collect();
        let inventory_stack_expires_at_unix_ms = inventory_stacks
            .iter()
            .map(|stack| stack.expires_at_unix_ms)
            .collect();
        let equipped_top = equipped_item(&inventory, EquipmentSlot::Top);
        let equipped_bottom = equipped_item(&inventory, EquipmentSlot::Bottom);
        let equipped_shoes = equipped_item(&inventory, EquipmentSlot::Shoes);
        let key_binding_codes = player
            .key_bindings
            .iter()
            .map(|binding| binding.code.clone())
            .collect();
        let key_binding_actions = player
            .key_bindings
            .iter()
            .map(|binding| binding.action)
            .collect();
        let key_binding_skill_ids = player
            .key_bindings
            .iter()
            .map(|binding| binding.skill_id)
            .collect();
        let learned_skill_ids = player
            .learned_skills
            .iter()
            .map(|skill| skill.skill_id)
            .collect();
        let learned_skill_levels = player
            .learned_skills
            .iter()
            .map(|skill| skill.level)
            .collect();
        let learned_skill_master_levels = player
            .learned_skills
            .iter()
            .map(|skill| skill.master_level)
            .collect();
        let quest_ids = player.quests.iter().map(|quest| quest.quest_id).collect();
        let quest_statuses = player.quests.iter().map(|quest| quest.status).collect();
        let quest_accepted_at_unix_ms = player
            .quests
            .iter()
            .map(|quest| quest.accepted_at_unix_ms)
            .collect();
        let quest_completed_at_unix_ms = player
            .quests
            .iter()
            .map(|quest| quest.completed_at_unix_ms)
            .collect();
        let quest_mob_ids = player
            .quests
            .iter()
            .map(|quest| {
                quest
                    .mob_progress
                    .iter()
                    .map(|progress| progress.mob_id)
                    .collect()
            })
            .collect();
        let quest_mob_counts = player
            .quests
            .iter()
            .map(|quest| {
                quest
                    .mob_progress
                    .iter()
                    .map(|progress| progress.count)
                    .collect()
            })
            .collect();
        let quest_dialogue_steps = player
            .quests
            .iter()
            .map(|quest| quest.dialogue_step)
            .collect();
        let quest_completion_quiz_passed = player
            .quests
            .iter()
            .map(|quest| quest.completion_quiz_passed)
            .collect();
        let quest_record_ids = player
            .quest_records
            .iter()
            .map(|record| record.quest_id)
            .collect();
        let quest_record_entry_indices = player
            .quest_records
            .iter()
            .map(|record| record.entries.iter().map(|entry| entry.index).collect())
            .collect();
        let quest_record_entry_values = player
            .quest_records
            .iter()
            .map(|record| {
                record
                    .entries
                    .iter()
                    .map(|entry| entry.value.clone())
                    .collect()
            })
            .collect();
        let monster_book_card_item_ids = player
            .monster_book_cards
            .iter()
            .map(|card| card.card_item_id)
            .collect();
        let monster_book_card_counts = player
            .monster_book_cards
            .iter()
            .map(|card| card.count)
            .collect();
        Self {
            player_id: player.id.clone(),
            revision: None,
            name: player.name.clone(),
            level: player.level,
            job_id: stats.job_id,
            hp: stats.hp,
            max_hp: stats.max_hp,
            mp: stats.mp,
            max_mp: stats.max_mp,
            experience: stats.experience,
            experience_required: stats.experience_required,
            fame: stats.fame,
            ability_points: stats.ability_points,
            strength: stats.strength,
            dexterity: stats.dexterity,
            intelligence: stats.intelligence,
            luck: stats.luck,
            map_id: player.map_id,
            x: f64::from(position.x),
            y: f64::from(position.y),
            gender: appearance.map(|value| value.gender),
            skin_id: appearance.map(|value| value.skin_id),
            face_id: appearance.map(|value| value.face_id),
            hair_id: appearance.map(|value| value.hair_id),
            inventory_item_ids,
            inventory_stack_item_ids: Some(inventory_stack_item_ids),
            inventory_stack_quantities: Some(inventory_stack_quantities),
            inventory_stack_expires_at_unix_ms: Some(inventory_stack_expires_at_unix_ms),
            inventory_capacity: inventory.capacity,
            equipped_top: equipped_top.map(|equipped| equipped.item_id),
            equipped_bottom: equipped_bottom.map(|equipped| equipped.item_id),
            equipped_shoes: equipped_shoes.map(|equipped| equipped.item_id),
            equipped_top_expires_at_unix_ms: equipped_top
                .map(|equipped| equipped.expires_at_unix_ms),
            equipped_bottom_expires_at_unix_ms: equipped_bottom
                .map(|equipped| equipped.expires_at_unix_ms),
            equipped_shoes_expires_at_unix_ms: equipped_shoes
                .map(|equipped| equipped.expires_at_unix_ms),
            key_binding_codes,
            key_binding_actions,
            key_binding_skill_ids,
            skill_points: player.skill_points,
            learned_skill_ids,
            learned_skill_levels,
            learned_skill_master_levels,
            mesos: player.mesos,
            quest_ids,
            quest_statuses,
            quest_accepted_at_unix_ms: Some(quest_accepted_at_unix_ms),
            quest_completed_at_unix_ms: Some(quest_completed_at_unix_ms),
            quest_mob_ids: Some(quest_mob_ids),
            quest_mob_counts: Some(quest_mob_counts),
            quest_dialogue_steps: Some(quest_dialogue_steps),
            quest_completion_quiz_passed: Some(quest_completion_quiz_passed),
            quest_record_ids: Some(quest_record_ids),
            quest_record_entry_indices: Some(quest_record_entry_indices),
            quest_record_entry_values: Some(quest_record_entry_values),
            monster_book_card_item_ids: Some(monster_book_card_item_ids),
            monster_book_card_counts: Some(monster_book_card_counts),
        }
    }
}

fn legacy_inventory_item_ids(stacks: &[InventoryItemStack]) -> Vec<u32> {
    const MAX_COMPATIBILITY_ITEMS: usize = 100_000;

    let total = stacks.iter().try_fold(0_usize, |total, stack| {
        let quantity = usize::try_from(stack.quantity).ok()?;
        total.checked_add(quantity)
    });
    let Some(total) = total.filter(|total| *total <= MAX_COMPATIBILITY_ITEMS) else {
        return stacks.iter().map(|stack| stack.item_id).collect();
    };
    let mut item_ids = Vec::new();
    if item_ids.try_reserve_exact(total).is_err() {
        return stacks.iter().map(|stack| stack.item_id).collect();
    }
    for stack in stacks {
        item_ids.extend(std::iter::repeat_n(stack.item_id, stack.quantity as usize));
    }
    item_ids
}

impl From<&PlayerState> for PlayerPositionData {
    fn from(player: &PlayerState) -> Self {
        let position = player.position.as_ref().cloned().unwrap_or_default();
        Self {
            map_id: player.map_id,
            x: f64::from(position.x),
            y: f64::from(position.y),
        }
    }
}

impl From<&PlayerState> for PlayerSessionData {
    fn from(player: &PlayerState) -> Self {
        let position = PlayerPositionData::from(player);
        Self {
            map_id: position.map_id,
            x: position.x,
            y: position.y,
            key_binding_codes: player
                .key_bindings
                .iter()
                .map(|binding| binding.code.clone())
                .collect(),
            key_binding_actions: player
                .key_bindings
                .iter()
                .map(|binding| binding.action)
                .collect(),
            key_binding_skill_ids: player
                .key_bindings
                .iter()
                .map(|binding| binding.skill_id)
                .collect(),
        }
    }
}

fn equipped_item(
    inventory: &InventoryState,
    slot: EquipmentSlot,
) -> Option<&EquippedItem> {
    inventory
        .equipment
        .iter()
        .find(|equipped| equipped.slot == slot as i32)
}

#[cfg(test)]
mod tests {
    use oozems_proto::v1::CharacterAppearance;
    use oozems_proto::v1::CharacterGender;
    use oozems_proto::v1::PlayerState;
    use oozems_proto::v1::QuestStatus;
    use oozems_proto::v1::Vec2;
    use surrealdb::types::SurrealValue;

    use super::CharacterName;
    use super::PlayerId;
    use super::apply_player_preferences;
    use super::create_player;
    use super::load_player;
    use super::open_surreal_kv;
    use super::save_player;
    use super::save_player_position;
    use super::save_player_session;
    use super::starter_character_stats;

    #[derive(SurrealValue)]
    struct LegacyPlayerData {
        player_id: String,
        name: String,
        level: u32,
        map_id: u32,
        x: f64,
        y: f64,
        gender: Option<i32>,
        skin_id: Option<u32>,
        face_id: Option<u32>,
        hair_id: Option<u32>,
    }

    #[derive(SurrealValue)]
    struct LegacyInventoryPlayerData {
        player_id: String,
        name: String,
        level: u32,
        map_id: u32,
        x: f64,
        y: f64,
        inventory_item_ids: Vec<u32>,
        inventory_stack_item_ids: Option<Vec<u32>>,
        inventory_stack_quantities: Option<Vec<u32>>,
    }

    #[derive(SurrealValue)]
    struct LegacyQuestPlayerData {
        player_id: String,
        name: String,
        level: u32,
        map_id: u32,
        x: f64,
        y: f64,
        quest_ids: Vec<u32>,
        quest_statuses: Vec<i32>,
        quest_accepted_at_unix_ms: Option<Vec<i64>>,
        quest_completed_at_unix_ms: Option<Vec<i64>>,
        quest_mob_ids: Option<Vec<Vec<i64>>>,
        quest_mob_counts: Option<Vec<Vec<i64>>>,
    }

    #[derive(SurrealValue)]
    struct ExpiringInventoryPlayerData {
        player_id: String,
        name: String,
        level: u32,
        map_id: u32,
        x: f64,
        y: f64,
        inventory_item_ids: Vec<u32>,
        inventory_stack_item_ids: Option<Vec<u32>>,
        inventory_stack_quantities: Option<Vec<u32>>,
        inventory_stack_expires_at_unix_ms: Option<Vec<i64>>,
    }

    #[derive(SurrealValue)]
    struct LegacyEquipmentPlayerData {
        player_id: String,
        name: String,
        level: u32,
        map_id: u32,
        x: f64,
        y: f64,
        inventory_item_ids: Vec<u32>,
        equipped_top: Option<u32>,
        equipped_bottom: Option<u32>,
        equipped_shoes: Option<u32>,
    }

    #[derive(SurrealValue)]
    struct ExpiringEquipmentPlayerData {
        player_id: String,
        name: String,
        level: u32,
        map_id: u32,
        x: f64,
        y: f64,
        inventory_item_ids: Vec<u32>,
        equipped_top: Option<u32>,
        equipped_bottom: Option<u32>,
        equipped_shoes: Option<u32>,
        equipped_top_expires_at_unix_ms: Option<i64>,
        equipped_bottom_expires_at_unix_ms: Option<i64>,
        equipped_shoes_expires_at_unix_ms: Option<i64>,
    }

    #[derive(SurrealValue)]
    struct QuestRecordPlayerData {
        player_id: String,
        name: String,
        level: u32,
        map_id: u32,
        x: f64,
        y: f64,
        quest_record_ids: Option<Vec<i64>>,
        quest_record_entry_indices: Option<Vec<Vec<i64>>>,
        quest_record_entry_values: Option<Vec<Vec<String>>>,
    }

    #[derive(SurrealValue)]
    struct LearnedSkillPlayerData {
        player_id: String,
        name: String,
        level: u32,
        map_id: u32,
        x: f64,
        y: f64,
        learned_skill_ids: Option<Vec<u32>>,
        learned_skill_levels: Option<Vec<u32>>,
        learned_skill_master_levels: Option<Vec<u32>>,
    }

    #[derive(SurrealValue)]
    struct MonsterBookPlayerData {
        player_id: String,
        name: String,
        level: u32,
        map_id: u32,
        x: f64,
        y: f64,
        monster_book_card_item_ids: Option<Vec<i64>>,
        monster_book_card_counts: Option<Vec<i64>>,
    }

    #[test]
    fn preference_updates_do_not_change_authoritative_fields() {
        let current = PlayerState {
            id: "local".to_owned(),
            name: "Newcomer".to_owned(),
            level: 7,
            map_id: 1,
            position: Some(Vec2 { x: 5.0, y: 5.0 }),
            appearance: Some(appearance()),
            stats: Some(starter_character_stats()),
            inventory: Some(crate::items::starter_inventory()),
            key_bindings: crate::keymap::default_bindings(),
            skill_points: 3,
            learned_skills: Vec::new(),
            mesos: 500,
            quests: vec![oozems_proto::v1::PlayerQuest {
                quest_id: 100,
                status: QuestStatus::Started as i32,
                ..oozems_proto::v1::PlayerQuest::default()
            }],
            revision: 10,
            quest_records: vec![oozems_proto::v1::QuestRecord {
                quest_id: 100,
                entries: vec![oozems_proto::v1::QuestRecordEntry {
                    index: 0,
                    value: "current".to_owned(),
                }],
            }],
            monster_book_cards: vec![oozems_proto::v1::MonsterBookCard {
                card_item_id: 2_380_000,
                count: 3,
            }],
        };
        let requested = PlayerState {
            id: "local".to_owned(),
            name: "Changed".to_owned(),
            level: 99,
            map_id: 2,
            position: Some(Vec2 { x: -10.0, y: 900.0 }),
            appearance: None,
            stats: None,
            inventory: None,
            key_bindings: crate::keymap::default_bindings(),
            skill_points: 99,
            learned_skills: vec![oozems_proto::v1::LearnedSkill {
                skill_id: 1_000,
                level: 2,
                master_level: 0,
            }],
            mesos: 99_999,
            quests: vec![oozems_proto::v1::PlayerQuest {
                quest_id: 100,
                status: QuestStatus::Completed as i32,
                ..oozems_proto::v1::PlayerQuest::default()
            }],
            revision: u64::MAX,
            quest_records: vec![oozems_proto::v1::QuestRecord {
                quest_id: 100,
                entries: vec![oozems_proto::v1::QuestRecordEntry {
                    index: 0,
                    value: "requested".to_owned(),
                }],
            }],
            monster_book_cards: vec![oozems_proto::v1::MonsterBookCard {
                card_item_id: 2_380_000,
                count: 5,
            }],
        };

        let result = apply_player_preferences(current, &requested);

        assert_eq!(result.name, "Newcomer");
        assert_eq!(result.level, 7);
        assert_eq!(result.map_id, 1);
        assert_eq!(result.position, Some(Vec2 { x: 5.0, y: 5.0 }));
        assert_eq!(result.appearance, Some(appearance()));
        assert_eq!(result.stats, Some(starter_character_stats()));
        assert_eq!(result.inventory, Some(crate::items::starter_inventory()));
        assert_eq!(result.skill_points, 3);
        assert!(result.learned_skills.is_empty());
        assert_eq!(result.mesos, 500);
        assert_eq!(result.quests[0].status, QuestStatus::Started as i32);
        assert_eq!(result.quest_records[0].entries[0].value, "current");
        assert_eq!(result.monster_book_cards[0].count, 3);
        assert_eq!(result.revision, 10);
    }

    #[tokio::test]
    async fn player_round_trip_uses_surreal_kv() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let database = open_surreal_kv(&directory.path().join("database"))
            .await
            .expect("open SurrealKV");
        let player_id = PlayerId::parse("database-test").expect("valid player ID");
        assert_eq!(
            load_player(&database, &player_id, 3)
                .await
                .expect("load absent player"),
            None
        );
        let name = CharacterName::parse("Mina").expect("valid name");
        let mut player = create_player(
            &database,
            &player_id,
            &name,
            appearance(),
            10_000,
            Vec2 { x: 160.0, y: 420.0 },
            123,
            3,
        )
        .await
        .expect("create player");
        assert_eq!(player.revision, 1);
        assert_eq!(player.map_id, 10_000);
        assert_eq!(
            player.stats.as_ref().map(|stats| stats.experience_required),
            Some(123)
        );
        player.position = Some(Vec2 { x: 321.0, y: 456.0 });
        player.skill_points = 2;
        player.learned_skills.push(oozems_proto::v1::LearnedSkill {
            skill_id: 1_000,
            level: 1,
            master_level: 3,
        });
        player.learned_skills.push(oozems_proto::v1::LearnedSkill {
            skill_id: 2_321_003,
            level: 0,
            master_level: 10,
        });
        player.mesos = 1_234;
        player.monster_book_cards = vec![
            oozems_proto::v1::MonsterBookCard {
                card_item_id: 2_380_001,
                count: 5,
            },
            oozems_proto::v1::MonsterBookCard {
                card_item_id: 2_380_000,
                count: 2,
            },
        ];
        player.quests.push(oozems_proto::v1::PlayerQuest {
            quest_id: 1_009,
            status: QuestStatus::Started as i32,
            mob_progress: vec![oozems_proto::v1::QuestMobProgress {
                mob_id: 100_100,
                count: 3,
            }],
            accepted_at_unix_ms: 123_456,
            completed_at_unix_ms: 0,
            dialogue_step: 2,
            completion_quiz_passed: false,
        });
        player.key_bindings.push(oozems_proto::v1::KeyBinding {
            code: "F1".to_owned(),
            action: oozems_proto::v1::KeyAction::Unspecified as i32,
            skill_id: 1_000,
        });
        player.inventory.as_mut().expect("inventory").stacks[0].quantity = 3;
        player.inventory.as_mut().expect("inventory").stacks[0].expires_at_unix_ms =
            1_900_000_000_000;
        player
            .inventory
            .as_mut()
            .expect("inventory")
            .equipment
            .iter_mut()
            .find(|equipped| equipped.slot == oozems_proto::v1::EquipmentSlot::Top as i32)
            .expect("equipped top")
            .expires_at_unix_ms = 1_900_000_000_123;
        crate::quest_records::set(&mut player, 2_236, 42, "sparse".to_owned())
            .expect("sparse quest record");
        crate::quest_records::set(&mut player, 2_236, 0, "000000".to_owned())
            .expect("primary quest record");
        crate::quest_records::set(&mut player, 100, 9, "helper".to_owned())
            .expect("helper quest record");

        player = save_player(&database, &player).await.expect("save player");
        assert_eq!(player.revision, 2);
        let loaded = load_player(&database, &player_id, 3)
            .await
            .expect("load player")
            .expect("saved player exists");

        assert_eq!(loaded, player);
        assert_eq!(loaded.monster_book_cards[0].card_item_id, 2_380_000);
        assert_eq!(
            loaded
                .quest_records
                .iter()
                .map(|record| record.quest_id)
                .collect::<Vec<_>>(),
            vec![100, 2_236]
        );
        assert_eq!(
            loaded.quest_records[1]
                .entries
                .iter()
                .map(|entry| entry.index)
                .collect::<Vec<_>>(),
            vec![0, 42]
        );
    }

    #[tokio::test]
    async fn learned_skill_master_arrays_recover_legacy_and_malformed_records() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let database = open_surreal_kv(&directory.path().join("database"))
            .await
            .expect("open SurrealKV");
        for (player_id, ids, levels, masters, expected) in [
            (
                "legacy-skills",
                vec![1_000],
                vec![2],
                None,
                vec![(1_000, 2, 0)],
            ),
            (
                "master-only",
                vec![2_321_003],
                vec![0],
                Some(vec![10]),
                vec![(2_321_003, 0, 10)],
            ),
            (
                "canonical-skills",
                vec![2_321_003, 1_000, 2_321_003],
                vec![0, 2, 1],
                Some(vec![10, 0, 5]),
                vec![(1_000, 2, 0), (2_321_003, 1, 10)],
            ),
            (
                "malformed-skills",
                vec![1_000],
                vec![2],
                Some(Vec::new()),
                Vec::new(),
            ),
        ] {
            let data = LearnedSkillPlayerData {
                player_id: player_id.to_owned(),
                name: "Mina".to_owned(),
                level: 1,
                map_id: 10_000,
                x: 0.0,
                y: 0.0,
                learned_skill_ids: Some(ids),
                learned_skill_levels: Some(levels),
                learned_skill_master_levels: masters,
            };
            let _: Option<super::PlayerRecord> = database
                .upsert(("player", player_id))
                .content(data)
                .await
                .expect("insert learned skill record");
            let loaded = load_player(
                &database,
                &PlayerId::parse(player_id).expect("player ID"),
                3,
            )
            .await
            .expect("load learned skills")
            .expect("player record");
            assert_eq!(
                loaded
                    .learned_skills
                    .iter()
                    .map(|skill| (skill.skill_id, skill.level, skill.master_level))
                    .collect::<Vec<_>>(),
                expected,
                "{player_id}",
            );
        }
    }

    #[tokio::test]
    async fn position_and_session_updates_do_not_increment_the_revision() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let database = open_surreal_kv(&directory.path().join("database"))
            .await
            .expect("open SurrealKV");
        let player_id = PlayerId::parse("position-test").expect("valid player ID");
        let name = CharacterName::parse("Mina").expect("valid name");
        let mut player = create_player(
            &database,
            &player_id,
            &name,
            appearance(),
            10_000,
            Vec2 { x: 160.0, y: 420.0 },
            123,
            3,
        )
        .await
        .expect("create player");
        let original_stats = player.stats;
        let original_revision = player.revision;
        player.position = Some(Vec2 { x: 300.0, y: 250.0 });
        player.stats.as_mut().expect("stats").hp = 1;

        save_player_position(&database, &player)
            .await
            .expect("save position");
        save_player_session(&database, &player)
            .await
            .expect("save session");
        let loaded = load_player(&database, &player_id, 3)
            .await
            .expect("load player")
            .expect("saved player exists");

        assert_eq!(loaded.position, Some(Vec2 { x: 300.0, y: 250.0 }));
        assert_eq!(loaded.stats, original_stats);
        assert_eq!(loaded.revision, original_revision);
    }

    #[tokio::test]
    async fn full_saves_increment_past_the_input_and_saved_revisions() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let database = open_surreal_kv(&directory.path().join("database"))
            .await
            .expect("open SurrealKV");
        let player_id = PlayerId::parse("revision-test").expect("valid player ID");
        let name = CharacterName::parse("Mina").expect("valid name");
        let created = create_player(
            &database,
            &player_id,
            &name,
            appearance(),
            10_000,
            Vec2 { x: 160.0, y: 420.0 },
            123,
            3,
        )
        .await
        .expect("create player");

        let saved = save_player(&database, &created).await.expect("save player");
        let mut ahead = created.clone();
        ahead.revision = 7;
        let saved_ahead = save_player(&database, &ahead)
            .await
            .expect("save player with an ahead revision");
        let saved_again = save_player(&database, &created)
            .await
            .expect("save stale player");

        assert_eq!(created.revision, 1);
        assert_eq!(saved.revision, 2);
        assert_eq!(saved_ahead.revision, 8);
        assert_eq!(saved_again.revision, 9);
    }

    #[test]
    fn revision_increment_reports_overflow() {
        let error =
            super::next_player_revision(i64::MAX as u64, 0).expect_err("revision must overflow");

        assert!(error.to_string().contains("player revision overflow"));
    }

    #[tokio::test]
    async fn legacy_player_records_receive_starter_stats() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let database = open_surreal_kv(&directory.path().join("database"))
            .await
            .expect("open SurrealKV");
        let player_id = PlayerId::parse("legacy-test").expect("valid player ID");
        let legacy = LegacyPlayerData {
            player_id: player_id.as_str().to_owned(),
            name: "Legacy".to_owned(),
            level: 3,
            map_id: 100_000_000,
            x: 160.0,
            y: 420.0,
            gender: Some(CharacterGender::Male as i32),
            skin_id: Some(2_000),
            face_id: Some(20_000),
            hair_id: Some(30_000),
        };
        let _: Option<super::PlayerRecord> = database
            .upsert(("player", player_id.as_str()))
            .content(legacy)
            .await
            .expect("insert legacy player");

        let loaded = load_player(&database, &player_id, 7)
            .await
            .expect("load legacy player")
            .expect("legacy player exists");

        assert_eq!(loaded.stats, Some(starter_character_stats()));
        assert_eq!(loaded.inventory, Some(crate::items::starter_inventory()));
        assert_eq!(loaded.key_bindings, crate::keymap::default_bindings());
        assert_eq!(loaded.skill_points, 7);
        assert!(loaded.learned_skills.is_empty());
        assert_eq!(loaded.mesos, 0);
        assert!(loaded.quests.is_empty());
        assert!(loaded.quest_records.is_empty());
        assert!(loaded.monster_book_cards.is_empty());
        assert_eq!(loaded.revision, 0);
    }

    #[tokio::test]
    async fn legacy_inventory_ids_migrate_to_single_unit_stacks() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let database = open_surreal_kv(&directory.path().join("database"))
            .await
            .expect("open SurrealKV");
        let player_id = PlayerId::parse("legacy-inventory").expect("valid player ID");
        let item_ids = vec![
            crate::items::SPARE_TOP_ID,
            crate::items::SPARE_TOP_ID,
            crate::items::SPARE_BOTTOM_ID,
        ];
        let legacy = LegacyInventoryPlayerData {
            player_id: player_id.as_str().to_owned(),
            name: "LegacyItems".to_owned(),
            level: 3,
            map_id: 100_000_000,
            x: 160.0,
            y: 420.0,
            inventory_item_ids: item_ids.clone(),
            inventory_stack_item_ids: None,
            inventory_stack_quantities: None,
        };
        let _: Option<super::PlayerRecord> = database
            .upsert(("player", player_id.as_str()))
            .content(legacy)
            .await
            .expect("insert legacy inventory");

        let loaded = load_player(&database, &player_id, 3)
            .await
            .expect("load legacy inventory")
            .expect("legacy player exists");
        let inventory = loaded.inventory.expect("inventory");

        assert!(inventory.item_ids.is_empty());
        assert_eq!(
            inventory.stacks,
            crate::items::normalize_legacy_item_ids(&item_ids)
        );
    }

    #[tokio::test]
    async fn malformed_stack_arrays_fall_back_to_legacy_inventory() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let database = open_surreal_kv(&directory.path().join("database"))
            .await
            .expect("open SurrealKV");
        let player_id = PlayerId::parse("malformed-stacks").expect("valid player ID");
        let legacy_item_ids = vec![crate::items::SPARE_TOP_ID, crate::items::SPARE_BOTTOM_ID];
        let legacy = LegacyInventoryPlayerData {
            player_id: player_id.as_str().to_owned(),
            name: "Fallback".to_owned(),
            level: 3,
            map_id: 100_000_000,
            x: 160.0,
            y: 420.0,
            inventory_item_ids: legacy_item_ids.clone(),
            inventory_stack_item_ids: Some(vec![crate::items::SPARE_SHOES_ID]),
            inventory_stack_quantities: Some(Vec::new()),
        };
        let _: Option<super::PlayerRecord> = database
            .upsert(("player", player_id.as_str()))
            .content(legacy)
            .await
            .expect("insert malformed stacks");

        let loaded = load_player(&database, &player_id, 3)
            .await
            .expect("load fallback inventory")
            .expect("legacy player exists");

        assert_eq!(
            loaded.inventory.expect("inventory").stacks,
            crate::items::normalize_legacy_item_ids(&legacy_item_ids)
        );
    }

    #[tokio::test]
    async fn valid_stack_arrays_take_precedence_over_legacy_inventory() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let database = open_surreal_kv(&directory.path().join("database"))
            .await
            .expect("open SurrealKV");
        let player_id = PlayerId::parse("stack-precedence").expect("valid player ID");
        let data = LegacyInventoryPlayerData {
            player_id: player_id.as_str().to_owned(),
            name: "NewStacks".to_owned(),
            level: 3,
            map_id: 100_000_000,
            x: 160.0,
            y: 420.0,
            inventory_item_ids: vec![crate::items::SPARE_TOP_ID],
            inventory_stack_item_ids: Some(vec![crate::items::SPARE_BOTTOM_ID]),
            inventory_stack_quantities: Some(vec![7]),
        };
        let _: Option<super::PlayerRecord> = database
            .upsert(("player", player_id.as_str()))
            .content(data)
            .await
            .expect("insert stack inventory");

        let loaded = load_player(&database, &player_id, 3)
            .await
            .expect("load stack inventory")
            .expect("player exists");

        assert_eq!(
            loaded.inventory.expect("inventory").stacks,
            vec![oozems_proto::v1::InventoryItemStack {
                item_id: crate::items::SPARE_BOTTOM_ID,
                quantity: 7,
                expires_at_unix_ms: 0,
            }]
        );
    }

    #[tokio::test]
    async fn malformed_expiration_array_falls_back_to_permanent_legacy_inventory() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let database = open_surreal_kv(&directory.path().join("database"))
            .await
            .expect("open SurrealKV");
        let player_id = PlayerId::parse("malformed-expiry").expect("valid player ID");
        let data = ExpiringInventoryPlayerData {
            player_id: player_id.as_str().to_owned(),
            name: "ExpiryFallback".to_owned(),
            level: 3,
            map_id: 100_000_000,
            x: 160.0,
            y: 420.0,
            inventory_item_ids: vec![crate::items::SPARE_TOP_ID],
            inventory_stack_item_ids: Some(vec![crate::items::SPARE_BOTTOM_ID]),
            inventory_stack_quantities: Some(vec![7]),
            inventory_stack_expires_at_unix_ms: Some(Vec::new()),
        };
        let _: Option<super::PlayerRecord> = database
            .upsert(("player", player_id.as_str()))
            .content(data)
            .await
            .expect("insert malformed expiration inventory");

        let loaded = load_player(&database, &player_id, 3)
            .await
            .expect("load fallback inventory")
            .expect("player exists");

        assert_eq!(
            loaded.inventory.expect("inventory").stacks,
            vec![oozems_proto::v1::InventoryItemStack {
                item_id: crate::items::SPARE_TOP_ID,
                quantity: 1,
                expires_at_unix_ms: 0,
            }]
        );
    }

    #[tokio::test]
    async fn aligned_expiration_array_round_trips_stack_deadlines() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let database = open_surreal_kv(&directory.path().join("database"))
            .await
            .expect("open SurrealKV");
        let player_id = PlayerId::parse("stack-expiry").expect("valid player ID");
        let deadline = 1_900_000_000_000_i64;
        let data = ExpiringInventoryPlayerData {
            player_id: player_id.as_str().to_owned(),
            name: "ExpiringStack".to_owned(),
            level: 3,
            map_id: 100_000_000,
            x: 160.0,
            y: 420.0,
            inventory_item_ids: vec![crate::items::SPARE_TOP_ID],
            inventory_stack_item_ids: Some(vec![crate::items::SPARE_BOTTOM_ID]),
            inventory_stack_quantities: Some(vec![7]),
            inventory_stack_expires_at_unix_ms: Some(vec![deadline]),
        };
        let _: Option<super::PlayerRecord> = database
            .upsert(("player", player_id.as_str()))
            .content(data)
            .await
            .expect("insert expiring inventory");

        let loaded = load_player(&database, &player_id, 3)
            .await
            .expect("load expiring inventory")
            .expect("player exists");

        assert_eq!(
            loaded.inventory.expect("inventory").stacks,
            vec![oozems_proto::v1::InventoryItemStack {
                item_id: crate::items::SPARE_BOTTOM_ID,
                quantity: 7,
                expires_at_unix_ms: deadline as u64,
            }]
        );
    }

    #[tokio::test]
    async fn equipment_expiration_loads_legacy_exact_and_malformed_slots_safely() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let database = open_surreal_kv(&directory.path().join("database"))
            .await
            .expect("open SurrealKV");
        let legacy_id = PlayerId::parse("legacy-equipment").expect("valid player ID");
        let legacy = LegacyEquipmentPlayerData {
            player_id: legacy_id.as_str().to_owned(),
            name: "LegacyEquip".to_owned(),
            level: 3,
            map_id: 100_000_000,
            x: 160.0,
            y: 420.0,
            inventory_item_ids: vec![crate::items::SPARE_TOP_ID],
            equipped_top: Some(crate::items::STARTER_TOP_ID),
            equipped_bottom: None,
            equipped_shoes: None,
        };
        let _: Option<super::PlayerRecord> = database
            .upsert(("player", legacy_id.as_str()))
            .content(legacy)
            .await
            .expect("insert legacy equipment");

        let legacy = load_player(&database, &legacy_id, 3)
            .await
            .expect("load legacy equipment")
            .expect("legacy player");
        assert_eq!(
            legacy.inventory.expect("inventory").equipment,
            vec![oozems_proto::v1::EquippedItem {
                slot: oozems_proto::v1::EquipmentSlot::Top as i32,
                item_id: crate::items::STARTER_TOP_ID,
                expires_at_unix_ms: 0,
            }]
        );

        let exact_id = PlayerId::parse("exact-equipment").expect("valid player ID");
        let exact = ExpiringEquipmentPlayerData {
            player_id: exact_id.as_str().to_owned(),
            name: "ExactEquip".to_owned(),
            level: 3,
            map_id: 100_000_000,
            x: 160.0,
            y: 420.0,
            inventory_item_ids: vec![crate::items::SPARE_TOP_ID],
            equipped_top: Some(crate::items::STARTER_TOP_ID),
            equipped_bottom: Some(crate::items::STARTER_BOTTOM_ID),
            equipped_shoes: Some(crate::items::STARTER_SHOES_ID),
            equipped_top_expires_at_unix_ms: Some(1_900_000_000_123),
            equipped_bottom_expires_at_unix_ms: Some(-1),
            equipped_shoes_expires_at_unix_ms: None,
        };
        let _: Option<super::PlayerRecord> = database
            .upsert(("player", exact_id.as_str()))
            .content(exact)
            .await
            .expect("insert exact and malformed equipment");

        let exact = load_player(&database, &exact_id, 3)
            .await
            .expect("load exact and malformed equipment")
            .expect("exact player");
        let inventory = exact.inventory.expect("inventory");
        assert_eq!(
            inventory.stacks,
            crate::items::normalize_legacy_item_ids(&[crate::items::SPARE_TOP_ID])
        );
        assert_eq!(
            inventory
                .equipment
                .iter()
                .map(|equipped| { (equipped.slot, equipped.item_id, equipped.expires_at_unix_ms,) })
                .collect::<Vec<_>>(),
            vec![
                (
                    oozems_proto::v1::EquipmentSlot::Top as i32,
                    crate::items::STARTER_TOP_ID,
                    1_900_000_000_123,
                ),
                (
                    oozems_proto::v1::EquipmentSlot::Shoes as i32,
                    crate::items::STARTER_SHOES_ID,
                    0,
                ),
            ]
        );
    }

    #[tokio::test]
    async fn legacy_quest_arrays_receive_zero_progress_metadata() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let database = open_surreal_kv(&directory.path().join("database"))
            .await
            .expect("open SurrealKV");
        let player_id = PlayerId::parse("legacy-quests").expect("valid player ID");
        let data = LegacyQuestPlayerData {
            player_id: player_id.as_str().to_owned(),
            name: "LegacyQuest".to_owned(),
            level: 3,
            map_id: 100_000_000,
            x: 160.0,
            y: 420.0,
            quest_ids: vec![100, 200],
            quest_statuses: vec![QuestStatus::Started as i32, QuestStatus::Completed as i32],
            quest_accepted_at_unix_ms: None,
            quest_completed_at_unix_ms: None,
            quest_mob_ids: None,
            quest_mob_counts: None,
        };
        let _: Option<super::PlayerRecord> = database
            .upsert(("player", player_id.as_str()))
            .content(data)
            .await
            .expect("insert legacy quests");

        let loaded = load_player(&database, &player_id, 3)
            .await
            .expect("load legacy quests")
            .expect("player exists");

        assert_eq!(loaded.quests.len(), 2);
        assert_eq!(loaded.quests[0].quest_id, 100);
        assert_eq!(loaded.quests[0].status, QuestStatus::Started as i32);
        assert!(loaded.quests[0].mob_progress.is_empty());
        assert_eq!(loaded.quests[0].accepted_at_unix_ms, 0);
        assert_eq!(loaded.quests[1].quest_id, 200);
        assert_eq!(loaded.quests[1].status, QuestStatus::Completed as i32);
        assert_eq!(loaded.quests[1].completed_at_unix_ms, 0);
    }

    #[tokio::test]
    async fn malformed_quest_metadata_preserves_ids_and_statuses() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let database = open_surreal_kv(&directory.path().join("database"))
            .await
            .expect("open SurrealKV");
        let player_id = PlayerId::parse("malformed-quests").expect("valid player ID");
        let data = LegacyQuestPlayerData {
            player_id: player_id.as_str().to_owned(),
            name: "Malformed".to_owned(),
            level: 3,
            map_id: 100_000_000,
            x: 160.0,
            y: 420.0,
            quest_ids: vec![100, 200],
            quest_statuses: vec![QuestStatus::Started as i32, QuestStatus::Completed as i32],
            quest_accepted_at_unix_ms: Some(vec![-1]),
            quest_completed_at_unix_ms: Some(vec![0, 500]),
            quest_mob_ids: Some(vec![vec![100_100], vec![200_200]]),
            quest_mob_counts: Some(vec![Vec::new(), vec![-1]]),
        };
        let _: Option<super::PlayerRecord> = database
            .upsert(("player", player_id.as_str()))
            .content(data)
            .await
            .expect("insert malformed quest metadata");

        let loaded = load_player(&database, &player_id, 3)
            .await
            .expect("load malformed quests")
            .expect("player exists");

        assert_eq!(
            loaded
                .quests
                .iter()
                .map(|quest| (quest.quest_id, quest.status))
                .collect::<Vec<_>>(),
            vec![
                (100, QuestStatus::Started as i32),
                (200, QuestStatus::Completed as i32),
            ]
        );
        assert!(loaded.quests.iter().all(|quest| {
            quest.mob_progress.is_empty()
                && quest.accepted_at_unix_ms == 0
                && quest.completed_at_unix_ms == 0
        }));
    }

    #[tokio::test]
    async fn malformed_record_arrays_recover_to_an_empty_collection() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let database = open_surreal_kv(&directory.path().join("database"))
            .await
            .expect("open SurrealKV");
        let player_id = PlayerId::parse("malformed-records").expect("valid player ID");
        let data = QuestRecordPlayerData {
            player_id: player_id.as_str().to_owned(),
            name: "BadRecords".to_owned(),
            level: 3,
            map_id: 100_000_000,
            x: 160.0,
            y: 420.0,
            quest_record_ids: Some(vec![100, 200]),
            quest_record_entry_indices: Some(vec![vec![0]]),
            quest_record_entry_values: Some(vec![vec!["first".to_owned()], Vec::new()]),
        };
        let _: Option<super::PlayerRecord> = database
            .upsert(("player", player_id.as_str()))
            .content(data)
            .await
            .expect("insert malformed records");

        let loaded = load_player(&database, &player_id, 3)
            .await
            .expect("load malformed records")
            .expect("player exists");

        assert!(loaded.quest_records.is_empty());
    }

    #[tokio::test]
    async fn monster_book_arrays_load_sorted_or_fail_as_one_collection() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let database = open_surreal_kv(&directory.path().join("database"))
            .await
            .expect("open SurrealKV");
        let cases = [
            ("legacy-book", None, None, Vec::new()),
            (
                "sorted-book",
                Some(vec![2_380_001, 2_380_000]),
                Some(vec![5, 2]),
                vec![(2_380_000, 2), (2_380_001, 5)],
            ),
            (
                "misaligned-book",
                Some(vec![2_380_000]),
                Some(Vec::new()),
                Vec::new(),
            ),
            (
                "invalid-book",
                Some(vec![2_380_000, 2_380_001]),
                Some(vec![1, 6]),
                Vec::new(),
            ),
            (
                "duplicate-book",
                Some(vec![2_380_000, 2_380_000]),
                Some(vec![1, 2]),
                Vec::new(),
            ),
        ];
        for (player_id, card_item_ids, counts, expected) in cases {
            let data = MonsterBookPlayerData {
                player_id: player_id.to_owned(),
                name: "Mina".to_owned(),
                level: 1,
                map_id: 10_000,
                x: 0.0,
                y: 0.0,
                monster_book_card_item_ids: card_item_ids,
                monster_book_card_counts: counts,
            };
            let _: Option<super::PlayerRecord> = database
                .upsert(("player", player_id))
                .content(data)
                .await
                .expect("insert Monster Book record");
            let loaded = load_player(
                &database,
                &PlayerId::parse(player_id).expect("player ID"),
                3,
            )
            .await
            .expect("load Monster Book")
            .expect("player record");
            assert_eq!(
                loaded
                    .monster_book_cards
                    .iter()
                    .map(|card| (card.card_item_id, card.count))
                    .collect::<Vec<_>>(),
                expected,
                "{player_id}",
            );
        }
    }

    #[tokio::test]
    async fn duplicate_or_invalid_record_data_is_not_partially_loaded_or_saved() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let database = open_surreal_kv(&directory.path().join("database"))
            .await
            .expect("open SurrealKV");
        let player_id = PlayerId::parse("duplicate-records").expect("valid player ID");
        let data = QuestRecordPlayerData {
            player_id: player_id.as_str().to_owned(),
            name: "Duplicate".to_owned(),
            level: 3,
            map_id: 100_000_000,
            x: 160.0,
            y: 420.0,
            quest_record_ids: Some(vec![100, 100]),
            quest_record_entry_indices: Some(vec![vec![0], vec![1]]),
            quest_record_entry_values: Some(vec![
                vec!["first".to_owned()],
                vec!["second".to_owned()],
            ]),
        };
        let _: Option<super::PlayerRecord> = database
            .upsert(("player", player_id.as_str()))
            .content(data)
            .await
            .expect("insert duplicate records");
        let mut loaded = load_player(&database, &player_id, 3)
            .await
            .expect("load duplicate records")
            .expect("player exists");
        assert!(loaded.quest_records.is_empty());

        loaded.quest_records = vec![
            oozems_proto::v1::QuestRecord {
                quest_id: 100,
                entries: Vec::new(),
            },
            oozems_proto::v1::QuestRecord {
                quest_id: 100,
                entries: Vec::new(),
            },
        ];
        let error = save_player(&database, &loaded)
            .await
            .expect_err("duplicate in-memory records must not save");
        assert!(error.to_string().contains("appears more than once"));
    }

    fn appearance() -> CharacterAppearance {
        CharacterAppearance {
            gender: CharacterGender::Female as i32,
            skin_id: 2_000,
            face_id: 21_000,
            hair_id: 31_000,
        }
    }
}
