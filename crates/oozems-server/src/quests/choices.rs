use super::*;

#[cfg(test)]
mod tests;

pub fn answer_choice_id(
    phase: QuestQuestionPhase,
    step_index: usize,
    choice_index: usize,
) -> Option<u32> {
    let phase_start = match phase {
        QuestQuestionPhase::Start => ANSWER_CHOICE_OFFSET,
        QuestQuestionPhase::Completion => {
            ANSWER_CHOICE_OFFSET.checked_add(ANSWER_CHOICE_PHASE_CAPACITY)?
        }
    };
    let phase_end = phase_start.checked_add(ANSWER_CHOICE_PHASE_CAPACITY)?;
    let step_index = u32::try_from(step_index).ok()?;
    let choice_index = u32::try_from(choice_index).ok()?;
    if choice_index >= ANSWER_CHOICE_STEP_STRIDE {
        return None;
    }
    phase_start
        .checked_add(step_index.checked_mul(ANSWER_CHOICE_STEP_STRIDE)?)?
        .checked_add(choice_index)
        .filter(|choice_id| *choice_id < phase_end)
}

pub fn begin_start_question(
    mut player: PlayerState,
    effects: &PlayerEffects,
    quest: &QuestDefinition,
    npc_id: u32,
    item_definitions: &[ItemDefinition],
    scripts: &QuestScriptCatalog,
    environment: QuestEnvironment,
) -> Result<QuestSelection, QuestRuleError> {
    require_npc(quest.start.npc_id, npc_id)?;
    let question = quest
        .dialogue
        .start_question
        .as_ref()
        .ok_or(QuestRuleError::InvalidChoice {
            choice_id: ACCEPT_CHOICE_ID,
        })?;
    let existing_index = player
        .quests
        .iter()
        .position(|entry| entry.quest_id == quest.id);
    if let Some(index) = existing_index
        && QuestStatus::try_from(player.quests[index].status) == Ok(QuestStatus::Unspecified)
        && start_decision_entry_is_pending(&player.quests[index], quest, question)
    {
        return Ok(QuestSelection {
            player,
            pages: question.trailing_pages.clone(),
            changed: false,
            npc_animation_action: None,
            next_interaction: Some(QuestNextInteraction::StartDecision),
            next_quest_id: None,
        });
    }
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
    if let Some(index) = existing_index
        && QuestStatus::try_from(player.quests[index].status) == Ok(QuestStatus::Unspecified)
        && let Some(step_index) = question_step_index(&player.quests[index], question)
    {
        return Ok(QuestSelection {
            player,
            pages: Vec::new(),
            changed: false,
            npc_animation_action: None,
            next_interaction: Some(QuestNextInteraction::Question {
                phase: QuestQuestionPhase::Start,
                step_index,
            }),
            next_quest_id: None,
        });
    }

    let mut entry = existing_index
        .map(|index| player.quests[index].clone())
        .unwrap_or_default();
    entry.quest_id = quest.id;
    entry.status = QuestStatus::Unspecified as i32;
    entry.mob_progress.clear();
    entry.dialogue_step = 1;
    entry.completion_quiz_passed = false;
    if let Some(index) = existing_index {
        player.quests[index] = entry;
    } else {
        player.quests.push(entry);
        player.quests.sort_by_key(|entry| entry.quest_id);
    }
    Ok(QuestSelection {
        player,
        pages: question.leading_pages.clone(),
        changed: true,
        npc_animation_action: None,
        next_interaction: Some(QuestNextInteraction::Question {
            phase: QuestQuestionPhase::Start,
            step_index: 0,
        }),
        next_quest_id: None,
    })
}

pub fn begin_completion_question(
    mut player: PlayerState,
    effects: &PlayerEffects,
    quest: &QuestDefinition,
    quest_definitions: &[&QuestDefinition],
    npc_id: u32,
    item_definitions: &[ItemDefinition],
    scripts: &QuestScriptCatalog,
    environment: QuestEnvironment,
) -> Result<QuestSelection, QuestRuleError> {
    require_npc(quest.completion.npc_id, npc_id)?;
    let question = quest
        .dialogue
        .question
        .as_ref()
        .ok_or(QuestRuleError::InvalidChoice {
            choice_id: COMPLETE_CHOICE_ID,
        })?;
    require_completion_ready(
        &player,
        effects,
        quest,
        quest_definitions,
        item_definitions,
        scripts,
        environment,
    )?;
    let entry = player
        .quests
        .iter_mut()
        .find(|entry| entry.quest_id == quest.id)
        .filter(|entry| QuestStatus::try_from(entry.status) == Ok(QuestStatus::Started))
        .ok_or(QuestRuleError::NotActive { quest_id: quest.id })?;
    if entry.completion_quiz_passed {
        return Ok(QuestSelection {
            player,
            pages: Vec::new(),
            changed: false,
            npc_animation_action: None,
            next_interaction: Some(QuestNextInteraction::SelectableReward),
            next_quest_id: None,
        });
    }
    let (step_index, changed, pages) = match question_step_index(entry, question) {
        Some(step_index) => (step_index, false, Vec::new()),
        None => {
            entry.dialogue_step = 1;
            (0, true, question.leading_pages.clone())
        }
    };
    Ok(QuestSelection {
        player,
        pages,
        changed,
        npc_animation_action: None,
        next_interaction: Some(QuestNextInteraction::Question {
            phase: QuestQuestionPhase::Completion,
            step_index,
        }),
        next_quest_id: None,
    })
}

pub fn select_choice(
    player: PlayerState,
    effects: &mut PlayerEffects,
    quest: &QuestDefinition,
    quest_definitions: &[&QuestDefinition],
    npc_id: u32,
    choice_id: u32,
    curve: &ExperienceCurve,
    item_definitions: &[ItemDefinition],
    consume_effects: &[ConsumeEffectDefinition],
    scripts: &QuestScriptCatalog,
    environment: QuestEnvironment,
) -> Result<QuestSelection, QuestRuleError> {
    if choice_id == RESTORE_ITEMS_CHOICE_ID
        && progress(&player, quest.id) == QuestProgress::Completed
        && quest.dialogue.completion.lost.as_ref().is_some_and(|lost| {
            lost.items
                .iter()
                .any(|item| item.eligibility.owner_state == RequiredQuestState::Completed)
        })
    {
        require_npc(quest.completion.npc_id, npc_id)?;
        return restore_lost_quest_items(player, quest, item_definitions, environment.now_unix_ms);
    }
    match progress(&player, quest.id) {
        QuestProgress::Started => select_active_choice(
            player,
            effects,
            quest,
            quest_definitions,
            npc_id,
            choice_id,
            curve,
            item_definitions,
            consume_effects,
            scripts,
            environment,
        ),
        QuestProgress::NotStarted => select_offer_choice(
            player,
            effects,
            quest,
            npc_id,
            choice_id,
            curve,
            item_definitions,
            consume_effects,
            scripts,
            environment,
        ),
        QuestProgress::Completed => {
            if quest.dialogue.start_question.is_none()
                && !is_available(
                    &player,
                    effects,
                    quest,
                    item_definitions,
                    scripts,
                    environment,
                )
            {
                return Err(QuestRuleError::Unavailable { quest_id: quest.id });
            }
            select_offer_choice(
                player,
                effects,
                quest,
                npc_id,
                choice_id,
                curve,
                item_definitions,
                consume_effects,
                scripts,
                environment,
            )
        }
    }
}

pub(crate) fn select_offer_choice(
    player: PlayerState,
    effects: &mut PlayerEffects,
    quest: &QuestDefinition,
    npc_id: u32,
    choice_id: u32,
    curve: &ExperienceCurve,
    item_definitions: &[ItemDefinition],
    consume_effects: &[ConsumeEffectDefinition],
    scripts: &QuestScriptCatalog,
    environment: QuestEnvironment,
) -> Result<QuestSelection, QuestRuleError> {
    require_npc(quest.start.npc_id, npc_id)?;
    if let Some(question) = &quest.dialogue.start_question {
        if has_pending_start_decision(&player, quest) {
            return match choice_id {
                ACCEPT_CHOICE_ID => start_quest(
                    player,
                    effects,
                    quest,
                    false,
                    curve,
                    item_definitions,
                    consume_effects,
                    scripts,
                    environment,
                ),
                DECLINE_CHOICE_ID => Ok(decline_start_decision(player, quest)),
                _ => Err(QuestRuleError::InvalidChoice { choice_id }),
            };
        }
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
        let step_index = player
            .quests
            .iter()
            .find(|entry| entry.quest_id == quest.id)
            .filter(|entry| QuestStatus::try_from(entry.status) == Ok(QuestStatus::Unspecified))
            .and_then(|entry| question_step_index(entry, question))
            .ok_or(QuestRuleError::InvalidChoice { choice_id })?;
        let step = &question.steps[step_index];
        let answer_id = question_answer_id(QuestQuestionPhase::Start, step_index, step, choice_id)?;
        if answer_id != step.correct_choice_id {
            return Ok(QuestSelection {
                player,
                pages: step
                    .failure_pages
                    .get(&answer_id)
                    .cloned()
                    .unwrap_or_default(),
                changed: false,
                npc_animation_action: None,
                next_interaction: None,
                next_quest_id: None,
            });
        }
        if step_index + 1 < question.steps.len() {
            let mut player = player;
            let entry = player
                .quests
                .iter_mut()
                .find(|entry| entry.quest_id == quest.id)
                .expect("validated pending start question");
            entry.dialogue_step = question_dialogue_step(step_index + 1)
                .ok_or(QuestRuleError::QuestionStateOverflow { quest_id: quest.id })?;
            return Ok(QuestSelection {
                player,
                pages: step.continuation_pages.clone(),
                changed: true,
                npc_animation_action: None,
                next_interaction: Some(QuestNextInteraction::Question {
                    phase: QuestQuestionPhase::Start,
                    step_index: step_index + 1,
                }),
                next_quest_id: None,
            });
        }
        if quest.dialogue.has_start_decision {
            let mut player = player;
            let entry = player
                .quests
                .iter_mut()
                .find(|entry| entry.quest_id == quest.id)
                .expect("validated pending start question");
            entry.dialogue_step = pending_start_decision_step(question)
                .ok_or(QuestRuleError::QuestionStateOverflow { quest_id: quest.id })?;
            return Ok(QuestSelection {
                player,
                pages: question.trailing_pages.clone(),
                changed: true,
                npc_animation_action: None,
                next_interaction: Some(QuestNextInteraction::StartDecision),
                next_quest_id: None,
            });
        }
        return start_quest(
            player,
            effects,
            quest,
            true,
            curve,
            item_definitions,
            consume_effects,
            scripts,
            environment,
        );
    }
    if choice_id == DECLINE_CHOICE_ID {
        return Ok(QuestSelection {
            player,
            pages: quest.dialogue.declined_pages.clone(),
            changed: false,
            npc_animation_action: None,
            next_interaction: None,
            next_quest_id: None,
        });
    }
    if choice_id != ACCEPT_CHOICE_ID {
        return Err(QuestRuleError::InvalidChoice { choice_id });
    }
    start_quest(
        player,
        effects,
        quest,
        false,
        curve,
        item_definitions,
        consume_effects,
        scripts,
        environment,
    )
}
pub(crate) fn select_active_choice(
    player: PlayerState,
    effects: &mut PlayerEffects,
    quest: &QuestDefinition,
    quest_definitions: &[&QuestDefinition],
    npc_id: u32,
    choice_id: u32,
    curve: &ExperienceCurve,
    item_definitions: &[ItemDefinition],
    consume_effects: &[ConsumeEffectDefinition],
    scripts: &QuestScriptCatalog,
    environment: QuestEnvironment,
) -> Result<QuestSelection, QuestRuleError> {
    require_npc(quest.completion.npc_id, npc_id)?;
    if choice_id == RESTORE_ITEMS_CHOICE_ID {
        return restore_lost_quest_items(player, quest, item_definitions, environment.now_unix_ms);
    }
    if let Some(question) = &quest.dialogue.question {
        require_completion_ready(
            &player,
            effects,
            quest,
            quest_definitions,
            item_definitions,
            scripts,
            environment,
        )?;
        let entry = player
            .quests
            .iter()
            .find(|entry| entry.quest_id == quest.id)
            .filter(|entry| QuestStatus::try_from(entry.status) == Ok(QuestStatus::Started))
            .ok_or(QuestRuleError::NotActive { quest_id: quest.id })?;
        if entry.completion_quiz_passed {
            let selected_reward = selected_reward_for_choice(&player, quest, choice_id)?;
            return complete_quest(
                player,
                effects,
                quest,
                CompletionChecks::Normal,
                Some(selected_reward),
                curve,
                item_definitions,
                consume_effects,
                scripts,
                quest_definitions,
                environment,
            );
        }
        let step_index = question_step_index(entry, question)
            .ok_or(QuestRuleError::InvalidChoice { choice_id })?;
        let step = &question.steps[step_index];
        let answer_id =
            question_answer_id(QuestQuestionPhase::Completion, step_index, step, choice_id)?;
        if answer_id != step.correct_choice_id {
            return Ok(QuestSelection {
                player,
                pages: step
                    .failure_pages
                    .get(&answer_id)
                    .cloned()
                    .unwrap_or_default(),
                changed: false,
                npc_animation_action: None,
                next_interaction: None,
                next_quest_id: None,
            });
        }
        if step_index + 1 < question.steps.len() {
            let mut player = player;
            let entry = player
                .quests
                .iter_mut()
                .find(|entry| entry.quest_id == quest.id)
                .expect("validated pending completion question");
            entry.dialogue_step = question_dialogue_step(step_index + 1)
                .ok_or(QuestRuleError::QuestionStateOverflow { quest_id: quest.id })?;
            return Ok(QuestSelection {
                player,
                pages: step.continuation_pages.clone(),
                changed: true,
                npc_animation_action: None,
                next_interaction: Some(QuestNextInteraction::Question {
                    phase: QuestQuestionPhase::Completion,
                    step_index: step_index + 1,
                }),
                next_quest_id: None,
            });
        }
        if !quest.completion_actions.selectable_items.is_empty() {
            if eligible_selectable_reward_choices(&player, quest).is_empty() {
                return Err(QuestRuleError::NoEligibleSelectableReward { quest_id: quest.id });
            }
            let mut player = player;
            let entry = player
                .quests
                .iter_mut()
                .find(|entry| entry.quest_id == quest.id)
                .expect("validated pending completion question");
            entry.dialogue_step = 0;
            entry.completion_quiz_passed = true;
            return Ok(QuestSelection {
                player,
                pages: question.trailing_pages.clone(),
                changed: true,
                npc_animation_action: None,
                next_interaction: Some(QuestNextInteraction::SelectableReward),
                next_quest_id: None,
            });
        }
        return complete_quest(
            player,
            effects,
            quest,
            CompletionChecks::Normal,
            None,
            curve,
            item_definitions,
            consume_effects,
            scripts,
            quest_definitions,
            environment,
        );
    }
    if quest.completion_actions.selectable_items.is_empty() {
        if choice_id != COMPLETE_CHOICE_ID {
            return Err(QuestRuleError::InvalidChoice { choice_id });
        }
        return complete_quest(
            player,
            effects,
            quest,
            CompletionChecks::Normal,
            None,
            curve,
            item_definitions,
            consume_effects,
            scripts,
            quest_definitions,
            environment,
        );
    }
    let selected_reward = selected_reward_for_choice(&player, quest, choice_id)?;
    complete_quest(
        player,
        effects,
        quest,
        CompletionChecks::Normal,
        Some(selected_reward),
        curve,
        item_definitions,
        consume_effects,
        scripts,
        quest_definitions,
        environment,
    )
}

pub(crate) fn question_answer_id(
    phase: QuestQuestionPhase,
    step_index: usize,
    question: &QuestQuestionStep,
    choice_id: u32,
) -> Result<u32, QuestRuleError> {
    question
        .choices
        .iter()
        .enumerate()
        .find(|(choice_index, _)| {
            answer_choice_id(phase, step_index, *choice_index) == Some(choice_id)
        })
        .map(|(_, choice)| choice.id)
        .ok_or(QuestRuleError::InvalidChoice { choice_id })
}

pub(crate) fn question_dialogue_step(step_index: usize) -> Option<u32> {
    u32::try_from(step_index).ok()?.checked_add(1)
}

pub(crate) fn pending_start_decision_step(question: &QuestQuestionSequence) -> Option<u32> {
    question_dialogue_step(question.steps.len())
}

pub(crate) fn start_decision_entry_is_pending(
    entry: &PlayerQuest,
    quest: &QuestDefinition,
    question: &QuestQuestionSequence,
) -> bool {
    quest.dialogue.has_start_decision
        && pending_start_decision_step(question) == Some(entry.dialogue_step)
}

pub(crate) fn has_pending_start_decision(
    player: &PlayerState,
    quest: &QuestDefinition,
) -> bool {
    let Some(question) = &quest.dialogue.start_question else {
        return false;
    };
    player
        .quests
        .iter()
        .find(|entry| entry.quest_id == quest.id)
        .filter(|entry| QuestStatus::try_from(entry.status) == Ok(QuestStatus::Unspecified))
        .is_some_and(|entry| start_decision_entry_is_pending(entry, quest, question))
}

pub(crate) fn decline_start_decision(
    mut player: PlayerState,
    quest: &QuestDefinition,
) -> QuestSelection {
    let pending = player
        .quests
        .iter()
        .position(|entry| entry.quest_id == quest.id)
        .expect("validated pending start decision");
    if player.quests[pending].completed_at_unix_ms > 0 {
        let entry = &mut player.quests[pending];
        entry.status = QuestStatus::Completed as i32;
        entry.dialogue_step = 0;
        entry.completion_quiz_passed = false;
    } else {
        player.quests.remove(pending);
    }
    QuestSelection {
        player,
        pages: quest.dialogue.declined_pages.clone(),
        changed: true,
        npc_animation_action: None,
        next_interaction: None,
        next_quest_id: None,
    }
}

pub(crate) fn question_step_index(
    entry: &PlayerQuest,
    question: &QuestQuestionSequence,
) -> Option<usize> {
    entry
        .dialogue_step
        .checked_sub(1)
        .and_then(|index| usize::try_from(index).ok())
        .filter(|index| *index < question.steps.len())
}

pub(crate) fn require_completion_ready(
    player: &PlayerState,
    effects: &PlayerEffects,
    quest: &QuestDefinition,
    quest_definitions: &[&QuestDefinition],
    item_definitions: &[ItemDefinition],
    scripts: &QuestScriptCatalog,
    environment: QuestEnvironment,
) -> Result<(), QuestRuleError> {
    let entry = player
        .quests
        .iter()
        .find(|entry| entry.quest_id == quest.id)
        .filter(|entry| QuestStatus::try_from(entry.status) == Ok(QuestStatus::Started))
        .ok_or(QuestRuleError::NotActive { quest_id: quest.id })?;
    if quest_is_expired(entry, quest, environment.now_unix_ms) {
        return Err(QuestRuleError::Expired { quest_id: quest.id });
    }
    if !completion_readiness(
        player,
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
    Ok(())
}
