use super::*;

#[cfg(test)]
mod tests;

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
pub(crate) struct AuditedActionCorrections {
    pub(crate) quest_10272_completion_item_props: bool,
}
pub(crate) fn audited_action_root(
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

pub(crate) fn validate_audited_fingerprint(
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

pub(crate) fn validate_4944_4960_relationship(
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

pub(crate) fn validate_audited_4944_action(
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

pub(crate) fn validate_audited_4960_parsed_actions(
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

pub(crate) fn audited_action_corrections(
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

pub(crate) fn validate_audited_10272_action(
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

pub(crate) fn validate_audited_10272_completion_check(
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

pub(crate) fn validate_audited_10272_dialogue(
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

pub(crate) fn validate_check_phase_tree(
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
#[derive(PartialEq, Eq)]
enum AuditedNodeValue {
    Property,
    Null,
    Int(i32),
    Short(i16),
    Long(i64),
    String(String),
}

pub(crate) fn audited_nodes_equal(
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

pub(crate) fn audited_node_fingerprint(
    quest_id: u32,
    node: &WzNodeArc,
) -> Result<u64, QuestContentError> {
    let mut fingerprint = 0xcbf29ce484222325_u64;
    update_audited_node_fingerprint(quest_id, node, &mut fingerprint)?;
    Ok(fingerprint)
}

pub(crate) fn update_audited_node_fingerprint(
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

pub(crate) fn update_fingerprint(
    fingerprint: &mut u64,
    bytes: &[u8],
) {
    for byte in bytes {
        *fingerprint ^= u64::from(*byte);
        *fingerprint = fingerprint.wrapping_mul(0x100000001b3);
    }
}
pub(crate) fn retain_audited_stray_selected_mob(
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

pub(crate) fn validate_misplaced_quest_info(
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

pub(crate) fn validate_audited_action_field_enter(
    quest_id: u32,
    phase: &str,
    field: &WzNodeArc,
    authoritative_check: Option<&WzNodeArc>,
) -> Result<(), QuestContentError> {
    if quest_id != 9_866 || phase != "0" {
        return Err(unsupported(
            quest_id,
            "map action",
            format!("action {phase} field \"fieldEnter\" is not safely representable"),
        ));
    }
    let check = authoritative_check.ok_or_else(|| {
        invalid(
            quest_id,
            "Act/9866/0/fieldEnter has no authoritative Check/9866/0 to compare",
        )
    })?;
    let check_field = required_child(check, "fieldEnter", quest_id)?;
    if !audited_nodes_equal(quest_id, field, &check_field)? {
        return Err(invalid(
            quest_id,
            "Act/9866/0/fieldEnter does not exactly duplicate Check/9866/0/fieldEnter",
        ));
    }
    Ok(())
}

pub(crate) fn audited_action_item_correction(
    corrections: &AuditedActionCorrections,
    quest_id: u32,
    phase: &str,
    entry_name: &str,
    item_id: u32,
    count: i64,
    prop: Option<i64>,
    expiration: Option<QuestItemExpiration>,
    eligibility: QuestRewardEligibility,
) -> Result<Option<(QuestItemDelta, String)>, QuestContentError> {
    if !corrections.quest_10272_completion_item_props {
        return Ok(None);
    }
    let expected = match entry_name {
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
                "audited Act/10272/1/item/{entry_name}/prop=-1 correction no longer has its exact \
                 item removal shape"
            ),
        ));
    }
    Ok(Some((
        QuestItemDelta {
            item_id,
            count,
            expiration,
        },
        format!("act/1/item/{entry_name}/prop=-1"),
    )))
}

pub(crate) fn retain_audited_misplaced_quest_info(
    quest_id: u32,
    status: u32,
    node: &WzNodeArc,
    info_root: &WzNodeArc,
) -> Result<bool, QuestContentError> {
    if quest_id != 8_833 || status != 4_963 {
        return Ok(false);
    }
    validate_misplaced_quest_info(quest_id, node, info_root)?;
    Ok(true)
}

pub(crate) fn validate_audited_lost_item_restoration(
    quest_id: u32,
    completion: &QuestCompletionRequirements,
    start_actions: &QuestActions,
    completion_actions: &QuestActions,
    dialogue: &QuestDialogue,
    rule: &super::super::restoration::AuditedRestorationRule,
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
