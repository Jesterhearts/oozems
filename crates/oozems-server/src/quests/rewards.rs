use super::*;

#[cfg(test)]
mod restoration_tests;
#[cfg(test)]
mod tests;

pub(crate) fn eligible_selectable_reward_choices(
    player: &PlayerState,
    quest: &QuestDefinition,
) -> Vec<(u32, QuestSelectableItemReward)> {
    quest
        .completion_actions
        .selectable_items
        .iter()
        .copied()
        .enumerate()
        .filter(|(_, reward)| reward_is_eligible(player, reward.eligibility))
        .filter_map(|(index, reward)| {
            selectable_reward_choice_id(index).map(|choice_id| (choice_id, reward))
        })
        .collect()
}

pub(crate) fn lost_item_restoration_needed(
    player: &PlayerState,
    quest: &QuestDefinition,
    item_definitions: &[ItemDefinition],
    now_unix_ms: u64,
) -> bool {
    let Some(lost) = quest.dialogue.completion.lost.as_ref() else {
        return false;
    };
    missing_restoration_grants(player, quest, lost, item_definitions, now_unix_ms)
        .is_ok_and(|grants| !grants.is_empty())
}

pub(crate) fn missing_restoration_grants(
    player: &PlayerState,
    quest: &QuestDefinition,
    lost: &crate::content::QuestLostItemDialogue,
    item_definitions: &[ItemDefinition],
    now_unix_ms: u64,
) -> Result<Vec<(u32, u64, u64)>, QuestRuleError> {
    let inventory = player
        .inventory
        .as_ref()
        .ok_or(ItemRuleError::MissingInventory)?;
    let mut grants = Vec::new();
    let mut has_eligible_item = false;
    for item in &lost.items {
        if !restoration_item_is_eligible(player, quest, item, item_definitions, now_unix_ms)? {
            continue;
        }
        has_eligible_item = true;
        let current = quest_item_quantity(inventory, item_definitions, item.item_id)?;
        let missing = item.target_count.saturating_sub(current);
        if missing == 0 {
            continue;
        }
        let expires_at_unix_ms = resolve_item_expiration(quest.id, item.expiration, now_unix_ms)?;
        if expires_at_unix_ms != 0 && expires_at_unix_ms <= now_unix_ms {
            continue;
        }
        grants.push((item.item_id, missing, expires_at_unix_ms));
    }
    if !has_eligible_item {
        return Err(QuestRuleError::LostItemRestorationUnavailable { quest_id: quest.id });
    }
    Ok(grants)
}

pub(crate) fn restoration_item_is_eligible(
    player: &PlayerState,
    quest: &QuestDefinition,
    item: &crate::content::QuestRestorableItem,
    item_definitions: &[ItemDefinition],
    now_unix_ms: u64,
) -> Result<bool, QuestRuleError> {
    let eligibility = item.eligibility;
    if progress(player, quest.id) != required_progress(eligibility.owner_state) {
        return Ok(false);
    }
    if eligibility.owner_state == RequiredQuestState::Started {
        let Some(owner) = player
            .quests
            .iter()
            .find(|entry| entry.quest_id == quest.id)
        else {
            return Ok(false);
        };
        if quest_is_expired(owner, quest, now_unix_ms) {
            return Ok(false);
        }
    }
    if !requirements_have_quest_states(player, eligibility.required_quests)
        || eligibility.forbidden_quests.iter().any(|requirement| {
            progress(player, requirement.quest_id) == required_progress(requirement.state)
        })
        || eligibility.absent_skill_ids.iter().any(|skill_id| {
            player.learned_skills.iter().any(|skill| {
                skill.skill_id == *skill_id && (skill.level > 0 || skill.master_level > 0)
            })
        })
    {
        return Ok(false);
    }
    let inventory = player
        .inventory
        .as_ref()
        .ok_or(ItemRuleError::MissingInventory)?;
    for absent_item_id in eligibility
        .absent_item_ids
        .iter()
        .copied()
        .filter(|absent_item_id| *absent_item_id != item.item_id)
    {
        if quest_item_quantity(inventory, item_definitions, absent_item_id)? != 0 {
            return Ok(false);
        }
    }
    Ok(true)
}

pub(crate) fn selectable_reward_choice_id(index: usize) -> Option<u32> {
    SELECTABLE_REWARD_CHOICE_OFFSET.checked_add(u32::try_from(index).ok()?)
}
pub(crate) fn restore_lost_quest_items(
    mut player: PlayerState,
    quest: &QuestDefinition,
    item_definitions: &[ItemDefinition],
    now_unix_ms: u64,
) -> Result<QuestSelection, QuestRuleError> {
    let lost = quest
        .dialogue
        .completion
        .lost
        .as_ref()
        .filter(|lost| !lost.prompt_pages.is_empty() && !lost.items.is_empty());
    let lost = lost.ok_or(QuestRuleError::LostItemRestorationUnavailable { quest_id: quest.id })?;
    let inventory = player
        .inventory
        .as_ref()
        .ok_or(ItemRuleError::MissingInventory)?;
    let grants = missing_restoration_grants(&player, quest, lost, item_definitions, now_unix_ms)?;
    if grants.is_empty() {
        return Err(QuestRuleError::NoMissingRestorableItems { quest_id: quest.id });
    }

    let mut restored_inventory = inventory.clone();
    for (item_id, quantity, expires_at_unix_ms) in grants {
        crate::items::apply_item_grant(
            &mut restored_inventory,
            item_definitions,
            item_id,
            quantity,
            expires_at_unix_ms,
        )?;
    }
    player.inventory = Some(restored_inventory);
    Ok(QuestSelection {
        player,
        pages: lost.success_pages.clone(),
        changed: true,
        npc_animation_action: None,
        next_interaction: None,
        next_quest_id: None,
    })
}

pub(crate) fn selected_reward_for_choice(
    player: &PlayerState,
    quest: &QuestDefinition,
    choice_id: u32,
) -> Result<QuestSelectableItemReward, QuestRuleError> {
    let index = choice_id
        .checked_sub(SELECTABLE_REWARD_CHOICE_OFFSET)
        .and_then(|index| usize::try_from(index).ok())
        .ok_or(QuestRuleError::InvalidChoice { choice_id })?;
    let reward = quest
        .completion_actions
        .selectable_items
        .get(index)
        .copied()
        .ok_or(QuestRuleError::InvalidChoice { choice_id })?;
    if !reward_is_eligible(player, reward.eligibility) {
        return Err(QuestRuleError::InvalidChoice { choice_id });
    }
    Ok(reward)
}
pub fn select_weighted_item(
    player_id: &str,
    quest_id: u32,
    accepted_at_unix_ms: u64,
    rewards: &[QuestWeightedItem],
) -> Option<QuestWeightedItem> {
    let total = rewards.iter().fold(0_u64, |total, reward| {
        total.saturating_add(u64::from(reward.weight))
    });
    if total == 0 {
        return None;
    }
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in player_id
        .bytes()
        .chain(quest_id.to_le_bytes())
        .chain(accepted_at_unix_ms.to_le_bytes())
    {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    let mut ticket = hash % total;
    rewards.iter().copied().find(|reward| {
        if ticket < u64::from(reward.weight) {
            true
        } else {
            ticket -= u64::from(reward.weight);
            false
        }
    })
}
pub(crate) fn reward_is_eligible(
    player: &PlayerState,
    eligibility: QuestRewardEligibility,
) -> bool {
    let job_matches = eligibility.job_mask.is_none_or(|required| {
        player
            .stats
            .as_ref()
            .and_then(|stats| job_family_mask(stats.job_id))
            .is_some_and(|actual| required & actual != 0)
    });
    let gender_matches = eligibility.gender.is_none_or(|required| {
        player
            .appearance
            .as_ref()
            .and_then(|appearance| CharacterGender::try_from(appearance.gender).ok())
            .is_some_and(|actual| {
                matches!(
                    (required, actual),
                    (QuestRewardGender::Male, CharacterGender::Male)
                        | (QuestRewardGender::Female, CharacterGender::Female)
                )
            })
    });
    job_matches && gender_matches
}

pub(crate) fn job_family_mask(job_id: u32) -> Option<u32> {
    // WZ assigns one reward-mask bit to each hundreds-level job family.
    1_u32.checked_shl(job_id / 100)
}
