use std::collections::BTreeMap;

use oozems_proto::v1::EquippedItem;
use oozems_proto::v1::InventoryItemStack;
use oozems_proto::v1::KeyBinding;
use oozems_proto::v1::LearnedSkill;
use oozems_proto::v1::MonsterBookCard;
use oozems_proto::v1::PlayerQuest;
use oozems_proto::v1::QuestMobProgress;
use oozems_proto::v1::QuestRecord;
use oozems_proto::v1::QuestRecordEntry;
use rusqlite::Connection;
use rusqlite::OptionalExtension;
use rusqlite::Transaction;
use rusqlite::TransactionBehavior;
use rusqlite::params;

use super::DatabaseError;
use super::player_record::DurablePlayer;
use super::player_record::DurablePlayerData;

pub(super) fn create_player(
    connection: &mut Connection,
    player: &DurablePlayer,
) -> Result<(), DatabaseError> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    match insert_parent(&transaction, player) {
        Ok(_) => {}
        Err(DatabaseError::Storage(error)) if is_primary_key_constraint(&error) => {
            return Err(DatabaseError::Exists {
                player_id: player.id.clone(),
            });
        }
        Err(error) => return Err(error),
    }
    insert_all_children(&transaction, player)?;
    transaction.commit()?;
    Ok(())
}

pub(super) fn save_player(
    connection: &mut Connection,
    original: &DurablePlayer,
    staged: &DurablePlayer,
) -> Result<(), DatabaseError> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let changed = if parent_data_equal(&original.data, &staged.data) {
        transaction.execute(
            "UPDATE players SET revision = ?1 WHERE player_id = ?2 AND revision = ?3",
            params![
                signed(staged.revision, "revision")?,
                staged.id,
                signed(original.revision, "revision")?
            ],
        )?
    } else {
        update_parent(&transaction, original.revision, staged)?
    };
    if changed == 0 {
        return Err(revision_error(&transaction, original));
    }

    if original.data != staged.data {
        diff_inventory_stacks(
            &transaction,
            &staged.id,
            &original.data.inventory.stacks,
            &staged.data.inventory.stacks,
        )?;
        diff_equipped_items(
            &transaction,
            &staged.id,
            &original.data.inventory.equipment,
            &staged.data.inventory.equipment,
        )?;
        diff_learned_skills(
            &transaction,
            &staged.id,
            &original.data.learned_skills,
            &staged.data.learned_skills,
        )?;
        diff_key_bindings(
            &transaction,
            &staged.id,
            &original.data.key_bindings,
            &staged.data.key_bindings,
        )?;
        diff_quests(
            &transaction,
            &staged.id,
            &original.data.quests,
            &staged.data.quests,
        )?;
        diff_quest_records(
            &transaction,
            &staged.id,
            &original.data.quest_records,
            &staged.data.quest_records,
        )?;
        diff_monster_book_cards(
            &transaction,
            &staged.id,
            &original.data.monster_book_cards,
            &staged.data.monster_book_cards,
        )?;
    }

    transaction.commit()?;
    Ok(())
}

fn insert_parent(
    transaction: &Transaction<'_>,
    player: &DurablePlayer,
) -> Result<usize, DatabaseError> {
    let data = &player.data;
    let appearance = &data.appearance;
    let stats = &data.stats;
    Ok(transaction.execute(
        "INSERT INTO players (
             player_id, revision, name, level, map_id,
             appearance_gender, skin_id, face_id, hair_id,
             job_id, hp, max_hp, mp, max_mp, experience,
             experience_required, fame, ability_points, strength,
             dexterity, intelligence, luck, inventory_capacity,
             skill_points, mesos, cash_points
         ) VALUES (
             ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
             ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24,
             ?25, ?26
         )",
        params![
            player.id,
            signed(player.revision, "revision")?,
            data.name,
            data.level,
            data.map_id,
            appearance.gender,
            appearance.skin_id,
            appearance.face_id,
            appearance.hair_id,
            stats.job_id,
            stats.hp,
            stats.max_hp,
            stats.mp,
            stats.max_mp,
            signed(stats.experience, "stats.experience")?,
            signed(stats.experience_required, "stats.experience_required")?,
            stats.fame,
            stats.ability_points,
            stats.strength,
            stats.dexterity,
            stats.intelligence,
            stats.luck,
            data.inventory.capacity,
            data.skill_points,
            signed(data.mesos, "mesos")?,
            signed(data.cash_points, "cash_points")?,
        ],
    )?)
}

fn is_primary_key_constraint(error: &rusqlite::Error) -> bool {
    matches!(
        error,
        rusqlite::Error::SqliteFailure(code, _)
            if code.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_PRIMARYKEY
    )
}

fn update_parent(
    transaction: &Transaction<'_>,
    expected_revision: u64,
    player: &DurablePlayer,
) -> Result<usize, DatabaseError> {
    let data = &player.data;
    let appearance = &data.appearance;
    let stats = &data.stats;
    Ok(transaction.execute(
        "UPDATE players SET
             revision = ?1, name = ?2, level = ?3, map_id = ?4,
             appearance_gender = ?5, skin_id = ?6, face_id = ?7,
             hair_id = ?8, job_id = ?9, hp = ?10, max_hp = ?11,
             mp = ?12, max_mp = ?13, experience = ?14,
             experience_required = ?15, fame = ?16, ability_points = ?17,
             strength = ?18, dexterity = ?19, intelligence = ?20,
             luck = ?21, inventory_capacity = ?22, skill_points = ?23,
             mesos = ?24, cash_points = ?25
         WHERE player_id = ?26 AND revision = ?27",
        params![
            signed(player.revision, "revision")?,
            data.name,
            data.level,
            data.map_id,
            appearance.gender,
            appearance.skin_id,
            appearance.face_id,
            appearance.hair_id,
            stats.job_id,
            stats.hp,
            stats.max_hp,
            stats.mp,
            stats.max_mp,
            signed(stats.experience, "stats.experience")?,
            signed(stats.experience_required, "stats.experience_required")?,
            stats.fame,
            stats.ability_points,
            stats.strength,
            stats.dexterity,
            stats.intelligence,
            stats.luck,
            data.inventory.capacity,
            data.skill_points,
            signed(data.mesos, "mesos")?,
            signed(data.cash_points, "cash_points")?,
            player.id,
            signed(expected_revision, "revision")?,
        ],
    )?)
}

fn insert_all_children(
    transaction: &Transaction<'_>,
    player: &DurablePlayer,
) -> Result<(), DatabaseError> {
    diff_inventory_stacks(transaction, &player.id, &[], &player.data.inventory.stacks)?;
    diff_equipped_items(
        transaction,
        &player.id,
        &[],
        &player.data.inventory.equipment,
    )?;
    diff_learned_skills(transaction, &player.id, &[], &player.data.learned_skills)?;
    diff_key_bindings(transaction, &player.id, &[], &player.data.key_bindings)?;
    diff_quests(transaction, &player.id, &[], &player.data.quests)?;
    diff_quest_records(transaction, &player.id, &[], &player.data.quest_records)?;
    diff_monster_book_cards(
        transaction,
        &player.id,
        &[],
        &player.data.monster_book_cards,
    )?;
    Ok(())
}

fn diff_inventory_stacks(
    transaction: &Transaction<'_>,
    player_id: &str,
    original: &[InventoryItemStack],
    staged: &[InventoryItemStack],
) -> Result<(), DatabaseError> {
    for (slot_index, stack) in staged.iter().enumerate() {
        if original.get(slot_index) == Some(stack) {
            continue;
        }
        transaction.execute(
            "INSERT INTO inventory_stacks (
                 player_id, slot_index, item_id, quantity, expires_at_unix_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (player_id, slot_index) DO UPDATE SET
                 item_id = excluded.item_id,
                 quantity = excluded.quantity,
                 expires_at_unix_ms = excluded.expires_at_unix_ms",
            params![
                player_id,
                index(slot_index)?,
                stack.item_id,
                stack.quantity,
                signed(
                    stack.expires_at_unix_ms,
                    "inventory.stacks.expires_at_unix_ms"
                )?,
            ],
        )?;
    }
    if staged.len() < original.len() {
        transaction.execute(
            "DELETE FROM inventory_stacks WHERE player_id = ?1 AND slot_index >= ?2",
            params![player_id, index(staged.len())?],
        )?;
    }
    Ok(())
}

fn diff_equipped_items(
    transaction: &Transaction<'_>,
    player_id: &str,
    original: &[EquippedItem],
    staged: &[EquippedItem],
) -> Result<(), DatabaseError> {
    let original = original
        .iter()
        .map(|item| (item.slot, item))
        .collect::<BTreeMap<_, _>>();
    let staged = staged
        .iter()
        .map(|item| (item.slot, item))
        .collect::<BTreeMap<_, _>>();
    for slot in original.keys().filter(|slot| !staged.contains_key(slot)) {
        transaction.execute(
            "DELETE FROM equipped_items WHERE player_id = ?1 AND equipment_slot = ?2",
            params![player_id, slot],
        )?;
    }
    for (slot, item) in staged {
        if original.get(&slot).copied() == Some(item) {
            continue;
        }
        transaction.execute(
            "INSERT INTO equipped_items (
                 player_id, equipment_slot, item_id, expires_at_unix_ms
             ) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (player_id, equipment_slot) DO UPDATE SET
                 item_id = excluded.item_id,
                 expires_at_unix_ms = excluded.expires_at_unix_ms",
            params![
                player_id,
                slot,
                item.item_id,
                signed(
                    item.expires_at_unix_ms,
                    "inventory.equipment.expires_at_unix_ms"
                )?,
            ],
        )?;
    }
    Ok(())
}

fn diff_learned_skills(
    transaction: &Transaction<'_>,
    player_id: &str,
    original: &[LearnedSkill],
    staged: &[LearnedSkill],
) -> Result<(), DatabaseError> {
    let original = original
        .iter()
        .map(|skill| (skill.skill_id, skill))
        .collect::<BTreeMap<_, _>>();
    let staged = staged
        .iter()
        .map(|skill| (skill.skill_id, skill))
        .collect::<BTreeMap<_, _>>();
    for skill_id in original
        .keys()
        .filter(|skill_id| !staged.contains_key(skill_id))
    {
        transaction.execute(
            "DELETE FROM learned_skills WHERE player_id = ?1 AND skill_id = ?2",
            params![player_id, skill_id],
        )?;
    }
    for (skill_id, skill) in staged {
        if original.get(&skill_id).copied() == Some(skill) {
            continue;
        }
        transaction.execute(
            "INSERT INTO learned_skills (player_id, skill_id, level, master_level)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (player_id, skill_id) DO UPDATE SET
                 level = excluded.level, master_level = excluded.master_level",
            params![player_id, skill_id, skill.level, skill.master_level],
        )?;
    }
    Ok(())
}

fn diff_key_bindings(
    transaction: &Transaction<'_>,
    player_id: &str,
    original: &[KeyBinding],
    staged: &[KeyBinding],
) -> Result<(), DatabaseError> {
    let original = original
        .iter()
        .enumerate()
        .map(|(order, binding)| (binding.code.as_str(), (order, binding)))
        .collect::<BTreeMap<_, _>>();
    let staged = staged
        .iter()
        .enumerate()
        .map(|(order, binding)| (binding.code.as_str(), (order, binding)))
        .collect::<BTreeMap<_, _>>();

    // Delete changed targets and orders first so swaps cannot transiently violate
    // the partial target indexes or the binding-order uniqueness constraint.
    for (code, original_binding) in &original {
        if staged.get(code) == Some(original_binding) {
            continue;
        }
        transaction.execute(
            "DELETE FROM key_bindings WHERE player_id = ?1 AND code = ?2",
            params![player_id, code],
        )?;
    }
    for (code, (order, binding)) in staged {
        if original.get(code) == Some(&(order, binding)) {
            continue;
        }
        transaction.execute(
            "INSERT INTO key_bindings (
                 player_id, code, binding_order, action, skill_id
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                player_id,
                code,
                index(order)?,
                binding.action,
                binding.skill_id
            ],
        )?;
    }
    Ok(())
}

fn diff_quests(
    transaction: &Transaction<'_>,
    player_id: &str,
    original: &[PlayerQuest],
    staged: &[PlayerQuest],
) -> Result<(), DatabaseError> {
    let original = original
        .iter()
        .map(|quest| (quest.quest_id, quest))
        .collect::<BTreeMap<_, _>>();
    let staged = staged
        .iter()
        .map(|quest| (quest.quest_id, quest))
        .collect::<BTreeMap<_, _>>();
    for quest_id in original
        .keys()
        .filter(|quest_id| !staged.contains_key(quest_id))
    {
        transaction.execute(
            "DELETE FROM player_quests WHERE player_id = ?1 AND quest_id = ?2",
            params![player_id, quest_id],
        )?;
    }
    for (quest_id, quest) in staged {
        let old = original.get(&quest_id).copied();
        if old.is_none() || !quest_parent_equal(old.expect("checked above"), quest) {
            transaction.execute(
                "INSERT INTO player_quests (
                     player_id, quest_id, status, accepted_at_unix_ms,
                     completed_at_unix_ms, dialogue_step, completion_quiz_passed
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT (player_id, quest_id) DO UPDATE SET
                     status = excluded.status,
                     accepted_at_unix_ms = excluded.accepted_at_unix_ms,
                     completed_at_unix_ms = excluded.completed_at_unix_ms,
                     dialogue_step = excluded.dialogue_step,
                     completion_quiz_passed = excluded.completion_quiz_passed",
                params![
                    player_id,
                    quest_id,
                    quest.status,
                    signed(quest.accepted_at_unix_ms, "quests.accepted_at_unix_ms")?,
                    signed(quest.completed_at_unix_ms, "quests.completed_at_unix_ms")?,
                    quest.dialogue_step,
                    quest.completion_quiz_passed,
                ],
            )?;
        }
        let old_progress = old.map_or(&[][..], |old| old.mob_progress.as_slice());
        diff_quest_mob_progress(
            transaction,
            player_id,
            quest_id,
            old_progress,
            &quest.mob_progress,
        )?;
    }
    Ok(())
}

fn diff_quest_mob_progress(
    transaction: &Transaction<'_>,
    player_id: &str,
    quest_id: u32,
    original: &[QuestMobProgress],
    staged: &[QuestMobProgress],
) -> Result<(), DatabaseError> {
    let original = original
        .iter()
        .map(|progress| (progress.mob_id, progress))
        .collect::<BTreeMap<_, _>>();
    let staged = staged
        .iter()
        .map(|progress| (progress.mob_id, progress))
        .collect::<BTreeMap<_, _>>();
    for mob_id in original
        .keys()
        .filter(|mob_id| !staged.contains_key(mob_id))
    {
        transaction.execute(
            "DELETE FROM quest_mob_progress
             WHERE player_id = ?1 AND quest_id = ?2 AND mob_id = ?3",
            params![player_id, quest_id, mob_id],
        )?;
    }
    for (mob_id, progress) in staged {
        if original.get(&mob_id).copied() == Some(progress) {
            continue;
        }
        transaction.execute(
            "INSERT INTO quest_mob_progress (player_id, quest_id, mob_id, count)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (player_id, quest_id, mob_id) DO UPDATE SET
                 count = excluded.count",
            params![player_id, quest_id, mob_id, progress.count],
        )?;
    }
    Ok(())
}

fn diff_quest_records(
    transaction: &Transaction<'_>,
    player_id: &str,
    original: &[QuestRecord],
    staged: &[QuestRecord],
) -> Result<(), DatabaseError> {
    let original = original
        .iter()
        .map(|record| (record.quest_id, record))
        .collect::<BTreeMap<_, _>>();
    let staged = staged
        .iter()
        .map(|record| (record.quest_id, record))
        .collect::<BTreeMap<_, _>>();
    for quest_id in original
        .keys()
        .filter(|quest_id| !staged.contains_key(quest_id))
    {
        transaction.execute(
            "DELETE FROM quest_records WHERE player_id = ?1 AND quest_id = ?2",
            params![player_id, quest_id],
        )?;
    }
    for (quest_id, record) in staged {
        let old = original.get(&quest_id).copied();
        if old.is_none() {
            transaction.execute(
                "INSERT INTO quest_records (player_id, quest_id) VALUES (?1, ?2)",
                params![player_id, quest_id],
            )?;
        }
        let old_entries = old.map_or(&[][..], |old| old.entries.as_slice());
        diff_quest_record_entries(
            transaction,
            player_id,
            quest_id,
            old_entries,
            &record.entries,
        )?;
    }
    Ok(())
}

fn diff_quest_record_entries(
    transaction: &Transaction<'_>,
    player_id: &str,
    quest_id: u32,
    original: &[QuestRecordEntry],
    staged: &[QuestRecordEntry],
) -> Result<(), DatabaseError> {
    let original = original
        .iter()
        .map(|entry| (entry.index, entry))
        .collect::<BTreeMap<_, _>>();
    let staged = staged
        .iter()
        .map(|entry| (entry.index, entry))
        .collect::<BTreeMap<_, _>>();
    for entry_index in original
        .keys()
        .filter(|entry_index| !staged.contains_key(entry_index))
    {
        transaction.execute(
            "DELETE FROM quest_record_entries
             WHERE player_id = ?1 AND quest_id = ?2 AND entry_index = ?3",
            params![player_id, quest_id, entry_index],
        )?;
    }
    for (entry_index, entry) in staged {
        if original.get(&entry_index).copied() == Some(entry) {
            continue;
        }
        transaction.execute(
            "INSERT INTO quest_record_entries (player_id, quest_id, entry_index, value)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (player_id, quest_id, entry_index) DO UPDATE SET
                 value = excluded.value",
            params![player_id, quest_id, entry_index, entry.value],
        )?;
    }
    Ok(())
}

fn diff_monster_book_cards(
    transaction: &Transaction<'_>,
    player_id: &str,
    original: &[MonsterBookCard],
    staged: &[MonsterBookCard],
) -> Result<(), DatabaseError> {
    let original = original
        .iter()
        .map(|card| (card.card_item_id, card))
        .collect::<BTreeMap<_, _>>();
    let staged = staged
        .iter()
        .map(|card| (card.card_item_id, card))
        .collect::<BTreeMap<_, _>>();
    for card_item_id in original
        .keys()
        .filter(|card_item_id| !staged.contains_key(card_item_id))
    {
        transaction.execute(
            "DELETE FROM monster_book_cards WHERE player_id = ?1 AND card_item_id = ?2",
            params![player_id, card_item_id],
        )?;
    }
    for (card_item_id, card) in staged {
        if original.get(&card_item_id).copied() == Some(card) {
            continue;
        }
        transaction.execute(
            "INSERT INTO monster_book_cards (player_id, card_item_id, count)
             VALUES (?1, ?2, ?3)
             ON CONFLICT (player_id, card_item_id) DO UPDATE SET count = excluded.count",
            params![player_id, card_item_id, card.count],
        )?;
    }
    Ok(())
}

fn parent_data_equal(
    left: &DurablePlayerData,
    right: &DurablePlayerData,
) -> bool {
    left.name == right.name
        && left.level == right.level
        && left.map_id == right.map_id
        && left.appearance == right.appearance
        && left.stats == right.stats
        && left.inventory.capacity == right.inventory.capacity
        && left.skill_points == right.skill_points
        && left.mesos == right.mesos
        && left.cash_points == right.cash_points
}

fn quest_parent_equal(
    left: &PlayerQuest,
    right: &PlayerQuest,
) -> bool {
    left.quest_id == right.quest_id
        && left.status == right.status
        && left.accepted_at_unix_ms == right.accepted_at_unix_ms
        && left.completed_at_unix_ms == right.completed_at_unix_ms
        && left.dialogue_step == right.dialogue_step
        && left.completion_quiz_passed == right.completion_quiz_passed
}

fn revision_error(
    transaction: &Transaction<'_>,
    original: &DurablePlayer,
) -> DatabaseError {
    let actual = transaction
        .query_row(
            "SELECT revision FROM players WHERE player_id = ?1",
            [&original.id],
            |row| row.get::<_, i64>(0),
        )
        .optional();
    match actual {
        Ok(None) => DatabaseError::NotFound {
            player_id: original.id.clone(),
        },
        Ok(Some(actual)) => DatabaseError::RevisionConflict {
            player_id: original.id.clone(),
            expected: original.revision,
            actual: u64::try_from(actual).unwrap_or_default(),
        },
        Err(error) => DatabaseError::Storage(error),
    }
}

fn signed(
    value: u64,
    field: &'static str,
) -> Result<i64, DatabaseError> {
    i64::try_from(value).map_err(|_| DatabaseError::Overflow { field })
}

fn index(value: usize) -> Result<i64, DatabaseError> {
    i64::try_from(value).map_err(|_| DatabaseError::Overflow {
        field: "collection index",
    })
}
