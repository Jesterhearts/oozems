use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::num::NonZeroU32;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct QuestDefinition {
    pub id: u32,
    pub name: String,
    pub start: QuestStartRequirements,
    pub completion: QuestCompletionRequirements,
    pub start_actions: QuestActions,
    pub completion_actions: QuestActions,
    pub dialogue: QuestDialogue,
    pub info: QuestInfo,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct QuestStartRequirements {
    pub npc_id: Option<u32>,
    pub minimum_fame: Option<i32>,
    pub minimum_world_id: Option<u32>,
    pub maximum_world_id: Option<u32>,
    pub allowed_jobs: Vec<u32>,
    pub allowed_map_ids: Vec<u32>,
    pub minimum_level: Option<u32>,
    pub maximum_level: Option<u32>,
    pub items: Vec<QuestItemRequirement>,
    pub monster_book: QuestMonsterBookRequirements,
    pub equipped_items: QuestEquippedItemRequirements,
    pub quests: Vec<QuestStateRequirement>,
    pub skills: Vec<QuestSkillRequirement>,
    pub effects: Vec<QuestEffectRequirement>,
    pub required_morph_id: Option<NonZeroU32>,
    pub record_conditions: Vec<QuestRecordCondition>,
    pub available_from: Option<QuestCalendar>,
    pub available_until: Option<QuestCalendar>,
    pub repeat: QuestRepeatMetadata,
    pub normal_auto_start: bool,
    pub script: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct QuestCompletionRequirements {
    pub npc_id: Option<u32>,
    pub minimum_mesos: Option<u64>,
    pub minimum_completed_quest_count: Option<u32>,
    pub items: Vec<QuestItemRequirement>,
    pub monster_book: QuestMonsterBookRequirements,
    pub equipped_items: QuestEquippedItemRequirements,
    pub mobs: Vec<QuestMobObjective>,
    pub quests: Vec<QuestStateRequirement>,
    pub effects: Vec<QuestEffectRequirement>,
    pub required_morph_id: Option<NonZeroU32>,
    pub record_conditions: Vec<QuestRecordCondition>,
    pub required_level: Option<u32>,
    pub available_from: Option<QuestCalendar>,
    pub available_until: Option<QuestCalendar>,
    pub script: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct QuestItemRequirement {
    pub item_id: u32,
    pub condition: QuestItemCondition,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum QuestItemCondition {
    Absent,
    AtLeast(NonZeroU32),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct QuestMonsterBookRequirements {
    pub cards: Vec<QuestMonsterBookCardRequirement>,
    pub minimum_unique_cards: Option<u32>,
    pub maximum_unique_cards: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct QuestMonsterBookCardRequirement {
    pub card_item_id: u32,
    pub minimum_count: Option<u32>,
    pub maximum_count: Option<u32>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct QuestEquippedItemRequirements {
    pub all_of: Vec<u32>,
    pub any_of: Vec<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct QuestMobObjective {
    pub mob_id: u32,
    pub count: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct QuestStateRequirement {
    pub quest_id: u32,
    pub state: RequiredQuestState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct QuestSkillRequirement {
    pub skill_id: u32,
    pub acquired: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct QuestEffectRequirement {
    pub item_id: u32,
    pub active: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RequiredQuestState {
    NotStarted,
    Started,
    Completed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct QuestStateAction {
    pub quest_id: u32,
    pub state: QuestStateActionState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum QuestStateActionState {
    NotStarted,
    Started,
    Completed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct QuestRecordCondition {
    pub quest_id: u32,
    pub index: u32,
    pub alternatives: Vec<QuestRecordPredicate>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum QuestRecordPredicate {
    Equal(String),
    AtLeast(u64),
    AtMost(u64),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct QuestCalendar {
    pub source: String,
    pub unix_ms: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct QuestRepeatMetadata {
    pub interval_ms: Option<u64>,
    pub day_by_day: bool,
    pub days_of_week: BTreeSet<QuestWeekday>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum QuestWeekday {
    Monday,
    Tuesday,
    Wednesday,
    Thursday,
    Friday,
    Saturday,
    Sunday,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct QuestActions {
    pub fixed_items: Vec<QuestItemDelta>,
    pub conditional_items: Vec<QuestConditionalItemReward>,
    pub weighted_items: Vec<QuestWeightedItem>,
    pub selectable_items: Vec<QuestSelectableItemReward>,
    pub money: i64,
    pub experience: u64,
    pub fame: i32,
    pub next_quest_id: Option<u32>,
    pub quest_state_actions: Vec<QuestStateAction>,
    pub record_writes: Vec<QuestRecordWrite>,
    pub skill_changes: Vec<QuestSkillChange>,
    pub buff_item_ids: Vec<u32>,
    pub presentation_npc_id: Option<u32>,
    pub npc_animation_action: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct QuestSkillChange {
    pub skill_id: u32,
    pub operation: QuestSkillOperation,
    pub job_ids: Vec<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum QuestSkillOperation {
    Grant { skill_level: u32, master_level: u32 },
    Remove,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct QuestRecordWrite {
    pub quest_id: u32,
    pub index: u32,
    pub value: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct QuestItemDelta {
    pub item_id: u32,
    pub count: i64,
    pub expiration: Option<QuestItemExpiration>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum QuestItemExpiration {
    RelativeMilliseconds(u64),
    AbsoluteUnixMilliseconds(u64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct QuestConditionalItemReward {
    pub item_id: u32,
    pub count: u32,
    pub expiration: Option<QuestItemExpiration>,
    pub eligibility: QuestRewardEligibility,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct QuestWeightedItem {
    pub item_id: u32,
    pub count: u32,
    pub expiration: Option<QuestItemExpiration>,
    pub weight: u32,
    pub eligibility: QuestRewardEligibility,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct QuestSelectableItemReward {
    pub item_id: u32,
    pub count: u32,
    pub expiration: Option<QuestItemExpiration>,
    pub eligibility: QuestRewardEligibility,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct QuestRewardEligibility {
    pub job_mask: Option<u32>,
    pub gender: Option<QuestRewardGender>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum QuestRewardGender {
    Male,
    Female,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct QuestDialogue {
    pub offer_pages: Vec<String>,
    pub accepted_pages: Vec<String>,
    pub declined_pages: Vec<String>,
    pub has_start_decision: bool,
    pub start_question: Option<QuestQuestionSequence>,
    pub completion: QuestCompletionDialogue,
    pub question: Option<QuestQuestionSequence>,
    pub retained_fields: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct QuestCompletionDialogue {
    pub pages: Vec<String>,
    pub success_pages: Vec<String>,
    pub declined_pages: Vec<String>,
    pub incomplete: QuestIncompleteDialogue,
    pub lost: Option<QuestLostItemDialogue>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct QuestLostItemDialogue {
    pub prompt_pages: Vec<String>,
    pub success_pages: Vec<String>,
    pub items: Vec<QuestRestorableItem>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct QuestRestorableItem {
    pub item_id: u32,
    pub target_count: u64,
    pub expiration: Option<QuestItemExpiration>,
    pub provenance: QuestRestorationProvenance,
    pub eligibility: QuestRestorationEligibility,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum QuestRestorationProvenance {
    InferredStartGrant,
    AuditedCompletionGrant,
    AuditedReactorDevice,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct QuestRestorationEligibility {
    pub owner_state: RequiredQuestState,
    pub required_quests: &'static [QuestStateRequirement],
    pub forbidden_quests: &'static [QuestStateRequirement],
    pub absent_skill_ids: &'static [u32],
    pub absent_item_ids: &'static [u32],
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct QuestIncompleteDialogue {
    pub item_pages: Vec<String>,
    pub mob_pages: Vec<String>,
    pub quest_pages: Vec<String>,
    pub default_pages: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct QuestQuestionSequence {
    pub leading_pages: Vec<String>,
    pub steps: Vec<QuestQuestionStep>,
    pub trailing_pages: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct QuestQuestionStep {
    pub archive_index: u32,
    pub prompt: String,
    pub choices: Vec<QuestChoice>,
    pub correct_choice_id: u32,
    pub continuation_pages: Vec<String>,
    pub failure_pages: HashMap<u32, Vec<String>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct QuestChoice {
    pub id: u32,
    pub label: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct QuestInfo {
    pub area: Option<u32>,
    pub status_text: BTreeMap<u32, String>,
    pub summary: Option<String>,
    pub demand_summary: Option<String>,
    pub reward_summary: Option<String>,
    pub time_limit_ms: Option<u64>,
    pub time_limit2_ms: Option<u64>,
    pub auto_start: bool,
    pub auto_accept: bool,
    pub auto_complete: bool,
    pub auto_pre_complete: bool,
    pub selected_skill: Option<QuestSelectedSkill>,
    pub retained_metadata_fields: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct QuestSelectedSkill {
    pub id: NonZeroU32,
    pub name: Option<String>,
}
