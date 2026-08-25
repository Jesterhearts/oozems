use super::*;

#[cfg(test)]
mod tests;

const RETAINED_INFO_FIELDS: &[&str] = &[
    "order",
    "parent",
    "type",
    "sortkey",
    "medalCategory",
    "viewMedalItem",
    "showLayerTag",
    "timerUI",
];
const UNSUPPORTED_INFO_FIELDS: &[&str] = &["dailyPlayTime", "oneShot", "selectedMob"];
pub(crate) fn read_info(
    quest_id: u32,
    node: &WzNodeArc,
    info_root: &WzNodeArc,
    skill_ids: &BTreeSet<u32>,
    skill_names: &BTreeMap<u32, String>,
) -> Result<QuestInfo, QuestContentError> {
    let mut status_text = BTreeMap::new();
    let mut retained_metadata_fields = Vec::new();
    for child in wz::sorted_children(node)? {
        let name = wz::node_name(&child)?;
        if let Ok(status) = name.parse::<u32>() {
            if retain_audited_misplaced_quest_info(quest_id, status, &child, info_root)? {
                retained_metadata_fields.push("questInfo/4963".to_owned());
                continue;
            }
            let Some(value) = scalar_string(&child)? else {
                if is_null(&child)? {
                    continue;
                }
                return Err(unsupported(
                    quest_id,
                    "quest status metadata",
                    format!("QuestInfo status {status} is not a string"),
                ));
            };
            status_text.insert(status, value);
            continue;
        }
        if [
            "name",
            "area",
            "summary",
            "demandSummary",
            "rewardSummary",
            "timeLimit",
            "timeLimit2",
            "autoStart",
            "autoAccept",
            "autoComplete",
            "autoPreComplete",
            "selectedSkillID",
        ]
        .contains(&name.as_str())
        {
            continue;
        }
        if name == "selectedMob" && retain_audited_stray_selected_mob(quest_id, node)? {
            retained_metadata_fields.push(name);
            continue;
        }
        if UNSUPPORTED_INFO_FIELDS.contains(&name.as_str()) {
            return Err(unsupported(
                quest_id,
                "quest info mechanic",
                format!("QuestInfo field {name:?} has no implemented semantics"),
            ));
        }
        retained_metadata_fields.push(if RETAINED_INFO_FIELDS.contains(&name.as_str()) {
            name
        } else {
            format!("unknown/{name}")
        });
    }
    retained_metadata_fields.sort();
    retained_metadata_fields.dedup();
    Ok(QuestInfo {
        area: optional_u32(node, "area", quest_id)?,
        status_text,
        summary: optional_string(node, "summary")?,
        demand_summary: optional_string(node, "demandSummary")?,
        reward_summary: optional_string(node, "rewardSummary")?,
        time_limit_ms: optional_time_limit_seconds(node, "timeLimit", quest_id)?,
        time_limit2_ms: optional_time_limit_seconds(node, "timeLimit2", quest_id)?,
        auto_start: optional_bool(node, "autoStart", quest_id)?.unwrap_or(false),
        auto_accept: optional_bool(node, "autoAccept", quest_id)?.unwrap_or(false),
        auto_complete: optional_bool(node, "autoComplete", quest_id)?.unwrap_or(false),
        auto_pre_complete: optional_bool(node, "autoPreComplete", quest_id)?.unwrap_or(false),
        selected_skill: read_selected_skill(quest_id, node, skill_ids, skill_names)?,
        retained_metadata_fields,
    })
}
pub(crate) fn read_selected_skill(
    quest_id: u32,
    node: &WzNodeArc,
    skill_ids: &BTreeSet<u32>,
    skill_names: &BTreeMap<u32, String>,
) -> Result<Option<QuestSelectedSkill>, QuestContentError> {
    let Some(skill_id) = optional_i64(node, "selectedSkillID", quest_id)? else {
        return Ok(None);
    };
    let skill_id = u32::try_from(skill_id)
        .ok()
        .and_then(NonZeroU32::new)
        .ok_or_else(|| invalid(quest_id, "QuestInfo selectedSkillID must be positive"))?;
    if !skill_ids.contains(&skill_id.get()) {
        return Err(unsupported(
            quest_id,
            "unknown skill reference",
            format!(
                "QuestInfo selectedSkillID {} is absent from authoritative Skill.wz",
                skill_id.get()
            ),
        ));
    }
    Ok(Some(QuestSelectedSkill {
        id: skill_id,
        name: skill_names.get(&skill_id.get()).cloned(),
    }))
}

pub(crate) fn read_days_of_week(
    quest_id: u32,
    node: &WzNodeArc,
) -> Result<BTreeSet<QuestWeekday>, QuestContentError> {
    let Some(days) = wz::child(node, "dayOfWeek")? else {
        return Ok(BTreeSet::new());
    };
    let mut output = BTreeSet::new();
    for day in wz::sorted_children(&days)? {
        let name = wz::node_name(&day)?;
        let weekday = match name.as_str() {
            "mon" => QuestWeekday::Monday,
            "tue" => QuestWeekday::Tuesday,
            "wed" => QuestWeekday::Wednesday,
            "thu" => QuestWeekday::Thursday,
            "fri" => QuestWeekday::Friday,
            "sat" => QuestWeekday::Saturday,
            "sun" => QuestWeekday::Sunday,
            _ => {
                return Err(invalid(
                    quest_id,
                    format!("dayOfWeek contains unknown day {name:?}"),
                ));
            }
        };
        let enabled = match scalar_i64(&day)? {
            Some(value) => value,
            None => scalar_string(&day)?
                .ok_or_else(|| invalid(quest_id, format!("dayOfWeek {name:?} is not an integer")))?
                .parse::<i64>()
                .map_err(|_| invalid(quest_id, format!("dayOfWeek {name:?} is not an integer")))?,
        };
        if enabled != 0 {
            output.insert(weekday);
        }
    }
    Ok(output)
}

pub(crate) fn optional_calendar(
    node: &WzNodeArc,
    name: &str,
    quest_id: u32,
) -> Result<Option<QuestCalendar>, QuestContentError> {
    optional_nonempty_string(node, name, quest_id)?
        .map(|source| {
            calendar_unix_ms(&source)
                .map(|unix_ms| QuestCalendar { source, unix_ms })
                .map_err(|message| invalid(quest_id, format!("calendar field {name:?} {message}")))
        })
        .transpose()
}

pub(crate) fn calendar_unix_ms(source: &str) -> Result<u64, String> {
    let missing_time = match source.len() {
        8 => "000000",
        10 => "0000",
        12 => "00",
        14 => "",
        _ => {
            return Err(format!("has invalid YYYYMMDD[hh[mm[ss]]] value {source:?}"));
        }
    };
    if !source.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(format!("has invalid YYYYMMDD[hh[mm[ss]]] value {source:?}"));
    }
    let normalized = format!("{source}{missing_time}");
    let datetime = DateTime::strptime("%Y%m%d%H%M%S", &normalized)
        .map_err(|error| format!("has invalid calendar value {source:?}: {error}"))?;
    let timestamp = Offset::UTC
        .to_timestamp(datetime)
        .map_err(|error| format!("is outside the supported calendar range: {error}"))?;
    u64::try_from(timestamp.as_millisecond())
        .map_err(|_| format!("is before the Unix epoch: {source:?}"))
}

pub(crate) fn optional_time_limit_seconds(
    node: &WzNodeArc,
    name: &str,
    quest_id: u32,
) -> Result<Option<u64>, QuestContentError> {
    optional_positive_u64(node, name, quest_id)?
        .map(|seconds| quest_timer_milliseconds(seconds, name, quest_id))
        .transpose()
}

pub(crate) fn quest_timer_milliseconds(
    seconds: u64,
    field: &str,
    quest_id: u32,
) -> Result<u64, QuestContentError> {
    seconds
        .checked_mul(1_000)
        .ok_or_else(|| invalid(quest_id, format!("{field} duration is too large")))
}

pub(crate) fn validate_item_id(
    quest_id: u32,
    item_id: u32,
    item_ids: &BTreeSet<u32>,
) -> Result<(), QuestContentError> {
    if item_ids.contains(&item_id) {
        Ok(())
    } else {
        Err(unsupported(
            quest_id,
            "unknown item reference",
            format!("item {item_id} is absent from the unified item catalog"),
        ))
    }
}
