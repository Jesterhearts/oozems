use super::*;

#[cfg(test)]
mod tests;

pub fn completion_readiness(
    player: &PlayerState,
    effects: &PlayerEffects,
    quest: &QuestDefinition,
    quest_definitions: &[&QuestDefinition],
    item_definitions: &[ItemDefinition],
    scripts: &QuestScriptCatalog,
    environment: QuestEnvironment,
) -> QuestReadiness {
    let active = player
        .quests
        .iter()
        .find(|entry| entry.quest_id == quest.id)
        .filter(|entry| QuestStatus::try_from(entry.status) == Ok(QuestStatus::Started));
    let mut objectives = Vec::new();
    for requirement in &quest.completion.effects {
        let current = effects.contains_item(requirement.item_id);
        objectives.push(QuestObjectiveProgress {
            kind: QuestObjectiveKind::Buff,
            label: if requirement.active {
                format!("Effect item {} is active", requirement.item_id)
            } else {
                format!("Effect item {} is not active", requirement.item_id)
            },
            current: u64::from(current),
            required: u64::from(requirement.active),
            complete: current == requirement.active,
        });
    }
    if let Some(morph_id) = quest.completion.required_morph_id {
        let current = effects.projected().morph_id;
        objectives.push(QuestObjectiveProgress {
            kind: QuestObjectiveKind::Buff,
            label: format!("Active morph {}", morph_id),
            current: u64::from(current.unwrap_or_default()),
            required: u64::from(morph_id.get()),
            complete: current == Some(morph_id.get()),
        });
    }
    if let Some(start) = &quest.completion.available_from {
        let complete = environment.now_unix_ms >= start.unix_ms;
        objectives.push(QuestObjectiveProgress {
            kind: QuestObjectiveKind::Availability,
            label: format!("Completion available from {}", start.source),
            current: environment.now_unix_ms,
            required: start.unix_ms,
            complete,
        });
    }
    if let Some(end) = &quest.completion.available_until {
        let complete = environment.now_unix_ms <= end.unix_ms;
        objectives.push(QuestObjectiveProgress {
            kind: QuestObjectiveKind::Availability,
            label: format!("Completion available through {}", end.source),
            current: environment.now_unix_ms,
            required: end.unix_ms,
            complete,
        });
    }
    if let Some(required) = quest.completion.minimum_mesos {
        objectives.push(QuestObjectiveProgress {
            kind: QuestObjectiveKind::Mesos,
            label: "Mesos".to_owned(),
            current: player.mesos,
            required,
            complete: player.mesos >= required,
        });
    }
    if let Some(required) = quest.completion.minimum_completed_quest_count {
        let current = eligible_completed_quest_count(player, quest_definitions);
        objectives.push(QuestObjectiveProgress {
            kind: QuestObjectiveKind::CompletedQuests,
            label: "Eligible completed quests".to_owned(),
            current,
            required: u64::from(required),
            complete: current >= u64::from(required),
        });
    }
    for requirement in &quest.completion.items {
        let current = player.inventory.as_ref().and_then(|inventory| {
            quest_item_quantity(inventory, item_definitions, requirement.item_id).ok()
        });
        let name = item_definitions
            .iter()
            .find(|definition| definition.item_id == requirement.item_id)
            .map(|definition| definition.name.as_str())
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| format!("Item {}", requirement.item_id));
        let (label, required, complete) = match requirement.condition {
            QuestItemCondition::Absent => (format!("Do not possess {name}"), 0, current == Some(0)),
            QuestItemCondition::AtLeast(count) => {
                let required = u64::from(count.get());
                (
                    name,
                    required,
                    current.is_some_and(|current| current >= required),
                )
            }
        };
        objectives.push(QuestObjectiveProgress {
            kind: QuestObjectiveKind::Item,
            label,
            current: current.unwrap_or_default(),
            required,
            complete,
        });
    }
    for requirement in &quest.completion.monster_book.cards {
        let current = u64::from(crate::monster_book::count(
            &player.monster_book_cards,
            requirement.card_item_id,
        ));
        if let Some(required) = requirement.minimum_count {
            objectives.push(QuestObjectiveProgress {
                kind: QuestObjectiveKind::MonsterBookCardMinimum,
                label: format!("Monster Book card {}", requirement.card_item_id),
                current,
                required: u64::from(required),
                complete: current >= u64::from(required),
            });
        }
        if let Some(required) = requirement.maximum_count {
            objectives.push(QuestObjectiveProgress {
                kind: QuestObjectiveKind::MonsterBookCardMaximum,
                label: format!("At most Monster Book card {}", requirement.card_item_id),
                current,
                required: u64::from(required),
                complete: current <= u64::from(required),
            });
        }
    }
    let unique_card_count = player.monster_book_cards.len() as u64;
    if let Some(required) = quest.completion.monster_book.minimum_unique_cards {
        objectives.push(QuestObjectiveProgress {
            kind: QuestObjectiveKind::MonsterBookUniqueMinimum,
            label: "Unique Monster Book cards".to_owned(),
            current: unique_card_count,
            required: u64::from(required),
            complete: unique_card_count >= u64::from(required),
        });
    }
    if let Some(required) = quest.completion.monster_book.maximum_unique_cards {
        objectives.push(QuestObjectiveProgress {
            kind: QuestObjectiveKind::MonsterBookUniqueMaximum,
            label: "Maximum unique Monster Book cards".to_owned(),
            current: unique_card_count,
            required: u64::from(required),
            complete: unique_card_count <= u64::from(required),
        });
    }
    let equipped_item_ids = player
        .inventory
        .as_ref()
        .map(|inventory| {
            inventory
                .equipment
                .iter()
                .map(|item| item.item_id)
                .collect::<BTreeSet<_>>()
        })
        .unwrap_or_default();
    for item_id in &quest.completion.equipped_items.all_of {
        let complete = equipped_item_ids.contains(item_id);
        objectives.push(QuestObjectiveProgress {
            kind: QuestObjectiveKind::Equipment,
            label: format!("Equip {}", item_name(item_definitions, *item_id)),
            current: u64::from(complete),
            required: 1,
            complete,
        });
    }
    if !quest.completion.equipped_items.any_of.is_empty() {
        let complete = quest
            .completion
            .equipped_items
            .any_of
            .iter()
            .any(|item_id| equipped_item_ids.contains(item_id));
        let names = quest
            .completion
            .equipped_items
            .any_of
            .iter()
            .map(|item_id| item_name(item_definitions, *item_id))
            .collect::<Vec<_>>()
            .join(", ");
        objectives.push(QuestObjectiveProgress {
            kind: QuestObjectiveKind::Equipment,
            label: format!("Equip one of: {names}"),
            current: u64::from(complete),
            required: 1,
            complete,
        });
    }
    for objective in &quest.completion.mobs {
        let current = active
            .and_then(|entry| {
                entry
                    .mob_progress
                    .iter()
                    .find(|progress| progress.mob_id == objective.mob_id)
            })
            .map_or(0, |progress| progress.count.min(objective.count));
        let label = quest.info.selected_skill.as_ref().map_or_else(
            || format!("Mob {}", objective.mob_id),
            |skill| match &skill.name {
                Some(name) => format!(
                    "Mob {} using {} (skill {})",
                    objective.mob_id,
                    name,
                    skill.id.get()
                ),
                None => format!("Mob {} using skill {}", objective.mob_id, skill.id.get()),
            },
        );
        objectives.push(QuestObjectiveProgress {
            kind: QuestObjectiveKind::Mob,
            label,
            current: u64::from(current),
            required: u64::from(objective.count),
            complete: current >= objective.count,
        });
    }
    for requirement in &quest.completion.quests {
        let current = progress(player, requirement.quest_id);
        objectives.push(QuestObjectiveProgress {
            kind: QuestObjectiveKind::Quest,
            label: format!(
                "Quest {}: {}",
                requirement.quest_id,
                state_label(requirement.state)
            ),
            current: u64::from(current == required_progress(requirement.state)),
            required: 1,
            complete: current == required_progress(requirement.state),
        });
    }
    for requirement in &quest.completion.record_conditions {
        let complete = record_condition_matches(player, requirement);
        let current = crate::quest_records::get(player, requirement.quest_id, requirement.index);
        objectives.push(QuestObjectiveProgress {
            kind: QuestObjectiveKind::Record,
            label: match (complete, current) {
                (true, _) => format!(
                    "Quest record {}[{}] progress",
                    requirement.quest_id, requirement.index
                ),
                (false, Some(_)) => format!(
                    "Quest record {}[{}] does not match the required progress",
                    requirement.quest_id, requirement.index
                ),
                (false, None) => format!(
                    "Quest record {}[{}] is missing",
                    requirement.quest_id, requirement.index
                ),
            },
            current: u64::from(complete),
            required: 1,
            complete,
        });
    }
    if let Some(required_level) = quest.completion.required_level {
        objectives.push(QuestObjectiveProgress {
            kind: QuestObjectiveKind::Level,
            label: format!("Level {required_level}"),
            current: u64::from(player.level),
            required: u64::from(required_level),
            complete: player.level >= required_level,
        });
    }
    if let Some(script) = &quest.completion.script {
        let resolution = resolve_quest_script(
            scripts,
            quest,
            QuestScriptPhase::Completion,
            player,
            item_definitions,
        );
        let (label, complete) = match resolution {
            QuestScriptResolution::Missing { .. } => {
                (format!("Missing quest script: {script}"), false)
            }
            QuestScriptResolution::ConditionsNotMet { .. } => {
                (format!("Script conditions not met: {script}"), false)
            }
            QuestScriptResolution::Ready(_) => (format!("Script: {script}"), true),
            QuestScriptResolution::NotReferenced => {
                (format!("Missing quest script: {script}"), false)
            }
        };
        objectives.push(QuestObjectiveProgress {
            kind: QuestObjectiveKind::Script,
            label,
            current: u64::from(complete),
            required: 1,
            complete,
        });
    }
    QuestReadiness {
        ready: active.is_some() && objectives.iter().all(|objective| objective.complete),
        objectives,
    }
}

pub fn objective_progress_text(
    player: &PlayerState,
    effects: &PlayerEffects,
    quest: &QuestDefinition,
    quest_definitions: &[&QuestDefinition],
    item_definitions: &[ItemDefinition],
    scripts: &QuestScriptCatalog,
    environment: QuestEnvironment,
) -> Vec<String> {
    completion_readiness(
        player,
        effects,
        quest,
        quest_definitions,
        item_definitions,
        scripts,
        environment,
    )
    .objectives
    .into_iter()
    .map(|objective| match objective.kind {
        QuestObjectiveKind::Script
        | QuestObjectiveKind::Record
        | QuestObjectiveKind::Availability => objective.label,
        _ => format!(
            "{}: {}/{}",
            objective.label, objective.current, objective.required
        ),
    })
    .collect()
}

pub fn incomplete_dialogue_pages(
    player: &PlayerState,
    effects: &PlayerEffects,
    quest: &QuestDefinition,
    quest_definitions: &[&QuestDefinition],
    item_definitions: &[ItemDefinition],
    scripts: &QuestScriptCatalog,
    environment: QuestEnvironment,
) -> Vec<String> {
    let readiness = completion_readiness(
        player,
        effects,
        quest,
        quest_definitions,
        item_definitions,
        scripts,
        environment,
    );
    let branch = readiness
        .objectives
        .iter()
        .find(|objective| !objective.complete)
        .map(|objective| match objective.kind {
            QuestObjectiveKind::Item => &quest.dialogue.completion.incomplete.item_pages,
            QuestObjectiveKind::Mob => &quest.dialogue.completion.incomplete.mob_pages,
            QuestObjectiveKind::Quest => &quest.dialogue.completion.incomplete.quest_pages,
            QuestObjectiveKind::Level
            | QuestObjectiveKind::Mesos
            | QuestObjectiveKind::CompletedQuests
            | QuestObjectiveKind::Equipment
            | QuestObjectiveKind::Availability
            | QuestObjectiveKind::Script
            | QuestObjectiveKind::Record => &quest.dialogue.completion.incomplete.default_pages,
            QuestObjectiveKind::Buff
            | QuestObjectiveKind::MonsterBookCardMinimum
            | QuestObjectiveKind::MonsterBookCardMaximum
            | QuestObjectiveKind::MonsterBookUniqueMinimum
            | QuestObjectiveKind::MonsterBookUniqueMaximum => {
                &quest.dialogue.completion.incomplete.default_pages
            }
        })
        .filter(|pages| !pages.is_empty())
        .cloned()
        .unwrap_or_else(|| quest.dialogue.completion.pages.clone());
    let mut pages = branch;
    if let QuestScriptResolution::ConditionsNotMet {
        incomplete_pages, ..
    } = resolve_quest_script(
        scripts,
        quest,
        QuestScriptPhase::Completion,
        player,
        item_definitions,
    ) {
        pages.extend(incomplete_pages);
    }
    let progress = readiness
        .objectives
        .iter()
        .zip(objective_progress_text(
            player,
            effects,
            quest,
            quest_definitions,
            item_definitions,
            scripts,
            environment,
        ))
        .filter_map(|(objective, text)| (!objective.complete).then_some(text))
        .collect::<Vec<_>>();
    if !progress.is_empty() {
        pages.push(progress.join("\n"));
    }
    pages
}

pub fn record_mob_kills<'a>(
    mut player: PlayerState,
    mob_kills: &[(u32, Option<u32>)],
    quest_definitions: impl IntoIterator<Item = &'a QuestDefinition>,
) -> MobKillResult {
    if mob_kills.is_empty() {
        return MobKillResult {
            player,
            changed_quest_ids: Vec::new(),
        };
    }
    let definitions = quest_definitions
        .into_iter()
        .map(|quest| (quest.id, quest))
        .collect::<BTreeMap<_, _>>();
    let mut changed_quest_ids = BTreeSet::new();
    for entry in &mut player.quests {
        if QuestStatus::try_from(entry.status) != Ok(QuestStatus::Started) {
            continue;
        }
        let Some(quest) = definitions.get(&entry.quest_id) else {
            continue;
        };
        for objective in &quest.completion.mobs {
            let increment = mob_kills
                .iter()
                .filter(|(mob_id, source_skill_id)| {
                    *mob_id == objective.mob_id
                        && quest
                            .info
                            .selected_skill
                            .as_ref()
                            .is_none_or(|required| *source_skill_id == Some(required.id.get()))
                })
                .fold(0_u32, |count, _| count.saturating_add(1));
            if increment == 0 {
                continue;
            }
            let index = entry
                .mob_progress
                .iter()
                .position(|progress| progress.mob_id == objective.mob_id);
            let old = index
                .map(|index| entry.mob_progress[index].count)
                .unwrap_or_default();
            let next = old.saturating_add(increment).min(objective.count);
            if next == old {
                continue;
            }
            if let Some(index) = index {
                entry.mob_progress[index].count = next;
            } else {
                entry.mob_progress.push(QuestMobProgress {
                    mob_id: objective.mob_id,
                    count: next,
                });
            }
            changed_quest_ids.insert(quest.id);
        }
        entry.mob_progress.sort_by_key(|progress| progress.mob_id);
    }
    MobKillResult {
        player,
        changed_quest_ids: changed_quest_ids.into_iter().collect(),
    }
}
