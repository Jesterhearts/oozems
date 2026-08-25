use super::*;

#[cfg(test)]
mod tests;

#[derive(Debug, Default)]
pub(crate) struct ImportedItemActions {
    pub(crate) fixed: Vec<QuestItemDelta>,
    pub(crate) conditional: Vec<QuestConditionalItemReward>,
    pub(crate) weighted: Vec<QuestWeightedItem>,
    pub(crate) selectable: Vec<QuestSelectableItemReward>,
    pub(crate) retained_fields: Vec<String>,
}
#[cfg(test)]
pub(super) fn read_action_items(
    quest_id: u32,
    phase: &str,
    node: &WzNodeArc,
    item_ids: &BTreeSet<u32>,
    equipment_item_ids: &BTreeSet<u32>,
) -> Result<ImportedItemActions, QuestContentError> {
    read_action_items_with_corrections(
        quest_id,
        phase,
        node,
        item_ids,
        equipment_item_ids,
        &AuditedActionCorrections::default(),
    )
}

pub(crate) fn read_action_items_with_corrections(
    quest_id: u32,
    phase: &str,
    node: &WzNodeArc,
    item_ids: &BTreeSet<u32>,
    _equipment_item_ids: &BTreeSet<u32>,
    audited_corrections: &AuditedActionCorrections,
) -> Result<ImportedItemActions, QuestContentError> {
    let Some(items) = wz::child(node, "item")? else {
        return Ok(ImportedItemActions::default());
    };
    let mut fixed = Vec::new();
    let mut conditional = Vec::new();
    let mut weighted = Vec::new();
    let mut selectable = Vec::new();
    let mut retained_fields = Vec::new();
    for entry in wz::sorted_children(&items)? {
        let entry_name = wz::node_name(&entry)?;
        for field in wz::sorted_children(&entry)? {
            let field_name = wz::node_name(&field)?;
            if ![
                "id",
                "count",
                "prop",
                "job",
                "gender",
                "period",
                "dateExpire",
            ]
            .contains(&field_name.as_str())
            {
                if matches!(field_name.as_str(), "name" | "var") {
                    let value = scalar_i64(&field)?.ok_or_else(|| {
                        invalid(
                            quest_id,
                            format!(
                                "action {phase} item {entry_name} metadata {field_name:?} is not \
                                 an integer"
                            ),
                        )
                    })?;
                    let valid = match field_name.as_str() {
                        "name" => value == 1,
                        "var" => matches!(value, 1 | 2),
                        _ => unreachable!(),
                    };
                    if !valid {
                        return Err(invalid(
                            quest_id,
                            format!(
                                "action {phase} item {entry_name} metadata {field_name:?} has \
                                 unknown value {value}"
                            ),
                        ));
                    }
                    retained_fields.push(format!("act/{phase}/item/{entry_name}/{field_name}"));
                    continue;
                }
                let category = match field_name.as_str() {
                    "job" | "gender" => "filtered item action",
                    "petskill" | "petspeed" | "pettameness" => "pet action",
                    _ => "item action metadata",
                };
                return Err(unsupported(
                    quest_id,
                    category,
                    format!("action {phase} item field {field_name:?} is not safely representable"),
                ));
            }
        }
        let item_id = required_u32(&entry, "id", quest_id)?;
        validate_item_id(quest_id, item_id, item_ids)?;
        let count = optional_i64(&entry, "count", quest_id)?.unwrap_or(1);
        if count == 0 {
            continue;
        }
        let expiration = read_item_expiration(quest_id, phase, item_id, count, &entry)?;
        let prop = optional_i64(&entry, "prop", quest_id)?;
        let eligibility = read_reward_eligibility(quest_id, &entry)?;
        if let Some((item, retained_field)) = audited_action_item_correction(
            audited_corrections,
            quest_id,
            phase,
            &entry_name,
            item_id,
            count,
            prop,
            expiration,
            eligibility,
        )? {
            fixed.push(item);
            retained_fields.push(retained_field);
            continue;
        }
        match prop {
            None if eligibility == QuestRewardEligibility::default() => {
                fixed.push(QuestItemDelta {
                    item_id,
                    count,
                    expiration,
                });
            }
            None => {
                let count = u32::try_from(count).map_err(|_| {
                    invalid(
                        quest_id,
                        format!("filtered action item {item_id} must have a positive count"),
                    )
                })?;
                conditional.push(QuestConditionalItemReward {
                    item_id,
                    count,
                    expiration,
                    eligibility,
                });
            }
            Some(-1) => {
                let count = u32::try_from(count).map_err(|_| {
                    invalid(
                        quest_id,
                        format!("selectable action item {item_id} must have a positive count"),
                    )
                })?;
                selectable.push(QuestSelectableItemReward {
                    item_id,
                    count,
                    expiration,
                    eligibility,
                });
            }
            Some(weight) => {
                let count = u32::try_from(count).map_err(|_| {
                    invalid(
                        quest_id,
                        format!("weighted action item {item_id} must have a positive count"),
                    )
                })?;
                let weight = u32::try_from(weight)
                    .ok()
                    .filter(|weight| *weight > 0)
                    .ok_or_else(|| {
                        invalid(
                            quest_id,
                            format!("weighted action item {item_id} has invalid weight {weight}"),
                        )
                    })?;
                weighted.push(QuestWeightedItem {
                    item_id,
                    count,
                    expiration,
                    weight,
                    eligibility,
                });
            }
        }
    }
    Ok(ImportedItemActions {
        fixed,
        conditional,
        weighted,
        selectable,
        retained_fields,
    })
}

pub(crate) fn read_item_expiration(
    quest_id: u32,
    phase: &str,
    item_id: u32,
    count: i64,
    entry: &WzNodeArc,
) -> Result<Option<QuestItemExpiration>, QuestContentError> {
    let period = wz::child(entry, "period")?;
    let date_expire = wz::child(entry, "dateExpire")?;
    if period.is_some() && date_expire.is_some() {
        return Err(invalid(
            quest_id,
            format!("action {phase} item {item_id} defines both period and dateExpire expiration"),
        ));
    }
    if count < 0 && (period.is_some() || date_expire.is_some()) {
        return Err(invalid(
            quest_id,
            format!("action {phase} removal item {item_id} defines expiration metadata"),
        ));
    }
    if let Some(period) = period {
        let minutes = scalar_i64(&period)?.ok_or_else(|| {
            invalid(
                quest_id,
                format!("action {phase} item {item_id} period is not an integer"),
            )
        })?;
        let minutes = u64::try_from(minutes).map_err(|_| {
            invalid(
                quest_id,
                format!("action {phase} item {item_id} period must not be negative"),
            )
        })?;
        if minutes == 0 {
            return Ok(None);
        }
        let milliseconds = minutes.checked_mul(60_000).ok_or_else(|| {
            invalid(
                quest_id,
                format!("action {phase} item {item_id} period is too large"),
            )
        })?;
        return Ok(Some(QuestItemExpiration::RelativeMilliseconds(
            milliseconds,
        )));
    }
    let Some(date_expire) = date_expire else {
        return Ok(None);
    };
    let source = if let Some(source) = scalar_string(&date_expire)? {
        Some(source)
    } else {
        scalar_i64(&date_expire)?.map(|value| value.to_string())
    }
    .ok_or_else(|| {
        invalid(
            quest_id,
            format!("action {phase} item {item_id} dateExpire is not a string or integer"),
        )
    })?;
    let unix_ms = item_expiration_unix_ms(&source).map_err(|message| {
        invalid(
            quest_id,
            format!("action {phase} item {item_id} dateExpire {message}"),
        )
    })?;
    Ok(Some(QuestItemExpiration::AbsoluteUnixMilliseconds(unix_ms)))
}

pub(crate) fn item_expiration_unix_ms(source: &str) -> Result<u64, String> {
    if source.len() != 10 || !source.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(format!("has invalid yyyyMMddHH value {source:?}"));
    }
    let datetime = DateTime::strptime("%Y%m%d%H", source)
        .map_err(|error| format!("has invalid yyyyMMddHH value {source:?}: {error}"))?;
    // GMS archive deadlines are civil times in the Pacific game-service zone.
    let timezone = TimeZone::get("America/Los_Angeles")
        .map_err(|error| format!("cannot load America/Los_Angeles: {error}"))?;
    let timestamp = timezone
        .to_timestamp(datetime)
        .map_err(|error| format!("is not a valid America/Los_Angeles civil time: {error}"))?;
    u64::try_from(timestamp.as_millisecond())
        .map_err(|_| format!("is before the Unix epoch: {source:?}"))
}

pub(crate) fn validate_selectable_reward_flow(
    quest_id: u32,
    start_actions: &QuestActions,
    _completion_actions: &QuestActions,
    _dialogue: &QuestDialogue,
) -> Result<(), QuestContentError> {
    if !start_actions.selectable_items.is_empty() {
        return Err(unsupported(
            quest_id,
            "selectable reward in start actions",
            "start actions cannot request a player completion-reward selection",
        ));
    }
    Ok(())
}
pub(crate) fn read_reward_eligibility(
    quest_id: u32,
    entry: &WzNodeArc,
) -> Result<QuestRewardEligibility, QuestContentError> {
    let job_mask = optional_u32(entry, "job", quest_id)?
        .map(|mask| {
            (mask > 0)
                .then_some(mask)
                .ok_or_else(|| invalid(quest_id, "reward job mask must be positive"))
        })
        .transpose()?;
    let gender = match optional_i64(entry, "gender", quest_id)? {
        None | Some(2) => None,
        Some(0) => Some(QuestRewardGender::Male),
        Some(1) => Some(QuestRewardGender::Female),
        Some(value) => {
            return Err(invalid(
                quest_id,
                format!("reward gender has invalid value {value}"),
            ));
        }
    };
    Ok(QuestRewardEligibility { job_mask, gender })
}
