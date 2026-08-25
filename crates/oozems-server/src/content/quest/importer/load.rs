use super::*;

#[cfg(test)]
mod tests;

pub(crate) fn load_definition(
    quest_id: u32,
    checks: &WzNodeArc,
    actions: &WzNodeArc,
    dialogue_root: &WzNodeArc,
    info_root: &WzNodeArc,
    item_ids: &BTreeSet<u32>,
    equipment_item_ids: &BTreeSet<u32>,
    consume_effect_ids: &BTreeSet<u32>,
    monster_book_card_ids: &BTreeSet<u32>,
    morph_ids: &BTreeSet<u32>,
    skill_ids: &BTreeSet<u32>,
    skill_names: &BTreeMap<u32, String>,
    archive_quest_ids: &BTreeSet<u32>,
) -> Result<QuestDefinition, QuestContentError> {
    let key = quest_id.to_string();
    let check = required_child(checks, &key, quest_id)?;
    let info_node = required_child(info_root, &key, quest_id)?;
    let say = wz::child(dialogue_root, &key)?;
    let action = audited_action_root(
        quest_id,
        checks,
        actions,
        dialogue_root,
        info_root,
        &check,
        say.as_ref(),
        &info_node,
    )?;
    let retained_check_fields = validate_check_phase_tree(
        quest_id,
        checks,
        &check,
        item_ids,
        equipment_item_ids,
        consume_effect_ids,
        monster_book_card_ids,
        morph_ids,
    )?;
    validate_children(quest_id, &action, &["0", "1"], "action phase")?;
    if let Some(say) = &say {
        validate_children(quest_id, say, &["0", "1"], "dialogue phase")?;
    }
    let start_check = required_child(&check, "0", quest_id)?;
    let completion_check = required_child(&check, "1", quest_id)?;
    let mut info = read_info(quest_id, &info_node, info_root, skill_ids, skill_names)?;
    let start = read_start_requirements(
        quest_id,
        &start_check,
        &info,
        item_ids,
        equipment_item_ids,
        consume_effect_ids,
        monster_book_card_ids,
        morph_ids,
    )?;
    let completion = read_completion_requirements(
        quest_id,
        &completion_check,
        &info,
        item_ids,
        equipment_item_ids,
        consume_effect_ids,
        monster_book_card_ids,
        morph_ids,
    )?;
    let audited_corrections =
        audited_action_corrections(quest_id, &action, &completion_check, say.as_ref())?;
    let (start_actions, mut retained_action_fields) = read_action_phase_with_corrections(
        quest_id,
        &action,
        "0",
        item_ids,
        equipment_item_ids,
        consume_effect_ids,
        skill_ids,
        archive_quest_ids,
        Some(&start_check),
        &audited_corrections,
    )?;
    let (completion_actions, completion_action_fields) = read_action_phase_with_corrections(
        quest_id,
        &action,
        "1",
        item_ids,
        equipment_item_ids,
        consume_effect_ids,
        skill_ids,
        archive_quest_ids,
        Some(&completion_check),
        &audited_corrections,
    )?;
    validate_audited_4960_parsed_actions(quest_id, &start_actions, &completion_actions)?;
    validate_npc_animation_transitions(
        quest_id,
        &start,
        &completion,
        &start_actions,
        &completion_actions,
        &info,
    )?;
    retained_action_fields.extend(retained_check_fields);
    retained_action_fields.extend(completion_action_fields);
    info.retained_metadata_fields.extend(retained_action_fields);
    info.retained_metadata_fields.sort();
    info.retained_metadata_fields.dedup();
    let mut dialogue = dialogue::read_dialogue(quest_id, say.as_ref(), &action)?;
    validate_start_question_reachability(quest_id, &start, &dialogue)?;
    validate_selectable_reward_flow(quest_id, &start_actions, &completion_actions, &dialogue)?;
    let restorable_items = validate_lost_item_restoration_flow(
        quest_id,
        &completion,
        &start_actions,
        &completion_actions,
        &dialogue,
    )?;
    if let Some(lost) = dialogue.completion.lost.as_mut() {
        lost.items = restorable_items;
    }
    let name = required_nonempty_string(&info_node, "name", quest_id)?;

    Ok(QuestDefinition {
        id: quest_id,
        name,
        start,
        completion,
        start_actions,
        completion_actions,
        dialogue,
        info,
    })
}
pub(crate) fn script_reference_names(
    checks: &WzNodeArc
) -> Result<BTreeSet<String>, QuestContentError> {
    let mut names = BTreeSet::new();
    for quest in wz::sorted_children(checks)? {
        if wz::node_name(&quest)?.parse::<u32>().is_err() {
            continue;
        }
        for (phase_name, field_name) in [("0", "startscript"), ("1", "endscript")] {
            let Some(phase) = wz::child(&quest, phase_name)? else {
                continue;
            };
            let Some(name) = optional_string(&phase, field_name)? else {
                continue;
            };
            let name = name.trim();
            if !name.is_empty() {
                names.insert(name.to_owned());
            }
        }
    }
    Ok(names)
}

pub(crate) fn item_reference_ids(
    quest_id: u32,
    checks: &WzNodeArc,
    actions: &WzNodeArc,
) -> Result<BTreeSet<u32>, QuestContentError> {
    let mut item_ids = BTreeSet::new();
    let key = quest_id.to_string();
    for (root, is_action) in [(checks, false), (actions, true)] {
        let Some(quest) = wz::child(root, &key)? else {
            continue;
        };
        for phase_name in ["0", "1"] {
            let Some(phase) = wz::child(&quest, phase_name)? else {
                continue;
            };
            if !is_action {
                if let Some(cards) = wz::child(&phase, "mbcard")? {
                    for entry in wz::sorted_children(&cards)? {
                        if let Some(card_item_id) = optional_i64(&entry, "id", quest_id)?
                            .and_then(|item_id| u32::try_from(item_id).ok())
                        {
                            item_ids.insert(card_item_id);
                        }
                    }
                }
                for field_name in ["equipAllNeed", "equipSelectNeed"] {
                    let Some(values) = wz::child(&phase, field_name)? else {
                        continue;
                    };
                    for value in wz::sorted_children(&values)? {
                        if let Some(item_id) =
                            scalar_i64(&value)?.and_then(|item_id| u32::try_from(item_id).ok())
                        {
                            item_ids.insert(item_id);
                        }
                    }
                }
                for field_name in ["buff", "exceptbuff"] {
                    let Some(value) = wz::child(&phase, field_name)? else {
                        continue;
                    };
                    if let Some(item_id) = raw_scalar_string(&value)?
                        .and_then(|value| value.parse::<u32>().ok())
                        .filter(|item_id| *item_id > 0)
                    {
                        item_ids.insert(item_id);
                    }
                }
            } else if let Some(item_id) = optional_i64(&phase, "buffItemID", quest_id)?
                .and_then(|item_id| u32::try_from(item_id).ok())
                .filter(|item_id| *item_id > 0)
            {
                item_ids.insert(item_id);
            }
            let Some(items) = wz::child(&phase, "item")? else {
                continue;
            };
            for entry in wz::sorted_children(&items)? {
                if is_action && optional_i64(&entry, "count", quest_id)? == Some(0) {
                    continue;
                }
                let Some(item_id) = optional_i64(&entry, "id", quest_id)? else {
                    continue;
                };
                if let Ok(item_id) = u32::try_from(item_id) {
                    item_ids.insert(item_id);
                }
            }
        }
    }
    Ok(item_ids)
}
