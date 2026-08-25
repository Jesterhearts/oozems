use super::*;

#[cfg(test)]
mod tests;

const START_CHECK_FIELDS: &[&str] = &[
    "npc",
    "pop",
    "worldmin",
    "worldmax",
    "job",
    "lvmin",
    "lvmax",
    "item",
    "mbcard",
    "mbmin",
    "mbmax",
    "equipAllNeed",
    "equipSelectNeed",
    "quest",
    "start",
    "end",
    "interval",
    "dayByDay",
    "dayOfWeek",
    "normalAutoStart",
    "startscript",
    "fieldEnter",
    "skill",
    "buff",
    "exceptbuff",
    "morph",
    "info",
    "infoNumber",
    "infoex",
];
const COMPLETION_CHECK_FIELDS: &[&str] = &[
    "npc",
    "endmeso",
    "questComplete",
    "item",
    "mbcard",
    "mbmin",
    "mbmax",
    "equipAllNeed",
    "equipSelectNeed",
    "mob",
    "quest",
    "lvmin",
    "level",
    "start",
    "end",
    "endscript",
    "info",
    "infoNumber",
    "infoex",
    "buff",
    "exceptbuff",
    "morph",
];
const UNRESOLVED_MAP_SENTINEL: u32 = 999_999_999;
pub(crate) fn read_start_requirements(
    quest_id: u32,
    node: &WzNodeArc,
    info: &QuestInfo,
    item_ids: &BTreeSet<u32>,
    equipment_item_ids: &BTreeSet<u32>,
    consume_effect_ids: &BTreeSet<u32>,
    monster_book_card_ids: &BTreeSet<u32>,
    morph_ids: &BTreeSet<u32>,
) -> Result<QuestStartRequirements, QuestContentError> {
    validate_check_fields(quest_id, node, START_CHECK_FIELDS, "start")?;
    let available_from = optional_calendar(node, "start", quest_id)?;
    let available_until = optional_calendar(node, "end", quest_id)?;
    if let (Some(start), Some(end)) = (&available_from, &available_until)
        && start.unix_ms > end.unix_ms
    {
        return Err(invalid(
            quest_id,
            "start calendar timestamp is after the end timestamp",
        ));
    }
    let interval_ms = optional_nonnegative_u64(node, "interval", quest_id)?
        .map(|minutes| {
            minutes
                .checked_mul(60_000)
                .ok_or_else(|| invalid(quest_id, "repeat interval is too large"))
        })
        .transpose()?;
    let repeat = QuestRepeatMetadata {
        interval_ms,
        day_by_day: optional_bool(node, "dayByDay", quest_id)?.unwrap_or(false),
        days_of_week: read_days_of_week(quest_id, node)?,
    };
    let normal_auto_start = optional_bool(node, "normalAutoStart", quest_id)?.unwrap_or(false);
    let script = optional_nonempty_string(node, "startscript", quest_id)?;
    let npc_id = optional_u32(node, "npc", quest_id)?;
    let minimum_fame = optional_u32(node, "pop", quest_id)?
        .map(|value| {
            i32::try_from(value)
                .map_err(|_| invalid(quest_id, "integer \"pop\" exceeds the fame range"))
        })
        .transpose()?;
    let minimum_world_id = optional_strict_u32(node, "worldmin", quest_id)?;
    let maximum_world_id = optional_strict_u32(node, "worldmax", quest_id)?;
    if minimum_world_id
        .zip(maximum_world_id)
        .is_some_and(|(minimum, maximum)| minimum > maximum)
    {
        return Err(invalid(
            quest_id,
            "start check worldmin is greater than worldmax",
        ));
    }
    if npc_id.is_none()
        && !normal_auto_start
        && !info.auto_start
        && !info.auto_accept
        && script.is_none()
    {
        return Err(invalid(
            quest_id,
            "start check has no NPC or automatic/scripted start metadata",
        ));
    }

    Ok(QuestStartRequirements {
        npc_id,
        minimum_fame,
        minimum_world_id,
        maximum_world_id,
        allowed_jobs: read_u32_list(quest_id, node, "job")?,
        allowed_map_ids: read_allowed_map_ids(quest_id, node)?,
        minimum_level: optional_u32(node, "lvmin", quest_id)?,
        maximum_level: optional_u32(node, "lvmax", quest_id)?,
        items: read_item_requirements(quest_id, node, item_ids)?,
        monster_book: read_monster_book_requirements(quest_id, node, monster_book_card_ids)?,
        equipped_items: read_equipped_item_requirements(quest_id, node, equipment_item_ids)?,
        quests: read_quest_requirements(quest_id, node)?,
        skills: read_skill_requirements(quest_id, node)?,
        effects: read_effect_requirements(quest_id, node, consume_effect_ids)?,
        required_morph_id: read_required_morph(quest_id, node, morph_ids)?,
        record_conditions: read_record_conditions(quest_id, node)?,
        available_from,
        available_until,
        repeat,
        normal_auto_start,
        script,
    })
}

pub(crate) fn read_completion_requirements(
    quest_id: u32,
    node: &WzNodeArc,
    _info: &QuestInfo,
    item_ids: &BTreeSet<u32>,
    equipment_item_ids: &BTreeSet<u32>,
    consume_effect_ids: &BTreeSet<u32>,
    monster_book_card_ids: &BTreeSet<u32>,
    morph_ids: &BTreeSet<u32>,
) -> Result<QuestCompletionRequirements, QuestContentError> {
    validate_check_fields(quest_id, node, COMPLETION_CHECK_FIELDS, "completion")?;
    let lvmin = optional_u32(node, "lvmin", quest_id)?;
    let level = optional_u32(node, "level", quest_id)?;
    if lvmin.is_some() && level.is_some() && lvmin != level {
        return Err(invalid(
            quest_id,
            "completion check defines conflicting lvmin and level values",
        ));
    }
    let available_from = optional_calendar(node, "start", quest_id)?;
    let available_until = optional_calendar(node, "end", quest_id)?;
    if let (Some(start), Some(end)) = (&available_from, &available_until)
        && start.unix_ms > end.unix_ms
    {
        return Err(invalid(
            quest_id,
            "completion start calendar timestamp is after the end timestamp",
        ));
    }
    Ok(QuestCompletionRequirements {
        npc_id: optional_u32(node, "npc", quest_id)?,
        minimum_mesos: optional_nonnegative_u64(node, "endmeso", quest_id)?,
        minimum_completed_quest_count: optional_u32(node, "questComplete", quest_id)?,
        items: read_item_requirements(quest_id, node, item_ids)?,
        monster_book: read_monster_book_requirements(quest_id, node, monster_book_card_ids)?,
        equipped_items: read_equipped_item_requirements(quest_id, node, equipment_item_ids)?,
        mobs: read_mob_objectives(quest_id, node)?,
        quests: read_quest_requirements(quest_id, node)?,
        effects: read_effect_requirements(quest_id, node, consume_effect_ids)?,
        required_morph_id: read_required_morph(quest_id, node, morph_ids)?,
        record_conditions: read_record_conditions(quest_id, node)?,
        required_level: level.or(lvmin),
        available_from,
        available_until,
        script: optional_nonempty_string(node, "endscript", quest_id)?,
    })
}

pub(crate) fn validate_check_fields(
    quest_id: u32,
    node: &WzNodeArc,
    allowed: &[&str],
    phase: &str,
) -> Result<(), QuestContentError> {
    for child in wz::sorted_children(node)? {
        let name = wz::node_name(&child)?;
        if allowed.contains(&name.as_str()) {
            continue;
        }
        let category = match name.as_str() {
            "pet"
            | "pettamenessmin"
            | "petAutoSpeakingLimit"
            | "petRecallLimit"
            | "tamingmoblevelmin" => "pet check",
            "partyQuest_S" => "party check",
            "info" | "infoNumber" | "infoex" => "info check",
            "buff" | "exceptbuff" => "buff check",
            "skill" => "skill check",
            "fieldEnter" => "map check",
            _ => "unknown check field",
        };
        return Err(unsupported(
            quest_id,
            category,
            format!("{phase} check field {name:?} has no implemented semantics"),
        ));
    }
    Ok(())
}

pub(crate) fn read_item_requirements(
    quest_id: u32,
    node: &WzNodeArc,
    item_ids: &BTreeSet<u32>,
) -> Result<Vec<QuestItemRequirement>, QuestContentError> {
    let Some(items) = wz::child(node, "item")? else {
        return Ok(Vec::new());
    };
    let mut output = Vec::new();
    let mut seen = BTreeSet::new();
    for entry in wz::sorted_children(&items)? {
        validate_children(quest_id, &entry, &["id", "count"], "item requirement")?;
        let item_id = required_u32(&entry, "id", quest_id)?;
        validate_item_id(quest_id, item_id, item_ids)?;
        let condition = match optional_i64(&entry, "count", quest_id)? {
            None => QuestItemCondition::AtLeast(NonZeroU32::MIN),
            Some(value) if value <= 0 => QuestItemCondition::Absent,
            Some(value) => {
                let count = u32::try_from(value)
                    .ok()
                    .and_then(NonZeroU32::new)
                    .ok_or_else(|| invalid(quest_id, "integer \"count\" must be positive"))?;
                QuestItemCondition::AtLeast(count)
            }
        };
        if !seen.insert(item_id) {
            return Err(invalid(
                quest_id,
                format!("item requirement {item_id} appears more than once"),
            ));
        }
        output.push(QuestItemRequirement { item_id, condition });
    }
    Ok(output)
}

pub(crate) fn read_monster_book_requirements(
    quest_id: u32,
    node: &WzNodeArc,
    known_card_ids: &BTreeSet<u32>,
) -> Result<QuestMonsterBookRequirements, QuestContentError> {
    let minimum_unique_cards = optional_u32(node, "mbmin", quest_id)?;
    let maximum_unique_cards = optional_u32(node, "mbmax", quest_id)?;
    if minimum_unique_cards
        .zip(maximum_unique_cards)
        .is_some_and(|(minimum, maximum)| minimum > maximum)
    {
        return Err(invalid(
            quest_id,
            "Monster Book mbmin is greater than mbmax",
        ));
    }
    let Some(cards) = wz::child(node, "mbcard")? else {
        return Ok(QuestMonsterBookRequirements {
            cards: Vec::new(),
            minimum_unique_cards,
            maximum_unique_cards,
        });
    };
    require_property(quest_id, &cards, "Monster Book card requirements")?;
    let mut indexed = Vec::new();
    let mut seen = BTreeSet::new();
    for entry in wz::sorted_children(&cards)? {
        let name = wz::node_name(&entry)?;
        let index = parse_decimal_name(quest_id, &name, "Monster Book card requirement")?;
        require_property(
            quest_id,
            &entry,
            &format!("Monster Book card requirement {name}"),
        )?;
        validate_children(
            quest_id,
            &entry,
            &["id", "min", "max"],
            "Monster Book card requirement",
        )?;
        let card_item_id = required_positive_u32(&entry, "id", quest_id)?;
        if !known_card_ids.contains(&card_item_id) {
            return Err(unsupported(
                quest_id,
                "unknown Monster Book card reference",
                format!(
                    "Monster Book card {card_item_id} is absent from the authoritative card \
                     catalog"
                ),
            ));
        }
        if !seen.insert(card_item_id) {
            return Err(invalid(
                quest_id,
                format!("Monster Book card {card_item_id} appears more than once"),
            ));
        }
        let minimum_count = optional_card_count(quest_id, &entry, "min", card_item_id)?;
        let maximum_count = optional_card_count(quest_id, &entry, "max", card_item_id)?;
        if minimum_count.is_none() && maximum_count.is_none() {
            return Err(invalid(
                quest_id,
                format!("Monster Book card {card_item_id} has neither min nor max"),
            ));
        }
        if minimum_count
            .zip(maximum_count)
            .is_some_and(|(minimum, maximum)| minimum > maximum)
        {
            return Err(invalid(
                quest_id,
                format!("Monster Book card {card_item_id} min is greater than max"),
            ));
        }
        indexed.push((
            index,
            QuestMonsterBookCardRequirement {
                card_item_id,
                minimum_count,
                maximum_count,
            },
        ));
    }
    let mut indexes = indexed.iter().map(|(index, _)| *index).collect::<Vec<_>>();
    require_entries(quest_id, &indexes, "Monster Book card requirements")?;
    validate_contiguous_indexes(quest_id, &mut indexes, "Monster Book card requirements")?;
    indexed.sort_unstable_by_key(|(index, _)| *index);
    Ok(QuestMonsterBookRequirements {
        cards: indexed
            .into_iter()
            .map(|(_, requirement)| requirement)
            .collect(),
        minimum_unique_cards,
        maximum_unique_cards,
    })
}

pub(crate) fn optional_card_count(
    quest_id: u32,
    entry: &WzNodeArc,
    name: &str,
    card_item_id: u32,
) -> Result<Option<u32>, QuestContentError> {
    let count = optional_u32(entry, name, quest_id)?;
    if count.is_some_and(|count| count > crate::monster_book::MAX_CARD_COUNT) {
        return Err(invalid(
            quest_id,
            format!(
                "Monster Book card {card_item_id} {name} exceeds {}",
                crate::monster_book::MAX_CARD_COUNT
            ),
        ));
    }
    Ok(count)
}

pub(crate) fn read_equipped_item_requirements(
    quest_id: u32,
    node: &WzNodeArc,
    equipment_item_ids: &BTreeSet<u32>,
) -> Result<QuestEquippedItemRequirements, QuestContentError> {
    Ok(QuestEquippedItemRequirements {
        all_of: read_equipped_item_list(quest_id, node, "equipAllNeed", equipment_item_ids)?,
        any_of: read_equipped_item_list(quest_id, node, "equipSelectNeed", equipment_item_ids)?,
    })
}

pub(crate) fn read_equipped_item_list(
    quest_id: u32,
    node: &WzNodeArc,
    name: &str,
    equipment_item_ids: &BTreeSet<u32>,
) -> Result<Vec<u32>, QuestContentError> {
    let Some(values) = wz::child(node, name)? else {
        return Ok(Vec::new());
    };
    require_property(quest_id, &values, &format!("{name} equipment requirements"))?;
    let mut indexes = Vec::new();
    let mut seen = BTreeSet::new();
    let mut output = Vec::new();
    for value in wz::sorted_children(&values)? {
        let index_name = wz::node_name(&value)?;
        indexes.push(parse_decimal_name(
            quest_id,
            &index_name,
            &format!("{name} equipment requirement"),
        )?);
        let item_id = scalar_i64(&value)?
            .and_then(|value| u32::try_from(value).ok())
            .filter(|item_id| *item_id > 0)
            .ok_or_else(|| {
                invalid(
                    quest_id,
                    format!("{name} equipment requirement {index_name} is not a positive integer"),
                )
            })?;
        if !equipment_item_ids.contains(&item_id) {
            return Err(unsupported(
                quest_id,
                "unknown equipment reference",
                format!(
                    "{name} equipment requirement {item_id} is absent from the authoritative \
                     equipment catalog"
                ),
            ));
        }
        if !seen.insert(item_id) {
            return Err(invalid(
                quest_id,
                format!("{name} equipment requirement {item_id} appears more than once"),
            ));
        }
        output.push(item_id);
    }
    require_entries(
        quest_id,
        &indexes,
        &format!("{name} equipment requirements"),
    )?;
    validate_contiguous_indexes(
        quest_id,
        &mut indexes,
        &format!("{name} equipment requirements"),
    )?;
    Ok(output)
}

pub(crate) fn read_mob_objectives(
    quest_id: u32,
    node: &WzNodeArc,
) -> Result<Vec<QuestMobObjective>, QuestContentError> {
    let Some(mobs) = wz::child(node, "mob")? else {
        return Ok(Vec::new());
    };
    let mut output = Vec::new();
    let mut seen = BTreeSet::new();
    for entry in wz::sorted_children(&mobs)? {
        validate_children(quest_id, &entry, &["id", "count"], "mob objective")?;
        let mob_id = required_positive_u32(&entry, "id", quest_id)?;
        let count = required_positive_u32(&entry, "count", quest_id)?;
        if !seen.insert(mob_id) {
            return Err(invalid(
                quest_id,
                format!("mob objective {mob_id} appears more than once"),
            ));
        }
        output.push(QuestMobObjective { mob_id, count });
    }
    Ok(output)
}

pub(crate) fn read_quest_requirements(
    quest_id: u32,
    node: &WzNodeArc,
) -> Result<Vec<QuestStateRequirement>, QuestContentError> {
    let Some(quests) = wz::child(node, "quest")? else {
        return Ok(Vec::new());
    };
    let mut output = Vec::new();
    let mut seen = BTreeSet::new();
    for entry in wz::sorted_children(&quests)? {
        validate_children(quest_id, &entry, &["id", "state"], "quest prerequisite")?;
        let required_id = required_positive_u32(&entry, "id", quest_id)?;
        let state = match optional_i64(&entry, "state", quest_id)?.unwrap_or(0) {
            0 => RequiredQuestState::NotStarted,
            1 => RequiredQuestState::Started,
            2 => RequiredQuestState::Completed,
            value => {
                return Err(invalid(
                    quest_id,
                    format!("quest prerequisite {required_id} has invalid state {value}"),
                ));
            }
        };
        if !seen.insert(required_id) {
            return Err(invalid(
                quest_id,
                format!("quest prerequisite {required_id} appears more than once"),
            ));
        }
        output.push(QuestStateRequirement {
            quest_id: required_id,
            state,
        });
    }
    Ok(output)
}

pub(crate) fn read_allowed_map_ids(
    quest_id: u32,
    node: &WzNodeArc,
) -> Result<Vec<u32>, QuestContentError> {
    let map_ids = read_u32_list(quest_id, node, "fieldEnter")?;
    if map_ids.contains(&UNRESOLVED_MAP_SENTINEL) {
        return Err(unsupported(
            quest_id,
            "map check",
            format!("map sentinel {UNRESOLVED_MAP_SENTINEL} has unknown semantics"),
        ));
    }
    Ok(map_ids)
}

pub(crate) fn read_skill_requirements(
    quest_id: u32,
    node: &WzNodeArc,
) -> Result<Vec<QuestSkillRequirement>, QuestContentError> {
    let Some(skills) = wz::child(node, "skill")? else {
        return Ok(Vec::new());
    };
    let mut output = Vec::new();
    let mut seen = BTreeSet::new();
    for entry in wz::sorted_children(&skills)? {
        validate_children(quest_id, &entry, &["id", "acquire"], "skill requirement")?;
        let skill_id = required_positive_u32(&entry, "id", quest_id)?;
        if !seen.insert(skill_id) {
            return Err(invalid(
                quest_id,
                format!("skill requirement {skill_id} appears more than once"),
            ));
        }
        output.push(QuestSkillRequirement {
            skill_id,
            acquired: optional_bool(&entry, "acquire", quest_id)?.unwrap_or(false),
        });
    }
    Ok(output)
}

pub(crate) fn read_effect_requirements(
    quest_id: u32,
    node: &WzNodeArc,
    consume_effect_ids: &BTreeSet<u32>,
) -> Result<Vec<QuestEffectRequirement>, QuestContentError> {
    let mut output = Vec::new();
    for (name, active) in [("buff", true), ("exceptbuff", false)] {
        let Some(value) = wz::child(node, name)? else {
            continue;
        };
        let source = raw_scalar_string(&value)?.ok_or_else(|| {
            invalid(
                quest_id,
                format!("{name} must be an exact decimal string item ID"),
            )
        })?;
        let item_id = source
            .parse::<u32>()
            .ok()
            .filter(|item_id| *item_id > 0 && item_id.to_string() == source)
            .ok_or_else(|| {
                invalid(
                    quest_id,
                    format!("{name} value {source:?} is not a canonical positive decimal u32"),
                )
            })?;
        if !consume_effect_ids.contains(&item_id) {
            return Err(unsupported(
                quest_id,
                "unsupported consume item effect",
                format!("{name} references consume item {item_id} without supported semantics"),
            ));
        }
        if output
            .iter()
            .any(|requirement: &QuestEffectRequirement| requirement.item_id == item_id)
        {
            return Err(invalid(
                quest_id,
                format!("consume effect requirement {item_id} appears more than once"),
            ));
        }
        output.push(QuestEffectRequirement { item_id, active });
    }
    Ok(output)
}

pub(crate) fn read_required_morph(
    quest_id: u32,
    node: &WzNodeArc,
    morph_ids: &BTreeSet<u32>,
) -> Result<Option<NonZeroU32>, QuestContentError> {
    let Some(value) = wz::child(node, "morph")? else {
        return Ok(None);
    };
    let morph_id = scalar_i64(&value)?
        .and_then(|value| u32::try_from(value).ok())
        .and_then(NonZeroU32::new)
        .ok_or_else(|| invalid(quest_id, "morph must be an exact positive integer ID"))?;
    if !morph_ids.contains(&morph_id.get()) {
        return Err(unsupported(
            quest_id,
            "unknown morph check",
            format!(
                "morph {} is absent from required Morph.wz content",
                morph_id
            ),
        ));
    }
    Ok(Some(morph_id))
}

pub(crate) fn read_record_conditions(
    quest_id: u32,
    node: &WzNodeArc,
) -> Result<Vec<QuestRecordCondition>, QuestContentError> {
    let info = wz::child(node, "info")?;
    let infoex = wz::child(node, "infoex")?;
    let record_id = optional_record_quest_id(node, "infoNumber", quest_id)?;
    if info.is_none() && infoex.is_none() {
        if record_id.is_some() {
            return Err(invalid(
                quest_id,
                "infoNumber is present without info or infoex alternatives",
            ));
        }
        return Ok(Vec::new());
    }

    let mut alternatives = Vec::new();
    if let Some(info) = info {
        require_property(quest_id, &info, "direct info alternatives")?;
        let mut indexes = Vec::new();
        for child in wz::sorted_children(&info)? {
            let name = wz::node_name(&child)?;
            indexes.push(parse_decimal_name(
                quest_id,
                &name,
                "direct info alternative",
            )?);
            let value = required_record_string(
                quest_id,
                &child,
                &format!("direct info alternative {name}"),
            )?;
            alternatives.push(read_record_predicate(quest_id, 0, value)?);
        }
        require_entries(quest_id, &indexes, "direct info alternatives")?;
        validate_contiguous_indexes(quest_id, &mut indexes, "direct info alternatives")?;
    }
    if let Some(infoex) = infoex {
        require_property(quest_id, &infoex, "infoex alternatives")?;
        let mut indexes = Vec::new();
        for entry in wz::sorted_children(&infoex)? {
            let name = wz::node_name(&entry)?;
            indexes.push(parse_decimal_name(quest_id, &name, "infoex alternative")?);
            require_property(quest_id, &entry, &format!("infoex alternative {name}"))?;
            validate_children(quest_id, &entry, &["cond", "value"], "infoex alternative")?;
            let value_node = required_child(&entry, "value", quest_id)?;
            let value = required_record_string(
                quest_id,
                &value_node,
                &format!("infoex alternative {name} value"),
            )?;
            let condition = optional_i64(&entry, "cond", quest_id)?.unwrap_or_default();
            alternatives.push(read_record_predicate(quest_id, condition, value)?);
        }
        require_entries(quest_id, &indexes, "infoex alternatives")?;
        validate_contiguous_indexes(quest_id, &mut indexes, "infoex alternatives")?;
    }

    Ok(vec![QuestRecordCondition {
        quest_id: record_id.unwrap_or(quest_id),
        index: 0,
        alternatives,
    }])
}

pub(crate) fn optional_record_quest_id(
    node: &WzNodeArc,
    name: &str,
    quest_id: u32,
) -> Result<Option<u32>, QuestContentError> {
    let Some(value) = wz::child(node, name)? else {
        return Ok(None);
    };
    let parsed = if let Some(value) = scalar_i64(&value)? {
        u32::try_from(value).ok()
    } else if let Some(value) = raw_scalar_string(&value)? {
        (!value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))
            .then(|| value.parse::<u32>().ok())
            .flatten()
    } else {
        None
    };
    parsed.filter(|value| *value != 0).map(Some).ok_or_else(|| {
        invalid(
            quest_id,
            format!("{name} must be a nonzero integer or strictly decimal string"),
        )
    })
}

pub(crate) fn read_record_predicate(
    quest_id: u32,
    condition: i64,
    value: String,
) -> Result<QuestRecordPredicate, QuestContentError> {
    if value.is_empty() {
        return Err(invalid(
            quest_id,
            "record predicate alternatives cannot be empty",
        ));
    }
    crate::quest_records::validate_value(&value)
        .map_err(|error| invalid(quest_id, error.to_string()))?;
    match condition {
        0 => Ok(QuestRecordPredicate::Equal(value)),
        1 | 2 => {
            let numeric = crate::quest_records::strict_decimal(&value).ok_or_else(|| {
                invalid(
                    quest_id,
                    format!("record predicate value {value:?} is not strictly decimal"),
                )
            })?;
            if condition == 1 {
                Ok(QuestRecordPredicate::AtLeast(numeric))
            } else {
                Ok(QuestRecordPredicate::AtMost(numeric))
            }
        }
        _ => Err(invalid(
            quest_id,
            format!("record predicate has unknown cond {condition}"),
        )),
    }
}
