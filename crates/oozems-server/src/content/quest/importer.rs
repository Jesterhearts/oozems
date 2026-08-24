use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::num::NonZeroU32;

use jiff::civil::DateTime;
use jiff::tz::Offset;
use jiff::tz::TimeZone;
use wz_reader::WzNodeArc;
use wz_reader::WzNodeCast;

use super::QuestContentError;
use super::dialogue;
use super::invalid;
use super::model::*;
use super::unsupported;
use crate::content::wz;

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
const UNRESOLVED_MAP_SENTINEL: u32 = 999_999_999;
const MAP_PROTECTION_EFFECT_ID: u32 = 2_022_187;
const QUEST_4944_CHECK_FINGERPRINT: u64 = 0x814b56fb48fa385c;
const QUEST_4944_ACTION_FINGERPRINT: u64 = 0xcdaf7ec5eed08bff;
const QUEST_4944_SAY_FINGERPRINT: u64 = 0x29d1971031938cff;
const QUEST_4944_INFO_FINGERPRINT: u64 = 0xe722de393de89f64;
const QUEST_4960_CHECK_FINGERPRINT: u64 = 0x98077d573bf7b90b;
const QUEST_4960_SAY_FINGERPRINT: u64 = 0xd6c80cc1ee6e5c0e;
const QUEST_4960_INFO_FINGERPRINT: u64 = 0xe14e00e29f73c020;
const AUDITED_STRAY_SELECTED_MOB_INFO_FINGERPRINTS: &[(u32, u64)] = &[
    (3_954, 0x6052d34ca49a0dc4),
    (4_006, 0x610c393f74e90ccc),
    (4_484, 0xc446205d5e33e263),
    (6_012, 0xf101c9e7a9666dff),
];

#[derive(Debug, Default)]
struct ImportedItemActions {
    fixed: Vec<QuestItemDelta>,
    conditional: Vec<QuestConditionalItemReward>,
    weighted: Vec<QuestWeightedItem>,
    selectable: Vec<QuestSelectableItemReward>,
    retained_fields: Vec<String>,
}

#[derive(Debug, Default)]
struct AuditedActionCorrections {
    quest_10272_completion_item_props: bool,
}

pub(super) fn load_definition(
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

fn audited_action_root(
    quest_id: u32,
    checks: &WzNodeArc,
    actions: &WzNodeArc,
    dialogue_root: &WzNodeArc,
    info_root: &WzNodeArc,
    check: &WzNodeArc,
    say: Option<&WzNodeArc>,
    info: &WzNodeArc,
) -> Result<WzNodeArc, QuestContentError> {
    if quest_id != 4_960 {
        return required_child(actions, &quest_id.to_string(), quest_id);
    }
    if wz::child(actions, "4960")?.is_some() {
        return Err(invalid(
            quest_id,
            "audited Act/4944 alias is invalid because Act/4960 now exists",
        ));
    }
    let say = say.ok_or_else(|| {
        invalid(
            quest_id,
            "audited Act/4944 alias requires the exact Say/4960 source",
        )
    })?;
    let check_4944 = required_child(checks, "4944", quest_id)?;
    let action_4944 = required_child(actions, "4944", quest_id)?;
    let say_4944 = required_child(dialogue_root, "4944", quest_id)?;
    let info_4944 = required_child(info_root, "4944", quest_id)?;
    for (node, expected, context) in [
        (check, QUEST_4960_CHECK_FINGERPRINT, "Check/4960"),
        (say, QUEST_4960_SAY_FINGERPRINT, "Say/4960"),
        (info, QUEST_4960_INFO_FINGERPRINT, "QuestInfo/4960"),
        (&check_4944, QUEST_4944_CHECK_FINGERPRINT, "Check/4944"),
        (&action_4944, QUEST_4944_ACTION_FINGERPRINT, "Act/4944"),
        (&say_4944, QUEST_4944_SAY_FINGERPRINT, "Say/4944"),
        (&info_4944, QUEST_4944_INFO_FINGERPRINT, "QuestInfo/4944"),
    ] {
        validate_audited_fingerprint(quest_id, node, expected, context)?;
    }
    validate_4944_4960_relationship(
        quest_id,
        &check_4944,
        check,
        &say_4944,
        say,
        &info_4944,
        info,
    )?;
    validate_audited_4944_action(quest_id, &action_4944)?;
    Ok(action_4944)
}

fn validate_audited_fingerprint(
    quest_id: u32,
    node: &WzNodeArc,
    expected: u64,
    context: &str,
) -> Result<(), QuestContentError> {
    let actual = audited_node_fingerprint(quest_id, node)?;
    if actual != expected {
        return Err(invalid(
            quest_id,
            format!(
                "audited {context} fingerprint changed from {expected:#018x} to {actual:#018x}"
            ),
        ));
    }
    Ok(())
}

fn validate_4944_4960_relationship(
    quest_id: u32,
    check_4944: &WzNodeArc,
    check_4960: &WzNodeArc,
    say_4944: &WzNodeArc,
    say_4960: &WzNodeArc,
    info_4944: &WzNodeArc,
    info_4960: &WzNodeArc,
) -> Result<(), QuestContentError> {
    let start_4944 = required_child(check_4944, "0", quest_id)?;
    let start_4960 = required_child(check_4960, "0", quest_id)?;
    validate_exact_children(
        quest_id,
        &start_4944,
        &["end", "lvmin", "npc", "quest"],
        "Check/4944/0",
    )?;
    validate_exact_children(
        quest_id,
        &start_4960,
        &["end", "npc", "quest"],
        "Check/4960/0",
    )?;
    if required_u32(&start_4944, "lvmin", quest_id)? != 15 {
        return Err(invalid(quest_id, "audited Check/4944/0 lvmin is not 15"));
    }
    for field in ["end", "npc", "quest"] {
        let left = required_child(&start_4944, field, quest_id)?;
        let right = required_child(&start_4960, field, quest_id)?;
        if !audited_nodes_equal(quest_id, &left, &right)? {
            return Err(invalid(
                quest_id,
                format!("Check/4944/0/{field} and Check/4960/0/{field} differ"),
            ));
        }
    }
    let completion_4944 = required_child(check_4944, "1", quest_id)?;
    let completion_4960 = required_child(check_4960, "1", quest_id)?;
    if !audited_nodes_equal(quest_id, &completion_4944, &completion_4960)? {
        return Err(invalid(
            quest_id,
            "Check/4960/1 does not exactly duplicate Check/4944/1",
        ));
    }

    for field in ["0", "1", "area", "name"] {
        let left = required_child(info_4944, field, quest_id)?;
        let right = required_child(info_4960, field, quest_id)?;
        if !audited_nodes_equal(quest_id, &left, &right)? {
            return Err(invalid(
                quest_id,
                format!("QuestInfo/4944/{field} and QuestInfo/4960/{field} differ"),
            ));
        }
    }
    let status_4944 = required_nonempty_string(info_4944, "2", quest_id)?;
    let status_4960 = required_nonempty_string(info_4960, "2", quest_id)?;
    let expected_status_4960 = status_4944.replacen("Omni Key", "#dOmni Key#k", 1);
    if status_4960 != expected_status_4960 {
        return Err(invalid(
            quest_id,
            "QuestInfo/4960/2 is not the audited formatted QuestInfo/4944/2 variant",
        ));
    }

    let completion_say_4944 = required_child(say_4944, "1", quest_id)?;
    let completion_say_4960 = required_child(say_4960, "1", quest_id)?;
    for field in ["lost", "stop"] {
        let left = required_child(&completion_say_4944, field, quest_id)?;
        let right = required_child(&completion_say_4960, field, quest_id)?;
        if !audited_nodes_equal(quest_id, &left, &right)? {
            return Err(invalid(
                quest_id,
                format!("Say/4944/1/{field} and Say/4960/1/{field} differ"),
            ));
        }
    }
    Ok(())
}

fn validate_audited_4944_action(
    quest_id: u32,
    action: &WzNodeArc,
) -> Result<(), QuestContentError> {
    require_property(quest_id, action, "audited Act/4944")?;
    validate_exact_children(quest_id, action, &["0", "1"], "audited Act/4944")?;
    let start = required_child(action, "0", quest_id)?;
    require_property(quest_id, &start, "audited Act/4944/0")?;
    validate_exact_children(quest_id, &start, &[], "audited Act/4944/0")?;
    let completion = required_child(action, "1", quest_id)?;
    require_property(quest_id, &completion, "audited Act/4944/1")?;
    validate_exact_children(
        quest_id,
        &completion,
        &["exp", "item"],
        "audited Act/4944/1",
    )?;
    if required_u32(&completion, "exp", quest_id)? != 8_000 {
        return Err(invalid(quest_id, "audited Act/4944/1 EXP is not 8000"));
    }
    let items = required_child(&completion, "item", quest_id)?;
    validate_exact_children(
        quest_id,
        &items,
        &["0", "1", "2", "3", "4", "5"],
        "audited Act/4944/1/item",
    )?;
    for (index, expected_id, expected_count) in [
        ("0", 4_031_771, 1),
        ("1", 2_022_247, -20),
        ("2", 2_022_248, -20),
        ("3", 2_022_249, -20),
        ("4", 2_022_250, -20),
        ("5", 2_022_251, 5),
    ] {
        let entry = required_child(&items, index, quest_id)?;
        validate_exact_children(
            quest_id,
            &entry,
            &["count", "id"],
            &format!("audited Act/4944/1/item/{index}"),
        )?;
        if required_u32(&entry, "id", quest_id)? != expected_id
            || required_i64(&entry, "count", quest_id)? != expected_count
        {
            return Err(invalid(
                quest_id,
                format!("audited Act/4944/1/item/{index} changed"),
            ));
        }
    }
    Ok(())
}

fn validate_audited_4960_parsed_actions(
    quest_id: u32,
    start: &QuestActions,
    completion: &QuestActions,
) -> Result<(), QuestContentError> {
    if quest_id != 4_960 {
        return Ok(());
    }
    let expected_completion = QuestActions {
        fixed_items: vec![
            QuestItemDelta {
                item_id: 4_031_771,
                count: 1,
                expiration: None,
            },
            QuestItemDelta {
                item_id: 2_022_247,
                count: -20,
                expiration: None,
            },
            QuestItemDelta {
                item_id: 2_022_248,
                count: -20,
                expiration: None,
            },
            QuestItemDelta {
                item_id: 2_022_249,
                count: -20,
                expiration: None,
            },
            QuestItemDelta {
                item_id: 2_022_250,
                count: -20,
                expiration: None,
            },
            QuestItemDelta {
                item_id: 2_022_251,
                count: 5,
                expiration: None,
            },
        ],
        experience: 8_000,
        ..QuestActions::default()
    };
    if start != &QuestActions::default() || completion != &expected_completion {
        return Err(invalid(
            quest_id,
            "audited Act/4944 alias did not parse to the exact quest 4960 actions",
        ));
    }
    Ok(())
}

fn audited_action_corrections(
    quest_id: u32,
    action: &WzNodeArc,
    completion_check: &WzNodeArc,
    say: Option<&WzNodeArc>,
) -> Result<AuditedActionCorrections, QuestContentError> {
    if quest_id != 10_272 {
        return Ok(AuditedActionCorrections::default());
    }
    let say = say.ok_or_else(|| {
        invalid(
            quest_id,
            "audited Act/10272 item metadata correction requires Say/10272 evidence",
        )
    })?;
    validate_audited_10272_action(quest_id, action)?;
    validate_audited_10272_completion_check(quest_id, completion_check)?;
    validate_audited_10272_dialogue(quest_id, say)?;
    Ok(AuditedActionCorrections {
        quest_10272_completion_item_props: true,
    })
}

fn validate_audited_10272_action(
    quest_id: u32,
    action: &WzNodeArc,
) -> Result<(), QuestContentError> {
    require_property(quest_id, action, "audited Act/10272")?;
    validate_exact_children(quest_id, action, &["0", "1"], "audited Act/10272")?;
    let start = required_child(action, "0", quest_id)?;
    require_property(quest_id, &start, "audited Act/10272/0")?;
    validate_exact_children(quest_id, &start, &[], "audited Act/10272/0")?;
    let completion = required_child(action, "1", quest_id)?;
    validate_exact_children(quest_id, &completion, &["item"], "audited Act/10272/1")?;
    let items = required_child(&completion, "item", quest_id)?;
    validate_exact_children(quest_id, &items, &["0", "1"], "audited Act/10272/1/item")?;
    for (index, expected_id) in [("0", 4_032_283), ("1", 4_032_280)] {
        let entry = required_child(&items, index, quest_id)?;
        validate_exact_children(
            quest_id,
            &entry,
            &["count", "id", "prop"],
            &format!("audited Act/10272/1/item/{index}"),
        )?;
        if required_u32(&entry, "id", quest_id)? != expected_id
            || required_i64(&entry, "count", quest_id)? != -10
            || required_i64(&entry, "prop", quest_id)? != -1
        {
            return Err(invalid(
                quest_id,
                format!("audited Act/10272/1/item/{index} changed"),
            ));
        }
    }
    Ok(())
}

fn validate_audited_10272_completion_check(
    quest_id: u32,
    completion: &WzNodeArc,
) -> Result<(), QuestContentError> {
    validate_exact_children(
        quest_id,
        completion,
        &["endscript", "item", "npc"],
        "audited Check/10272/1",
    )?;
    if required_nonempty_string(completion, "endscript", quest_id)? != "q10272e"
        || required_u32(completion, "npc", quest_id)? != 9_000_021
    {
        return Err(invalid(
            quest_id,
            "audited Check/10272/1 script or NPC changed",
        ));
    }
    let items = required_child(completion, "item", quest_id)?;
    validate_exact_children(quest_id, &items, &["0", "1"], "audited Check/10272/1/item")?;
    for (index, expected_id) in [("0", 4_032_280), ("1", 4_032_283)] {
        let entry = required_child(&items, index, quest_id)?;
        validate_exact_children(
            quest_id,
            &entry,
            &["count", "id"],
            &format!("audited Check/10272/1/item/{index}"),
        )?;
        if required_u32(&entry, "id", quest_id)? != expected_id
            || required_u32(&entry, "count", quest_id)? != 10
        {
            return Err(invalid(
                quest_id,
                format!("audited Check/10272/1/item/{index} changed"),
            ));
        }
    }
    Ok(())
}

fn validate_audited_10272_dialogue(
    quest_id: u32,
    say: &WzNodeArc,
) -> Result<(), QuestContentError> {
    validate_exact_children(quest_id, say, &["0", "1"], "audited Say/10272")?;
    let start = required_child(say, "0", quest_id)?;
    validate_exact_children(
        quest_id,
        &start,
        &["0", "1", "no", "stop", "yes"],
        "audited Say/10272/0",
    )?;
    for page in ["0", "1"] {
        require_string(
            quest_id,
            &required_child(&start, page, quest_id)?,
            &format!("audited Say/10272/0/{page}"),
        )?;
    }
    let no = required_child(&start, "no", quest_id)?;
    validate_exact_children(quest_id, &no, &["0"], "audited Say/10272/0/no")?;
    require_string(
        quest_id,
        &required_child(&no, "0", quest_id)?,
        "audited Say/10272/0/no/0",
    )?;
    let start_stop = required_child(&start, "stop", quest_id)?;
    require_property(quest_id, &start_stop, "audited Say/10272/0/stop")?;
    validate_exact_children(quest_id, &start_stop, &[], "audited Say/10272/0/stop")?;
    let yes = required_child(&start, "yes", quest_id)?;
    validate_exact_children(quest_id, &yes, &["0"], "audited Say/10272/0/yes")?;
    let materials = required_child(&yes, "0", quest_id)?;
    let materials = scalar_string(&materials)?.ok_or_else(|| {
        invalid(
            quest_id,
            "audited Say/10272/0/yes/0 material evidence is not a string",
        )
    })?;
    let references = dialogue_item_references(&materials).collect::<BTreeSet<_>>();
    if references != BTreeSet::from([4_032_280, 4_032_283])
        || !materials.contains("#t4032280# 10")
        || !materials.contains("#t4032283# 10")
    {
        return Err(invalid(
            quest_id,
            "audited Say/10272 material evidence no longer names both fixed quantities",
        ));
    }

    let completion = required_child(say, "1", quest_id)?;
    validate_exact_children(
        quest_id,
        &completion,
        &["0", "stop", "yes"],
        "audited Say/10272/1",
    )?;
    require_string(
        quest_id,
        &required_child(&completion, "0", quest_id)?,
        "audited Say/10272/1/0",
    )?;
    let stop = required_child(&completion, "stop", quest_id)?;
    validate_exact_children(quest_id, &stop, &["item"], "audited Say/10272/1/stop")?;
    let item = required_child(&stop, "item", quest_id)?;
    validate_exact_children(quest_id, &item, &["0"], "audited Say/10272/1/stop/item")?;
    require_string(
        quest_id,
        &required_child(&item, "0", quest_id)?,
        "audited Say/10272/1/stop/item/0",
    )?;
    let completion_yes = required_child(&completion, "yes", quest_id)?;
    validate_exact_children(
        quest_id,
        &completion_yes,
        &["0", "1"],
        "audited Say/10272/1/yes",
    )?;
    for page in ["0", "1"] {
        require_string(
            quest_id,
            &required_child(&completion_yes, page, quest_id)?,
            &format!("audited Say/10272/1/yes/{page}"),
        )?;
    }
    Ok(())
}

pub(super) fn item_reference_ids(
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

fn validate_check_phase_tree(
    quest_id: u32,
    checks: &WzNodeArc,
    check: &WzNodeArc,
    item_ids: &BTreeSet<u32>,
    equipment_item_ids: &BTreeSet<u32>,
    consume_effect_ids: &BTreeSet<u32>,
    monster_book_card_ids: &BTreeSet<u32>,
    morph_ids: &BTreeSet<u32>,
) -> Result<Vec<String>, QuestContentError> {
    let mut retained = Vec::new();
    for child in wz::sorted_children(check)? {
        let name = wz::node_name(&child)?;
        if matches!(name.as_str(), "0" | "1") {
            continue;
        }
        if quest_id != 4_940 || name != "4961" {
            return Err(unsupported(
                quest_id,
                "check phase metadata",
                format!("check phase field {name:?} is not supported"),
            ));
        }
        if wz::child(checks, "4961")?.is_some() {
            return Err(invalid(
                quest_id,
                "misplaced Check/4940/4961 collides with a Check root quest 4961",
            ));
        }
        require_property(quest_id, &child, "misplaced Check/4940/4961")?;
        validate_exact_children(quest_id, &child, &["0", "1"], "misplaced Check/4940/4961")?;
        let nested_start = required_child(&child, "0", quest_id)?;
        let nested_completion = required_child(&child, "1", quest_id)?;
        validate_exact_children(
            quest_id,
            &nested_start,
            &["npc", "quest"],
            "misplaced Check/4940/4961/0",
        )?;
        let nested_quests = required_child(&nested_start, "quest", quest_id)?;
        validate_exact_children(
            quest_id,
            &nested_quests,
            &["0"],
            "misplaced Check/4940/4961/0/quest",
        )?;
        let nested_quest = required_child(&nested_quests, "0", quest_id)?;
        validate_exact_children(
            quest_id,
            &nested_quest,
            &["id", "state"],
            "misplaced Check/4940/4961/0/quest/0",
        )?;
        validate_exact_children(
            quest_id,
            &nested_completion,
            &["mob", "npc"],
            "misplaced Check/4940/4961/1",
        )?;
        let nested_mobs = required_child(&nested_completion, "mob", quest_id)?;
        validate_exact_children(
            quest_id,
            &nested_mobs,
            &["0"],
            "misplaced Check/4940/4961/1/mob",
        )?;
        let nested_mob = required_child(&nested_mobs, "0", quest_id)?;
        validate_exact_children(
            quest_id,
            &nested_mob,
            &["count", "id"],
            "misplaced Check/4940/4961/1/mob/0",
        )?;
        read_start_requirements(
            4_961,
            &nested_start,
            &QuestInfo::default(),
            item_ids,
            equipment_item_ids,
            consume_effect_ids,
            monster_book_card_ids,
            morph_ids,
        )?;
        read_completion_requirements(
            4_961,
            &nested_completion,
            &QuestInfo::default(),
            item_ids,
            equipment_item_ids,
            consume_effect_ids,
            monster_book_card_ids,
            morph_ids,
        )?;
        retained.push("check/4961".to_owned());
    }
    Ok(retained)
}

fn read_start_requirements(
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

fn read_completion_requirements(
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

fn validate_check_fields(
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

fn read_item_requirements(
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

fn read_monster_book_requirements(
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

fn optional_card_count(
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

fn read_equipped_item_requirements(
    quest_id: u32,
    node: &WzNodeArc,
    equipment_item_ids: &BTreeSet<u32>,
) -> Result<QuestEquippedItemRequirements, QuestContentError> {
    Ok(QuestEquippedItemRequirements {
        all_of: read_equipped_item_list(quest_id, node, "equipAllNeed", equipment_item_ids)?,
        any_of: read_equipped_item_list(quest_id, node, "equipSelectNeed", equipment_item_ids)?,
    })
}

fn read_equipped_item_list(
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

fn read_mob_objectives(
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

fn read_quest_requirements(
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

fn read_allowed_map_ids(
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

fn read_skill_requirements(
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

fn read_effect_requirements(
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

fn read_required_morph(
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

fn read_record_conditions(
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

fn optional_record_quest_id(
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

fn read_record_predicate(
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

#[cfg(test)]
fn read_action_phase(
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

fn read_action_phase_with_corrections(
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

fn read_buff_item_action(
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

fn read_npc_animation_action(
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

fn validate_npc_animation_transitions(
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

fn validate_start_question_reachability(
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

fn read_quest_state_actions(
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

fn read_action_skills(
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

fn read_action_skill_jobs(
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

fn validate_action_phase_fields(
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
                "fieldEnter" if quest_id == 9_866 && phase == "0" => {
                    let check = authoritative_check.ok_or_else(|| {
                        invalid(
                            quest_id,
                            "Act/9866/0/fieldEnter has no authoritative Check/9866/0 to compare",
                        )
                    })?;
                    let check_field = required_child(check, "fieldEnter", quest_id)?;
                    if !audited_nodes_equal(quest_id, &child, &check_field)? {
                        return Err(invalid(
                            quest_id,
                            "Act/9866/0/fieldEnter does not exactly duplicate \
                             Check/9866/0/fieldEnter",
                        ));
                    }
                }
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

#[derive(PartialEq, Eq)]
enum AuditedNodeValue {
    Property,
    Null,
    Int(i32),
    Short(i16),
    Long(i64),
    String(String),
}

fn audited_nodes_equal(
    quest_id: u32,
    left: &WzNodeArc,
    right: &WzNodeArc,
) -> Result<bool, QuestContentError> {
    if wz::node_name(left)? != wz::node_name(right)?
        || audited_node_value(quest_id, left)? != audited_node_value(quest_id, right)?
    {
        return Ok(false);
    }
    let left_children = wz::sorted_children(left)?;
    let right_children = wz::sorted_children(right)?;
    if left_children.len() != right_children.len() {
        return Ok(false);
    }
    for (left, right) in left_children.iter().zip(&right_children) {
        if !audited_nodes_equal(quest_id, left, right)? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn audited_node_value(
    quest_id: u32,
    node: &WzNodeArc,
) -> Result<AuditedNodeValue, QuestContentError> {
    let read = node.read().map_err(|_| wz::WzContentError::Lock {
        context: "audited quest metadata",
    })?;
    if read.is_sub_property() {
        return Ok(AuditedNodeValue::Property);
    }
    if read.is_null() {
        return Ok(AuditedNodeValue::Null);
    }
    if let Some(value) = read.try_as_int() {
        return Ok(AuditedNodeValue::Int(*value));
    }
    if let Some(value) = read.try_as_short() {
        return Ok(AuditedNodeValue::Short(*value));
    }
    if let Some(value) = read.try_as_long() {
        return Ok(AuditedNodeValue::Long(*value));
    }
    if let Some(value) = read.try_as_string() {
        return value
            .get_string()
            .map(AuditedNodeValue::String)
            .map_err(|error| {
                invalid(
                    quest_id,
                    format!("audited quest metadata string could not be read: {error}"),
                )
            });
    }
    Err(invalid(
        quest_id,
        "audited quest metadata contains an unsupported WZ node type",
    ))
}

pub(super) fn audited_node_fingerprint(
    quest_id: u32,
    node: &WzNodeArc,
) -> Result<u64, QuestContentError> {
    let mut fingerprint = 0xcbf29ce484222325_u64;
    update_audited_node_fingerprint(quest_id, node, &mut fingerprint)?;
    Ok(fingerprint)
}

fn update_audited_node_fingerprint(
    quest_id: u32,
    node: &WzNodeArc,
    fingerprint: &mut u64,
) -> Result<(), QuestContentError> {
    let name = wz::node_name(node)?;
    update_fingerprint(fingerprint, &(name.len() as u64).to_le_bytes());
    update_fingerprint(fingerprint, name.as_bytes());
    match audited_node_value(quest_id, node)? {
        AuditedNodeValue::Property => update_fingerprint(fingerprint, &[0]),
        AuditedNodeValue::Null => update_fingerprint(fingerprint, &[1]),
        AuditedNodeValue::Int(value) => {
            update_fingerprint(fingerprint, &[2]);
            update_fingerprint(fingerprint, &value.to_le_bytes());
        }
        AuditedNodeValue::Short(value) => {
            update_fingerprint(fingerprint, &[3]);
            update_fingerprint(fingerprint, &value.to_le_bytes());
        }
        AuditedNodeValue::Long(value) => {
            update_fingerprint(fingerprint, &[4]);
            update_fingerprint(fingerprint, &value.to_le_bytes());
        }
        AuditedNodeValue::String(value) => {
            update_fingerprint(fingerprint, &[5]);
            update_fingerprint(fingerprint, &(value.len() as u64).to_le_bytes());
            update_fingerprint(fingerprint, value.as_bytes());
        }
    }
    let children = wz::sorted_children(node)?;
    update_fingerprint(fingerprint, &(children.len() as u64).to_le_bytes());
    for child in children {
        update_audited_node_fingerprint(quest_id, &child, fingerprint)?;
    }
    Ok(())
}

fn update_fingerprint(
    fingerprint: &mut u64,
    bytes: &[u8],
) {
    for byte in bytes {
        *fingerprint ^= u64::from(*byte);
        *fingerprint = fingerprint.wrapping_mul(0x100000001b3);
    }
}

fn read_action_record_writes(
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

fn validate_action_question_answers(
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

fn validate_numbered_strings(
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

fn validate_numbered_integers(
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

fn require_entries(
    quest_id: u32,
    indexes: &[usize],
    context: &str,
) -> Result<(), QuestContentError> {
    (!indexes.is_empty())
        .then_some(())
        .ok_or_else(|| invalid(quest_id, format!("{context} has no entries")))
}

fn validate_contiguous_indexes(
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

fn parse_decimal_name(
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

fn is_decimal_name(name: &str) -> bool {
    !name.is_empty() && name.bytes().all(|byte| byte.is_ascii_digit())
}

fn require_property(
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

fn require_string(
    quest_id: u32,
    node: &WzNodeArc,
    context: &str,
) -> Result<(), QuestContentError> {
    scalar_string(node)?
        .map(|_| ())
        .ok_or_else(|| invalid(quest_id, format!("{context} is not a string")))
}

#[cfg(test)]
fn read_action_items(
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

fn read_action_items_with_corrections(
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
        if audited_corrections.quest_10272_completion_item_props {
            let expected = match entry_name.as_str() {
                "0" => Some((4_032_283, -10, Some(-1))),
                "1" => Some((4_032_280, -10, Some(-1))),
                _ => None,
            };
            if quest_id != 10_272
                || phase != "1"
                || expected != Some((item_id, count, prop))
                || eligibility != QuestRewardEligibility::default()
            {
                return Err(invalid(
                    quest_id,
                    format!(
                        "audited Act/10272/1/item/{entry_name}/prop=-1 correction no longer has \
                         its exact item removal shape"
                    ),
                ));
            }
            fixed.push(QuestItemDelta {
                item_id,
                count,
                expiration,
            });
            retained_fields.push(format!("act/1/item/{entry_name}/prop=-1"));
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

fn read_item_expiration(
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

fn item_expiration_unix_ms(source: &str) -> Result<u64, String> {
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

fn validate_selectable_reward_flow(
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

fn validate_lost_item_restoration_flow(
    quest_id: u32,
    completion: &QuestCompletionRequirements,
    start_actions: &QuestActions,
    completion_actions: &QuestActions,
    dialogue: &QuestDialogue,
) -> Result<Vec<QuestRestorableItem>, QuestContentError> {
    if dialogue.completion.lost.is_none() {
        return Ok(Vec::new());
    }
    if let Some(rule) = super::restoration::audited_rule(quest_id) {
        return validate_audited_lost_item_restoration(
            quest_id,
            completion,
            start_actions,
            completion_actions,
            dialogue,
            rule,
        );
    }
    let ambiguous_item_id = start_actions
        .conditional_items
        .iter()
        .map(|action| action.item_id)
        .chain(
            start_actions
                .weighted_items
                .iter()
                .map(|action| action.item_id),
        )
        .chain(
            start_actions
                .selectable_items
                .iter()
                .map(|action| action.item_id),
        )
        .next();
    if let Some(item_id) = ambiguous_item_id {
        return Err(unsupported(
            quest_id,
            "lost-item restoration action ambiguity",
            format!(
                "lost dialogue has a conditional, weighted, or selectable start grant for item \
                 {item_id}"
            ),
        ));
    }

    let mut restorable_items = Vec::new();
    let item_ids = start_actions
        .fixed_items
        .iter()
        .filter(|action| action.count > 0)
        .map(|action| action.item_id)
        .collect::<BTreeSet<_>>();
    let referenced_item_ids = dialogue
        .completion
        .lost
        .iter()
        .flat_map(|lost| lost.prompt_pages.iter().chain(&lost.success_pages))
        .flat_map(|page| dialogue_item_references(page))
        .collect::<BTreeSet<_>>();
    let completion_item_ids = completion
        .items
        .iter()
        .filter(|requirement| matches!(requirement.condition, QuestItemCondition::AtLeast(_)))
        .map(|requirement| requirement.item_id)
        .collect::<BTreeSet<_>>();
    if item_ids.is_disjoint(&completion_item_ids)
        && !referenced_item_ids.is_empty()
        && item_ids.is_disjoint(&referenced_item_ids)
    {
        return Err(unsupported(
            quest_id,
            "lost-item restoration action ambiguity",
            format!(
                "fixed start grants {item_ids:?} contradict the items named by the lost dialogue"
            ),
        ));
    }
    for item_id in item_ids {
        let actions = start_actions
            .fixed_items
            .iter()
            .filter(|action| action.item_id == item_id)
            .collect::<Vec<_>>();
        if actions.len() != 1 {
            return Err(unsupported(
                quest_id,
                "lost-item restoration action ambiguity",
                format!("restorable item {item_id} has multiple or mixed-sign fixed start actions"),
            ));
        }
        let action = actions[0];
        if action.count <= 0 {
            continue;
        }
        restorable_items.push(QuestRestorableItem {
            item_id,
            target_count: action.count.unsigned_abs(),
            expiration: action.expiration,
            provenance: QuestRestorationProvenance::InferredStartGrant,
            eligibility: QuestRestorationEligibility {
                owner_state: RequiredQuestState::Started,
                required_quests: &[],
                forbidden_quests: &[],
                absent_skill_ids: &[],
                absent_item_ids: &[],
            },
        });
    }
    if restorable_items.is_empty() {
        return Err(unsupported(
            quest_id,
            "lost-item restoration item mapping",
            "completion lost dialogue has no positive unconditional fixed start grant",
        ));
    }
    Ok(restorable_items)
}

fn validate_audited_lost_item_restoration(
    quest_id: u32,
    completion: &QuestCompletionRequirements,
    start_actions: &QuestActions,
    completion_actions: &QuestActions,
    dialogue: &QuestDialogue,
    rule: &super::restoration::AuditedRestorationRule,
) -> Result<Vec<QuestRestorableItem>, QuestContentError> {
    match rule.provenance {
        QuestRestorationProvenance::AuditedCompletionGrant => {
            let fixed = completion_actions
                .fixed_items
                .iter()
                .filter(|action| action.item_id == rule.item_id)
                .collect::<Vec<_>>();
            let ambiguous = completion_actions
                .conditional_items
                .iter()
                .map(|action| action.item_id)
                .chain(
                    completion_actions
                        .weighted_items
                        .iter()
                        .map(|action| action.item_id),
                )
                .chain(
                    completion_actions
                        .selectable_items
                        .iter()
                        .map(|action| action.item_id),
                )
                .any(|item_id| item_id == rule.item_id);
            if fixed.len() != 1 || fixed[0].count != 1 || fixed[0].expiration.is_some() || ambiguous
            {
                return Err(unsupported(
                    quest_id,
                    "audited lost-item restoration evidence",
                    format!(
                        "audited item {} is not exactly one permanent unconditional fixed +1 \
                         completion action",
                        rule.item_id
                    ),
                ));
            }
        }
        QuestRestorationProvenance::AuditedReactorDevice => {
            let referenced_item_ids = dialogue
                .completion
                .lost
                .iter()
                .flat_map(|lost| lost.prompt_pages.iter().chain(&lost.success_pages))
                .flat_map(|page| dialogue_item_references(page))
                .collect::<BTreeSet<_>>();
            let action_references_item = |actions: &QuestActions| {
                actions
                    .fixed_items
                    .iter()
                    .map(|action| action.item_id)
                    .chain(
                        actions
                            .conditional_items
                            .iter()
                            .map(|action| action.item_id),
                    )
                    .chain(actions.weighted_items.iter().map(|action| action.item_id))
                    .chain(actions.selectable_items.iter().map(|action| action.item_id))
                    .any(|item_id| item_id == rule.item_id)
            };
            if quest_id != 3_310
                || rule.item_id != 4_031_698
                || referenced_item_ids != BTreeSet::from([rule.item_id])
                || !matches!(
                    completion.items.as_slice(),
                    [QuestItemRequirement {
                        item_id: 4_031_709,
                        condition: QuestItemCondition::AtLeast(count),
                    }] if count.get() == 1
                )
                || action_references_item(start_actions)
                || action_references_item(completion_actions)
            {
                return Err(unsupported(
                    quest_id,
                    "audited lost-item restoration evidence",
                    "the reactor-device exception does not have the exact quest 3310/item 4031698 \
                     action shape",
                ));
            }
        }
        QuestRestorationProvenance::InferredStartGrant => {
            return Err(invalid(
                quest_id,
                "an audited restoration rule cannot use inferred start-grant provenance",
            ));
        }
    }

    Ok(vec![QuestRestorableItem {
        item_id: rule.item_id,
        target_count: rule.target_count,
        expiration: None,
        provenance: rule.provenance,
        eligibility: rule.eligibility,
    }])
}

fn dialogue_item_references(page: &str) -> impl Iterator<Item = u32> + '_ {
    page.match_indices("#t").filter_map(|(index, _)| {
        let digits = page[index + 2..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>();
        (!digits.is_empty())
            .then(|| digits.parse::<u32>().ok())
            .flatten()
    })
}

fn read_reward_eligibility(
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

fn unsupported_action_field(
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

fn read_info(
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
            if quest_id == 8_833 && status == 4_963 {
                validate_misplaced_quest_info(quest_id, &child, info_root)?;
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

fn retain_audited_stray_selected_mob(
    quest_id: u32,
    info: &WzNodeArc,
) -> Result<bool, QuestContentError> {
    let Some((_, expected_fingerprint)) = AUDITED_STRAY_SELECTED_MOB_INFO_FINGERPRINTS
        .iter()
        .find(|(audited_quest_id, _)| *audited_quest_id == quest_id)
    else {
        return Ok(false);
    };
    if required_u32(info, "selectedMob", quest_id)? != 1 {
        return Err(invalid(
            quest_id,
            "audited inert QuestInfo selectedMob marker is not 1",
        ));
    }
    validate_audited_fingerprint(
        quest_id,
        info,
        *expected_fingerprint,
        &format!("QuestInfo/{quest_id} stray selectedMob metadata"),
    )?;
    Ok(true)
}

fn validate_misplaced_quest_info(
    quest_id: u32,
    node: &WzNodeArc,
    info_root: &WzNodeArc,
) -> Result<(), QuestContentError> {
    require_property(quest_id, node, "misplaced QuestInfo/8833/4963")?;
    validate_exact_children(
        quest_id,
        node,
        &["0", "1", "2", "area", "name", "order", "parent"],
        "misplaced QuestInfo/8833/4963",
    )?;
    for status in ["0", "1", "2"] {
        required_nonempty_string(node, status, quest_id)?;
    }
    required_u32(node, "area", quest_id)?;
    required_nonempty_string(node, "name", quest_id)?;
    required_u32(node, "order", quest_id)?;
    required_nonempty_string(node, "parent", quest_id)?;

    let canonical = required_child(info_root, "4963", quest_id)?;
    require_property(quest_id, &canonical, "canonical QuestInfo/4963")?;
    let misplaced_name = required_nonempty_string(node, "name", quest_id)?;
    let canonical_name = required_nonempty_string(&canonical, "name", quest_id)?;
    if misplaced_name == canonical_name {
        return Err(invalid(
            quest_id,
            "misplaced QuestInfo/8833/4963 unexpectedly duplicates canonical QuestInfo/4963",
        ));
    }
    Ok(())
}

fn read_selected_skill(
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

fn read_days_of_week(
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

fn optional_calendar(
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

fn calendar_unix_ms(source: &str) -> Result<u64, String> {
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

fn optional_time_limit_seconds(
    node: &WzNodeArc,
    name: &str,
    quest_id: u32,
) -> Result<Option<u64>, QuestContentError> {
    optional_positive_u64(node, name, quest_id)?
        .map(|seconds| quest_timer_milliseconds(seconds, name, quest_id))
        .transpose()
}

fn quest_timer_milliseconds(
    seconds: u64,
    field: &str,
    quest_id: u32,
) -> Result<u64, QuestContentError> {
    seconds
        .checked_mul(1_000)
        .ok_or_else(|| invalid(quest_id, format!("{field} duration is too large")))
}

fn validate_item_id(
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

fn read_u32_list(
    quest_id: u32,
    node: &WzNodeArc,
    name: &str,
) -> Result<Vec<u32>, QuestContentError> {
    let Some(values) = wz::child(node, name)? else {
        return Ok(Vec::new());
    };
    let mut output = Vec::new();
    for value in wz::sorted_children(&values)? {
        output.push(
            u32::try_from(scalar_i64(&value)?.ok_or_else(|| {
                invalid(quest_id, format!("{name} contains a non-integer value"))
            })?)
            .map_err(|_| invalid(quest_id, format!("{name} contains a negative value")))?,
        );
    }
    output.sort_unstable();
    output.dedup();
    Ok(output)
}

fn validate_children(
    quest_id: u32,
    node: &WzNodeArc,
    allowed: &[&str],
    context: &str,
) -> Result<(), QuestContentError> {
    for child in wz::sorted_children(node)? {
        let name = wz::node_name(&child)?;
        if !allowed.contains(&name.as_str()) {
            return Err(unsupported(
                quest_id,
                format!("{context} metadata"),
                format!("{context} field {name:?} is not supported"),
            ));
        }
    }
    Ok(())
}

fn validate_exact_children(
    quest_id: u32,
    node: &WzNodeArc,
    expected: &[&str],
    context: &str,
) -> Result<(), QuestContentError> {
    let actual = wz::sorted_children(node)?
        .iter()
        .map(wz::node_name)
        .collect::<Result<BTreeSet<_>, _>>()?;
    let expected = expected
        .iter()
        .map(|name| (*name).to_owned())
        .collect::<BTreeSet<_>>();
    if actual != expected {
        return Err(invalid(
            quest_id,
            format!("{context} has fields {actual:?}, expected exactly {expected:?}"),
        ));
    }
    Ok(())
}

pub(super) fn required_child(
    node: &WzNodeArc,
    name: &str,
    quest_id: u32,
) -> Result<WzNodeArc, QuestContentError> {
    wz::child(node, name)?
        .ok_or_else(|| invalid(quest_id, format!("required node {name:?} is missing")))
}

pub(super) fn required_u32(
    node: &WzNodeArc,
    name: &str,
    quest_id: u32,
) -> Result<u32, QuestContentError> {
    optional_u32(node, name, quest_id)?
        .ok_or_else(|| invalid(quest_id, format!("required integer {name:?} is missing")))
}

fn required_positive_u32(
    node: &WzNodeArc,
    name: &str,
    quest_id: u32,
) -> Result<u32, QuestContentError> {
    required_u32(node, name, quest_id).and_then(|value| {
        (value > 0)
            .then_some(value)
            .ok_or_else(|| invalid(quest_id, format!("integer {name:?} must be positive")))
    })
}

pub(super) fn optional_u32(
    node: &WzNodeArc,
    name: &str,
    quest_id: u32,
) -> Result<Option<u32>, QuestContentError> {
    optional_i64(node, name, quest_id)?
        .map(|value| {
            u32::try_from(value).map_err(|_| {
                invalid(
                    quest_id,
                    format!("integer {name:?} is negative or too large"),
                )
            })
        })
        .transpose()
}

fn optional_positive_u32(
    node: &WzNodeArc,
    name: &str,
    quest_id: u32,
) -> Result<Option<u32>, QuestContentError> {
    optional_u32(node, name, quest_id)?
        .map(|value| {
            (value > 0)
                .then_some(value)
                .ok_or_else(|| invalid(quest_id, format!("integer {name:?} must be positive")))
        })
        .transpose()
}

fn optional_strict_u32(
    node: &WzNodeArc,
    name: &str,
    quest_id: u32,
) -> Result<Option<u32>, QuestContentError> {
    let Some(value) = wz::child(node, name)? else {
        return Ok(None);
    };
    if let Some(value) = scalar_i64(&value)? {
        return u32::try_from(value).map(Some).map_err(|_| {
            invalid(
                quest_id,
                format!("integer {name:?} is negative or too large"),
            )
        });
    }
    let source = raw_scalar_string(&value)?.ok_or_else(|| {
        invalid(
            quest_id,
            format!("property {name:?} is not an integer or strictly decimal string"),
        )
    })?;
    crate::quest_records::strict_decimal(&source)
        .and_then(|value| u32::try_from(value).ok())
        .map(Some)
        .ok_or_else(|| {
            invalid(
                quest_id,
                format!("property {name:?} is not a valid u32 decimal value"),
            )
        })
}

fn optional_positive_u64(
    node: &WzNodeArc,
    name: &str,
    quest_id: u32,
) -> Result<Option<u64>, QuestContentError> {
    optional_i64(node, name, quest_id)?
        .map(|value| {
            u64::try_from(value)
                .ok()
                .filter(|value| *value > 0)
                .ok_or_else(|| invalid(quest_id, format!("integer {name:?} must be positive")))
        })
        .transpose()
}

fn optional_nonnegative_u64(
    node: &WzNodeArc,
    name: &str,
    quest_id: u32,
) -> Result<Option<u64>, QuestContentError> {
    optional_i64(node, name, quest_id)?
        .map(|value| {
            u64::try_from(value)
                .map_err(|_| invalid(quest_id, format!("integer {name:?} must not be negative")))
        })
        .transpose()
}

fn required_i64(
    node: &WzNodeArc,
    name: &str,
    quest_id: u32,
) -> Result<i64, QuestContentError> {
    optional_i64(node, name, quest_id)?
        .ok_or_else(|| invalid(quest_id, format!("required integer {name:?} is missing")))
}

pub(super) fn optional_i64(
    node: &WzNodeArc,
    name: &str,
    quest_id: u32,
) -> Result<Option<i64>, QuestContentError> {
    let Some(value) = wz::child(node, name)? else {
        return Ok(None);
    };
    scalar_i64(&value)?.map(Some).ok_or_else(|| {
        invalid(
            quest_id,
            format!("property {name:?} is not a supported integer"),
        )
    })
}

fn optional_bool(
    node: &WzNodeArc,
    name: &str,
    quest_id: u32,
) -> Result<Option<bool>, QuestContentError> {
    optional_i64(node, name, quest_id).map(|value| value.map(|value| value != 0))
}

fn required_nonempty_string(
    node: &WzNodeArc,
    name: &str,
    quest_id: u32,
) -> Result<String, QuestContentError> {
    optional_nonempty_string(node, name, quest_id)?.ok_or_else(|| {
        invalid(
            quest_id,
            format!("required string {name:?} is missing or empty"),
        )
    })
}

fn optional_nonempty_string(
    node: &WzNodeArc,
    name: &str,
    quest_id: u32,
) -> Result<Option<String>, QuestContentError> {
    optional_string(node, name).and_then(|value| {
        value
            .map(|value| {
                let value = value.trim().to_owned();
                (!value.is_empty())
                    .then_some(value)
                    .ok_or_else(|| invalid(quest_id, format!("string {name:?} is empty")))
            })
            .transpose()
    })
}

fn optional_string(
    node: &WzNodeArc,
    name: &str,
) -> Result<Option<String>, QuestContentError> {
    let Some(value) = wz::child(node, name)? else {
        return Ok(None);
    };
    scalar_string(&value).map(|value| value.map(normalize_text))
}

fn scalar_i64(node: &WzNodeArc) -> Result<Option<i64>, QuestContentError> {
    let read = node.read().map_err(|_| wz::WzContentError::Lock {
        context: "quest integer value",
    })?;
    Ok(read
        .try_as_int()
        .map(|value| i64::from(*value))
        .or_else(|| read.try_as_short().map(|value| i64::from(*value)))
        .or_else(|| read.try_as_long().copied()))
}

pub(super) fn scalar_string(node: &WzNodeArc) -> Result<Option<String>, QuestContentError> {
    raw_scalar_string(node).map(|value| value.map(normalize_text))
}

fn raw_scalar_string(node: &WzNodeArc) -> Result<Option<String>, QuestContentError> {
    let read = node.read().map_err(|_| wz::WzContentError::Lock {
        context: "quest string value",
    })?;
    Ok(read
        .try_as_string()
        .and_then(|value| value.get_string().ok()))
}

fn required_record_string(
    quest_id: u32,
    node: &WzNodeArc,
    context: &str,
) -> Result<String, QuestContentError> {
    let value = raw_scalar_string(node)?
        .ok_or_else(|| invalid(quest_id, format!("{context} is not a string")))?;
    crate::quest_records::validate_value(&value)
        .map_err(|error| invalid(quest_id, format!("{context}: {error}")))?;
    Ok(value)
}

fn is_null(node: &WzNodeArc) -> Result<bool, QuestContentError> {
    let read = node.read().map_err(|_| wz::WzContentError::Lock {
        context: "quest null value",
    })?;
    Ok(read.is_null())
}

fn normalize_text(value: String) -> String {
    value.replace("\\n", "\n")
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::collections::BTreeSet;
    use std::num::NonZeroU32;
    use std::path::Path;

    use wz_reader::WzNode;
    use wz_reader::WzNodeArc;
    use wz_reader::WzObjectType;
    use wz_reader::property::WzString;
    use wz_reader::property::WzSubProperty;

    use super::audited_action_corrections;
    use super::audited_action_root;
    use super::calendar_unix_ms;
    use super::item_expiration_unix_ms;
    use super::quest_timer_milliseconds;
    use super::read_action_items;
    use super::read_action_items_with_corrections;
    use super::read_action_phase as read_action_phase_with_effects;
    use super::read_completion_requirements as read_completion_requirements_with_effects;
    use super::read_info as read_info_with_skills;
    use super::read_record_conditions;
    use super::read_start_requirements as read_start_requirements_with_effects;
    use super::validate_audited_4944_action;
    use super::validate_audited_4960_parsed_actions;
    use super::validate_lost_item_restoration_flow;
    use super::validate_selectable_reward_flow;
    use crate::content::QuestActions;
    use crate::content::QuestCompletionRequirements;
    use crate::content::QuestConditionalItemReward;
    use crate::content::QuestDialogue;
    use crate::content::QuestInfo;
    use crate::content::QuestItemCondition;
    use crate::content::QuestItemDelta;
    use crate::content::QuestItemExpiration;
    use crate::content::QuestItemRequirement;
    use crate::content::QuestLostItemDialogue;
    use crate::content::QuestRecordPredicate;
    use crate::content::QuestRecordWrite;
    use crate::content::QuestRewardEligibility;
    use crate::content::QuestRewardGender;
    use crate::content::QuestSelectableItemReward;
    use crate::content::QuestStateAction;
    use crate::content::QuestStateActionState;
    use crate::content::QuestWeightedItem;
    use crate::content::quest::QuestContentError;

    #[test]
    fn local_unknown_check_fields_only_contain_user_interact() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/Quest.wz");
        if !path.exists() {
            return;
        }
        let root = crate::content::wz::open_archive(&path).expect("quest archive");
        crate::content::wz::parse(&root, "quest archive root".to_owned())
            .expect("parse quest archive");
        let checks = super::required_child(&root, "Check.img", 0).expect("Check.img");
        crate::content::wz::parse(&checks, "quest checks".to_owned()).expect("parse checks");
        let mut unknown = Vec::new();
        for quest in crate::content::wz::sorted_children(&checks).expect("quest checks") {
            for phase_name in ["0", "1"] {
                let Some(phase) = crate::content::wz::child(&quest, phase_name).expect("phase")
                else {
                    continue;
                };
                let allowed = if phase_name == "0" {
                    super::START_CHECK_FIELDS
                } else {
                    super::COMPLETION_CHECK_FIELDS
                };
                unknown.extend(
                    crate::content::wz::sorted_children(&phase)
                        .expect("check fields")
                        .into_iter()
                        .map(|field| crate::content::wz::node_name(&field).expect("field name"))
                        .filter(|name| {
                            !allowed.contains(&name.as_str())
                                && !matches!(
                                    name.as_str(),
                                    "pet"
                                        | "pettamenessmin"
                                        | "petAutoSpeakingLimit"
                                        | "petRecallLimit"
                                        | "tamingmoblevelmin"
                                        | "partyQuest_S"
                                        | "info"
                                        | "infoNumber"
                                        | "infoex"
                                        | "buff"
                                        | "exceptbuff"
                                        | "skill"
                                        | "fieldEnter"
                                )
                        }),
                );
            }
        }
        assert_eq!(unknown, vec!["userInteract"]);
    }

    #[test]
    fn local_stray_selected_mob_metadata_matches_audited_fingerprints() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/Quest.wz");
        if !path.exists() {
            return;
        }
        let root = crate::content::wz::open_archive(&path).expect("quest archive");
        crate::content::wz::parse(&root, "quest archive root".to_owned())
            .expect("parse quest archive");
        let info = super::required_child(&root, "QuestInfo.img", 0).expect("QuestInfo.img");
        crate::content::wz::parse(&info, "quest info".to_owned()).expect("parse quest info");
        for quest_id in [3_954, 4_006, 4_484, 6_012] {
            let node =
                super::required_child(&info, &quest_id.to_string(), quest_id).expect("quest info");
            assert!(
                super::retain_audited_stray_selected_mob(quest_id, &node)
                    .expect("audited selectedMob metadata")
            );
        }
    }

    #[test]
    fn selected_mob_metadata_fails_closed_outside_the_audited_records() {
        let info = property("3954");
        add_integer(&info, "selectedMob", 1);
        assert!(matches!(
            super::retain_audited_stray_selected_mob(3_954, &info),
            Err(QuestContentError::Invalid { .. })
        ));

        let unaudited = property("100");
        add_integer(&unaudited, "selectedMob", 1);
        assert!(matches!(
            read_info(100, &unaudited),
            Err(QuestContentError::Unsupported { category, .. })
                if category == "quest info mechanic"
        ));
    }

    #[test]
    fn malformed_item_reference_scan_returns_a_quest_error() {
        let checks = property("checks");
        let actions = property("actions");
        let quest = property("100");
        let phase = property("0");
        let items = property("item");
        let entry = property("0");
        add_string(&entry, "count", "not-an-integer");
        add_integer(&entry, "id", 4_000_000);
        add_child(&items, &entry);
        add_child(&phase, &items);
        add_child(&quest, &phase);
        add_child(&actions, &quest);

        let error = super::item_reference_ids(100, &checks, &actions)
            .expect_err("malformed reference metadata must fail closed for its quest");

        assert!(matches!(
            error,
            QuestContentError::Invalid { quest_id: 100, message }
                if message.contains("count")
        ));
    }

    #[test]
    fn monster_book_checks_parse_strict_card_and_unique_bounds() {
        let node = property("1");
        add_integer(&node, "mbmin", 1);
        add_integer(&node, "mbmax", 2);
        let cards = property("mbcard");
        let first = property("0");
        add_integer(&first, "id", 2_380_000);
        add_integer(&first, "min", 1);
        add_integer(&first, "max", 4);
        add_child(&cards, &first);
        add_child(&node, &cards);

        let requirements =
            super::read_monster_book_requirements(100, &node, &BTreeSet::from([2_380_000]))
                .expect("Monster Book requirements");
        assert_eq!(requirements.minimum_unique_cards, Some(1));
        assert_eq!(requirements.maximum_unique_cards, Some(2));
        assert_eq!(requirements.cards.len(), 1);
        assert_eq!(requirements.cards[0].card_item_id, 2_380_000);
        assert_eq!(requirements.cards[0].minimum_count, Some(1));
        assert_eq!(requirements.cards[0].maximum_count, Some(4));

        let malformed = property("1");
        let cards = property("mbcard");
        let entry = property("0");
        add_integer(&entry, "id", 2_380_000);
        add_string(&entry, "min", "1");
        add_child(&cards, &entry);
        add_child(&malformed, &cards);
        assert!(
            super::read_monster_book_requirements(100, &malformed, &BTreeSet::from([2_380_000]),)
                .is_err()
        );

        let invalid_bounds = property("1");
        add_integer(&invalid_bounds, "mbmin", 2);
        add_integer(&invalid_bounds, "mbmax", 1);
        assert!(
            super::read_monster_book_requirements(100, &invalid_bounds, &BTreeSet::new()).is_err()
        );
    }

    fn read_start_requirements(
        quest_id: u32,
        node: &WzNodeArc,
        info: &QuestInfo,
        item_ids: &BTreeSet<u32>,
        equipment_item_ids: &BTreeSet<u32>,
    ) -> Result<crate::content::QuestStartRequirements, QuestContentError> {
        read_start_requirements_with_effects(
            quest_id,
            node,
            info,
            item_ids,
            equipment_item_ids,
            &BTreeSet::new(),
            &BTreeSet::new(),
            &BTreeSet::new(),
        )
    }

    fn read_completion_requirements(
        quest_id: u32,
        node: &WzNodeArc,
        info: &QuestInfo,
        item_ids: &BTreeSet<u32>,
        equipment_item_ids: &BTreeSet<u32>,
    ) -> Result<crate::content::QuestCompletionRequirements, QuestContentError> {
        read_completion_requirements_with_effects(
            quest_id,
            node,
            info,
            item_ids,
            equipment_item_ids,
            &BTreeSet::new(),
            &BTreeSet::new(),
            &BTreeSet::new(),
        )
    }

    fn read_action_phase_with_skills(
        quest_id: u32,
        action: &WzNodeArc,
        phase: &str,
        item_ids: &BTreeSet<u32>,
        equipment_item_ids: &BTreeSet<u32>,
        skill_ids: &BTreeSet<u32>,
        archive_quest_ids: &BTreeSet<u32>,
        authoritative_check: Option<&WzNodeArc>,
    ) -> Result<(QuestActions, Vec<String>), QuestContentError> {
        read_action_phase_with_effects(
            quest_id,
            action,
            phase,
            item_ids,
            equipment_item_ids,
            &BTreeSet::new(),
            skill_ids,
            archive_quest_ids,
            authoritative_check,
        )
    }

    #[test]
    fn local_skill_action_field_distribution_is_stable() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/Quest.wz");
        if !path.exists() {
            return;
        }
        let root = crate::content::wz::open_archive(&path).expect("quest archive");
        crate::content::wz::parse(&root, "quest archive root".to_owned())
            .expect("parse quest archive");
        let actions = super::required_child(&root, "Act.img", 0).expect("Act.img");
        crate::content::wz::parse(&actions, "quest action archive".to_owned())
            .expect("parse quest actions");
        let mut quest_count = 0;
        let mut entry_count = 0;
        let mut field_counts = BTreeMap::<String, usize>::new();
        for quest in crate::content::wz::sorted_children(&actions).expect("quests") {
            for phase_name in ["0", "1"] {
                let Some(phase) = crate::content::wz::child(&quest, phase_name).expect("phase")
                else {
                    continue;
                };
                let Some(skills) = crate::content::wz::child(&phase, "skill").expect("skills")
                else {
                    continue;
                };
                quest_count += 1;
                for entry in crate::content::wz::sorted_children(&skills).expect("skill entries") {
                    entry_count += 1;
                    let mut entry_fields = BTreeSet::new();
                    for field in crate::content::wz::sorted_children(&entry).expect("fields") {
                        let name = crate::content::wz::node_name(&field).expect("field name");
                        *field_counts.entry(name.clone()).or_default() += 1;
                        entry_fields.insert(name);
                    }
                    if entry_fields.contains("onlyMasterLevel") {
                        assert!(
                            !entry_fields.contains("skillLevel"),
                            "local onlyMasterLevel actions must not author a skillLevel"
                        );
                    }
                }
            }
        }

        assert_eq!(quest_count, 65);
        assert_eq!(entry_count, 80);
        assert_eq!(
            field_counts,
            BTreeMap::from([
                ("acquire".to_owned(), 1),
                ("id".to_owned(), 80),
                ("job".to_owned(), 77),
                ("masterLevel".to_owned(), 79),
                ("onlyMasterLevel".to_owned(), 8),
                ("skillLevel".to_owned(), 22),
            ])
        );
    }

    #[test]
    fn local_quest_state_action_distribution_is_stable() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/Quest.wz");
        if !path.exists() {
            return;
        }
        let root = crate::content::wz::open_archive(&path).expect("quest archive");
        crate::content::wz::parse(&root, "quest archive root".to_owned())
            .expect("parse quest archive");
        let actions = super::required_child(&root, "Act.img", 0).expect("Act.img");
        crate::content::wz::parse(&actions, "quest action archive".to_owned())
            .expect("parse quest actions");
        let mut state_actions = BTreeMap::<u32, Vec<(u32, i64)>>::new();
        for quest in crate::content::wz::sorted_children(&actions).expect("quests") {
            let quest_id = crate::content::wz::node_name(&quest)
                .expect("quest name")
                .parse::<u32>()
                .expect("numeric quest name");
            for phase_name in ["0", "1"] {
                let Some(phase) = crate::content::wz::child(&quest, phase_name).expect("phase")
                else {
                    continue;
                };
                let Some(entries) =
                    crate::content::wz::child(&phase, "quest").expect("quest field")
                else {
                    continue;
                };
                for entry in crate::content::wz::sorted_children(&entries).expect("state entries") {
                    let target = super::required_u32(&entry, "id", quest_id).expect("target ID");
                    let state = super::required_i64(&entry, "state", quest_id).expect("state");
                    state_actions
                        .entry(quest_id)
                        .or_default()
                        .push((target, state));
                }
            }
        }

        assert_eq!(
            state_actions,
            BTreeMap::from([
                (2_101, vec![(2_100, 2)]),
                (2_145, vec![(2_144, 2)]),
                (2_199, vec![(2_198, 2)]),
                (2_200, vec![(2_198, 2)]),
                (2_201, vec![(2_199, 2), (2_200, 2)]),
                (2_202, vec![(2_201, 2)]),
                (2_203, vec![(2_202, 2)]),
                (2_206, vec![(2_205, 2)]),
                (3_081, vec![(3_080, 2)]),
                (3_082, vec![(3_081, 2)]),
                (3_335, vec![(3_334, 2)]),
                (3_528, vec![(3_521, 2)]),
                (3_537, vec![(3_521, 2)]),
                (3_642, vec![(3_641, 2)]),
                (3_946, vec![(3_926, 2)]),
                (4_308, vec![(4_307, 2)]),
                (6_034, vec![(6_033, 2)]),
                (20_301, vec![(20_300, 2)]),
            ])
        );
    }

    #[test]
    fn local_skill_edge_cases_keep_their_strict_classification() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/Quest.wz");
        if !path.exists() {
            return;
        }
        let root = crate::content::wz::open_archive(&path).expect("quest archive");
        crate::content::wz::parse(&root, "quest archive root".to_owned())
            .expect("parse quest archive");
        let actions = super::required_child(&root, "Act.img", 0).expect("Act.img");
        crate::content::wz::parse(&actions, "quest action archive".to_owned())
            .expect("parse quest actions");

        let phase = local_action_phase(&actions, 6_121, "1");
        let mastered = super::read_action_skills(6_121, "1", &phase, &BTreeSet::from([2_321_003]))
            .expect("local master-only action");
        assert_eq!(
            mastered[0].operation,
            crate::content::QuestSkillOperation::Grant {
                skill_level: 0,
                master_level: 15,
            }
        );
        assert_eq!(mastered[0].job_ids, vec![232]);

        let duplicate_jobs = local_action_phase(&actions, 6_012, "1");
        assert!(matches!(
            super::read_action_skills(6_012, "1", &duplicate_jobs, &BTreeSet::from([1_003, 1_004]),),
            Err(QuestContentError::Invalid { .. })
        ));

        let unknown = local_action_phase(&actions, 6_000, "1");
        assert!(matches!(
            super::read_action_skills(
                6_000,
                "1",
                &unknown,
                &BTreeSet::from([1_003, 10_001_003, 20_001_003]),
            ),
            Err(QuestContentError::Unsupported { category, .. })
                if category == "unknown skill reference"
        ));

        let removal = local_action_phase(&actions, 6_034, "0");
        let removed = super::read_action_skills(6_034, "0", &removal, &BTreeSet::from([1_007]))
            .expect("local exact skill removal");
        assert_eq!(removed[0].skill_id, 1_007);
        assert_eq!(
            removed[0].operation,
            crate::content::QuestSkillOperation::Remove
        );
        assert!(removed[0].job_ids.is_empty());
    }

    #[test]
    fn skill_actions_import_master_only_defaults_and_exact_jobs() {
        let action = property("quest");
        let phase = property("1");
        let skills = property("skill");
        let entry = property("0");
        add_integer(&entry, "id", 2_321_003);
        add_integer(&entry, "masterLevel", 15);
        add_integer(&entry, "onlyMasterLevel", 1);
        let jobs = property("job");
        add_integer(&jobs, "0", 232);
        add_integer(&jobs, "1", 0);
        add_child(&entry, &jobs);
        add_child(&skills, &entry);
        add_child(&phase, &skills);
        add_child(&action, &phase);

        let (actions, retained) = read_action_phase_with_skills(
            100,
            &action,
            "1",
            &BTreeSet::new(),
            &BTreeSet::new(),
            &BTreeSet::from([2_321_003]),
            &BTreeSet::from([100]),
            None,
        )
        .expect("master-only skill action");

        assert!(retained.is_empty());
        assert_eq!(
            actions.skill_changes,
            vec![crate::content::QuestSkillChange {
                skill_id: 2_321_003,
                operation: crate::content::QuestSkillOperation::Grant {
                    skill_level: 0,
                    master_level: 15,
                },
                job_ids: vec![232, 0],
            }]
        );
    }

    #[test]
    fn malformed_skill_actions_and_unknown_references_fail_closed() {
        for malformed in [
            "duplicate_job",
            "unknown_field",
            "bad_level",
            "bad_only_master",
            "missing_id",
            "zero_id",
            "missing_master",
            "duplicate_skill",
        ] {
            let action = property("quest");
            let phase = property("1");
            let skills = property("skill");
            let entry = property("0");
            if malformed != "missing_id" {
                add_integer(&entry, "id", if malformed == "zero_id" { 0 } else { 1_000 });
            }
            if malformed != "missing_master" {
                add_integer(&entry, "masterLevel", 1);
            }
            match malformed {
                "duplicate_job" => {
                    let jobs = property("job");
                    add_integer(&jobs, "0", 100);
                    add_integer(&jobs, "1", 100);
                    add_child(&entry, &jobs);
                }
                "unknown_field" => add_integer(&entry, "mystery", 1),
                "bad_level" => add_long(&entry, "skillLevel", i64::from(u32::MAX) + 1),
                "bad_only_master" => add_integer(&entry, "onlyMasterLevel", 2),
                "missing_id" | "zero_id" | "missing_master" => {}
                "duplicate_skill" => {
                    let duplicate = property("1");
                    add_integer(&duplicate, "id", 1_000);
                    add_integer(&duplicate, "masterLevel", 1);
                    add_child(&skills, &duplicate);
                }
                _ => unreachable!(),
            }
            add_child(&skills, &entry);
            add_child(&phase, &skills);
            add_child(&action, &phase);

            assert!(
                read_action_phase_with_skills(
                    100,
                    &action,
                    "1",
                    &BTreeSet::new(),
                    &BTreeSet::new(),
                    &BTreeSet::from([1_000]),
                    &BTreeSet::from([100]),
                    None,
                )
                .is_err(),
                "{malformed} must fail",
            );
        }

        let unknown = skill_action(9_999, 1);
        assert!(matches!(
            read_action_phase_with_skills(
                100,
                &unknown,
                "1",
                &BTreeSet::new(),
                &BTreeSet::new(),
                &BTreeSet::new(),
                &BTreeSet::from([100]),
                None,
            ),
            Err(QuestContentError::Unsupported { category, .. })
                if category == "unknown skill reference"
        ));
    }

    #[test]
    fn acquire_minus_one_imports_an_exact_skill_removal() {
        let action = property("quest");
        let phase = property("0");
        let skills = property("skill");
        let entry = property("0");
        add_integer(&entry, "id", 1_007);
        add_integer(&entry, "acquire", -1);
        let jobs = property("job");
        add_integer(&jobs, "0", 0);
        add_integer(&jobs, "1", 100);
        add_child(&entry, &jobs);
        add_child(&skills, &entry);
        add_child(&phase, &skills);
        add_child(&action, &phase);

        let (actions, retained) = read_action_phase_with_skills(
            6_034,
            &action,
            "0",
            &BTreeSet::new(),
            &BTreeSet::new(),
            &BTreeSet::from([1_007]),
            &BTreeSet::from([6_034]),
            None,
        )
        .expect("exact skill removal");

        assert!(retained.is_empty());
        assert_eq!(
            actions.skill_changes,
            vec![crate::content::QuestSkillChange {
                skill_id: 1_007,
                operation: crate::content::QuestSkillOperation::Remove,
                job_ids: vec![0, 100],
            }]
        );
    }

    #[test]
    fn malformed_skill_removals_fail_closed() {
        for malformed in [
            "negative_acquire",
            "positive_acquire",
            "skill_level",
            "master_level",
            "only_master_level",
            "duplicate_job",
        ] {
            let action = property("quest");
            let phase = property("0");
            let skills = property("skill");
            let entry = property("0");
            add_integer(&entry, "id", 1_007);
            add_integer(
                &entry,
                "acquire",
                match malformed {
                    "negative_acquire" => -2,
                    "positive_acquire" => 1,
                    _ => -1,
                },
            );
            match malformed {
                "skill_level" => add_integer(&entry, "skillLevel", 1),
                "master_level" => add_integer(&entry, "masterLevel", 1),
                "only_master_level" => add_integer(&entry, "onlyMasterLevel", 1),
                "duplicate_job" => {
                    let jobs = property("job");
                    add_integer(&jobs, "0", 100);
                    add_integer(&jobs, "1", 100);
                    add_child(&entry, &jobs);
                }
                "negative_acquire" | "positive_acquire" => {}
                _ => unreachable!(),
            }
            add_child(&skills, &entry);
            add_child(&phase, &skills);
            add_child(&action, &phase);

            assert!(
                read_action_phase_with_skills(
                    6_034,
                    &action,
                    "0",
                    &BTreeSet::new(),
                    &BTreeSet::new(),
                    &BTreeSet::from([1_007]),
                    &BTreeSet::from([6_034]),
                    None,
                )
                .is_err(),
                "{malformed} removal must fail",
            );
        }
    }

    #[test]
    fn calendar_values_become_unix_milliseconds() {
        assert_eq!(calendar_unix_ms("19700101"), Ok(0));
        assert_eq!(calendar_unix_ms("19700102010203"), Ok(90_123_000));
        assert!(calendar_unix_ms("20230229").is_err());
    }

    #[test]
    fn item_expiration_civil_times_use_the_gms_pacific_timezone() {
        assert_eq!(
            item_expiration_unix_ms("2024011500"),
            Ok(1_705_305_600_000),
            "midnight PST is 08:00 UTC"
        );
        assert_eq!(
            item_expiration_unix_ms("2024071500"),
            Ok(1_721_026_800_000),
            "midnight PDT is 07:00 UTC"
        );
        assert!(item_expiration_unix_ms("202401150000").is_err());
        assert!(item_expiration_unix_ms("2024023000").is_err());
    }

    #[test]
    fn item_period_is_a_relative_minute_lifetime() {
        let phase = action_phase(None);
        let items = phase.read().expect("phase").at("item").expect("items");
        let entry = items.read().expect("items").at("0").expect("first item");
        add_integer(&entry, "period", 60);

        let imported = read_action_items(
            100,
            "1",
            &phase,
            &BTreeSet::from([4_000_000]),
            &BTreeSet::new(),
        )
        .expect("relative item expiration");

        assert_eq!(
            imported.fixed[0].expiration,
            Some(QuestItemExpiration::RelativeMilliseconds(3_600_000))
        );

        let permanent = action_phase(None);
        let items = permanent.read().expect("phase").at("item").expect("items");
        let entry = items.read().expect("items").at("0").expect("first item");
        add_integer(&entry, "period", 0);
        let imported = read_action_items(
            100,
            "1",
            &permanent,
            &BTreeSet::from([4_000_000]),
            &BTreeSet::new(),
        )
        .expect("zero period is permanent");
        assert_eq!(imported.fixed[0].expiration, None);

        let negative = action_phase(None);
        let items = negative.read().expect("phase").at("item").expect("items");
        let entry = items.read().expect("items").at("0").expect("first item");
        add_integer(&entry, "period", -1);
        assert!(matches!(
            read_action_items(
                100,
                "1",
                &negative,
                &BTreeSet::from([4_000_000]),
                &BTreeSet::new(),
            ),
            Err(QuestContentError::Invalid { .. })
        ));
    }

    #[test]
    fn both_archive_timer_fields_are_seconds() {
        let info = property("info");
        add_integer(&info, "timeLimit", 3_600);
        add_integer(&info, "timeLimit2", 2_592_000);

        let info = read_info(100, &info).expect("quest timer metadata");

        assert_eq!(info.time_limit_ms, Some(3_600_000));
        assert_eq!(info.time_limit2_ms, Some(2_592_000_000));
        assert_eq!(
            quest_timer_milliseconds(15, "timeLimit", 100).expect("seconds duration"),
            15_000
        );
        assert!(quest_timer_milliseconds(u64::MAX, "timeLimit2", 100).is_err());
    }

    #[test]
    fn current_state_checks_are_typed_and_calendar_bounds_are_validated() {
        let start = property("0");
        add_integer(&start, "npc", 1);
        add_integer(&start, "pop", i32::MAX);
        add_string(&start, "worldmin", "0002");
        add_integer(&start, "worldmax", 4);

        let start = read_start_requirements(
            100,
            &start,
            &QuestInfo::default(),
            &BTreeSet::new(),
            &BTreeSet::new(),
        )
        .expect("typed start checks");
        assert_eq!(start.minimum_fame, Some(i32::MAX));
        assert_eq!(start.minimum_world_id, Some(2));
        assert_eq!(start.maximum_world_id, Some(4));

        let completion = property("1");
        add_long(&completion, "endmeso", i64::MAX);
        add_integer(&completion, "questComplete", 7);
        add_string(&completion, "start", "19700102010203");
        add_string(&completion, "end", "19700102010204");
        let completion = read_completion_requirements(
            100,
            &completion,
            &QuestInfo::default(),
            &BTreeSet::new(),
            &BTreeSet::new(),
        )
        .expect("typed completion checks");
        assert_eq!(completion.minimum_mesos, Some(i64::MAX as u64));
        assert_eq!(completion.minimum_completed_quest_count, Some(7));
        assert_eq!(
            completion
                .available_from
                .as_ref()
                .map(|calendar| calendar.unix_ms),
            Some(90_123_000)
        );
        assert_eq!(
            completion
                .available_until
                .as_ref()
                .map(|calendar| calendar.unix_ms),
            Some(90_124_000)
        );
    }

    #[test]
    fn malformed_current_state_checks_fail_closed() {
        for (name, value) in [
            ("worldmin", "+1"),
            ("worldmin", " 1"),
            ("worldmax", "1x"),
            ("worldmax", "4294967296"),
        ] {
            let start = property("0");
            add_integer(&start, "npc", 1);
            add_string(&start, name, value);
            assert!(
                read_start_requirements(
                    100,
                    &start,
                    &QuestInfo::default(),
                    &BTreeSet::new(),
                    &BTreeSet::new(),
                )
                .is_err(),
                "{name}={value:?} must fail",
            );
        }

        let negative_fame = property("0");
        add_integer(&negative_fame, "npc", 1);
        add_integer(&negative_fame, "pop", -1);
        assert!(
            read_start_requirements(
                100,
                &negative_fame,
                &QuestInfo::default(),
                &BTreeSet::new(),
                &BTreeSet::new(),
            )
            .is_err()
        );

        let reversed_worlds = property("0");
        add_integer(&reversed_worlds, "npc", 1);
        add_integer(&reversed_worlds, "worldmin", 4);
        add_integer(&reversed_worlds, "worldmax", 3);
        assert!(
            read_start_requirements(
                100,
                &reversed_worlds,
                &QuestInfo::default(),
                &BTreeSet::new(),
                &BTreeSet::new(),
            )
            .is_err()
        );

        let negative_mesos = property("1");
        add_long(&negative_mesos, "endmeso", -1);
        assert!(
            read_completion_requirements(
                100,
                &negative_mesos,
                &QuestInfo::default(),
                &BTreeSet::new(),
                &BTreeSet::new(),
            )
            .is_err()
        );

        let reversed_dates = property("1");
        add_string(&reversed_dates, "start", "19700103");
        add_string(&reversed_dates, "end", "19700102");
        assert!(
            read_completion_requirements(
                100,
                &reversed_dates,
                &QuestInfo::default(),
                &BTreeSet::new(),
                &BTreeSet::new(),
            )
            .is_err()
        );
    }

    #[test]
    fn equipped_item_checks_are_typed_in_both_phases() {
        let equipment_ids = BTreeSet::from([1_002_800, 1_032_058, 1_052_166, 1_052_167]);
        let start = property("0");
        add_integer(&start, "npc", 1);
        let all = property("equipAllNeed");
        add_integer(&all, "0", 1_002_800);
        add_integer(&all, "1", 1_032_058);
        add_child(&start, &all);
        let any = property("equipSelectNeed");
        add_integer(&any, "0", 1_052_167);
        add_integer(&any, "1", 1_052_166);
        add_child(&start, &any);

        let start = read_start_requirements(
            100,
            &start,
            &QuestInfo::default(),
            &BTreeSet::new(),
            &equipment_ids,
        )
        .expect("typed start equipment checks");
        assert_eq!(start.equipped_items.all_of, vec![1_002_800, 1_032_058]);
        assert_eq!(start.equipped_items.any_of, vec![1_052_167, 1_052_166]);

        let completion = property("1");
        let all = property("equipAllNeed");
        add_integer(&all, "0", 1_032_058);
        add_child(&completion, &all);
        let any = property("equipSelectNeed");
        add_integer(&any, "0", 1_052_166);
        add_integer(&any, "1", 1_052_167);
        add_child(&completion, &any);

        let completion = read_completion_requirements(
            100,
            &completion,
            &QuestInfo::default(),
            &BTreeSet::new(),
            &equipment_ids,
        )
        .expect("typed completion equipment checks");
        assert_eq!(completion.equipped_items.all_of, vec![1_032_058]);
        assert_eq!(completion.equipped_items.any_of, vec![1_052_166, 1_052_167]);
    }

    #[test]
    fn malformed_or_unknown_equipped_item_checks_fail_closed() {
        let equipment_ids = BTreeSet::from([1_002_800, 1_032_058]);
        for (name, entries) in [
            ("zero ID", vec![("0", 0)]),
            ("negative ID", vec![("0", -1)]),
            ("unknown ID", vec![("0", 1_002_801)]),
            ("ordinary item ID", vec![("0", 4_000_000)]),
            ("duplicate ID", vec![("0", 1_002_800), ("1", 1_002_800)]),
            ("noncanonical index", vec![("00", 1_002_800)]),
            ("index gap", vec![("1", 1_002_800)]),
        ] {
            let check = property("0");
            let all = property("equipAllNeed");
            for (index, item_id) in entries {
                add_integer(&all, index, item_id);
            }
            add_child(&check, &all);
            assert!(
                super::read_equipped_item_requirements(100, &check, &equipment_ids).is_err(),
                "{name} must fail",
            );
        }

        let empty = property("0");
        add_child(&empty, &property("equipSelectNeed"));
        assert!(super::read_equipped_item_requirements(100, &empty, &equipment_ids).is_err());

        let string_id = property("0");
        let any = property("equipSelectNeed");
        add_string(&any, "0", "1002800");
        add_child(&string_id, &any);
        assert!(super::read_equipped_item_requirements(100, &string_id, &equipment_ids).is_err());

        let scalar_list = property("0");
        add_integer(&scalar_list, "equipAllNeed", 1_002_800);
        assert!(super::read_equipped_item_requirements(100, &scalar_list, &equipment_ids).is_err());
    }

    #[test]
    fn quest_info_area_is_typed_instead_of_retained_as_unknown_metadata() {
        let info = property("info");
        add_integer(&info, "area", 51);

        let info = read_info(100, &info).expect("typed quest area");

        assert_eq!(info.area, Some(51));
        assert!(
            !info
                .retained_metadata_fields
                .iter()
                .any(|field| field == "area")
        );
    }

    #[test]
    fn record_checks_import_redirects_or_alternatives_and_all_conditions() {
        let check = property("0");
        add_integer(&check, "infoNumber", 200);
        let infoex = property("infoex");
        for (index, condition, value) in [(0, None, "007"), (1, Some(1), "10"), (2, Some(2), "20")]
        {
            let entry = property(&index.to_string());
            if let Some(condition) = condition {
                add_integer(&entry, "cond", condition);
            }
            add_string(&entry, "value", value);
            add_child(&infoex, &entry);
        }
        add_child(&check, &infoex);

        let conditions = read_record_conditions(100, &check).expect("record predicates");

        assert_eq!(conditions.len(), 1);
        assert_eq!(conditions[0].quest_id, 200);
        assert_eq!(conditions[0].index, 0);
        assert_eq!(
            conditions[0].alternatives,
            vec![
                QuestRecordPredicate::Equal("007".to_owned()),
                QuestRecordPredicate::AtLeast(10),
                QuestRecordPredicate::AtMost(20),
            ]
        );
    }

    #[test]
    fn direct_info_preserves_exact_strings_and_decimal_string_redirects() {
        let check = property("1");
        add_string(&check, "infoNumber", "00200");
        let info = property("info");
        add_string(&info, "0", "AbC");
        add_string(&info, "1", "007");
        add_child(&check, &info);

        let conditions = read_record_conditions(100, &check).expect("direct record predicates");

        assert_eq!(conditions[0].quest_id, 200);
        assert_eq!(
            conditions[0].alternatives,
            vec![
                QuestRecordPredicate::Equal("AbC".to_owned()),
                QuestRecordPredicate::Equal("007".to_owned()),
            ]
        );
    }

    #[test]
    fn local_archive_quest_28288_preserves_its_redirected_direct_info_check() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/Quest.wz");
        if !path.exists() {
            return;
        }
        let root = crate::content::wz::open_archive(&path).expect("quest archive");
        crate::content::wz::parse(&root, "quest archive root".to_owned())
            .expect("parse quest archive");
        let checks = super::required_child(&root, "Check.img", 0).expect("check archive");
        crate::content::wz::parse(&checks, "quest check archive".to_owned())
            .expect("parse quest checks");
        let quest = super::required_child(&checks, "28288", 28_288).expect("quest 28288");
        let start = super::required_child(&quest, "0", 28_288).expect("start check");

        let conditions = read_record_conditions(28_288, &start).expect("record condition");

        assert_eq!(conditions[0].quest_id, 28_301);
        assert!(matches!(
            &conditions[0].alternatives[..],
            [QuestRecordPredicate::Equal(value)] if value == "0"
        ));
    }

    #[test]
    fn malformed_record_checks_are_rejected() {
        for mutate in 0..7 {
            let check = property("0");
            let infoex = property("infoex");
            let entry = property(if mutate == 0 { "1" } else { "0" });
            match mutate {
                1 => add_integer(&entry, "cond", 3),
                2 => add_integer(&entry, "cond", 1),
                3 => add_string(&entry, "value", ""),
                4 => add_string(&entry, "value", "1234567890123456"),
                5 => add_integer(&entry, "value", 1),
                6 => add_string(&check, "infoNumber", "+200"),
                _ => {}
            }
            if mutate != 3 && mutate != 4 && mutate != 5 {
                add_string(&entry, "value", if mutate == 2 { "1x" } else { "1" });
            }
            add_child(&infoex, &entry);
            add_child(&check, &infoex);

            assert!(
                read_record_conditions(100, &check).is_err(),
                "malformed record case {mutate} must fail"
            );
        }

        let redirect_only = property("0");
        add_integer(&redirect_only, "infoNumber", 200);
        assert!(read_record_conditions(100, &redirect_only).is_err());
    }

    #[test]
    fn start_action_info_is_an_exact_record_write_and_completion_info_is_rejected() {
        let action = property("quest");
        let start = property("0");
        add_string(&start, "info", "000007");
        add_child(&action, &start);

        let (actions, retained) =
            read_action_phase(100, &action, "0", &BTreeSet::new(), &BTreeSet::new())
                .expect("start record write");
        assert_eq!(
            actions.record_writes,
            vec![QuestRecordWrite {
                quest_id: 100,
                index: 0,
                value: "000007".to_owned(),
            }]
        );
        assert!(retained.is_empty());

        let completion_action = property("quest");
        let completion = property("1");
        add_string(&completion, "info", "done");
        add_child(&completion_action, &completion);
        assert!(matches!(
            read_action_phase(
                100,
                &completion_action,
                "1",
                &BTreeSet::new(),
                &BTreeSet::new()
            ),
            Err(QuestContentError::Unsupported { category, .. })
                if category == "quest progress action phase 1"
        ));
    }

    #[test]
    fn selectable_item_rewards_preserve_order_count_and_eligibility() {
        let phase = property("1");
        let items = property("item");
        let later = property("1");
        add_integer(&later, "id", 4_000_001);
        add_integer(&later, "count", 3);
        add_integer(&later, "prop", -1);
        add_child(&items, &later);
        let earlier = property("0");
        add_integer(&earlier, "id", 4_000_000);
        add_integer(&earlier, "count", 2);
        add_integer(&earlier, "prop", -1);
        add_integer(&earlier, "job", 2);
        add_integer(&earlier, "gender", 1);
        add_integer(&earlier, "period", 60);
        add_child(&items, &earlier);
        add_child(&phase, &items);

        let imported = read_action_items(
            100,
            "1",
            &phase,
            &BTreeSet::from([4_000_000, 4_000_001]),
            &BTreeSet::new(),
        )
        .expect("selectable rewards");

        assert!(imported.fixed.is_empty());
        assert_eq!(
            imported
                .selectable
                .iter()
                .map(|reward| (reward.item_id, reward.count))
                .collect::<Vec<_>>(),
            vec![(4_000_000, 2), (4_000_001, 3)]
        );
        assert_eq!(
            imported.selectable[0].eligibility,
            QuestRewardEligibility {
                job_mask: Some(2),
                gender: Some(QuestRewardGender::Female),
            }
        );
        assert_eq!(
            imported.selectable[0].expiration,
            Some(QuestItemExpiration::RelativeMilliseconds(3_600_000))
        );

        let fixed = read_action_items(
            100,
            "1",
            &action_phase(None),
            &BTreeSet::from([4_000_000]),
            &BTreeSet::new(),
        )
        .expect("an item without prop remains a fixed grant");
        assert_eq!(fixed.fixed.len(), 1);
        assert!(fixed.conditional.is_empty());
        assert!(fixed.weighted.is_empty());
        assert!(fixed.selectable.is_empty());
    }

    #[test]
    fn omitted_action_item_count_defaults_to_one_and_explicit_zero_is_a_noop() {
        let phase = property("1");
        let items = property("item");
        let defaulted = property("0");
        add_integer(&defaulted, "id", 4_000_000);
        add_child(&items, &defaulted);
        let removal = property("1");
        add_integer(&removal, "id", 4_000_001);
        add_integer(&removal, "count", -2);
        add_child(&items, &removal);
        let selectable = property("2");
        add_integer(&selectable, "id", 4_000_002);
        add_integer(&selectable, "prop", -1);
        add_child(&items, &selectable);
        add_child(&phase, &items);
        let item_ids = BTreeSet::from([4_000_000, 4_000_001, 4_000_002]);

        let imported = read_action_items(100, "1", &phase, &item_ids, &BTreeSet::new())
            .expect("omitted counts use the WZ default");

        assert_eq!(
            imported.fixed,
            vec![
                QuestItemDelta {
                    item_id: 4_000_000,
                    count: 1,
                    expiration: None,
                },
                QuestItemDelta {
                    item_id: 4_000_001,
                    count: -2,
                    expiration: None,
                },
            ]
        );
        assert_eq!(imported.selectable[0].count, 1);

        add_integer(&defaulted, "count", 0);
        let imported = read_action_items(100, "1", &phase, &item_ids, &BTreeSet::new())
            .expect("an explicit zero count is inert");
        assert_eq!(imported.fixed.len(), 1);
        assert_eq!(imported.fixed[0].item_id, 4_000_001);
        assert_eq!(imported.selectable.len(), 1);
    }

    #[test]
    fn exact_inert_action_item_metadata_is_retained_without_changing_actions() {
        let phase = property("0");
        let items = property("item");
        for (index, item_id, var) in [(0, 4_000_000, 1), (1, 4_000_001, 2)] {
            let entry = property(&index.to_string());
            add_integer(&entry, "id", item_id);
            add_integer(&entry, "name", 1);
            add_integer(&entry, "var", var);
            add_child(&items, &entry);
        }
        add_child(&phase, &items);
        let action = property("quest");
        add_child(&action, &phase);

        let (actions, retained) = read_action_phase(
            100,
            &action,
            "0",
            &BTreeSet::from([4_000_000, 4_000_001]),
            &BTreeSet::new(),
        )
        .expect("exact inert item metadata");

        assert_eq!(
            retained,
            vec![
                "act/0/item/0/name",
                "act/0/item/0/var",
                "act/0/item/1/name",
                "act/0/item/1/var",
            ]
        );
        assert_eq!(
            actions.fixed_items,
            vec![
                QuestItemDelta {
                    item_id: 4_000_000,
                    count: 1,
                    expiration: None,
                },
                QuestItemDelta {
                    item_id: 4_000_001,
                    count: 1,
                    expiration: None,
                },
            ]
        );
    }

    #[test]
    fn unknown_or_noninteger_action_item_metadata_fails_closed() {
        for (field, value) in [("name", 0), ("name", 2), ("var", 0), ("var", 3)] {
            let phase = action_phase(None);
            let items = phase.read().expect("phase").at("item").expect("items");
            let entry = items.read().expect("items").at("0").expect("entry");
            add_integer(&entry, field, value);

            assert!(
                read_action_items(
                    100,
                    "1",
                    &phase,
                    &BTreeSet::from([4_000_000]),
                    &BTreeSet::new(),
                )
                .is_err(),
                "{field}={value} must fail",
            );
        }

        for field in ["name", "var"] {
            let phase = action_phase(None);
            let items = phase.read().expect("phase").at("item").expect("items");
            let entry = items.read().expect("items").at("0").expect("entry");
            add_string(&entry, field, "1");

            assert!(
                read_action_items(
                    100,
                    "1",
                    &phase,
                    &BTreeSet::from([4_000_000]),
                    &BTreeSet::new(),
                )
                .is_err(),
                "string {field} metadata must fail",
            );
        }
    }

    #[test]
    fn check_item_counts_default_to_one_and_nonpositive_counts_require_absence() {
        let check = property("1");
        let items = property("item");
        for (index, item_id, count) in [
            (0, 4_000_000, None),
            (1, 4_000_001, Some(0)),
            (2, 4_000_002, Some(-1)),
            (3, 4_000_003, Some(2)),
        ] {
            let entry = property(&index.to_string());
            add_integer(&entry, "id", item_id);
            if let Some(count) = count {
                add_integer(&entry, "count", count);
            }
            add_child(&items, &entry);
        }
        add_child(&check, &items);
        let item_ids = BTreeSet::from([4_000_000, 4_000_001, 4_000_002, 4_000_003]);

        let completion = read_completion_requirements(
            100,
            &check,
            &QuestInfo::default(),
            &item_ids,
            &BTreeSet::new(),
        )
        .expect("item count boundaries");

        assert_eq!(
            completion
                .items
                .iter()
                .map(|item| item.condition)
                .collect::<Vec<_>>(),
            vec![
                QuestItemCondition::AtLeast(NonZeroU32::MIN),
                QuestItemCondition::Absent,
                QuestItemCondition::Absent,
                QuestItemCondition::AtLeast(NonZeroU32::new(2).expect("positive")),
            ]
        );

        let unknown = property("1");
        let items = property("item");
        let entry = property("0");
        add_integer(&entry, "id", 4_000_099);
        add_integer(&entry, "count", -1);
        add_child(&items, &entry);
        add_child(&unknown, &items);
        assert!(matches!(
            read_completion_requirements(
                100,
                &unknown,
                &QuestInfo::default(),
                &BTreeSet::new(),
                &BTreeSet::new(),
            ),
            Err(QuestContentError::Unsupported { category, .. })
                if category == "unknown item reference"
        ));
    }

    #[test]
    fn zero_count_actions_do_not_enter_any_item_action_category() {
        let phase = property("1");
        let items = property("item");
        for (index, prop, job) in [
            (0, None, None),
            (1, None, Some(1)),
            (2, Some(10), None),
            (3, Some(-1), None),
        ] {
            let entry = property(&index.to_string());
            add_integer(&entry, "id", 4_000_000 + index);
            add_integer(&entry, "count", 0);
            if let Some(prop) = prop {
                add_integer(&entry, "prop", prop);
            }
            if let Some(job) = job {
                add_integer(&entry, "job", job);
            }
            add_child(&items, &entry);
        }
        add_child(&phase, &items);
        let item_ids = BTreeSet::from([4_000_000, 4_000_001, 4_000_002, 4_000_003]);

        let imported = read_action_items(100, "1", &phase, &item_ids, &BTreeSet::new())
            .expect("zero-count no-ops");

        assert!(imported.fixed.is_empty());
        assert!(imported.conditional.is_empty());
        assert!(imported.weighted.is_empty());
        assert!(imported.selectable.is_empty());

        let unknown = action_phase(None);
        let items = unknown.read().expect("phase").at("item").expect("items");
        let entry = items.read().expect("items").at("0").expect("entry");
        add_integer(&entry, "id", 4_000_099);
        add_integer(&entry, "count", 0);
        assert!(matches!(
            read_action_items(100, "1", &unknown, &BTreeSet::new(), &BTreeSet::new()),
            Err(QuestContentError::Unsupported { category, .. })
                if category == "unknown item reference"
        ));
    }

    #[test]
    fn audited_4944_action_alias_shape_is_exact_and_drift_is_rejected() {
        let action = quest_4944_action();
        validate_audited_4944_action(4_960, &action).expect("exact audited Act/4944 source");

        let completion = child(&action, "1");
        add_integer(&completion, "money", 1);
        assert!(matches!(
            validate_audited_4944_action(4_960, &action),
            Err(QuestContentError::Invalid {
                quest_id: 4_960,
                ..
            })
        ));
    }

    #[test]
    fn local_archive_quest_4960_uses_only_the_audited_4944_action_data() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/Quest.wz");
        if !path.exists() {
            return;
        }
        let root = crate::content::wz::open_archive(&path).expect("quest archive");
        crate::content::wz::parse(&root, "quest archive root".to_owned())
            .expect("parse quest archive");
        let checks = super::required_child(&root, "Check.img", 4_960).expect("Check.img");
        let actions = super::required_child(&root, "Act.img", 4_960).expect("Act.img");
        let say = super::required_child(&root, "Say.img", 4_960).expect("Say.img");
        let info = super::required_child(&root, "QuestInfo.img", 4_960).expect("QuestInfo.img");
        for (name, node) in [
            ("Check.img", &checks),
            ("Act.img", &actions),
            ("Say.img", &say),
            ("QuestInfo.img", &info),
        ] {
            crate::content::wz::parse(node, name.to_owned()).expect("parse quest root");
        }
        let check_4960 = super::required_child(&checks, "4960", 4_960).expect("Check/4960");
        let say_4960 = super::required_child(&say, "4960", 4_960).expect("Say/4960");
        let info_4960 = super::required_child(&info, "4960", 4_960).expect("QuestInfo/4960");
        assert!(
            crate::content::wz::child(&actions, "4960")
                .expect("Act/4960 lookup")
                .is_none()
        );

        let aliased = audited_action_root(
            4_960,
            &checks,
            &actions,
            &say,
            &info,
            &check_4960,
            Some(&say_4960),
            &info_4960,
        )
        .expect("audited Act/4944 alias");
        assert_eq!(
            crate::content::wz::node_name(&aliased).expect("action name"),
            "4944"
        );
        let item_ids = BTreeSet::from([
            2_022_247, 2_022_248, 2_022_249, 2_022_250, 2_022_251, 4_031_771,
        ]);
        let start_check = child(&check_4960, "0");
        let completion_check = child(&check_4960, "1");
        let (start, _) = read_action_phase_with_skills(
            4_960,
            &aliased,
            "0",
            &item_ids,
            &BTreeSet::new(),
            &BTreeSet::new(),
            &BTreeSet::from([4_944, 4_960]),
            Some(&start_check),
        )
        .expect("aliased start actions");
        let (completion, _) = read_action_phase_with_skills(
            4_960,
            &aliased,
            "1",
            &item_ids,
            &BTreeSet::new(),
            &BTreeSet::new(),
            &BTreeSet::from([4_944, 4_960]),
            Some(&completion_check),
        )
        .expect("aliased completion actions");

        validate_audited_4960_parsed_actions(4_960, &start, &completion)
            .expect("exact typed owner-4960 actions");
        assert_eq!(completion.experience, 8_000);
        assert_eq!(
            completion
                .fixed_items
                .iter()
                .map(|item| (item.item_id, item.count))
                .collect::<Vec<_>>(),
            vec![
                (4_031_771, 1),
                (2_022_247, -20),
                (2_022_248, -20),
                (2_022_249, -20),
                (2_022_250, -20),
                (2_022_251, 5),
            ]
        );
        assert!(completion.record_writes.is_empty());
    }

    #[test]
    fn quest_10272_exact_negative_removals_ignore_only_the_audited_prop_metadata() {
        let (action, completion_check, say) = quest_10272_sources();
        let corrections =
            audited_action_corrections(10_272, &action, &completion_check, Some(&say))
                .expect("exact quest 10272 evidence");
        let completion_action = child(&action, "1");
        let imported = read_action_items_with_corrections(
            10_272,
            "1",
            &completion_action,
            &BTreeSet::from([4_032_280, 4_032_283]),
            &BTreeSet::new(),
            &corrections,
        )
        .expect("audited fixed removals");

        assert_eq!(
            imported.fixed,
            vec![
                QuestItemDelta {
                    item_id: 4_032_283,
                    count: -10,
                    expiration: None,
                },
                QuestItemDelta {
                    item_id: 4_032_280,
                    count: -10,
                    expiration: None,
                },
            ]
        );
        assert!(imported.selectable.is_empty());
        assert_eq!(
            imported.retained_fields,
            vec![
                "act/1/item/0/prop=-1".to_owned(),
                "act/1/item/1/prop=-1".to_owned(),
            ]
        );
        let completion = read_completion_requirements(
            10_272,
            &completion_check,
            &QuestInfo::default(),
            &BTreeSet::from([4_032_280, 4_032_283]),
            &BTreeSet::new(),
        )
        .expect("quest 10272 completion requirements");
        assert_eq!(completion.script.as_deref(), Some("q10272e"));
        assert_eq!(
            completion
                .items
                .iter()
                .map(|item| (item.item_id, item.condition))
                .collect::<Vec<_>>(),
            vec![
                (
                    4_032_280,
                    QuestItemCondition::AtLeast(NonZeroU32::new(10).expect("positive")),
                ),
                (
                    4_032_283,
                    QuestItemCondition::AtLeast(NonZeroU32::new(10).expect("positive")),
                ),
            ]
        );
    }

    #[test]
    fn quest_10272_audited_item_metadata_fails_closed_on_source_drift() {
        let (action, completion_check, say) = quest_10272_sources();
        add_integer(
            &child(&child(&child(&action, "1"), "item"), "0"),
            "count",
            -9,
        );
        assert!(
            audited_action_corrections(10_272, &action, &completion_check, Some(&say)).is_err()
        );

        let (action, completion_check, say) = quest_10272_sources();
        add_integer(&completion_check, "extra", 1);
        assert!(
            audited_action_corrections(10_272, &action, &completion_check, Some(&say)).is_err()
        );

        let (action, completion_check, say) = quest_10272_sources();
        add_integer(&child(&child(&say, "0"), "yes"), "extra", 1);
        assert!(
            audited_action_corrections(10_272, &action, &completion_check, Some(&say)).is_err()
        );
        assert!(audited_action_corrections(10_272, &action, &completion_check, None).is_err());

        let corrections = audited_action_corrections(100, &action, &completion_check, Some(&say))
            .expect("other quests have no correction");
        assert!(
            read_action_items_with_corrections(
                100,
                "1",
                &child(&action, "1"),
                &BTreeSet::from([4_032_280, 4_032_283]),
                &BTreeSet::new(),
                &corrections,
            )
            .is_err(),
            "negative selectable entries remain invalid for every other quest",
        );
    }

    #[test]
    fn local_archive_quest_10272_matches_all_audited_item_evidence() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/Quest.wz");
        if !path.exists() {
            return;
        }
        let root = crate::content::wz::open_archive(&path).expect("quest archive");
        crate::content::wz::parse(&root, "quest archive root".to_owned())
            .expect("parse quest archive");
        let checks = super::required_child(&root, "Check.img", 10_272).expect("Check.img");
        let actions = super::required_child(&root, "Act.img", 10_272).expect("Act.img");
        let say = super::required_child(&root, "Say.img", 10_272).expect("Say.img");
        for (name, node) in [
            ("Check.img", &checks),
            ("Act.img", &actions),
            ("Say.img", &say),
        ] {
            crate::content::wz::parse(node, name.to_owned()).expect("parse quest root");
        }
        let check = child(
            &super::required_child(&checks, "10272", 10_272).expect("Check/10272"),
            "1",
        );
        let action = super::required_child(&actions, "10272", 10_272).expect("Act/10272");
        let say = super::required_child(&say, "10272", 10_272).expect("Say/10272");

        let corrections = audited_action_corrections(10_272, &action, &check, Some(&say))
            .expect("local audited quest 10272 evidence");
        let imported = read_action_items_with_corrections(
            10_272,
            "1",
            &child(&action, "1"),
            &BTreeSet::from([4_032_280, 4_032_283]),
            &BTreeSet::new(),
            &corrections,
        )
        .expect("local audited quest 10272 actions");
        assert_eq!(
            imported
                .fixed
                .iter()
                .map(|item| (item.item_id, item.count))
                .collect::<Vec<_>>(),
            vec![(4_032_283, -10), (4_032_280, -10)]
        );
    }

    #[test]
    fn audited_act_field_enter_must_exactly_duplicate_check_9866() {
        let action = property("quest");
        let action_phase = property("0");
        let action_maps = property("fieldEnter");
        add_integer(&action_maps, "0", 102_000_000);
        add_child(&action_phase, &action_maps);
        add_child(&action, &action_phase);
        let check = property("0");
        let check_maps = property("fieldEnter");
        add_integer(&check_maps, "0", 102_000_000);
        add_child(&check, &check_maps);

        let (_, retained) = read_action_phase_with_skills(
            9_866,
            &action,
            "0",
            &BTreeSet::new(),
            &BTreeSet::new(),
            &BTreeSet::new(),
            &BTreeSet::from([9_866]),
            Some(&check),
        )
        .expect("audited duplicate map metadata");
        assert_eq!(retained, vec!["act/0/fieldEnter"]);

        let mismatched = property("quest");
        let phase = property("0");
        let maps = property("fieldEnter");
        add_integer(&maps, "0", 102_000_001);
        add_child(&phase, &maps);
        add_child(&mismatched, &phase);
        assert!(matches!(
            read_action_phase_with_skills(
                9_866,
                &mismatched,
                "0",
                &BTreeSet::new(),
                &BTreeSet::new(),
                &BTreeSet::new(),
                &BTreeSet::from([9_866]),
                Some(&check),
            ),
            Err(QuestContentError::Invalid { .. })
        ));
        assert!(matches!(
            read_action_phase_with_skills(
                9_865,
                &action,
                "0",
                &BTreeSet::new(),
                &BTreeSet::new(),
                &BTreeSet::new(),
                &BTreeSet::from([9_865]),
                Some(&check),
            ),
            Err(QuestContentError::Unsupported { category, .. }) if category == "map action"
        ));
    }

    #[test]
    fn selected_skill_metadata_is_positive_authoritative_and_named() {
        let info = property("2415");
        add_integer(&info, "selectedSkillID", 1_001_004);
        let root = property("QuestInfo");

        let parsed = read_info_with_skills(
            2_415,
            &info,
            &root,
            &BTreeSet::from([1_001_004]),
            &BTreeMap::from([(1_001_004, "Power Strike".to_owned())]),
        )
        .expect("selected skill");
        let selected = parsed.selected_skill.expect("typed selected skill");
        assert_eq!(selected.id.get(), 1_001_004);
        assert_eq!(selected.name.as_deref(), Some("Power Strike"));

        let known_skills = BTreeSet::from([1_001_004]);
        let no_skills = BTreeSet::new();
        for (value, known) in [(0, true), (-1, true), (1_001_005, false)] {
            let info = property("2415");
            add_integer(&info, "selectedSkillID", value);
            assert!(
                read_info_with_skills(
                    2_415,
                    &info,
                    &root,
                    if known { &known_skills } else { &no_skills },
                    &BTreeMap::new(),
                )
                .is_err()
            );
        }
    }

    #[test]
    fn only_the_two_audited_nested_metadata_trees_are_retained() {
        let checks = property("Check");
        let quest = property("4940");
        add_child(&quest, &property("0"));
        add_child(&quest, &property("1"));
        let nested = property("4961");
        let start = property("0");
        add_integer(&start, "npc", 9_201_077);
        let prerequisites = property("quest");
        let prerequisite = property("0");
        add_integer(&prerequisite, "id", 4_954);
        add_integer(&prerequisite, "state", 1);
        add_child(&prerequisites, &prerequisite);
        add_child(&start, &prerequisites);
        add_child(&nested, &start);
        let completion = property("1");
        add_integer(&completion, "npc", 9_201_077);
        let mobs = property("mob");
        let mob = property("0");
        add_integer(&mob, "id", 9_400_558);
        add_integer(&mob, "count", 50);
        add_child(&mobs, &mob);
        add_child(&completion, &mobs);
        add_child(&nested, &completion);
        add_child(&quest, &nested);
        add_child(&checks, &quest);

        assert_eq!(
            super::validate_check_phase_tree(
                4_940,
                &checks,
                &quest,
                &BTreeSet::new(),
                &BTreeSet::new(),
                &BTreeSet::new(),
                &BTreeSet::new(),
                &BTreeSet::new(),
            )
            .expect("audited nested check"),
            vec!["check/4961"]
        );
        add_integer(&start, "lvmin", 1);
        assert!(matches!(
            super::validate_check_phase_tree(
                4_940,
                &checks,
                &quest,
                &BTreeSet::new(),
                &BTreeSet::new(),
                &BTreeSet::new(),
                &BTreeSet::new(),
                &BTreeSet::new(),
            ),
            Err(QuestContentError::Invalid { .. })
        ));
        let wrong_quest = property("4941");
        add_child(&wrong_quest, &nested);
        assert!(matches!(
            super::validate_check_phase_tree(
                4_941,
                &checks,
                &wrong_quest,
                &BTreeSet::new(),
                &BTreeSet::new(),
                &BTreeSet::new(),
                &BTreeSet::new(),
                &BTreeSet::new(),
            ),
            Err(QuestContentError::Unsupported { .. })
        ));

        let info_root = property("QuestInfo");
        let canonical = property("4963");
        add_string(&canonical, "name", "Dinner Fixins'");
        add_child(&info_root, &canonical);
        let info = property("8833");
        let nested = property("4963");
        add_string(&nested, "0", "Status zero");
        add_string(&nested, "1", "Status one");
        add_string(&nested, "2", "Status two");
        add_integer(&nested, "area", 30);
        add_string(&nested, "name", "Spot On");
        add_integer(&nested, "order", 1);
        add_string(&nested, "parent", "Spot On parent");
        add_child(&info, &nested);

        let parsed =
            read_info_with_skills(8_833, &info, &info_root, &BTreeSet::new(), &BTreeMap::new())
                .expect("audited nested quest info");
        assert_eq!(parsed.retained_metadata_fields, vec!["questInfo/4963"]);

        let malformed = property("8833");
        let nested = property("4963");
        add_string(&nested, "0", "Status zero");
        add_string(&nested, "1", "Status one");
        add_string(&nested, "2", "Status two");
        add_integer(&nested, "area", 30);
        add_string(&nested, "name", "Spot On");
        add_integer(&nested, "order", 1);
        add_string(&nested, "parent", "Spot On parent");
        add_integer(&nested, "extra", 1);
        add_child(&malformed, &nested);
        assert!(
            read_info_with_skills(
                8_833,
                &malformed,
                &info_root,
                &BTreeSet::new(),
                &BTreeMap::new(),
            )
            .is_err()
        );
    }

    #[test]
    fn recognized_inert_action_metadata_is_validated_and_retained() {
        let action = property("quest");
        let phase = property("0");
        add_string(&phase, "0", "Offer text");
        for branch_name in ["yes", "no"] {
            let branch = property(branch_name);
            add_string(&branch, "0", "Branch text");
            add_child(&phase, &branch);
        }
        add_integer(&phase, "ask", 1);
        let stop = property("stop");
        let answers = property("0");
        add_integer(&answers, "answer", 1);
        add_string(&answers, "0", "Response text");
        add_child(&stop, &answers);
        add_child(&phase, &stop);
        add_string(&phase, "start", "19700101");
        add_string(&phase, "end", "19700102");
        add_integer(&phase, "interval", 60);
        add_string(&phase, "message", "Legacy message");
        add_integer(&phase, "lvmin", 10);
        add_integer(&phase, "lvmax", 20);
        let jobs = property("job");
        add_integer(&jobs, "0", 100);
        add_integer(&jobs, "1", 200);
        add_child(&phase, &jobs);
        add_integer(&phase, "gender", 2);
        add_child(&action, &phase);

        let (actions, retained) =
            read_action_phase(100, &action, "0", &BTreeSet::new(), &BTreeSet::new())
                .expect("recognized inert action metadata");

        assert_eq!(actions, QuestActions::default());
        assert_eq!(
            retained,
            [
                "0", "ask", "end", "gender", "interval", "job", "lvmax", "lvmin", "message", "no",
                "start", "stop", "yes",
            ]
            .map(|name| format!("act/0/{name}"))
        );
    }

    #[test]
    fn act_zero_npc_is_positive_typed_presentation_metadata_only() {
        let action = property("quest");
        let phase = property("0");
        add_integer(&phase, "npc", 9_201_142);
        add_child(&action, &phase);

        let (actions, retained) =
            read_action_phase(100, &action, "0", &BTreeSet::new(), &BTreeSet::new())
                .expect("presentation NPC metadata");

        assert_eq!(actions.presentation_npc_id, Some(9_201_142));
        assert_eq!(retained, vec!["act/0/npc"]);

        let zero = property("quest");
        let phase = property("0");
        add_integer(&phase, "npc", 0);
        add_child(&zero, &phase);
        assert!(matches!(
            read_action_phase(100, &zero, "0", &BTreeSet::new(), &BTreeSet::new()),
            Err(QuestContentError::Invalid { .. })
        ));
    }

    #[test]
    fn npc_animation_actions_preserve_exact_nonempty_string_names() {
        for phase_name in ["0", "1"] {
            let action = property("quest");
            let phase = property(phase_name);
            add_string(&phase, "npcAct", "  exact Action  ");
            add_child(&action, &phase);
            let (actions, retained) =
                read_action_phase(100, &action, phase_name, &BTreeSet::new(), &BTreeSet::new())
                    .expect("typed NPC animation action");
            assert_eq!(
                actions.npc_animation_action.as_deref(),
                Some("  exact Action  ")
            );
            assert!(retained.is_empty());
        }
    }

    #[test]
    fn npc_animation_actions_reject_empty_and_non_string_values() {
        for add_invalid in [
            add_empty_npc_animation as fn(&WzNodeArc),
            add_integer_npc_animation,
        ] {
            let action = property("quest");
            let phase = property("1");
            add_invalid(&phase);
            add_child(&action, &phase);
            assert!(matches!(
                read_action_phase(100, &action, "1", &BTreeSet::new(), &BTreeSet::new()),
                Err(QuestContentError::Invalid { .. })
            ));
        }

        let action = property("quest");
        let phase = property("1");
        add_integer(&phase, "npc", 1);
        add_child(&action, &phase);
        assert!(matches!(
            read_action_phase(100, &action, "1", &BTreeSet::new(), &BTreeSet::new()),
            Err(QuestContentError::Unsupported { category, .. }) if category == "NPC action"
        ));
    }

    fn add_empty_npc_animation(phase: &WzNodeArc) {
        add_string(phase, "npcAct", "");
    }

    fn add_integer_npc_animation(phase: &WzNodeArc) {
        add_integer(phase, "npcAct", 1);
    }

    #[test]
    fn automatic_npc_animation_transitions_are_rejected_without_a_spawn_target() {
        let start = super::QuestStartRequirements {
            npc_id: Some(1),
            ..super::QuestStartRequirements::default()
        };
        let completion = QuestCompletionRequirements {
            npc_id: Some(2),
            ..QuestCompletionRequirements::default()
        };
        let start_actions = QuestActions {
            npc_animation_action: Some("start".to_owned()),
            ..QuestActions::default()
        };
        let completion_actions = QuestActions {
            npc_animation_action: Some("complete".to_owned()),
            ..QuestActions::default()
        };

        assert!(matches!(
            super::validate_npc_animation_transitions(
                100,
                &start,
                &completion,
                &start_actions,
                &QuestActions::default(),
                &QuestInfo {
                    auto_accept: true,
                    ..QuestInfo::default()
                },
            ),
            Err(QuestContentError::Unsupported { category, .. })
                if category == "automatic NPC animation action"
        ));
        assert!(matches!(
            super::validate_npc_animation_transitions(
                100,
                &start,
                &completion,
                &QuestActions::default(),
                &completion_actions,
                &QuestInfo {
                    auto_complete: true,
                    ..QuestInfo::default()
                },
            ),
            Err(QuestContentError::Unsupported { category, .. })
                if category == "automatic NPC animation action"
        ));
    }

    #[test]
    fn typed_start_questions_require_an_npc_but_allow_automatic_metadata() {
        let dialogue = QuestDialogue {
            start_question: Some(super::QuestQuestionSequence {
                leading_pages: Vec::new(),
                steps: Vec::new(),
                trailing_pages: Vec::new(),
            }),
            ..QuestDialogue::default()
        };
        assert!(matches!(
            super::validate_start_question_reachability(
                100,
                &super::QuestStartRequirements::default(),
                &dialogue,
            ),
            Err(QuestContentError::Unsupported { category, .. })
                if category == "unreachable start question"
        ));

        let start = super::QuestStartRequirements {
            npc_id: Some(20_000),
            normal_auto_start: true,
            ..super::QuestStartRequirements::default()
        };
        assert!(super::validate_start_question_reachability(100, &start, &dialogue).is_ok());
    }

    #[test]
    fn quest_state_actions_parse_in_numeric_order() {
        let action = quest_state_action(&[(2, 400, 2), (0, 200, 0), (1, 300, 1)]);

        let (actions, retained) = read_action_phase_with_skills(
            100,
            &action,
            "0",
            &BTreeSet::new(),
            &BTreeSet::new(),
            &BTreeSet::new(),
            &BTreeSet::from([100, 200, 300, 400]),
            None,
        )
        .expect("typed quest state actions");

        assert!(retained.is_empty());
        assert_eq!(
            actions.quest_state_actions,
            vec![
                QuestStateAction {
                    quest_id: 200,
                    state: QuestStateActionState::NotStarted,
                },
                QuestStateAction {
                    quest_id: 300,
                    state: QuestStateActionState::Started,
                },
                QuestStateAction {
                    quest_id: 400,
                    state: QuestStateActionState::Completed,
                },
            ]
        );
    }

    #[test]
    fn malformed_quest_state_actions_fail_closed() {
        let cases = [
            ("zero target", quest_state_action(&[(0, 0, 2)])),
            (
                "duplicate target",
                quest_state_action(&[(0, 200, 1), (1, 200, 2)]),
            ),
            ("self target", quest_state_action(&[(0, 100, 2)])),
            ("unknown target", quest_state_action(&[(0, 999, 2)])),
            ("unknown state", quest_state_action(&[(0, 200, 3)])),
            ("index gap", quest_state_action(&[(1, 200, 2)])),
        ];
        for (name, action) in cases {
            assert!(
                read_action_phase_with_skills(
                    100,
                    &action,
                    "0",
                    &BTreeSet::new(),
                    &BTreeSet::new(),
                    &BTreeSet::new(),
                    &BTreeSet::from([100, 200]),
                    None,
                )
                .is_err(),
                "{name} must fail",
            );
        }

        let unknown_field = quest_state_action(&[(0, 200, 2)]);
        let phase = unknown_field
            .read()
            .expect("action")
            .at("0")
            .expect("phase");
        let quests = phase
            .read()
            .expect("phase")
            .at("quest")
            .expect("quest actions");
        let entry = quests
            .read()
            .expect("quest actions")
            .at("0")
            .expect("entry");
        add_integer(&entry, "mystery", 1);
        assert!(
            read_action_phase_with_skills(
                100,
                &unknown_field,
                "0",
                &BTreeSet::new(),
                &BTreeSet::new(),
                &BTreeSet::new(),
                &BTreeSet::from([100, 200]),
                None,
            )
            .is_err()
        );
    }

    #[test]
    fn inert_action_metadata_with_invalid_shapes_is_rejected() {
        let invalid_fields = [
            ("0", false),
            ("yes", true),
            ("stop", true),
            ("start", false),
            ("interval", true),
            ("message", false),
            ("lvmin", true),
            ("job", false),
            ("gender", true),
        ];
        for (name, string_value) in invalid_fields {
            let action = property("quest");
            let phase = property("0");
            if string_value {
                add_string(&phase, name, "wrong shape");
            } else {
                add_integer(&phase, name, 1);
            }
            add_child(&action, &phase);

            let error = read_action_phase(100, &action, "0", &BTreeSet::new(), &BTreeSet::new())
                .expect_err("invalid inert metadata shape must fail");
            assert!(
                matches!(error, QuestContentError::Invalid { .. }),
                "field {name} produced {error}"
            );
        }
    }

    #[test]
    fn nested_inert_action_metadata_shapes_are_strict() {
        let action = property("quest");
        let phase = property("0");
        add_string(&phase, "0", "First");
        add_string(&phase, "2", "Gap");
        add_child(&action, &phase);
        assert_invalid_action_phase(&action);

        let action = property("quest");
        let phase = property("0");
        let yes = property("yes");
        add_integer(&yes, "0", 1);
        add_child(&phase, &yes);
        add_child(&action, &phase);
        assert_invalid_action_phase(&action);

        let action = property("quest");
        let phase = property("0");
        add_integer(&phase, "ask", 1);
        let stop = property("stop");
        let answers = property("0");
        add_integer(&answers, "answer", 1);
        add_integer(&answers, "mystery", 1);
        add_child(&stop, &answers);
        add_child(&phase, &stop);
        add_child(&action, &phase);
        assert_invalid_action_phase(&action);

        let action = property("quest");
        let phase = property("0");
        let jobs = property("job");
        add_string(&jobs, "0", "not a job ID");
        add_child(&phase, &jobs);
        add_child(&action, &phase);
        assert_invalid_action_phase(&action);
    }

    #[test]
    fn unknown_action_fields_are_unsupported_and_malformed_info_is_invalid() {
        let action = property("quest");
        let phase = property("0");
        add_integer(&phase, "mystery", 1);
        add_child(&action, &phase);
        assert!(matches!(
            read_action_phase(100, &action, "0", &BTreeSet::new(), &BTreeSet::new()),
            Err(QuestContentError::Unsupported { category, .. })
                if category == "unknown action field"
        ));

        let action = property("quest");
        let phase = property("0");
        add_integer(&phase, "info", 1);
        add_child(&action, &phase);
        assert!(matches!(
            read_action_phase(100, &action, "0", &BTreeSet::new(), &BTreeSet::new()),
            Err(QuestContentError::Invalid { .. })
        ));
    }

    #[test]
    fn selectable_rewards_require_a_positive_count() {
        let phase = action_phase(Some(-1));
        let items = phase.read().expect("phase").at("item").expect("items");
        let entry = items.read().expect("items").at("0").expect("first item");
        add_integer(&entry, "count", -1);

        let error = match read_action_items(
            100,
            "1",
            &phase,
            &BTreeSet::from([4_000_000]),
            &BTreeSet::new(),
        ) {
            Err(error) => error,
            Ok(_) => panic!("negative selectable count must fail"),
        };

        assert!(matches!(
            error,
            QuestContentError::Invalid { quest_id: 100, .. }
        ));
    }

    #[test]
    fn expiration_is_preserved_for_conditional_and_weighted_rewards() {
        let phase = property("1");
        let items = property("item");
        let conditional = property("0");
        add_integer(&conditional, "id", 4_000_000);
        add_integer(&conditional, "job", 1);
        add_string(&conditional, "dateExpire", "2030011500");
        add_child(&items, &conditional);
        let weighted = property("1");
        add_integer(&weighted, "id", 4_000_001);
        add_integer(&weighted, "prop", 10);
        add_integer(&weighted, "period", 30);
        add_child(&items, &weighted);
        add_child(&phase, &items);

        let imported = read_action_items(
            100,
            "1",
            &phase,
            &BTreeSet::from([4_000_000, 4_000_001]),
            &BTreeSet::new(),
        )
        .expect("filtered expiring rewards");

        assert_eq!(
            imported.conditional[0].expiration,
            Some(QuestItemExpiration::AbsoluteUnixMilliseconds(
                item_expiration_unix_ms("2030011500").expect("deadline"),
            ))
        );
        assert_eq!(
            imported.weighted[0].expiration,
            Some(QuestItemExpiration::RelativeMilliseconds(1_800_000))
        );
    }

    #[test]
    fn expiring_equipment_is_preserved_for_fixed_weighted_and_selectable_rewards() {
        let phase = property("1");
        let items = property("item");
        let fixed = property("0");
        add_integer(&fixed, "id", 1_040_002);
        add_string(&fixed, "dateExpire", "2030011500");
        add_child(&items, &fixed);
        let weighted = property("1");
        add_integer(&weighted, "id", 1_060_002);
        add_integer(&weighted, "prop", 10);
        add_integer(&weighted, "period", 30);
        add_child(&items, &weighted);
        let selectable = property("2");
        add_integer(&selectable, "id", 1_072_000);
        add_integer(&selectable, "prop", -1);
        add_string(&selectable, "dateExpire", "2030011600");
        add_child(&items, &selectable);
        add_child(&phase, &items);
        let item_ids = BTreeSet::from([1_040_002, 1_060_002, 1_072_000]);

        let imported = read_action_items(100, "1", &phase, &item_ids, &item_ids)
            .expect("expiring equipment rewards");

        assert_eq!(
            imported.fixed[0].expiration,
            Some(QuestItemExpiration::AbsoluteUnixMilliseconds(
                item_expiration_unix_ms("2030011500").expect("fixed deadline"),
            ))
        );
        assert_eq!(
            imported.weighted[0].expiration,
            Some(QuestItemExpiration::RelativeMilliseconds(1_800_000))
        );
        assert_eq!(
            imported.selectable[0].expiration,
            Some(QuestItemExpiration::AbsoluteUnixMilliseconds(
                item_expiration_unix_ms("2030011600").expect("selectable deadline"),
            ))
        );
    }

    #[test]
    fn invalid_expiration_metadata_is_explicit() {
        let phase = action_phase(None);
        let items = phase.read().expect("phase").at("item").expect("items");
        let entry = items.read().expect("items").at("0").expect("first item");
        add_integer(&entry, "period", 1);
        add_string(&entry, "dateExpire", "2030011500");
        assert!(matches!(
            read_action_items(
                100,
                "1",
                &phase,
                &BTreeSet::from([4_000_000]),
                &BTreeSet::new(),
            ),
            Err(QuestContentError::Invalid { .. })
        ));

        let removal = action_phase(None);
        let items = removal.read().expect("phase").at("item").expect("items");
        let entry = items.read().expect("items").at("0").expect("first item");
        add_integer(&entry, "count", -1);
        add_integer(&entry, "period", 0);
        assert!(matches!(
            read_action_items(
                100,
                "1",
                &removal,
                &BTreeSet::from([4_000_000]),
                &BTreeSet::new(),
            ),
            Err(QuestContentError::Invalid { .. })
        ));
    }

    #[test]
    fn selectable_start_rewards_are_rejected_but_completion_rewards_can_follow_quizzes() {
        let selectable = QuestSelectableItemReward {
            item_id: 4_000_000,
            count: 1,
            expiration: None,
            eligibility: QuestRewardEligibility::default(),
        };
        let start = QuestActions {
            selectable_items: vec![selectable],
            ..QuestActions::default()
        };
        let start_error = validate_selectable_reward_flow(
            100,
            &start,
            &QuestActions::default(),
            &QuestDialogue::default(),
        )
        .expect_err("start reward selection");
        assert!(matches!(
            start_error,
            QuestContentError::Unsupported { category, .. }
                if category == "selectable reward in start actions"
        ));

        let completion = QuestActions {
            selectable_items: vec![selectable],
            ..QuestActions::default()
        };
        validate_selectable_reward_flow(
            100,
            &QuestActions::default(),
            &completion,
            &QuestDialogue::default(),
        )
        .expect("completion quiz followed by reward selection");
    }

    #[test]
    fn lost_item_restoration_requires_an_unambiguous_fixed_start_grant() {
        let dialogue = QuestDialogue {
            completion: crate::content::QuestCompletionDialogue {
                lost: Some(QuestLostItemDialogue {
                    prompt_pages: vec!["Did you lose it?".to_owned()],
                    success_pages: vec!["Take another.".to_owned()],
                    items: Vec::new(),
                }),
                ..crate::content::QuestCompletionDialogue::default()
            },
            ..QuestDialogue::default()
        };
        let completion = QuestCompletionRequirements {
            items: vec![QuestItemRequirement {
                item_id: 4_000_000,
                condition: QuestItemCondition::AtLeast(
                    NonZeroU32::new(2).expect("positive requirement"),
                ),
            }],
            ..QuestCompletionRequirements::default()
        };

        let unmapped = validate_lost_item_restoration_flow(
            100,
            &completion,
            &QuestActions::default(),
            &QuestActions::default(),
            &dialogue,
        )
        .expect_err("an unmapped lost branch must remain unsupported");
        assert!(matches!(
            unmapped,
            QuestContentError::Unsupported { category, .. }
                if category == "lost-item restoration item mapping"
        ));
        let arbitrary_completion = QuestActions {
            fixed_items: vec![QuestItemDelta {
                item_id: 4_000_000,
                count: 1,
                expiration: None,
            }],
            ..QuestActions::default()
        };
        assert!(matches!(
            validate_lost_item_restoration_flow(
                100,
                &completion,
                &QuestActions::default(),
                &arbitrary_completion,
                &dialogue,
            ),
            Err(QuestContentError::Unsupported { category, .. })
                if category == "lost-item restoration item mapping"
        ));

        let fixed = QuestActions {
            fixed_items: vec![QuestItemDelta {
                item_id: 4_000_000,
                count: 3,
                expiration: Some(QuestItemExpiration::RelativeMilliseconds(60_000)),
            }],
            ..QuestActions::default()
        };
        let mapped = validate_lost_item_restoration_flow(
            100,
            &completion,
            &fixed,
            &QuestActions::default(),
            &dialogue,
        )
        .expect("fixed matching start grant");
        assert_eq!(
            mapped,
            vec![crate::content::QuestRestorableItem {
                item_id: 4_000_000,
                target_count: 3,
                expiration: Some(QuestItemExpiration::RelativeMilliseconds(60_000)),
                provenance: crate::content::QuestRestorationProvenance::InferredStartGrant,
                eligibility: crate::content::QuestRestorationEligibility {
                    owner_state: crate::content::RequiredQuestState::Started,
                    required_quests: &[],
                    forbidden_quests: &[],
                    absent_skill_ids: &[],
                    absent_item_ids: &[],
                },
            }]
        );
        let mapped_without_completion_objective = validate_lost_item_restoration_flow(
            100,
            &QuestCompletionRequirements::default(),
            &fixed,
            &QuestActions::default(),
            &dialogue,
        )
        .expect("a fixed start grant is sufficient provenance");
        assert_eq!(mapped_without_completion_objective, mapped);

        let contradictory_dialogue = QuestDialogue {
            completion: crate::content::QuestCompletionDialogue {
                lost: Some(QuestLostItemDialogue {
                    prompt_pages: vec!["Did you lose #t4000001#?".to_owned()],
                    success_pages: Vec::new(),
                    items: Vec::new(),
                }),
                ..crate::content::QuestCompletionDialogue::default()
            },
            ..QuestDialogue::default()
        };
        assert!(matches!(
            validate_lost_item_restoration_flow(
                100,
                &QuestCompletionRequirements::default(),
                &fixed,
                &QuestActions::default(),
                &contradictory_dialogue,
            ),
            Err(QuestContentError::Unsupported { category, .. })
                if category == "lost-item restoration action ambiguity"
        ));

        let ambiguous_actions = [
            QuestActions {
                conditional_items: vec![QuestConditionalItemReward {
                    item_id: 4_000_000,
                    count: 2,
                    expiration: None,
                    eligibility: QuestRewardEligibility::default(),
                }],
                ..QuestActions::default()
            },
            QuestActions {
                weighted_items: vec![QuestWeightedItem {
                    item_id: 4_000_000,
                    count: 2,
                    expiration: None,
                    weight: 1,
                    eligibility: QuestRewardEligibility::default(),
                }],
                ..QuestActions::default()
            },
            QuestActions {
                selectable_items: vec![QuestSelectableItemReward {
                    item_id: 4_000_000,
                    count: 2,
                    expiration: None,
                    eligibility: QuestRewardEligibility::default(),
                }],
                ..QuestActions::default()
            },
        ];
        for actions in ambiguous_actions {
            let error = validate_lost_item_restoration_flow(
                100,
                &completion,
                &actions,
                &QuestActions::default(),
                &dialogue,
            )
            .expect_err("conditional start grants must not become restoration grants");
            assert!(matches!(
                error,
                QuestContentError::Unsupported { category, .. }
                    if category == "lost-item restoration action ambiguity"
            ));
        }
    }

    #[test]
    fn audited_completion_restoration_requires_exact_permanent_fixed_evidence() {
        let dialogue = QuestDialogue {
            completion: crate::content::QuestCompletionDialogue {
                lost: Some(QuestLostItemDialogue {
                    prompt_pages: vec!["Did you lose it?".to_owned()],
                    success_pages: vec!["Take another.".to_owned()],
                    items: Vec::new(),
                }),
                ..crate::content::QuestCompletionDialogue::default()
            },
            ..QuestDialogue::default()
        };
        let exact = QuestActions {
            fixed_items: vec![QuestItemDelta {
                item_id: 4_031_890,
                count: 1,
                expiration: None,
            }],
            ..QuestActions::default()
        };
        let mapped = validate_lost_item_restoration_flow(
            2_208,
            &QuestCompletionRequirements::default(),
            &QuestActions::default(),
            &exact,
            &dialogue,
        )
        .expect("exact audited completion action");
        assert_eq!(mapped.len(), 1);
        assert_eq!(
            mapped[0].provenance,
            crate::content::QuestRestorationProvenance::AuditedCompletionGrant
        );

        for completion_actions in [
            QuestActions {
                fixed_items: vec![QuestItemDelta {
                    item_id: 4_031_890,
                    count: 2,
                    expiration: None,
                }],
                ..QuestActions::default()
            },
            QuestActions {
                fixed_items: vec![QuestItemDelta {
                    item_id: 4_031_890,
                    count: 1,
                    expiration: Some(QuestItemExpiration::RelativeMilliseconds(60_000)),
                }],
                ..QuestActions::default()
            },
            QuestActions {
                conditional_items: vec![QuestConditionalItemReward {
                    item_id: 4_031_890,
                    count: 1,
                    expiration: None,
                    eligibility: QuestRewardEligibility {
                        job_mask: Some(1),
                        gender: None,
                    },
                }],
                ..QuestActions::default()
            },
            QuestActions {
                weighted_items: vec![QuestWeightedItem {
                    item_id: 4_031_890,
                    count: 1,
                    expiration: None,
                    weight: 1,
                    eligibility: QuestRewardEligibility::default(),
                }],
                ..QuestActions::default()
            },
            QuestActions {
                selectable_items: vec![QuestSelectableItemReward {
                    item_id: 4_031_890,
                    count: 1,
                    expiration: None,
                    eligibility: QuestRewardEligibility::default(),
                }],
                ..QuestActions::default()
            },
        ] {
            assert!(matches!(
                validate_lost_item_restoration_flow(
                    2_208,
                    &QuestCompletionRequirements::default(),
                    &QuestActions::default(),
                    &completion_actions,
                    &dialogue,
                ),
                Err(QuestContentError::Unsupported { category, .. })
                    if category == "audited lost-item restoration evidence"
            ));
        }
    }

    #[test]
    fn quest_3310_exception_requires_its_exact_item_dialogue_and_output_objective() {
        let dialogue = QuestDialogue {
            completion: crate::content::QuestCompletionDialogue {
                lost: Some(QuestLostItemDialogue {
                    prompt_pages: vec!["Did you lose #t4031698#?".to_owned()],
                    success_pages: Vec::new(),
                    items: Vec::new(),
                }),
                ..crate::content::QuestCompletionDialogue::default()
            },
            ..QuestDialogue::default()
        };
        let completion = QuestCompletionRequirements {
            items: vec![QuestItemRequirement {
                item_id: 4_031_709,
                condition: QuestItemCondition::AtLeast(
                    NonZeroU32::new(1).expect("positive reactor output"),
                ),
            }],
            ..QuestCompletionRequirements::default()
        };
        let mapped = validate_lost_item_restoration_flow(
            3_310,
            &completion,
            &QuestActions::default(),
            &QuestActions::default(),
            &dialogue,
        )
        .expect("exact 3310 reactor-device exception");
        assert_eq!(mapped[0].item_id, 4_031_698);
        assert_eq!(
            mapped[0].provenance,
            crate::content::QuestRestorationProvenance::AuditedReactorDevice
        );

        let wrong_dialogue = QuestDialogue {
            completion: crate::content::QuestCompletionDialogue {
                lost: Some(QuestLostItemDialogue {
                    prompt_pages: vec!["Did you lose #t4031709#?".to_owned()],
                    success_pages: Vec::new(),
                    items: Vec::new(),
                }),
                ..crate::content::QuestCompletionDialogue::default()
            },
            ..QuestDialogue::default()
        };
        assert!(
            validate_lost_item_restoration_flow(
                3_310,
                &completion,
                &QuestActions::default(),
                &QuestActions::default(),
                &wrong_dialogue,
            )
            .is_err()
        );
        assert!(
            validate_lost_item_restoration_flow(
                3_310,
                &QuestCompletionRequirements::default(),
                &QuestActions::default(),
                &QuestActions::default(),
                &dialogue,
            )
            .is_err()
        );
    }

    fn action_phase(prop: Option<i32>) -> WzNodeArc {
        let phase = property("1");
        let items = property("item");
        let entry = property("0");
        add_integer(&entry, "id", 4_000_000);
        add_integer(&entry, "count", 1);
        if let Some(prop) = prop {
            add_integer(&entry, "prop", prop);
        }
        add_child(&items, &entry);
        add_child(&phase, &items);
        phase
    }

    fn quest_4944_action() -> WzNodeArc {
        let action = property("4944");
        add_child(&action, &property("0"));
        let completion = property("1");
        add_integer(&completion, "exp", 8_000);
        let items = property("item");
        for (index, item_id, count) in [
            (0, 4_031_771, 1),
            (1, 2_022_247, -20),
            (2, 2_022_248, -20),
            (3, 2_022_249, -20),
            (4, 2_022_250, -20),
            (5, 2_022_251, 5),
        ] {
            let entry = property(&index.to_string());
            add_integer(&entry, "id", item_id);
            add_integer(&entry, "count", count);
            add_child(&items, &entry);
        }
        add_child(&completion, &items);
        add_child(&action, &completion);
        action
    }

    fn quest_10272_sources() -> (WzNodeArc, WzNodeArc, WzNodeArc) {
        let action = property("10272");
        add_child(&action, &property("0"));
        let completion_action = property("1");
        let action_items = property("item");
        for (index, item_id) in [(0, 4_032_283), (1, 4_032_280)] {
            let entry = property(&index.to_string());
            add_integer(&entry, "id", item_id);
            add_integer(&entry, "count", -10);
            add_integer(&entry, "prop", -1);
            add_child(&action_items, &entry);
        }
        add_child(&completion_action, &action_items);
        add_child(&action, &completion_action);

        let completion_check = property("1");
        add_string(&completion_check, "endscript", "q10272e");
        add_integer(&completion_check, "npc", 9_000_021);
        let check_items = property("item");
        for (index, item_id) in [(0, 4_032_280), (1, 4_032_283)] {
            let entry = property(&index.to_string());
            add_integer(&entry, "id", item_id);
            add_integer(&entry, "count", 10);
            add_child(&check_items, &entry);
        }
        add_child(&completion_check, &check_items);

        let say = property("10272");
        let start = property("0");
        add_string(&start, "0", "Opening page");
        add_string(&start, "1", "Will you help?");
        let no = property("no");
        add_string(&no, "0", "Please help.");
        add_child(&start, &no);
        add_child(&start, &property("stop"));
        let yes = property("yes");
        add_string(
            &yes,
            "0",
            "Bring #t4032280# 10 sheets and #t4032283# 10 pencils.",
        );
        add_child(&start, &yes);
        add_child(&say, &start);
        let completion = property("1");
        add_string(&completion, "0", "The letter is ready.");
        let stop = property("stop");
        let item = property("item");
        add_string(&item, "0", "Both materials are still required.");
        add_child(&stop, &item);
        add_child(&completion, &stop);
        let completion_yes = property("yes");
        add_string(&completion_yes, "0", "A reply arrived.");
        add_string(&completion_yes, "1", "Be honest next time.");
        add_child(&completion, &completion_yes);
        add_child(&say, &completion);

        (action, completion_check, say)
    }

    fn skill_action(
        skill_id: i32,
        master_level: i32,
    ) -> WzNodeArc {
        let action = property("quest");
        let phase = property("1");
        let skills = property("skill");
        let entry = property("0");
        add_integer(&entry, "id", skill_id);
        add_integer(&entry, "masterLevel", master_level);
        add_child(&skills, &entry);
        add_child(&phase, &skills);
        add_child(&action, &phase);
        action
    }

    fn quest_state_action(entries: &[(i32, i32, i32)]) -> WzNodeArc {
        let action = property("quest");
        let phase = property("0");
        let quests = property("quest");
        for (index, quest_id, state) in entries {
            let entry = property(&index.to_string());
            add_integer(&entry, "id", *quest_id);
            add_integer(&entry, "state", *state);
            add_child(&quests, &entry);
        }
        add_child(&phase, &quests);
        add_child(&action, &phase);
        action
    }

    fn local_action_phase(
        actions: &WzNodeArc,
        quest_id: u32,
        phase: &str,
    ) -> WzNodeArc {
        let quest = super::required_child(actions, &quest_id.to_string(), quest_id)
            .expect("local quest action");
        super::required_child(&quest, phase, quest_id).expect("local quest action phase")
    }

    fn child(
        node: &WzNodeArc,
        name: &str,
    ) -> WzNodeArc {
        crate::content::wz::child(node, name)
            .expect("test child lookup")
            .unwrap_or_else(|| panic!("missing test child {name}"))
    }

    fn read_action_phase(
        quest_id: u32,
        action: &WzNodeArc,
        phase: &str,
        item_ids: &BTreeSet<u32>,
        equipment_item_ids: &BTreeSet<u32>,
    ) -> Result<(QuestActions, Vec<String>), QuestContentError> {
        read_action_phase_with_skills(
            quest_id,
            action,
            phase,
            item_ids,
            equipment_item_ids,
            &BTreeSet::new(),
            &BTreeSet::from([quest_id]),
            None,
        )
    }

    fn read_info(
        quest_id: u32,
        info: &WzNodeArc,
    ) -> Result<QuestInfo, QuestContentError> {
        read_info_with_skills(quest_id, info, info, &BTreeSet::new(), &BTreeMap::new())
    }

    fn assert_invalid_action_phase(action: &WzNodeArc) {
        let error = read_action_phase(100, action, "0", &BTreeSet::new(), &BTreeSet::new())
            .expect_err("invalid nested inert metadata must fail");
        assert!(matches!(error, QuestContentError::Invalid { .. }));
    }

    fn property(name: &str) -> WzNodeArc {
        WzNode::from_str(name, WzObjectType::Property(WzSubProperty::Property), None).into_lock()
    }

    fn add_integer(
        parent: &WzNodeArc,
        name: &str,
        value: i32,
    ) {
        let child = WzNode::from_str(name, value, Some(parent)).into_lock();
        add_child(parent, &child);
    }

    fn add_long(
        parent: &WzNodeArc,
        name: &str,
        value: i64,
    ) {
        let child = WzNode::from_str(name, value, Some(parent)).into_lock();
        add_child(parent, &child);
    }

    fn add_string(
        parent: &WzNodeArc,
        name: &str,
        value: &str,
    ) {
        let child =
            WzNode::from_str(name, WzString::from_str(value, [0; 4]), Some(parent)).into_lock();
        add_child(parent, &child);
    }

    fn add_child(
        parent: &WzNodeArc,
        child: &WzNodeArc,
    ) {
        parent.write().expect("test WZ parent").add(child);
    }
}
