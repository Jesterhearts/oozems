use oozems_proto::v1::CharacterAppearance;
use oozems_proto::v1::CharacterStats;
use oozems_proto::v1::EquippedItem;
use oozems_proto::v1::InventoryItemStack;
use oozems_proto::v1::InventoryState;
use oozems_proto::v1::KeyBinding;
use oozems_proto::v1::LearnedSkill;
use oozems_proto::v1::MonsterBookCard;
use oozems_proto::v1::PlayerQuest;
use oozems_proto::v1::QuestMobProgress;
use oozems_proto::v1::QuestRecord;
use oozems_proto::v1::QuestRecordEntry;
use rusqlite::Connection;
use rusqlite::OptionalExtension;
use rusqlite::TransactionBehavior;
use rusqlite::params;

use super::DatabaseError;
use super::player_record::DurablePlayer;
use super::player_record::DurablePlayerData;

struct ParentRow {
    revision: i64,
    name: String,
    level: i64,
    map_id: i64,
    appearance_gender: i64,
    skin_id: i64,
    face_id: i64,
    hair_id: i64,
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
    inventory_capacity: i64,
    skill_points: i64,
    mesos: i64,
    cash_points: i64,
}

pub(super) fn load_player(
    connection: &mut Connection,
    player_id: &str,
) -> Result<Option<DurablePlayer>, DatabaseError> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
    let player = load_player_in(&transaction, player_id)?;
    transaction.commit()?;
    Ok(player)
}

fn load_player_in(
    connection: &Connection,
    player_id: &str,
) -> Result<Option<DurablePlayer>, DatabaseError> {
    let parent = connection
        .query_row(
            "SELECT revision, name, level, map_id,
                    appearance_gender, skin_id, face_id, hair_id,
                    job_id, hp, max_hp, mp, max_mp, experience,
                    experience_required, fame, ability_points, strength,
                    dexterity, intelligence, luck, inventory_capacity,
                    skill_points, mesos, cash_points
             FROM players WHERE player_id = ?1",
            [player_id],
            |row| {
                Ok(ParentRow {
                    revision: row.get(0)?,
                    name: row.get(1)?,
                    level: row.get(2)?,
                    map_id: row.get(3)?,
                    appearance_gender: row.get(4)?,
                    skin_id: row.get(5)?,
                    face_id: row.get(6)?,
                    hair_id: row.get(7)?,
                    job_id: row.get(8)?,
                    hp: row.get(9)?,
                    max_hp: row.get(10)?,
                    mp: row.get(11)?,
                    max_mp: row.get(12)?,
                    experience: row.get(13)?,
                    experience_required: row.get(14)?,
                    fame: row.get(15)?,
                    ability_points: row.get(16)?,
                    strength: row.get(17)?,
                    dexterity: row.get(18)?,
                    intelligence: row.get(19)?,
                    luck: row.get(20)?,
                    inventory_capacity: row.get(21)?,
                    skill_points: row.get(22)?,
                    mesos: row.get(23)?,
                    cash_points: row.get(24)?,
                })
            },
        )
        .optional()?;
    let Some(parent) = parent else {
        return Ok(None);
    };

    let inventory = InventoryState {
        equipment: load_equipped_items(connection, player_id)?,
        capacity: u32_value(parent.inventory_capacity, player_id, "inventory.capacity")?,
        stacks: load_inventory_stacks(connection, player_id)?,
    };
    let data = DurablePlayerData {
        name: parent.name,
        level: u32_value(parent.level, player_id, "level")?,
        map_id: u32_value(parent.map_id, player_id, "map_id")?,
        appearance: CharacterAppearance {
            gender: i32_value(parent.appearance_gender, player_id, "appearance.gender")?,
            skin_id: u32_value(parent.skin_id, player_id, "appearance.skin_id")?,
            face_id: u32_value(parent.face_id, player_id, "appearance.face_id")?,
            hair_id: u32_value(parent.hair_id, player_id, "appearance.hair_id")?,
        },
        stats: CharacterStats {
            job_id: u32_value(parent.job_id, player_id, "stats.job_id")?,
            hp: u32_value(parent.hp, player_id, "stats.hp")?,
            max_hp: u32_value(parent.max_hp, player_id, "stats.max_hp")?,
            mp: u32_value(parent.mp, player_id, "stats.mp")?,
            max_mp: u32_value(parent.max_mp, player_id, "stats.max_mp")?,
            experience: u64_value(parent.experience, player_id, "stats.experience")?,
            fame: i32_value(parent.fame, player_id, "stats.fame")?,
            ability_points: u32_value(parent.ability_points, player_id, "stats.ability_points")?,
            strength: u32_value(parent.strength, player_id, "stats.strength")?,
            dexterity: u32_value(parent.dexterity, player_id, "stats.dexterity")?,
            intelligence: u32_value(parent.intelligence, player_id, "stats.intelligence")?,
            luck: u32_value(parent.luck, player_id, "stats.luck")?,
            experience_required: u64_value(
                parent.experience_required,
                player_id,
                "stats.experience_required",
            )?,
        },
        inventory,
        key_bindings: load_key_bindings(connection, player_id)?,
        skill_points: u32_value(parent.skill_points, player_id, "skill_points")?,
        learned_skills: load_learned_skills(connection, player_id)?,
        mesos: u64_value(parent.mesos, player_id, "mesos")?,
        cash_points: u64_value(parent.cash_points, player_id, "cash_points")?,
        quests: load_quests(connection, player_id)?,
        quest_records: load_quest_records(connection, player_id)?,
        monster_book_cards: load_monster_book_cards(connection, player_id)?,
    };
    Ok(Some(DurablePlayer {
        id: player_id.to_owned(),
        revision: u64_value(parent.revision, player_id, "revision")?,
        data,
    }))
}

fn load_inventory_stacks(
    connection: &Connection,
    player_id: &str,
) -> Result<Vec<InventoryItemStack>, DatabaseError> {
    let mut statement = connection.prepare(
        "SELECT slot_index, item_id, quantity, expires_at_unix_ms
         FROM inventory_stacks WHERE player_id = ?1 ORDER BY slot_index",
    )?;
    let rows = statement.query_map([player_id], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, i64>(3)?,
        ))
    })?;
    rows.enumerate()
        .map(|(expected_index, row)| {
            let (slot_index, item_id, quantity, expires_at_unix_ms) = row?;
            require_collection_index(
                slot_index,
                expected_index,
                player_id,
                "inventory.stacks.slot_index",
            )?;
            Ok(InventoryItemStack {
                item_id: u32_value(item_id, player_id, "inventory.stacks.item_id")?,
                quantity: u32_value(quantity, player_id, "inventory.stacks.quantity")?,
                expires_at_unix_ms: u64_value(
                    expires_at_unix_ms,
                    player_id,
                    "inventory.stacks.expires_at_unix_ms",
                )?,
            })
        })
        .collect()
}

fn load_equipped_items(
    connection: &Connection,
    player_id: &str,
) -> Result<Vec<EquippedItem>, DatabaseError> {
    let mut statement = connection.prepare(
        "SELECT equipment_slot, item_id, expires_at_unix_ms
         FROM equipped_items WHERE player_id = ?1 ORDER BY equipment_slot",
    )?;
    let rows = statement.query_map([player_id], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, i64>(2)?,
        ))
    })?;
    rows.map(|row| {
        let (slot, item_id, expires_at_unix_ms) = row?;
        Ok(EquippedItem {
            slot: i32_value(slot, player_id, "inventory.equipment.slot")?,
            item_id: u32_value(item_id, player_id, "inventory.equipment.item_id")?,
            expires_at_unix_ms: u64_value(
                expires_at_unix_ms,
                player_id,
                "inventory.equipment.expires_at_unix_ms",
            )?,
        })
    })
    .collect()
}

fn load_key_bindings(
    connection: &Connection,
    player_id: &str,
) -> Result<Vec<KeyBinding>, DatabaseError> {
    let mut statement = connection.prepare(
        "SELECT binding_order, code, action, skill_id
         FROM key_bindings WHERE player_id = ?1 ORDER BY binding_order",
    )?;
    let rows = statement.query_map([player_id], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, i64>(3)?,
        ))
    })?;
    rows.enumerate()
        .map(|(expected_order, row)| {
            let (binding_order, code, action, skill_id) = row?;
            require_collection_index(
                binding_order,
                expected_order,
                player_id,
                "key_bindings.binding_order",
            )?;
            Ok(KeyBinding {
                code,
                action: i32_value(action, player_id, "key_bindings.action")?,
                skill_id: u32_value(skill_id, player_id, "key_bindings.skill_id")?,
            })
        })
        .collect()
}

fn load_learned_skills(
    connection: &Connection,
    player_id: &str,
) -> Result<Vec<LearnedSkill>, DatabaseError> {
    let mut statement = connection.prepare(
        "SELECT skill_id, level, master_level
         FROM learned_skills WHERE player_id = ?1 ORDER BY skill_id",
    )?;
    let rows = statement.query_map([player_id], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, i64>(2)?,
        ))
    })?;
    rows.map(|row| {
        let (skill_id, level, master_level) = row?;
        Ok(LearnedSkill {
            skill_id: u32_value(skill_id, player_id, "learned_skills.skill_id")?,
            level: u32_value(level, player_id, "learned_skills.level")?,
            master_level: u32_value(master_level, player_id, "learned_skills.master_level")?,
        })
    })
    .collect()
}

fn load_quests(
    connection: &Connection,
    player_id: &str,
) -> Result<Vec<PlayerQuest>, DatabaseError> {
    let mut statement = connection.prepare(
        "SELECT quest_id, status, accepted_at_unix_ms, completed_at_unix_ms,
                dialogue_step, completion_quiz_passed
         FROM player_quests WHERE player_id = ?1 ORDER BY quest_id",
    )?;
    let rows = statement.query_map([player_id], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, i64>(4)?,
            row.get::<_, i64>(5)?,
        ))
    })?;
    let mut quests = Vec::new();
    for row in rows {
        let (quest_id, status, accepted, completed, dialogue_step, quiz_passed) = row?;
        let quest_id = u32_value(quest_id, player_id, "quests.quest_id")?;
        quests.push(PlayerQuest {
            quest_id,
            status: i32_value(status, player_id, "quests.status")?,
            mob_progress: load_quest_mob_progress(connection, player_id, quest_id)?,
            accepted_at_unix_ms: u64_value(accepted, player_id, "quests.accepted_at_unix_ms")?,
            completed_at_unix_ms: u64_value(completed, player_id, "quests.completed_at_unix_ms")?,
            dialogue_step: u32_value(dialogue_step, player_id, "quests.dialogue_step")?,
            completion_quiz_passed: bool_value(
                quiz_passed,
                player_id,
                "quests.completion_quiz_passed",
            )?,
        });
    }
    Ok(quests)
}

fn load_quest_mob_progress(
    connection: &Connection,
    player_id: &str,
    quest_id: u32,
) -> Result<Vec<QuestMobProgress>, DatabaseError> {
    let mut statement = connection.prepare(
        "SELECT mob_id, count FROM quest_mob_progress
         WHERE player_id = ?1 AND quest_id = ?2 ORDER BY mob_id",
    )?;
    let rows = statement.query_map(params![player_id, quest_id], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
    })?;
    rows.map(|row| {
        let (mob_id, count) = row?;
        Ok(QuestMobProgress {
            mob_id: u32_value(mob_id, player_id, "quests.mob_progress.mob_id")?,
            count: u32_value(count, player_id, "quests.mob_progress.count")?,
        })
    })
    .collect()
}

fn load_quest_records(
    connection: &Connection,
    player_id: &str,
) -> Result<Vec<QuestRecord>, DatabaseError> {
    let mut statement = connection
        .prepare("SELECT quest_id FROM quest_records WHERE player_id = ?1 ORDER BY quest_id")?;
    let rows = statement.query_map([player_id], |row| row.get::<_, i64>(0))?;
    let mut records = Vec::new();
    for row in rows {
        let quest_id = u32_value(row?, player_id, "quest_records.quest_id")?;
        records.push(QuestRecord {
            quest_id,
            entries: load_quest_record_entries(connection, player_id, quest_id)?,
        });
    }
    Ok(records)
}

fn load_quest_record_entries(
    connection: &Connection,
    player_id: &str,
    quest_id: u32,
) -> Result<Vec<QuestRecordEntry>, DatabaseError> {
    let mut statement = connection.prepare(
        "SELECT entry_index, value FROM quest_record_entries
         WHERE player_id = ?1 AND quest_id = ?2 ORDER BY entry_index",
    )?;
    let rows = statement.query_map(params![player_id, quest_id], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })?;
    rows.map(|row| {
        let (index, value) = row?;
        Ok(QuestRecordEntry {
            index: u32_value(index, player_id, "quest_records.entries.index")?,
            value,
        })
    })
    .collect()
}

fn load_monster_book_cards(
    connection: &Connection,
    player_id: &str,
) -> Result<Vec<MonsterBookCard>, DatabaseError> {
    let mut statement = connection.prepare(
        "SELECT card_item_id, count FROM monster_book_cards
         WHERE player_id = ?1 ORDER BY card_item_id",
    )?;
    let rows = statement.query_map([player_id], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
    })?;
    rows.map(|row| {
        let (card_item_id, count) = row?;
        Ok(MonsterBookCard {
            card_item_id: u32_value(card_item_id, player_id, "monster_book_cards.card_item_id")?,
            count: u32_value(count, player_id, "monster_book_cards.count")?,
        })
    })
    .collect()
}

fn u32_value(
    value: i64,
    player_id: &str,
    field: &'static str,
) -> Result<u32, DatabaseError> {
    u32::try_from(value).map_err(|_| corrupt_range(player_id, field))
}

fn i32_value(
    value: i64,
    player_id: &str,
    field: &'static str,
) -> Result<i32, DatabaseError> {
    i32::try_from(value).map_err(|_| corrupt_range(player_id, field))
}

fn u64_value(
    value: i64,
    player_id: &str,
    field: &'static str,
) -> Result<u64, DatabaseError> {
    u64::try_from(value).map_err(|_| corrupt_range(player_id, field))
}

fn bool_value(
    value: i64,
    player_id: &str,
    field: &'static str,
) -> Result<bool, DatabaseError> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(corrupt_range(player_id, field)),
    }
}

fn corrupt_range(
    player_id: &str,
    field: &'static str,
) -> DatabaseError {
    DatabaseError::Corrupt {
        player_id: player_id.to_owned(),
        message: format!("{field} is outside the supported range"),
    }
}

fn require_collection_index(
    value: i64,
    expected: usize,
    player_id: &str,
    field: &'static str,
) -> Result<(), DatabaseError> {
    if usize::try_from(value) == Ok(expected) {
        Ok(())
    } else {
        Err(DatabaseError::Corrupt {
            player_id: player_id.to_owned(),
            message: format!("{field} is not contiguous"),
        })
    }
}
