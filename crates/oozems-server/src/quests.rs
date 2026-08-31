use std::collections::BTreeMap;
use std::collections::BTreeSet;

use jiff::Timestamp;
use jiff::civil::Weekday;
use jiff::tz::Offset;
use oozems_proto::v1::CharacterGender;
use oozems_proto::v1::ItemDefinition;
use oozems_proto::v1::LearnedSkill;
use oozems_proto::v1::MobDefinition;
use oozems_proto::v1::PlayerQuest;
use oozems_proto::v1::PlayerState;
use oozems_proto::v1::QuestJournal;
use oozems_proto::v1::QuestMobProgress;
use oozems_proto::v1::QuestStatus;
use oozems_proto::v1::QuestTrackerEntry;
use oozems_proto::v1::QuestTrackerObjective;
use oozems_proto::v1::QuestTrackerProgressKind;
use thiserror::Error;

use crate::content::ConsumeEffectDefinition;
use crate::content::QuestActions;
use crate::content::QuestDefinition;
use crate::content::QuestEquippedItemRequirements;
use crate::content::QuestItemCondition;
use crate::content::QuestItemExpiration;
use crate::content::QuestItemRequirement;
use crate::content::QuestMonsterBookRequirements;
use crate::content::QuestQuestionSequence;
use crate::content::QuestQuestionStep;
use crate::content::QuestRecordCondition;
use crate::content::QuestRecordPredicate;
use crate::content::QuestRewardEligibility;
use crate::content::QuestRewardGender;
use crate::content::QuestSelectableItemReward;
use crate::content::QuestSkillChange;
use crate::content::QuestSkillOperation;
use crate::content::QuestSkillRequirement;
use crate::content::QuestStateAction;
use crate::content::QuestStateActionState;
use crate::content::QuestStateRequirement;
use crate::content::QuestWeekday;
use crate::content::QuestWeightedItem;
use crate::content::RequiredQuestState;
use crate::effects::PlayerEffects;
use crate::experience::ExperienceCurve;
use crate::experience::ExperienceRuleError;
use crate::items::ItemRuleError;
use crate::quest_scripts::QuestScriptCatalog;
use crate::quest_scripts::QuestScriptPhase;
use crate::quest_scripts::QuestScriptPlan;
use crate::quest_scripts::QuestScriptResolution;
use crate::quest_scripts::resolve as resolve_quest_script;

mod actions;
mod automatic;
mod availability;
mod choices;
mod indicators;
mod readiness;
mod rewards;

pub(crate) use actions::*;
pub(crate) use automatic::*;
pub(crate) use availability::*;
pub(crate) use choices::*;
pub(crate) use indicators::*;
pub(crate) use readiness::*;
pub(crate) use rewards::*;

pub const ACCEPT_CHOICE_ID: u32 = 1;
pub const DECLINE_CHOICE_ID: u32 = 2;
pub const COMPLETE_CHOICE_ID: u32 = 3;
pub const RESTORE_ITEMS_CHOICE_ID: u32 = 0x4000_0000;
const ANSWER_CHOICE_OFFSET: u32 = 0x1000_0000;
const ANSWER_CHOICE_PHASE_CAPACITY: u32 = 0x1800_0000;
const ANSWER_CHOICE_STEP_STRIDE: u32 = 0x0001_0000;
const SELECTABLE_REWARD_CHOICE_OFFSET: u32 = 0x8000_0000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QuestEnvironment {
    pub now_unix_ms: u64,
    pub world_id: u32,
    pub learned_skill_modifiers: crate::skills::LearnedSkillModifiers,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuestProgress {
    NotStarted,
    Started,
    Completed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuestObjectiveKind {
    Item,
    Mob,
    Quest,
    Level,
    Mesos,
    CompletedQuests,
    Equipment,
    Availability,
    Script,
    Record,
    Buff,
    MonsterBookCardMinimum,
    MonsterBookCardMaximum,
    MonsterBookUniqueMinimum,
    MonsterBookUniqueMaximum,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuestObjectiveProgress {
    pub kind: QuestObjectiveKind,
    pub tracker_kind: QuestTrackerProgressKind,
    pub target_ids: Vec<u32>,
    pub target_quest_status: QuestStatus,
    pub label: String,
    pub current: u64,
    pub required: u64,
    pub complete: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuestReadiness {
    pub ready: bool,
    pub objectives: Vec<QuestObjectiveProgress>,
}

#[derive(Clone, Debug)]
pub struct QuestSelection {
    pub player: PlayerState,
    pub pages: Vec<String>,
    pub changed: bool,
    pub npc_animation_action: Option<String>,
    pub next_interaction: Option<QuestNextInteraction>,
    #[allow(dead_code)]
    pub next_quest_id: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuestQuestionPhase {
    Start,
    Completion,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuestNextInteraction {
    Question {
        phase: QuestQuestionPhase,
        step_index: usize,
    },
    StartDecision,
    SelectableReward,
}

#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq)]
pub struct MobKillResult {
    pub player: PlayerState,
    pub changed_quest_ids: Vec<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum AutomaticQuestTransition {
    Start,
    Completion,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AutomaticQuestFailure {
    pub quest_id: u32,
    pub transition: AutomaticQuestTransition,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AutomaticQuestAdvance {
    pub player: PlayerState,
    pub effects: PlayerEffects,
    pub changed: bool,
    pub started_quest_ids: Vec<u32>,
    pub completed_quest_ids: Vec<u32>,
    pub expired_quest_ids: Vec<u32>,
    pub failures: Vec<AutomaticQuestFailure>,
}

#[derive(Debug, Error)]
pub enum QuestRuleError {
    #[error("quest {quest_id} is not available")]
    Unavailable { quest_id: u32 },
    #[error("quest {quest_id} is not active")]
    NotActive { quest_id: u32 },
    #[error("quest {quest_id} objectives are incomplete")]
    ObjectivesIncomplete { quest_id: u32 },
    #[error("quest {quest_id} has expired")]
    Expired { quest_id: u32 },
    #[error("NPC {npc_id} cannot perform that quest action")]
    WrongNpc { npc_id: u32 },
    #[error("quest choice {choice_id} is invalid")]
    InvalidChoice { choice_id: u32 },
    #[error("quest {quest_id} question state exceeds the supported range")]
    QuestionStateOverflow { quest_id: u32 },
    #[error("quest {quest_id} requires the player to select a completion reward")]
    RewardSelectionRequired { quest_id: u32 },
    #[error("quest {quest_id} has no selectable completion reward eligible for this player")]
    NoEligibleSelectableReward { quest_id: u32 },
    #[error("quest {quest_id} does not support lost-item restoration")]
    LostItemRestorationUnavailable { quest_id: u32 },
    #[error("quest {quest_id} has no missing restorable items")]
    NoMissingRestorableItems { quest_id: u32 },
    #[error("quest {quest_id} requires {phase:?} script {script:?}")]
    ScriptRequired {
        quest_id: u32,
        phase: QuestScriptPhase,
        script: String,
    },
    #[error("player {player_id:?} does not contain character stats")]
    MissingStats { player_id: String },
    #[error("player {player_id:?} fame exceeds the supported range")]
    FameOverflow { player_id: String },
    #[error("quest {quest_id} combined actions exceed the supported range")]
    ActionOverflow { quest_id: u32 },
    #[error("quest {quest_id} item removal {item_id} cannot define expiration")]
    ExpiringRemoval { quest_id: u32, item_id: u32 },
    #[error("quest {quest_id} references unavailable consume effect item {item_id}")]
    MissingConsumeEffect { quest_id: u32, item_id: u32 },
    #[error(transparent)]
    Item(#[from] ItemRuleError),
    #[error(transparent)]
    Experience(#[from] ExperienceRuleError),
    #[error(transparent)]
    Record(#[from] crate::quest_records::QuestRecordError),
}

#[cfg(test)]
mod integration_tests;
#[cfg(test)]
mod test_support;
