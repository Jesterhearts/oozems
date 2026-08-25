use super::*;

#[cfg(test)]
mod tests;

pub fn advance_automatic_quests<'a>(
    mut player: PlayerState,
    mut effects: PlayerEffects,
    quest_definitions: impl IntoIterator<Item = &'a QuestDefinition>,
    curve: &ExperienceCurve,
    item_definitions: &[ItemDefinition],
    consume_effects: &[ConsumeEffectDefinition],
    scripts: &QuestScriptCatalog,
    environment: QuestEnvironment,
) -> AutomaticQuestAdvance {
    let definitions = quest_definitions.into_iter().collect::<Vec<_>>();
    debug_assert!(definitions.windows(2).all(|pair| pair[0].id <= pair[1].id));

    let expired_quest_ids = expire_timed_quests(&mut player, &definitions, environment.now_unix_ms);
    let expired = expired_quest_ids.iter().copied().collect::<BTreeSet<_>>();
    let transition_limit = definitions.len().saturating_mul(2);
    let mut applied = BTreeSet::new();
    let mut started_quest_ids = Vec::new();
    let mut completed_quest_ids = Vec::new();
    let mut failures = BTreeMap::new();

    while applied.len() < transition_limit {
        let mut pass_changed = false;
        for quest in &definitions {
            let quest = *quest;
            match progress(&player, quest.id) {
                QuestProgress::NotStarted | QuestProgress::Completed
                    if !expired.contains(&quest.id)
                        && !applied.contains(&(quest.id, AutomaticQuestTransition::Start)) =>
                {
                    if !starts_automatically(quest) {
                        continue;
                    }
                    if !is_available(
                        &player,
                        &effects,
                        quest,
                        item_definitions,
                        scripts,
                        environment,
                    ) {
                        continue;
                    }
                    match start_quest(
                        player.clone(),
                        &mut effects,
                        quest,
                        false,
                        curve,
                        item_definitions,
                        consume_effects,
                        scripts,
                        environment,
                    ) {
                        Ok(selection) => {
                            player = selection.player;
                            applied.insert((quest.id, AutomaticQuestTransition::Start));
                            failures.remove(&(quest.id, AutomaticQuestTransition::Start));
                            started_quest_ids.push(quest.id);
                            pass_changed = true;
                        }
                        Err(error) => {
                            failures.insert(
                                (quest.id, AutomaticQuestTransition::Start),
                                AutomaticQuestFailure {
                                    quest_id: quest.id,
                                    transition: AutomaticQuestTransition::Start,
                                    message: error.to_string(),
                                },
                            );
                        }
                    }
                }
                QuestProgress::Started
                    if !applied.contains(&(quest.id, AutomaticQuestTransition::Completion)) =>
                {
                    let Some(checks) = automatic_completion_checks(quest) else {
                        continue;
                    };
                    let ready = match checks {
                        CompletionChecks::Normal => {
                            completion_readiness(
                                &player,
                                &effects,
                                quest,
                                &definitions,
                                item_definitions,
                                scripts,
                                environment,
                            )
                            .ready
                        }
                        CompletionChecks::Automatic => {
                            completion_window_allows(quest, environment.now_unix_ms)
                                && requirements_have_equipped_items(
                                    &player,
                                    &quest.completion.equipped_items,
                                )
                                && requirements_have_effects(&effects, &quest.completion.effects)
                                && required_morph_is_active(
                                    &effects,
                                    quest.completion.required_morph_id,
                                )
                                && requirements_have_monster_book(
                                    &player,
                                    &quest.completion.monster_book,
                                )
                                && script_conditions_pass(
                                    scripts,
                                    &player,
                                    quest,
                                    QuestScriptPhase::Completion,
                                    item_definitions,
                                )
                        }
                    };
                    if !ready {
                        continue;
                    }
                    match complete_quest(
                        player.clone(),
                        &mut effects,
                        quest,
                        checks,
                        None,
                        curve,
                        item_definitions,
                        consume_effects,
                        scripts,
                        &definitions,
                        environment,
                    ) {
                        Ok(selection) => {
                            player = selection.player;
                            applied.insert((quest.id, AutomaticQuestTransition::Completion));
                            failures.remove(&(quest.id, AutomaticQuestTransition::Completion));
                            completed_quest_ids.push(quest.id);
                            pass_changed = true;
                        }
                        Err(error) => {
                            failures.insert(
                                (quest.id, AutomaticQuestTransition::Completion),
                                AutomaticQuestFailure {
                                    quest_id: quest.id,
                                    transition: AutomaticQuestTransition::Completion,
                                    message: error.to_string(),
                                },
                            );
                        }
                    }
                }
                _ => {}
            }
            if applied.len() >= transition_limit {
                break;
            }
        }
        if !pass_changed {
            break;
        }
    }

    AutomaticQuestAdvance {
        changed: !expired_quest_ids.is_empty() || !applied.is_empty(),
        player,
        effects,
        started_quest_ids,
        completed_quest_ids,
        expired_quest_ids,
        failures: failures.into_values().collect(),
    }
}

pub(crate) fn expire_timed_quests(
    player: &mut PlayerState,
    definitions: &[&QuestDefinition],
    now_unix_ms: u64,
) -> Vec<u32> {
    let definitions = definitions
        .iter()
        .map(|quest| (quest.id, *quest))
        .collect::<BTreeMap<_, _>>();
    let mut expired = Vec::new();
    player.quests.retain(|entry| {
        let should_expire = QuestStatus::try_from(entry.status) == Ok(QuestStatus::Started)
            && definitions
                .get(&entry.quest_id)
                .is_some_and(|quest| quest_is_expired(entry, quest, now_unix_ms));
        if should_expire {
            expired.push(entry.quest_id);
        }
        !should_expire
    });
    expired.sort_unstable();
    for quest_id in &expired {
        crate::quest_records::clear(player, *quest_id);
    }
    expired
}

pub(crate) fn starts_automatically(quest: &QuestDefinition) -> bool {
    if quest.dialogue.start_question.is_some() {
        return false;
    }
    quest.info.auto_accept || quest.start.normal_auto_start || quest.info.auto_start
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CompletionChecks {
    Normal,
    Automatic,
}

pub(crate) fn automatic_completion_checks(quest: &QuestDefinition) -> Option<CompletionChecks> {
    if quest.dialogue.question.is_some() {
        return None;
    }
    if quest.info.auto_pre_complete {
        Some(CompletionChecks::Automatic)
    } else if quest.info.auto_complete {
        Some(CompletionChecks::Normal)
    } else {
        None
    }
}
