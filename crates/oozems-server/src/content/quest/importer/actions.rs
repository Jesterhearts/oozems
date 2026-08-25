use super::*;

#[cfg(test)]
mod tests;

const ACTION_FIELDS: &[&str] = &[
    "item",
    "money",
    "exp",
    "pop",
    "nextQuest",
    "quest",
    "skill",
    "npcAct",
    "buffItemID",
];
const MAP_PROTECTION_EFFECT_ID: u32 = 2_022_187;
#[cfg(test)]
pub(super) fn read_action_phase(
    quest_id: u32,
    action: &WzNodeArc,
    phase: &str,
    item_ids: &BTreeSet<u32>,
    equipment_item_ids: &BTreeSet<u32>,
    consume_effect_ids: &BTreeSet<u32>,
    skill_ids: &BTreeSet<u32>,
    archive_quest_ids: &BTreeSet<u32>,
    authoritative_check: Option<&WzNodeArc>,
) -> Result<(QuestActions, Vec<String>), QuestContentError> {
    read_action_phase_with_corrections(
        quest_id,
        action,
        phase,
        item_ids,
        equipment_item_ids,
        consume_effect_ids,
        skill_ids,
        archive_quest_ids,
        authoritative_check,
        &AuditedActionCorrections::default(),
    )
}

pub(crate) fn read_action_phase_with_corrections(
    quest_id: u32,
    action: &WzNodeArc,
    phase: &str,
    item_ids: &BTreeSet<u32>,
    equipment_item_ids: &BTreeSet<u32>,
    consume_effect_ids: &BTreeSet<u32>,
    skill_ids: &BTreeSet<u32>,
    archive_quest_ids: &BTreeSet<u32>,
    authoritative_check: Option<&WzNodeArc>,
    audited_corrections: &AuditedActionCorrections,
) -> Result<(QuestActions, Vec<String>), QuestContentError> {
    let Some(node) = wz::child(action, phase)? else {
        return Ok((QuestActions::default(), Vec::new()));
    };
    let mut retained_fields =
        validate_action_phase_fields(quest_id, phase, &node, authoritative_check)?;
    let item_actions = read_action_items_with_corrections(
        quest_id,
        phase,
        &node,
        item_ids,
        equipment_item_ids,
        audited_corrections,
    )?;
    retained_fields.extend(item_actions.retained_fields.iter().cloned());
    let experience = optional_i64(&node, "exp", quest_id)?
        .map(|value| {
            u64::try_from(value)
                .map_err(|_| invalid(quest_id, format!("action {phase} EXP is negative")))
        })
        .transpose()?
        .unwrap_or_default();
    let fame = optional_i64(&node, "pop", quest_id)?
        .map(|value| {
            i32::try_from(value)
                .map_err(|_| invalid(quest_id, format!("action {phase} fame is out of range")))
        })
        .transpose()?
        .unwrap_or_default();
    let record_writes = read_action_record_writes(quest_id, phase, &node)?;
    let skill_changes = read_action_skills(quest_id, phase, &node, skill_ids)?;
    Ok((
        QuestActions {
            fixed_items: item_actions.fixed,
            conditional_items: item_actions.conditional,
            weighted_items: item_actions.weighted,
            selectable_items: item_actions.selectable,
            money: optional_i64(&node, "money", quest_id)?.unwrap_or_default(),
            experience,
            fame,
            next_quest_id: optional_u32(&node, "nextQuest", quest_id)?,
            quest_state_actions: read_quest_state_actions(
                quest_id,
                phase,
                &node,
                archive_quest_ids,
            )?,
            record_writes,
            skill_changes,
            buff_item_ids: read_buff_item_action(quest_id, phase, &node, consume_effect_ids)?,
            presentation_npc_id: if phase == "0" {
                optional_positive_u32(&node, "npc", quest_id)?
            } else {
                None
            },
            npc_animation_action: read_npc_animation_action(quest_id, phase, &node)?,
        },
        retained_fields,
    ))
}

pub(crate) fn read_buff_item_action(
    quest_id: u32,
    phase: &str,
    node: &WzNodeArc,
    consume_effect_ids: &BTreeSet<u32>,
) -> Result<Vec<u32>, QuestContentError> {
    let Some(value) = optional_i64(node, "buffItemID", quest_id)? else {
        return Ok(Vec::new());
    };
    let item_id = u32::try_from(value)
        .ok()
        .filter(|item_id| *item_id > 0)
        .ok_or_else(|| {
            invalid(
                quest_id,
                format!("action {phase} buffItemID must be a positive integer"),
            )
        })?;
    if item_id == MAP_PROTECTION_EFFECT_ID {
        return Err(unsupported(
            quest_id,
            "map protection item effect",
            format!(
                "action {phase} applies item {item_id} thaw=-6, which requires a map hazard \
                 subsystem"
            ),
        ));
    }
    if !consume_effect_ids.contains(&item_id) {
        return Err(unsupported(
            quest_id,
            "unsupported consume item effect",
            format!("action {phase} applies consume item {item_id} without supported semantics"),
        ));
    }
    Ok(vec![item_id])
}

pub(crate) fn read_npc_animation_action(
    quest_id: u32,
    phase: &str,
    node: &WzNodeArc,
) -> Result<Option<String>, QuestContentError> {
    let Some(value) = wz::child(node, "npcAct")? else {
        return Ok(None);
    };
    let action_name = raw_scalar_string(&value)?
        .ok_or_else(|| invalid(quest_id, format!("action {phase} npcAct is not a string")))?;
    if action_name.is_empty() {
        return Err(invalid(quest_id, format!("action {phase} npcAct is empty")));
    }
    Ok(Some(action_name))
}

pub(crate) fn validate_npc_animation_transitions(
    quest_id: u32,
    start: &QuestStartRequirements,
    completion: &QuestCompletionRequirements,
    start_actions: &QuestActions,
    completion_actions: &QuestActions,
    info: &QuestInfo,
) -> Result<(), QuestContentError> {
    if start_actions.npc_animation_action.is_some() {
        if start.npc_id.is_none() {
            return Err(unsupported(
                quest_id,
                "NPC animation action",
                "start npcAct has no authoritative interacting NPC",
            ));
        }
        if start.normal_auto_start || info.auto_start || info.auto_accept {
            return Err(unsupported(
                quest_id,
                "automatic NPC animation action",
                "start npcAct cannot target an NPC spawn during an automatic transition",
            ));
        }
    }
    if completion_actions.npc_animation_action.is_some() {
        if completion.npc_id.is_none() {
            return Err(unsupported(
                quest_id,
                "NPC animation action",
                "completion npcAct has no authoritative interacting NPC",
            ));
        }
        if info.auto_complete || info.auto_pre_complete {
            return Err(unsupported(
                quest_id,
                "automatic NPC animation action",
                "completion npcAct cannot target an NPC spawn during an automatic transition",
            ));
        }
    }
    Ok(())
}

pub(crate) fn validate_start_question_reachability(
    quest_id: u32,
    start: &QuestStartRequirements,
    dialogue: &QuestDialogue,
) -> Result<(), QuestContentError> {
    if dialogue.start_question.is_some() && start.npc_id.is_none() {
        return Err(unsupported(
            quest_id,
            "unreachable start question",
            "a typed start question requires an authoritative interacting NPC",
        ));
    }
    Ok(())
}

pub(crate) fn read_quest_state_actions(
    quest_id: u32,
    phase: &str,
    node: &WzNodeArc,
    archive_quest_ids: &BTreeSet<u32>,
) -> Result<Vec<QuestStateAction>, QuestContentError> {
    let Some(quests) = wz::child(node, "quest")? else {
        return Ok(Vec::new());
    };
    let context = format!("action {phase} quest state entries");
    require_property(quest_id, &quests, &context)?;
    let mut indexed_actions = Vec::new();
    let mut seen = BTreeSet::new();
    for entry in wz::sorted_children(&quests)? {
        let name = wz::node_name(&entry)?;
        let index = parse_decimal_name(quest_id, &name, "quest state action entry")?;
        require_property(
            quest_id,
            &entry,
            &format!("action {phase} quest state entry {name}"),
        )?;
        validate_children(quest_id, &entry, &["id", "state"], "quest state action")?;
        let target_quest_id = required_positive_u32(&entry, "id", quest_id)?;
        if target_quest_id == quest_id {
            return Err(invalid(
                quest_id,
                format!(
                    "action {phase} quest state entry {name} targets its own transitioning quest"
                ),
            ));
        }
        if !archive_quest_ids.contains(&target_quest_id) {
            return Err(invalid(
                quest_id,
                format!(
                    "action {phase} quest state entry {name} targets unknown quest \
                     {target_quest_id}"
                ),
            ));
        }
        if !seen.insert(target_quest_id) {
            return Err(invalid(
                quest_id,
                format!(
                    "action {phase} quest state target {target_quest_id} appears more than once"
                ),
            ));
        }
        let state = match required_i64(&entry, "state", quest_id)? {
            0 => QuestStateActionState::NotStarted,
            1 => QuestStateActionState::Started,
            2 => QuestStateActionState::Completed,
            state => {
                return Err(invalid(
                    quest_id,
                    format!(
                        "action {phase} quest state target {target_quest_id} has invalid state \
                         {state}"
                    ),
                ));
            }
        };
        indexed_actions.push((
            index,
            QuestStateAction {
                quest_id: target_quest_id,
                state,
            },
        ));
    }
    let mut indexes = indexed_actions
        .iter()
        .map(|(index, _)| *index)
        .collect::<Vec<_>>();
    require_entries(quest_id, &indexes, &context)?;
    validate_contiguous_indexes(quest_id, &mut indexes, &context)?;
    indexed_actions.sort_by_key(|(index, _)| *index);
    Ok(indexed_actions
        .into_iter()
        .map(|(_, action)| action)
        .collect())
}

pub(crate) fn read_action_skills(
    quest_id: u32,
    phase: &str,
    node: &WzNodeArc,
    skill_ids: &BTreeSet<u32>,
) -> Result<Vec<QuestSkillChange>, QuestContentError> {
    let Some(skills) = wz::child(node, "skill")? else {
        return Ok(Vec::new());
    };
    require_property(quest_id, &skills, &format!("action {phase} skills"))?;
    let mut indexes = Vec::new();
    let mut seen_skill_ids = BTreeSet::new();
    let mut changes = Vec::new();
    for entry in wz::sorted_children(&skills)? {
        let name = wz::node_name(&entry)?;
        indexes.push(parse_decimal_name(quest_id, &name, "action skill entry")?);
        require_property(
            quest_id,
            &entry,
            &format!("action {phase} skill entry {name}"),
        )?;
        let acquire = optional_i64(&entry, "acquire", quest_id)?;
        let skill_id = required_positive_u32(&entry, "id", quest_id)?;
        if !seen_skill_ids.insert(skill_id) {
            return Err(invalid(
                quest_id,
                format!("action {phase} skill {skill_id} appears more than once"),
            ));
        }
        if !skill_ids.contains(&skill_id) {
            return Err(unsupported(
                quest_id,
                "unknown skill reference",
                format!("skill {skill_id} is absent from authoritative Skill.wz"),
            ));
        }
        let job_ids = read_action_skill_jobs(quest_id, phase, skill_id, &entry)?;
        if acquire == Some(-1) {
            validate_children(
                quest_id,
                &entry,
                &["id", "acquire", "job"],
                "skill removal action",
            )?;
            changes.push(QuestSkillChange {
                skill_id,
                operation: QuestSkillOperation::Remove,
                job_ids,
            });
            continue;
        }
        if let Some(acquire) = acquire {
            return Err(invalid(
                quest_id,
                format!(
                    "action {phase} skill {skill_id} acquire must be -1 when present, not \
                     {acquire}"
                ),
            ));
        }
        validate_children(
            quest_id,
            &entry,
            &["id", "job", "masterLevel", "skillLevel", "onlyMasterLevel"],
            "skill grant action",
        )?;
        let master_level = required_u32(&entry, "masterLevel", quest_id)?;
        let only_master_level = match optional_i64(&entry, "onlyMasterLevel", quest_id)? {
            None | Some(0) => false,
            Some(1) => true,
            Some(value) => {
                return Err(invalid(
                    quest_id,
                    format!(
                        "action {phase} skill {skill_id} onlyMasterLevel must be 0 or 1, not \
                         {value}"
                    ),
                ));
            }
        };
        let authored_skill_level = optional_u32(&entry, "skillLevel", quest_id)?;
        if only_master_level && authored_skill_level.is_some() {
            return Err(invalid(
                quest_id,
                format!(
                    "action {phase} skill {skill_id} cannot combine onlyMasterLevel=1 with \
                     skillLevel"
                ),
            ));
        }
        changes.push(QuestSkillChange {
            skill_id,
            operation: QuestSkillOperation::Grant {
                skill_level: authored_skill_level.unwrap_or_default(),
                master_level,
            },
            job_ids,
        });
    }
    require_entries(quest_id, &indexes, "action skill entries")?;
    validate_contiguous_indexes(quest_id, &mut indexes, "action skill entries")?;
    Ok(changes)
}

pub(crate) fn read_action_skill_jobs(
    quest_id: u32,
    phase: &str,
    skill_id: u32,
    entry: &WzNodeArc,
) -> Result<Vec<u32>, QuestContentError> {
    let Some(jobs) = wz::child(entry, "job")? else {
        return Ok(Vec::new());
    };
    let context = format!("action {phase} skill {skill_id} jobs");
    require_property(quest_id, &jobs, &context)?;
    let mut indexes = Vec::new();
    let mut seen = BTreeSet::new();
    let mut job_ids = Vec::new();
    for job in wz::sorted_children(&jobs)? {
        let name = wz::node_name(&job)?;
        indexes.push(parse_decimal_name(quest_id, &name, &context)?);
        let value = scalar_i64(&job)?.ok_or_else(|| {
            invalid(
                quest_id,
                format!("{context} entry {name} is not an integer"),
            )
        })?;
        let job_id = u32::try_from(value).map_err(|_| {
            invalid(
                quest_id,
                format!("{context} entry {name} is negative or too large"),
            )
        })?;
        if !seen.insert(job_id) {
            return Err(invalid(
                quest_id,
                format!("{context} contains duplicate job ID {job_id}"),
            ));
        }
        job_ids.push(job_id);
    }
    require_entries(quest_id, &indexes, &context)?;
    validate_contiguous_indexes(quest_id, &mut indexes, &context)?;
    Ok(job_ids)
}

pub(crate) fn validate_action_phase_fields(
    quest_id: u32,
    phase: &str,
    node: &WzNodeArc,
    authoritative_check: Option<&WzNodeArc>,
) -> Result<Vec<String>, QuestContentError> {
    // Act can duplicate Check requirements with conflicting values. Validate and
    // audit those copies here, but do not return them as gameplay requirements.
    let mut page_indexes = Vec::new();
    let mut retained_fields = Vec::new();
    for child in wz::sorted_children(node)? {
        let name = wz::node_name(&child)?;
        if ACTION_FIELDS.contains(&name.as_str()) {
            continue;
        }
        if name == "info" {
            if phase != "0" {
                return Err(unsupported(
                    quest_id,
                    "quest progress action phase 1",
                    "completion action info has no supported local ordering",
                ));
            }
            required_record_string(quest_id, &child, "action 0 info")?;
            continue;
        }
        if is_decimal_name(&name) {
            let index = parse_decimal_name(quest_id, &name, "action dialogue page")?;
            require_string(
                quest_id,
                &child,
                &format!("action {phase} dialogue page {name}"),
            )?;
            page_indexes.push(index);
        } else {
            match name.as_str() {
                "yes" | "no" => {
                    validate_numbered_strings(
                        quest_id,
                        &child,
                        &format!("action {phase} dialogue {name} branch"),
                    )?;
                }
                "ask" => {
                    required_i64(node, "ask", quest_id)?;
                }
                "stop" => validate_action_question_answers(quest_id, phase, &child)?,
                "start" | "end" => {
                    require_string(
                        quest_id,
                        &child,
                        &format!("action {phase} calendar metadata {name}"),
                    )?;
                    optional_calendar(node, &name, quest_id)?;
                }
                "interval" => {
                    optional_nonnegative_u64(node, "interval", quest_id)?;
                }
                "message" => {
                    require_string(
                        quest_id,
                        &child,
                        &format!("action {phase} message metadata"),
                    )?;
                    optional_nonempty_string(node, "message", quest_id)?;
                }
                "lvmin" | "lvmax" => {
                    optional_u32(node, &name, quest_id)?;
                }
                "job" => validate_numbered_integers(
                    quest_id,
                    &child,
                    &format!("action {phase} job metadata"),
                )?,
                "gender" => match required_i64(node, "gender", quest_id)? {
                    0..=2 => {}
                    value => {
                        return Err(invalid(
                            quest_id,
                            format!("action {phase} gender metadata has invalid value {value}"),
                        ));
                    }
                },
                "npc" if phase == "0" => {
                    optional_positive_u32(node, "npc", quest_id)?;
                }
                "fieldEnter" => validate_audited_action_field_enter(
                    quest_id,
                    phase,
                    &child,
                    authoritative_check,
                )?,
                _ => return Err(unsupported_action_field(quest_id, phase, &name)),
            }
        }
        retained_fields.push(format!("act/{phase}/{name}"));
    }
    validate_contiguous_indexes(quest_id, &mut page_indexes, "action dialogue pages")?;
    if optional_i64(node, "ask", quest_id)?.is_some() != wz::child(node, "stop")?.is_some() {
        return Err(invalid(
            quest_id,
            format!("action {phase} question metadata must define both ask and stop"),
        ));
    }
    let minimum_level = optional_u32(node, "lvmin", quest_id)?;
    let maximum_level = optional_u32(node, "lvmax", quest_id)?;
    if minimum_level
        .zip(maximum_level)
        .is_some_and(|(min, max)| min > max)
    {
        return Err(invalid(
            quest_id,
            format!("action {phase} level metadata has lvmin above lvmax"),
        ));
    }
    let start = optional_calendar(node, "start", quest_id)?;
    let end = optional_calendar(node, "end", quest_id)?;
    if start
        .zip(end)
        .is_some_and(|(start, end)| start.unix_ms > end.unix_ms)
    {
        return Err(invalid(
            quest_id,
            format!("action {phase} calendar metadata starts after it ends"),
        ));
    }
    Ok(retained_fields)
}
pub(crate) fn read_action_record_writes(
    quest_id: u32,
    phase: &str,
    node: &WzNodeArc,
) -> Result<Vec<QuestRecordWrite>, QuestContentError> {
    let Some(info) = wz::child(node, "info")? else {
        return Ok(Vec::new());
    };
    if phase != "0" {
        return Err(unsupported(
            quest_id,
            "quest progress action phase 1",
            "completion action info has no supported local ordering",
        ));
    }
    Ok(vec![QuestRecordWrite {
        quest_id,
        index: 0,
        value: required_record_string(quest_id, &info, "action 0 info")?,
    }])
}

pub(crate) fn validate_action_question_answers(
    quest_id: u32,
    phase: &str,
    stop: &WzNodeArc,
) -> Result<(), QuestContentError> {
    require_property(quest_id, stop, &format!("action {phase} question stop"))?;
    let mut answer_indexes = Vec::new();
    for answers in wz::sorted_children(stop)? {
        let name = wz::node_name(&answers)?;
        let index = parse_decimal_name(quest_id, &name, "action question answer set")?;
        answer_indexes.push(index);
        require_property(
            quest_id,
            &answers,
            &format!("action {phase} question answer set {name}"),
        )?;
        required_i64(&answers, "answer", quest_id)?;
        for child in wz::sorted_children(&answers)? {
            let child_name = wz::node_name(&child)?;
            if child_name == "answer" {
                continue;
            }
            parse_decimal_name(quest_id, &child_name, "action question response")?;
            require_string(
                quest_id,
                &child,
                &format!("action {phase} question response {child_name}"),
            )?;
        }
    }
    require_entries(quest_id, &answer_indexes, "action question answer sets")?;
    validate_contiguous_indexes(quest_id, &mut answer_indexes, "action question answer sets")
}

pub(crate) fn validate_numbered_strings(
    quest_id: u32,
    node: &WzNodeArc,
    context: &str,
) -> Result<(), QuestContentError> {
    require_property(quest_id, node, context)?;
    let mut indexes = Vec::new();
    for child in wz::sorted_children(node)? {
        let name = wz::node_name(&child)?;
        indexes.push(parse_decimal_name(quest_id, &name, context)?);
        require_string(quest_id, &child, &format!("{context} page {name}"))?;
    }
    require_entries(quest_id, &indexes, context)?;
    validate_contiguous_indexes(quest_id, &mut indexes, context)
}

pub(crate) fn validate_numbered_integers(
    quest_id: u32,
    node: &WzNodeArc,
    context: &str,
) -> Result<(), QuestContentError> {
    require_property(quest_id, node, context)?;
    let mut indexes = Vec::new();
    for child in wz::sorted_children(node)? {
        let name = wz::node_name(&child)?;
        indexes.push(parse_decimal_name(quest_id, &name, context)?);
        let value = scalar_i64(&child)?.ok_or_else(|| {
            invalid(
                quest_id,
                format!("{context} entry {name} is not an integer"),
            )
        })?;
        u32::try_from(value).map_err(|_| {
            invalid(
                quest_id,
                format!("{context} entry {name} is negative or too large"),
            )
        })?;
    }
    require_entries(quest_id, &indexes, context)?;
    validate_contiguous_indexes(quest_id, &mut indexes, context)
}

pub(crate) fn require_entries(
    quest_id: u32,
    indexes: &[usize],
    context: &str,
) -> Result<(), QuestContentError> {
    (!indexes.is_empty())
        .then_some(())
        .ok_or_else(|| invalid(quest_id, format!("{context} has no entries")))
}

pub(crate) fn validate_contiguous_indexes(
    quest_id: u32,
    indexes: &mut [usize],
    context: &str,
) -> Result<(), QuestContentError> {
    indexes.sort_unstable();
    if let Some((expected, actual)) = indexes
        .iter()
        .copied()
        .enumerate()
        .find(|(expected, actual)| expected != actual)
    {
        return Err(invalid(
            quest_id,
            format!("{context} expected index {expected}, found {actual}"),
        ));
    }
    Ok(())
}

pub(crate) fn parse_decimal_name(
    quest_id: u32,
    name: &str,
    context: &str,
) -> Result<usize, QuestContentError> {
    let index = name
        .parse::<usize>()
        .map_err(|_| invalid(quest_id, format!("{context} field {name:?} is not numeric")))?;
    if name != index.to_string() {
        return Err(invalid(
            quest_id,
            format!("{context} field {name:?} is not a canonical numeric index"),
        ));
    }
    Ok(index)
}

pub(crate) fn is_decimal_name(name: &str) -> bool {
    !name.is_empty() && name.bytes().all(|byte| byte.is_ascii_digit())
}

pub(crate) fn require_property(
    quest_id: u32,
    node: &WzNodeArc,
    context: &str,
) -> Result<(), QuestContentError> {
    let read = node.read().map_err(|_| wz::WzContentError::Lock {
        context: "quest property",
    })?;
    read.is_sub_property()
        .then_some(())
        .ok_or_else(|| invalid(quest_id, format!("{context} is not a property")))
}

pub(crate) fn require_string(
    quest_id: u32,
    node: &WzNodeArc,
    context: &str,
) -> Result<(), QuestContentError> {
    scalar_string(node)?
        .map(|_| ())
        .ok_or_else(|| invalid(quest_id, format!("{context} is not a string")))
}
pub(crate) fn unsupported_action_field(
    quest_id: u32,
    phase: &str,
    name: &str,
) -> QuestContentError {
    let (category, detail) = match name {
        "map" | "fieldEnter" => ("map action", "is not safely representable"),
        "buff" | "buffItemID" => ("buff action", "is not safely representable"),
        "npc" => (
            "NPC action",
            "is not valid outside Act phase 0 presentation metadata",
        ),
        "petskill" | "petspeed" | "pettameness" => ("pet action", "is not safely representable"),
        "info" => ("quest progress action", "is not safely representable"),
        _ => ("unknown action field", "is not safely representable"),
    };
    unsupported(
        quest_id,
        category,
        format!("action {phase} field {name:?} {detail}"),
    )
}
