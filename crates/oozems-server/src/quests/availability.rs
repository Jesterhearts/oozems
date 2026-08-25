use super::*;

#[cfg(test)]
mod tests;

pub fn progress(
    player: &PlayerState,
    quest_id: u32,
) -> QuestProgress {
    player
        .quests
        .iter()
        .find(|quest| quest.quest_id == quest_id)
        .and_then(|quest| QuestStatus::try_from(quest.status).ok())
        .map_or(QuestProgress::NotStarted, |status| match status {
            QuestStatus::Started => QuestProgress::Started,
            QuestStatus::Completed => QuestProgress::Completed,
            QuestStatus::Unspecified => QuestProgress::NotStarted,
        })
}

pub(crate) fn quest_expiration_deadline(
    entry: &PlayerQuest,
    quest: &QuestDefinition,
) -> Option<u64> {
    // HeavenMS applies timeLimit2 after timeLimit, so it takes precedence when both
    // exist.
    let duration_ms = quest.info.time_limit2_ms.or(quest.info.time_limit_ms)?;
    Some(entry.accepted_at_unix_ms.saturating_add(duration_ms))
}

pub(crate) fn quest_is_expired(
    entry: &PlayerQuest,
    quest: &QuestDefinition,
    now_unix_ms: u64,
) -> bool {
    quest_expiration_deadline(entry, quest).is_some_and(|deadline| now_unix_ms >= deadline)
}

pub fn is_available(
    player: &PlayerState,
    effects: &PlayerEffects,
    quest: &QuestDefinition,
    item_definitions: &[ItemDefinition],
    scripts: &QuestScriptCatalog,
    environment: QuestEnvironment,
) -> bool {
    if !start_status_date_and_script_allow(
        player,
        effects,
        quest,
        item_definitions,
        scripts,
        environment,
    ) || player.level < quest.start.minimum_level.unwrap_or(1)
        || quest
            .start
            .maximum_level
            .is_some_and(|maximum| player.level > maximum)
    {
        return false;
    }
    let stats = player.stats.as_ref();
    if quest
        .start
        .minimum_fame
        .is_some_and(|minimum| stats.is_none_or(|stats| stats.fame < minimum))
    {
        return false;
    }
    let job_id = stats.map_or(0, |stats| stats.job_id);
    if !quest.start.allowed_jobs.is_empty() && !quest.start.allowed_jobs.contains(&job_id) {
        return false;
    }
    if !quest.start.allowed_map_ids.is_empty()
        && !quest.start.allowed_map_ids.contains(&player.map_id)
    {
        return false;
    }
    requirements_have_items(player, &quest.start.items, item_definitions)
        && requirements_have_quest_states(player, &quest.start.quests)
        && requirements_have_skills(player, &quest.start.skills)
        && requirements_have_records(player, &quest.start.record_conditions)
        && requirements_have_effects(effects, &quest.start.effects)
        && required_morph_is_active(effects, quest.start.required_morph_id)
}

pub(crate) fn start_status_date_and_script_allow(
    player: &PlayerState,
    effects: &PlayerEffects,
    quest: &QuestDefinition,
    item_definitions: &[ItemDefinition],
    scripts: &QuestScriptCatalog,
    environment: QuestEnvironment,
) -> bool {
    script_conditions_pass(
        scripts,
        player,
        quest,
        QuestScriptPhase::Start,
        item_definitions,
    ) && repeat_policy_allows(player, quest, environment.now_unix_ms)
        && requirements_have_equipped_items(player, &quest.start.equipped_items)
        && quest
            .start
            .minimum_world_id
            .is_none_or(|minimum| environment.world_id >= minimum)
        && quest
            .start
            .maximum_world_id
            .is_none_or(|maximum| environment.world_id <= maximum)
        && quest
            .start
            .available_from
            .as_ref()
            .is_none_or(|start| environment.now_unix_ms >= start.unix_ms)
        && quest
            .start
            .available_until
            .as_ref()
            .is_none_or(|end| environment.now_unix_ms <= end.unix_ms)
        && weekday_allows(quest, environment.now_unix_ms)
        && requirements_have_effects(effects, &quest.start.effects)
        && required_morph_is_active(effects, quest.start.required_morph_id)
        && requirements_have_monster_book(player, &quest.start.monster_book)
}

pub(crate) fn completion_window_allows(
    quest: &QuestDefinition,
    now_unix_ms: u64,
) -> bool {
    quest
        .completion
        .available_from
        .as_ref()
        .is_none_or(|start| now_unix_ms >= start.unix_ms)
        && quest
            .completion
            .available_until
            .as_ref()
            .is_none_or(|end| now_unix_ms <= end.unix_ms)
}

pub(crate) fn eligible_completed_quest_count(
    player: &PlayerState,
    quest_definitions: &[&QuestDefinition],
) -> u64 {
    let mut eligible_ids = quest_definitions
        .iter()
        .filter(|quest| !(9_000..=10_999).contains(&quest.id) && quest.info.area != Some(51))
        .map(|quest| quest.id)
        .collect::<BTreeSet<_>>();
    player
        .quests
        .iter()
        .filter(|entry| QuestStatus::try_from(entry.status) == Ok(QuestStatus::Completed))
        .filter(|entry| eligible_ids.remove(&entry.quest_id))
        .count() as u64
}
pub(crate) fn repeat_policy_allows(
    player: &PlayerState,
    quest: &QuestDefinition,
    now_unix_ms: u64,
) -> bool {
    if let Some(entry) = player.quests.iter().find(|entry| {
        entry.quest_id == quest.id
            && QuestStatus::try_from(entry.status) == Ok(QuestStatus::Unspecified)
            && entry.dialogue_step > 0
            && entry.completed_at_unix_ms > 0
    }) {
        return repeat_completion_allows(quest, now_unix_ms, entry.completed_at_unix_ms);
    }
    match progress(player, quest.id) {
        QuestProgress::NotStarted => true,
        QuestProgress::Started => false,
        QuestProgress::Completed => {
            let Some(completed_at) = player
                .quests
                .iter()
                .find(|entry| entry.quest_id == quest.id)
                .map(|entry| entry.completed_at_unix_ms)
                .filter(|completed_at| *completed_at > 0)
            else {
                return false;
            };
            repeat_completion_allows(quest, now_unix_ms, completed_at)
        }
    }
}

pub(crate) fn repeat_completion_allows(
    quest: &QuestDefinition,
    now_unix_ms: u64,
    completed_at: u64,
) -> bool {
    let repeat = &quest.start.repeat;
    if repeat.interval_ms.is_none() && !repeat.day_by_day {
        return false;
    }
    repeat.interval_ms.is_none_or(|interval| {
        completed_at
            .checked_add(interval)
            .is_some_and(|available_at| now_unix_ms >= available_at)
    }) && (!repeat.day_by_day
        || utc_datetime(now_unix_ms)
            .zip(utc_datetime(completed_at))
            .is_some_and(|(now, completed)| now.date() > completed.date()))
}

pub(crate) fn weekday_allows(
    quest: &QuestDefinition,
    now_unix_ms: u64,
) -> bool {
    let days = &quest.start.repeat.days_of_week;
    days.is_empty() || weekday(now_unix_ms).is_some_and(|weekday| days.contains(&weekday))
}

pub(crate) fn weekday(now_unix_ms: u64) -> Option<QuestWeekday> {
    match utc_datetime(now_unix_ms).map(|datetime| datetime.weekday()) {
        Some(Weekday::Monday) => Some(QuestWeekday::Monday),
        Some(Weekday::Tuesday) => Some(QuestWeekday::Tuesday),
        Some(Weekday::Wednesday) => Some(QuestWeekday::Wednesday),
        Some(Weekday::Thursday) => Some(QuestWeekday::Thursday),
        Some(Weekday::Friday) => Some(QuestWeekday::Friday),
        Some(Weekday::Saturday) => Some(QuestWeekday::Saturday),
        Some(Weekday::Sunday) => Some(QuestWeekday::Sunday),
        None => None,
    }
}

pub(crate) fn utc_datetime(unix_ms: u64) -> Option<jiff::civil::DateTime> {
    let unix_ms = i64::try_from(unix_ms).ok()?;
    let timestamp = Timestamp::from_millisecond(unix_ms).ok()?;
    Some(Offset::UTC.to_datetime(timestamp))
}

pub(crate) fn requirements_have_items(
    player: &PlayerState,
    requirements: &[QuestItemRequirement],
    item_definitions: &[ItemDefinition],
) -> bool {
    requirements.iter().all(|requirement| {
        player.inventory.as_ref().is_some_and(|inventory| {
            quest_item_quantity(inventory, item_definitions, requirement.item_id).is_ok_and(
                |count| match requirement.condition {
                    QuestItemCondition::Absent => count == 0,
                    QuestItemCondition::AtLeast(required) => count >= u64::from(required.get()),
                },
            )
        })
    })
}

pub(crate) fn requirements_have_monster_book(
    player: &PlayerState,
    requirements: &QuestMonsterBookRequirements,
) -> bool {
    let cards_match = requirements.cards.iter().all(|requirement| {
        let count =
            crate::monster_book::count(&player.monster_book_cards, requirement.card_item_id);
        requirement
            .minimum_count
            .is_none_or(|minimum| count >= minimum)
            && requirement
                .maximum_count
                .is_none_or(|maximum| count <= maximum)
    });
    let unique_count = player.monster_book_cards.len() as u64;
    cards_match
        && requirements
            .minimum_unique_cards
            .is_none_or(|minimum| unique_count >= u64::from(minimum))
        && requirements
            .maximum_unique_cards
            .is_none_or(|maximum| unique_count <= u64::from(maximum))
}

pub(crate) fn requirements_have_equipped_items(
    player: &PlayerState,
    requirements: &QuestEquippedItemRequirements,
) -> bool {
    if requirements.all_of.is_empty() && requirements.any_of.is_empty() {
        return true;
    }
    let Some(inventory) = player.inventory.as_ref() else {
        return false;
    };
    let is_equipped = |item_id| {
        inventory
            .equipment
            .iter()
            .any(|equipped| equipped.item_id == item_id)
    };
    requirements.all_of.iter().copied().all(is_equipped)
        && (requirements.any_of.is_empty() || requirements.any_of.iter().copied().any(is_equipped))
}

pub(crate) fn item_name(
    item_definitions: &[ItemDefinition],
    item_id: u32,
) -> String {
    item_definitions
        .iter()
        .find(|definition| definition.item_id == item_id)
        .map(|definition| definition.name.as_str())
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| format!("Item {item_id}"))
}

pub(crate) fn quest_item_quantity(
    inventory: &oozems_proto::v1::InventoryState,
    item_definitions: &[ItemDefinition],
    item_id: u32,
) -> Result<u64, ItemRuleError> {
    let inventory_count = crate::items::count_inventory_item(inventory, item_definitions, item_id)?;
    let equipped_count = inventory
        .equipment
        .iter()
        .filter(|item| item.item_id == item_id)
        .count() as u64;
    inventory_count
        .checked_add(equipped_count)
        .ok_or(ItemRuleError::QuantityOverflow { item_id })
}

pub(crate) fn requirements_have_quest_states(
    player: &PlayerState,
    requirements: &[QuestStateRequirement],
) -> bool {
    requirements.iter().all(|requirement| {
        progress(player, requirement.quest_id) == required_progress(requirement.state)
    })
}

pub(crate) fn requirements_have_skills(
    player: &PlayerState,
    requirements: &[QuestSkillRequirement],
) -> bool {
    requirements.iter().all(|requirement| {
        let acquired = player
            .learned_skills
            .iter()
            .any(|skill| skill.skill_id == requirement.skill_id && skill.level > 0);
        acquired == requirement.acquired
    })
}

pub(crate) fn requirements_have_records(
    player: &PlayerState,
    requirements: &[QuestRecordCondition],
) -> bool {
    requirements
        .iter()
        .all(|requirement| record_condition_matches(player, requirement))
}

pub(crate) fn requirements_have_effects(
    effects: &PlayerEffects,
    requirements: &[crate::content::QuestEffectRequirement],
) -> bool {
    requirements
        .iter()
        .all(|requirement| effects.contains_item(requirement.item_id) == requirement.active)
}

pub(crate) fn required_morph_is_active(
    effects: &PlayerEffects,
    required: Option<std::num::NonZeroU32>,
) -> bool {
    required.is_none_or(|required| effects.projected().morph_id == Some(required.get()))
}

pub(crate) fn record_condition_matches(
    player: &PlayerState,
    requirement: &QuestRecordCondition,
) -> bool {
    let Some(current) = crate::quest_records::get(player, requirement.quest_id, requirement.index)
    else {
        return false;
    };
    requirement
        .alternatives
        .iter()
        .any(|predicate| match predicate {
            QuestRecordPredicate::Equal(expected) => current == expected,
            QuestRecordPredicate::AtLeast(expected) => {
                crate::quest_records::strict_decimal(current)
                    .is_some_and(|current| current >= *expected)
            }
            QuestRecordPredicate::AtMost(expected) => crate::quest_records::strict_decimal(current)
                .is_some_and(|current| current <= *expected),
        })
}
pub(crate) fn required_progress(state: RequiredQuestState) -> QuestProgress {
    match state {
        RequiredQuestState::NotStarted => QuestProgress::NotStarted,
        RequiredQuestState::Started => QuestProgress::Started,
        RequiredQuestState::Completed => QuestProgress::Completed,
    }
}

pub(crate) fn state_label(state: RequiredQuestState) -> &'static str {
    match state {
        RequiredQuestState::NotStarted => "not started",
        RequiredQuestState::Started => "started",
        RequiredQuestState::Completed => "completed",
    }
}

pub(crate) fn require_npc(
    expected: Option<u32>,
    actual: u32,
) -> Result<(), QuestRuleError> {
    if expected == Some(actual) {
        Ok(())
    } else {
        Err(QuestRuleError::WrongNpc { npc_id: actual })
    }
}
