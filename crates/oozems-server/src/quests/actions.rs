use super::*;

#[cfg(test)]
mod resource_tests;
#[cfg(test)]
mod state_tests;

pub(crate) fn start_quest(
    player: PlayerState,
    effects: &mut PlayerEffects,
    quest: &QuestDefinition,
    include_question_trailing_pages: bool,
    curve: &ExperienceCurve,
    item_definitions: &[ItemDefinition],
    consume_effects: &[ConsumeEffectDefinition],
    scripts: &QuestScriptCatalog,
    environment: QuestEnvironment,
) -> Result<QuestSelection, QuestRuleError> {
    let script_plan = resolve_action_plan(
        scripts,
        &player,
        quest,
        QuestScriptPhase::Start,
        item_definitions,
    )?;
    if !is_available(
        &player,
        effects,
        quest,
        item_definitions,
        scripts,
        environment,
    ) {
        return Err(QuestRuleError::Unavailable { quest_id: quest.id });
    }
    let actions = merge_actions(quest.id, &quest.start_actions, script_plan.as_ref())?;
    let mut player = player;
    crate::quest_records::clear(&mut player, quest.id);
    let mut player = apply_actions(
        player,
        quest.id,
        environment.now_unix_ms,
        environment.now_unix_ms,
        &actions,
        None,
        curve,
        item_definitions,
        consume_effects,
        effects,
    )?;
    let entry = PlayerQuest {
        quest_id: quest.id,
        status: QuestStatus::Started as i32,
        mob_progress: Vec::new(),
        accepted_at_unix_ms: environment.now_unix_ms,
        completed_at_unix_ms: 0,
        dialogue_step: 0,
        completion_quiz_passed: false,
    };
    if let Some(existing) = player
        .quests
        .iter_mut()
        .find(|existing| existing.quest_id == quest.id)
    {
        *existing = entry;
    } else {
        player.quests.push(entry);
        player.quests.sort_by_key(|entry| entry.quest_id);
    }
    let mut pages = if include_question_trailing_pages {
        quest
            .dialogue
            .start_question
            .as_ref()
            .map(|question| question.trailing_pages.clone())
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    pages.extend(quest.dialogue.accepted_pages.iter().cloned());
    if let Some(plan) = script_plan {
        pages.extend(plan.result_pages);
    }
    Ok(QuestSelection {
        player,
        pages,
        changed: true,
        npc_animation_action: actions.npc_animation_action.clone(),
        next_interaction: None,
        next_quest_id: None,
    })
}
pub(crate) fn complete_quest(
    player: PlayerState,
    effects: &mut PlayerEffects,
    quest: &QuestDefinition,
    checks: CompletionChecks,
    selected_reward: Option<QuestSelectableItemReward>,
    curve: &ExperienceCurve,
    item_definitions: &[ItemDefinition],
    consume_effects: &[ConsumeEffectDefinition],
    scripts: &QuestScriptCatalog,
    quest_definitions: &[&QuestDefinition],
    environment: QuestEnvironment,
) -> Result<QuestSelection, QuestRuleError> {
    let active = player
        .quests
        .iter()
        .find(|entry| entry.quest_id == quest.id)
        .filter(|entry| QuestStatus::try_from(entry.status) == Ok(QuestStatus::Started))
        .ok_or(QuestRuleError::NotActive { quest_id: quest.id })?;
    if quest_is_expired(active, quest, environment.now_unix_ms) {
        return Err(QuestRuleError::Expired { quest_id: quest.id });
    }
    if !completion_window_allows(quest, environment.now_unix_ms) {
        return Err(QuestRuleError::ObjectivesIncomplete { quest_id: quest.id });
    }
    if !requirements_have_equipped_items(&player, &quest.completion.equipped_items) {
        return Err(QuestRuleError::ObjectivesIncomplete { quest_id: quest.id });
    }
    if !requirements_have_monster_book(&player, &quest.completion.monster_book) {
        return Err(QuestRuleError::ObjectivesIncomplete { quest_id: quest.id });
    }
    if selected_reward.is_none() && !quest.completion_actions.selectable_items.is_empty() {
        return Err(
            if eligible_selectable_reward_choices(&player, quest).is_empty() {
                QuestRuleError::NoEligibleSelectableReward { quest_id: quest.id }
            } else {
                QuestRuleError::RewardSelectionRequired { quest_id: quest.id }
            },
        );
    }
    let accepted_at_unix_ms = active.accepted_at_unix_ms;
    let script_plan = resolve_action_plan(
        scripts,
        &player,
        quest,
        QuestScriptPhase::Completion,
        item_definitions,
    )?;
    if checks == CompletionChecks::Normal
        && !completion_readiness(
            &player,
            effects,
            quest,
            quest_definitions,
            item_definitions,
            scripts,
            environment,
        )
        .ready
    {
        return Err(QuestRuleError::ObjectivesIncomplete { quest_id: quest.id });
    }
    let actions = merge_actions(quest.id, &quest.completion_actions, script_plan.as_ref())?;
    let mut player = apply_actions(
        player,
        quest.id,
        accepted_at_unix_ms,
        environment.now_unix_ms,
        &actions,
        selected_reward,
        curve,
        item_definitions,
        consume_effects,
        effects,
    )?;
    let entry = player
        .quests
        .iter_mut()
        .find(|entry| entry.quest_id == quest.id)
        .ok_or(QuestRuleError::NotActive { quest_id: quest.id })?;
    entry.status = QuestStatus::Completed as i32;
    entry.completed_at_unix_ms = environment.now_unix_ms;
    entry.dialogue_step = 0;
    let quiz_was_passed = entry.completion_quiz_passed;
    entry.completion_quiz_passed = false;
    let mut pages = Vec::new();
    if !quiz_was_passed && let Some(question) = &quest.dialogue.question {
        pages.extend(question.trailing_pages.iter().cloned());
    }
    pages.extend(quest.dialogue.completion.success_pages.iter().cloned());
    if let Some(plan) = script_plan {
        pages.extend(plan.result_pages);
    }
    Ok(QuestSelection {
        player,
        pages,
        changed: true,
        npc_animation_action: actions.npc_animation_action.clone(),
        next_interaction: None,
        next_quest_id: quest.completion_actions.next_quest_id,
    })
}

pub(crate) fn resolve_action_plan(
    scripts: &QuestScriptCatalog,
    player: &PlayerState,
    quest: &QuestDefinition,
    phase: QuestScriptPhase,
    item_definitions: &[ItemDefinition],
) -> Result<Option<QuestScriptPlan>, QuestRuleError> {
    match resolve_quest_script(scripts, quest, phase, player, item_definitions) {
        QuestScriptResolution::NotReferenced => Ok(None),
        QuestScriptResolution::Ready(plan) => Ok(Some(plan)),
        QuestScriptResolution::Missing { script } => Err(QuestRuleError::ScriptRequired {
            quest_id: quest.id,
            phase,
            script,
        }),
        QuestScriptResolution::ConditionsNotMet { .. } => Err(match phase {
            QuestScriptPhase::Start => QuestRuleError::Unavailable { quest_id: quest.id },
            QuestScriptPhase::Completion => {
                QuestRuleError::ObjectivesIncomplete { quest_id: quest.id }
            }
        }),
    }
}

pub(crate) fn script_conditions_pass(
    scripts: &QuestScriptCatalog,
    player: &PlayerState,
    quest: &QuestDefinition,
    phase: QuestScriptPhase,
    item_definitions: &[ItemDefinition],
) -> bool {
    matches!(
        resolve_quest_script(scripts, quest, phase, player, item_definitions),
        QuestScriptResolution::NotReferenced | QuestScriptResolution::Ready(_)
    )
}

pub(crate) fn merge_actions(
    quest_id: u32,
    wz: &QuestActions,
    script: Option<&QuestScriptPlan>,
) -> Result<QuestActions, QuestRuleError> {
    let Some(script) = script else {
        return Ok(wz.clone());
    };
    let mut actions = wz.clone();
    actions.fixed_items.extend_from_slice(&script.item_deltas);
    actions.money = actions
        .money
        .checked_add(script.mesos)
        .ok_or(QuestRuleError::ActionOverflow { quest_id })?;
    actions.experience = actions
        .experience
        .checked_add(script.experience)
        .ok_or(QuestRuleError::ActionOverflow { quest_id })?;
    actions.fame = actions
        .fame
        .checked_add(script.fame)
        .ok_or(QuestRuleError::ActionOverflow { quest_id })?;
    for write in &script.record_writes {
        if actions
            .record_writes
            .iter()
            .any(|existing| existing.quest_id == write.quest_id && existing.index == write.index)
        {
            return Err(QuestRuleError::ActionOverflow { quest_id });
        }
        actions.record_writes.push(write.clone());
    }
    for action in &script.quest_state_actions {
        if actions
            .quest_state_actions
            .iter()
            .any(|existing| existing.quest_id == action.quest_id)
        {
            return Err(QuestRuleError::ActionOverflow { quest_id });
        }
        actions.quest_state_actions.push(*action);
    }
    actions
        .record_writes
        .sort_by_key(|write| (write.quest_id, write.index));
    Ok(actions)
}

pub(crate) fn apply_actions(
    mut player: PlayerState,
    quest_id: u32,
    selection_seed_unix_ms: u64,
    now_unix_ms: u64,
    actions: &QuestActions,
    selected_reward: Option<QuestSelectableItemReward>,
    curve: &ExperienceCurve,
    item_definitions: &[ItemDefinition],
    consume_effects: &[ConsumeEffectDefinition],
    effects: &mut PlayerEffects,
) -> Result<PlayerState, QuestRuleError> {
    let action_effects = actions
        .buff_item_ids
        .iter()
        .map(|item_id| {
            consume_effects
                .iter()
                .find(|definition| definition.item_id == *item_id)
                .copied()
                .ok_or(QuestRuleError::MissingConsumeEffect {
                    quest_id,
                    item_id: *item_id,
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    apply_skill_changes(&mut player, &actions.skill_changes);
    apply_quest_state_actions(&mut player, &actions.quest_state_actions, now_unix_ms);
    let eligible_weighted = actions
        .weighted_items
        .iter()
        .copied()
        .filter(|reward| reward_is_eligible(&player, reward.eligibility))
        .collect::<Vec<_>>();
    let weighted = select_weighted_item(
        &player.id,
        quest_id,
        selection_seed_unix_ms,
        &eligible_weighted,
    );
    let mut removals = BTreeMap::<u32, u64>::new();
    let mut grants = BTreeMap::<(u32, u64), u64>::new();
    for delta in actions
        .fixed_items
        .iter()
        .copied()
        .chain(
            actions
                .conditional_items
                .iter()
                .filter(|reward| reward_is_eligible(&player, reward.eligibility))
                .map(|reward| crate::content::QuestItemDelta {
                    item_id: reward.item_id,
                    count: i64::from(reward.count),
                    expiration: reward.expiration,
                }),
        )
        .chain(weighted.map(|reward| crate::content::QuestItemDelta {
            item_id: reward.item_id,
            count: i64::from(reward.count),
            expiration: reward.expiration,
        }))
        .chain(
            selected_reward.map(|reward| crate::content::QuestItemDelta {
                item_id: reward.item_id,
                count: i64::from(reward.count),
                expiration: reward.expiration,
            }),
        )
    {
        if delta.count < 0 && delta.expiration.is_some() {
            return Err(QuestRuleError::ExpiringRemoval {
                quest_id,
                item_id: delta.item_id,
            });
        }
        let quantity = delta.count.unsigned_abs();
        let total = if delta.count < 0 {
            removals.entry(delta.item_id).or_default()
        } else {
            let expires_at_unix_ms =
                resolve_item_expiration(quest_id, delta.expiration, now_unix_ms)?;
            if expires_at_unix_ms != 0 && expires_at_unix_ms <= now_unix_ms {
                continue;
            }
            grants
                .entry((delta.item_id, expires_at_unix_ms))
                .or_default()
        };
        *total = total
            .checked_add(quantity)
            .ok_or(ItemRuleError::QuantityOverflow {
                item_id: delta.item_id,
            })?;
    }
    if !removals.is_empty() || !grants.is_empty() {
        let inventory = player
            .inventory
            .as_mut()
            .ok_or(ItemRuleError::MissingInventory)?;
        for (item_id, quantity) in removals {
            let quantity =
                i64::try_from(quantity).map_err(|_| ItemRuleError::QuantityOverflow { item_id })?;
            crate::items::apply_item_delta(inventory, item_definitions, item_id, -quantity)?;
        }
        for ((item_id, expires_at_unix_ms), quantity) in grants {
            crate::items::apply_item_grant(
                inventory,
                item_definitions,
                item_id,
                quantity,
                expires_at_unix_ms,
            )?;
        }
    }
    player.mesos = apply_signed_u64(player.mesos, actions.money)?;
    if actions.fame != 0 {
        let player_id = player.id.clone();
        let stats = player
            .stats
            .as_mut()
            .ok_or_else(|| QuestRuleError::MissingStats {
                player_id: player_id.clone(),
            })?;
        stats.fame = stats
            .fame
            .checked_add(actions.fame)
            .ok_or(QuestRuleError::FameOverflow { player_id })?;
    }
    if actions.experience != 0 {
        player = crate::experience::grant_experience(player, actions.experience, curve)?;
    }
    for write in &actions.record_writes {
        crate::quest_records::set(
            &mut player,
            write.quest_id,
            write.index,
            write.value.clone(),
        )?;
    }
    let mut staged_effects = effects.clone();
    for definition in action_effects {
        player = crate::effects::apply_consume_effect(
            player,
            &mut staged_effects,
            definition,
            now_unix_ms,
        );
    }
    *effects = staged_effects;
    Ok(player)
}

pub(crate) fn apply_quest_state_actions(
    player: &mut PlayerState,
    actions: &[QuestStateAction],
    now_unix_ms: u64,
) {
    for action in actions {
        let accepted_at_unix_ms = player
            .quests
            .iter()
            .find(|entry| entry.quest_id == action.quest_id)
            .filter(|entry| {
                matches!(
                    QuestStatus::try_from(entry.status),
                    Ok(QuestStatus::Started | QuestStatus::Completed)
                ) && entry.accepted_at_unix_ms > 0
                    && entry.accepted_at_unix_ms <= now_unix_ms
            })
            .map_or(now_unix_ms, |entry| entry.accepted_at_unix_ms);
        player
            .quests
            .retain(|entry| entry.quest_id != action.quest_id);
        match action.state {
            QuestStateActionState::NotStarted => {
                crate::quest_records::clear(player, action.quest_id);
            }
            QuestStateActionState::Started => player.quests.push(PlayerQuest {
                quest_id: action.quest_id,
                status: QuestStatus::Started as i32,
                mob_progress: Vec::new(),
                accepted_at_unix_ms: now_unix_ms,
                completed_at_unix_ms: 0,
                dialogue_step: 0,
                completion_quiz_passed: false,
            }),
            QuestStateActionState::Completed => player.quests.push(PlayerQuest {
                quest_id: action.quest_id,
                status: QuestStatus::Completed as i32,
                mob_progress: Vec::new(),
                accepted_at_unix_ms,
                completed_at_unix_ms: now_unix_ms,
                dialogue_step: 0,
                completion_quiz_passed: false,
            }),
        }
    }
    player.quests.sort_by_key(|entry| entry.quest_id);
}

pub(crate) fn apply_skill_changes(
    player: &mut PlayerState,
    changes: &[QuestSkillChange],
) {
    let job_id = player.stats.as_ref().map_or(0, |stats| stats.job_id);
    for change in changes {
        if !change.job_ids.contains(&job_id) && change.skill_id % 10_000_000 >= 10_000 {
            continue;
        }
        match change.operation {
            QuestSkillOperation::Grant {
                skill_level,
                master_level,
            } => {
                if let Some(learned) = player
                    .learned_skills
                    .iter_mut()
                    .find(|learned| learned.skill_id == change.skill_id)
                {
                    learned.level = learned.level.max(skill_level);
                    learned.master_level = learned.master_level.max(master_level);
                } else if skill_level > 0 || master_level > 0 {
                    player.learned_skills.push(LearnedSkill {
                        skill_id: change.skill_id,
                        level: skill_level,
                        master_level,
                    });
                }
            }
            QuestSkillOperation::Remove => {
                player
                    .learned_skills
                    .retain(|learned| learned.skill_id != change.skill_id);
                player
                    .key_bindings
                    .retain(|binding| binding.skill_id != change.skill_id);
            }
        }
    }
    player
        .learned_skills
        .sort_by_key(|learned| learned.skill_id);
}

pub(crate) fn resolve_item_expiration(
    quest_id: u32,
    expiration: Option<QuestItemExpiration>,
    now_unix_ms: u64,
) -> Result<u64, QuestRuleError> {
    match expiration {
        None => Ok(0),
        Some(QuestItemExpiration::RelativeMilliseconds(duration_ms)) => now_unix_ms
            .checked_add(duration_ms)
            .ok_or(QuestRuleError::ActionOverflow { quest_id }),
        Some(QuestItemExpiration::AbsoluteUnixMilliseconds(deadline)) => Ok(deadline),
    }
}
pub(crate) fn apply_signed_u64(
    value: u64,
    delta: i64,
) -> Result<u64, ItemRuleError> {
    if delta >= 0 {
        value
            .checked_add(delta.unsigned_abs())
            .ok_or(ItemRuleError::MesosOverflow)
    } else {
        value
            .checked_sub(delta.unsigned_abs())
            .ok_or(ItemRuleError::InsufficientMesos)
    }
}
