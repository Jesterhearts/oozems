use std::collections::BTreeSet;

use oozems_proto::v1::CharacterAppearance;
use oozems_proto::v1::CharacterGender;
use oozems_proto::v1::CharacterStats;
use oozems_proto::v1::EquipmentSlot;
use oozems_proto::v1::InventoryState;
use oozems_proto::v1::KeyBinding;
use oozems_proto::v1::LearnedSkill;
use oozems_proto::v1::MonsterBookCard;
use oozems_proto::v1::PlayerQuest;
use oozems_proto::v1::PlayerState;
use oozems_proto::v1::QuestRecord;
use oozems_proto::v1::QuestStatus;
use oozems_proto::v1::Vec2;

use super::CharacterName;
use super::DatabaseError;
use super::PlayerId;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct DurablePlayer {
    pub id: String,
    pub revision: u64,
    pub data: DurablePlayerData,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct DurablePlayerData {
    pub name: String,
    pub level: u32,
    pub map_id: u32,
    pub appearance: CharacterAppearance,
    pub stats: CharacterStats,
    pub inventory: InventoryState,
    pub key_bindings: Vec<KeyBinding>,
    pub skill_points: u32,
    pub learned_skills: Vec<LearnedSkill>,
    pub mesos: u64,
    pub cash_points: u64,
    pub quests: Vec<PlayerQuest>,
    pub quest_records: Vec<QuestRecord>,
    pub monster_book_cards: Vec<MonsterBookCard>,
}

#[derive(Clone, Copy)]
enum ValidationContext<'a> {
    Invalid,
    Corrupt(&'a str),
}

impl ValidationContext<'_> {
    fn error(
        self,
        message: impl Into<String>,
    ) -> DatabaseError {
        match self {
            Self::Invalid => DatabaseError::Invalid {
                message: message.into(),
            },
            Self::Corrupt(player_id) => DatabaseError::Corrupt {
                player_id: player_id.to_owned(),
                message: message.into(),
            },
        }
    }
}

pub(super) fn durable_from_player(
    player: &PlayerState,
    revision: u64,
) -> Result<DurablePlayer, DatabaseError> {
    PlayerId::parse(&player.id)
        .map_err(|error| ValidationContext::Invalid.error(error.to_string()))?;
    let appearance = player
        .appearance
        .ok_or_else(|| ValidationContext::Invalid.error("appearance is required"))?;
    let stats = player
        .stats
        .ok_or_else(|| ValidationContext::Invalid.error("stats are required"))?;
    let inventory = player
        .inventory
        .clone()
        .ok_or_else(|| ValidationContext::Invalid.error("inventory is required"))?;
    let mut durable = DurablePlayer {
        id: player.id.clone(),
        revision,
        data: DurablePlayerData {
            name: player.name.clone(),
            level: player.level,
            map_id: player.map_id,
            appearance,
            stats,
            inventory,
            key_bindings: player.key_bindings.clone(),
            skill_points: player.skill_points,
            learned_skills: player.learned_skills.clone(),
            mesos: player.mesos,
            cash_points: player.cash_points,
            quests: player.quests.clone(),
            quest_records: player.quest_records.clone(),
            monster_book_cards: player.monster_book_cards.clone(),
        },
    };
    canonicalize_and_validate(&mut durable, ValidationContext::Invalid)?;
    Ok(durable)
}

pub(super) fn player_from_durable(
    mut durable: DurablePlayer,
    position: Option<Vec2>,
) -> Result<PlayerState, DatabaseError> {
    let player_id = durable.id.clone();
    canonicalize_and_validate(&mut durable, ValidationContext::Corrupt(&player_id))?;
    let data = durable.data;
    Ok(PlayerState {
        id: durable.id,
        name: data.name,
        level: data.level,
        map_id: data.map_id,
        position,
        appearance: Some(data.appearance),
        stats: Some(data.stats),
        inventory: Some(data.inventory),
        key_bindings: data.key_bindings,
        skill_points: data.skill_points,
        learned_skills: data.learned_skills,
        mesos: data.mesos,
        quests: data.quests,
        revision: durable.revision,
        quest_records: data.quest_records,
        monster_book_cards: data.monster_book_cards,
        cash_points: data.cash_points,
    })
}

fn canonicalize_and_validate(
    player: &mut DurablePlayer,
    context: ValidationContext<'_>,
) -> Result<(), DatabaseError> {
    PlayerId::parse(&player.id).map_err(|error| context.error(error.to_string()))?;
    CharacterName::parse(&player.data.name).map_err(|error| context.error(error.to_string()))?;
    if player.revision == 0 {
        return Err(context.error("revision must be positive"));
    }
    require_signed(player.revision, "revision", context)?;
    if player.data.level == 0 {
        return Err(context.error("level must be positive"));
    }

    validate_appearance(&player.data.appearance, context)?;
    validate_stats(&player.data.stats, context)?;
    canonicalize_inventory(&mut player.data.inventory, context)?;
    crate::keymap::validate_bindings(&player.data.key_bindings)
        .map_err(|error| context.error(error.to_string()))?;
    canonicalize_learned_skills(&mut player.data.learned_skills, context)?;
    canonicalize_quests(&mut player.data.quests, context)?;
    player.data.quest_records =
        crate::quest_records::canonicalize(std::mem::take(&mut player.data.quest_records))
            .map_err(|error| context.error(error.to_string()))?;
    player.data.monster_book_cards =
        crate::monster_book::canonicalize(std::mem::take(&mut player.data.monster_book_cards))
            .map_err(|error| context.error(error.to_string()))?;

    require_signed(player.data.stats.experience, "stats.experience", context)?;
    require_signed(
        player.data.stats.experience_required,
        "stats.experience_required",
        context,
    )?;
    require_signed(player.data.mesos, "mesos", context)?;
    require_signed(player.data.cash_points, "cash_points", context)?;
    for quest in &player.data.quests {
        require_signed(
            quest.accepted_at_unix_ms,
            "quests.accepted_at_unix_ms",
            context,
        )?;
        require_signed(
            quest.completed_at_unix_ms,
            "quests.completed_at_unix_ms",
            context,
        )?;
    }
    Ok(())
}

fn validate_appearance(
    appearance: &CharacterAppearance,
    context: ValidationContext<'_>,
) -> Result<(), DatabaseError> {
    let gender = CharacterGender::try_from(appearance.gender)
        .map_err(|_| context.error("appearance gender is invalid"))?;
    if gender == CharacterGender::Unspecified {
        return Err(context.error("appearance gender is unspecified"));
    }
    Ok(())
}

fn validate_stats(
    stats: &CharacterStats,
    context: ValidationContext<'_>,
) -> Result<(), DatabaseError> {
    if stats.max_hp == 0 || stats.hp > stats.max_hp {
        return Err(context.error("HP values are invalid"));
    }
    if stats.max_mp == 0 || stats.mp > stats.max_mp {
        return Err(context.error("MP values are invalid"));
    }
    if stats.experience_required == 0 {
        return Err(context.error("experience_required must be positive"));
    }
    Ok(())
}

fn canonicalize_inventory(
    inventory: &mut InventoryState,
    context: ValidationContext<'_>,
) -> Result<(), DatabaseError> {
    if inventory.capacity == 0 || inventory.stacks.len() > inventory.capacity as usize {
        return Err(context.error("inventory capacity is invalid"));
    }
    for stack in &inventory.stacks {
        if stack.item_id == 0 || stack.quantity == 0 {
            return Err(context.error("inventory stacks require positive item IDs and quantities"));
        }
        require_signed(
            stack.expires_at_unix_ms,
            "inventory.stacks.expires_at_unix_ms",
            context,
        )?;
    }

    let mut slots = BTreeSet::new();
    for equipped in &inventory.equipment {
        let slot = EquipmentSlot::try_from(equipped.slot)
            .map_err(|_| context.error("equipment slot is invalid"))?;
        if slot == EquipmentSlot::Unspecified || equipped.item_id == 0 {
            return Err(context.error("equipped item is invalid"));
        }
        if !slots.insert(equipped.slot) {
            return Err(context.error("equipment slot appears more than once"));
        }
        require_signed(
            equipped.expires_at_unix_ms,
            "inventory.equipment.expires_at_unix_ms",
            context,
        )?;
    }
    inventory.equipment.sort_unstable_by_key(|item| item.slot);
    Ok(())
}

fn canonicalize_learned_skills(
    skills: &mut [LearnedSkill],
    context: ValidationContext<'_>,
) -> Result<(), DatabaseError> {
    skills.sort_unstable_by_key(|skill| skill.skill_id);
    let mut previous = None;
    for skill in skills {
        if skill.skill_id == 0 || (skill.level == 0 && skill.master_level == 0) {
            return Err(context.error("learned skill is invalid"));
        }
        if previous == Some(skill.skill_id) {
            return Err(context.error(format!(
                "learned skill {} appears more than once",
                skill.skill_id
            )));
        }
        previous = Some(skill.skill_id);
    }
    Ok(())
}

fn canonicalize_quests(
    quests: &mut [PlayerQuest],
    context: ValidationContext<'_>,
) -> Result<(), DatabaseError> {
    for quest in quests.iter_mut() {
        if quest.quest_id == 0 {
            return Err(context.error("quest ID must be positive"));
        }
        let status = QuestStatus::try_from(quest.status)
            .map_err(|_| context.error("quest status is invalid"))?;
        if status == QuestStatus::Unspecified && quest.dialogue_step == 0 {
            return Err(context.error("an unspecified quest requires pending dialogue"));
        }
        quest
            .mob_progress
            .sort_unstable_by_key(|progress| progress.mob_id);
        let mut previous_mob = None;
        for progress in &quest.mob_progress {
            if progress.mob_id == 0 {
                return Err(context.error("quest mob ID must be positive"));
            }
            if previous_mob == Some(progress.mob_id) {
                return Err(context.error(format!(
                    "quest mob {} appears more than once",
                    progress.mob_id
                )));
            }
            previous_mob = Some(progress.mob_id);
        }
    }
    quests.sort_unstable_by_key(|quest| quest.quest_id);
    if let Some(pair) = quests
        .windows(2)
        .find(|pair| pair[0].quest_id == pair[1].quest_id)
    {
        return Err(context.error(format!("quest {} appears more than once", pair[0].quest_id)));
    }
    Ok(())
}

fn require_signed(
    value: u64,
    field: &'static str,
    context: ValidationContext<'_>,
) -> Result<(), DatabaseError> {
    if i64::try_from(value).is_ok() {
        return Ok(());
    }
    match context {
        ValidationContext::Invalid => Err(DatabaseError::Overflow { field }),
        ValidationContext::Corrupt(_) => {
            Err(context.error(format!("{field} is outside the supported range")))
        }
    }
}
