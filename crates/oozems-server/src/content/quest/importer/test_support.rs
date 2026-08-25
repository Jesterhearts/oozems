pub(super) use std::collections::BTreeMap;
pub(super) use std::collections::BTreeSet;
pub(super) use std::num::NonZeroU32;

pub(super) use wz_reader::WzNode;
pub(super) use wz_reader::WzNodeArc;
pub(super) use wz_reader::WzObjectType;
pub(super) use wz_reader::property::WzString;
pub(super) use wz_reader::property::WzSubProperty;

pub(super) use super::audited_action_corrections;
pub(super) use super::calendar_unix_ms;
pub(super) use super::item_expiration_unix_ms;
pub(super) use super::quest_timer_milliseconds;
pub(super) use super::read_action_items;
pub(super) use super::read_action_items_with_corrections;
pub(super) use super::read_action_phase as read_action_phase_with_effects;
pub(super) use super::read_completion_requirements as read_completion_requirements_with_effects;
pub(super) use super::read_info as read_info_with_skills;
pub(super) use super::read_record_conditions;
pub(super) use super::read_start_requirements as read_start_requirements_with_effects;
pub(super) use super::validate_audited_4944_action;
pub(super) use super::validate_lost_item_restoration_flow;
pub(super) use super::validate_selectable_reward_flow;
pub(super) use crate::content::QuestActions;
pub(super) use crate::content::QuestCompletionRequirements;
pub(super) use crate::content::QuestConditionalItemReward;
pub(super) use crate::content::QuestDialogue;
pub(super) use crate::content::QuestInfo;
pub(super) use crate::content::QuestItemCondition;
pub(super) use crate::content::QuestItemDelta;
pub(super) use crate::content::QuestItemExpiration;
pub(super) use crate::content::QuestItemRequirement;
pub(super) use crate::content::QuestLostItemDialogue;
pub(super) use crate::content::QuestRecordPredicate;
pub(super) use crate::content::QuestRecordWrite;
pub(super) use crate::content::QuestRewardEligibility;
pub(super) use crate::content::QuestRewardGender;
pub(super) use crate::content::QuestSelectableItemReward;
pub(super) use crate::content::QuestStateAction;
pub(super) use crate::content::QuestStateActionState;
pub(super) use crate::content::QuestWeightedItem;
pub(super) use crate::content::quest::QuestContentError;

pub(super) fn read_start_requirements(
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

pub(super) fn read_completion_requirements(
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

pub(super) fn read_action_phase_with_skills(
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

pub(super) fn add_empty_npc_animation(phase: &WzNodeArc) {
    add_string(phase, "npcAct", "");
}

pub(super) fn add_integer_npc_animation(phase: &WzNodeArc) {
    add_integer(phase, "npcAct", 1);
}

pub(super) fn action_phase(prop: Option<i32>) -> WzNodeArc {
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

pub(super) fn quest_4944_action() -> WzNodeArc {
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

pub(super) fn quest_10272_sources() -> (WzNodeArc, WzNodeArc, WzNodeArc) {
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

pub(super) fn skill_action(
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

pub(super) fn quest_state_action(entries: &[(i32, i32, i32)]) -> WzNodeArc {
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

pub(super) fn child(
    node: &WzNodeArc,
    name: &str,
) -> WzNodeArc {
    crate::content::wz::child(node, name)
        .expect("test child lookup")
        .unwrap_or_else(|| panic!("missing test child {name}"))
}

pub(super) fn read_action_phase(
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

pub(super) fn read_info(
    quest_id: u32,
    info: &WzNodeArc,
) -> Result<QuestInfo, QuestContentError> {
    read_info_with_skills(quest_id, info, info, &BTreeSet::new(), &BTreeMap::new())
}

pub(super) fn assert_invalid_action_phase(action: &WzNodeArc) {
    let error = read_action_phase(100, action, "0", &BTreeSet::new(), &BTreeSet::new())
        .expect_err("invalid nested inert metadata must fail");
    assert!(matches!(error, QuestContentError::Invalid { .. }));
}

pub(super) fn property(name: &str) -> WzNodeArc {
    WzNode::from_str(name, WzObjectType::Property(WzSubProperty::Property), None).into_lock()
}

pub(super) fn add_integer(
    parent: &WzNodeArc,
    name: &str,
    value: i32,
) {
    let child = WzNode::from_str(name, value, Some(parent)).into_lock();
    add_child(parent, &child);
}

pub(super) fn add_long(
    parent: &WzNodeArc,
    name: &str,
    value: i64,
) {
    let child = WzNode::from_str(name, value, Some(parent)).into_lock();
    add_child(parent, &child);
}

pub(super) fn add_string(
    parent: &WzNodeArc,
    name: &str,
    value: &str,
) {
    let child = WzNode::from_str(name, WzString::from_str(value, [0; 4]), Some(parent)).into_lock();
    add_child(parent, &child);
}

pub(super) fn add_child(
    parent: &WzNodeArc,
    child: &WzNodeArc,
) {
    parent.write().expect("test WZ parent").add(child);
}
