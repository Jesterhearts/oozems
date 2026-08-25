use std::collections::BTreeMap;
use std::collections::BTreeSet;

use jiff::Timestamp;
use jiff::civil::Weekday;
use jiff::tz::Offset;
use oozems_proto::v1::CharacterGender;
use oozems_proto::v1::ItemDefinition;
use oozems_proto::v1::LearnedSkill;
use oozems_proto::v1::PlayerQuest;
use oozems_proto::v1::PlayerState;
use oozems_proto::v1::QuestMobProgress;
use oozems_proto::v1::QuestStatus;
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

pub fn progress(
    player: &PlayerState,
    quest_id: u32,
) -> QuestProgress {
    player
        .quests
        .iter()
        .find(|quest| quest.quest_id == quest_id)
        .and_then(|quest| QuestStatus::try_from(quest.status).ok())
        .map_or(QuestProgress::NotStarted, |status| match status {
            QuestStatus::Started => QuestProgress::Started,
            QuestStatus::Completed => QuestProgress::Completed,
            QuestStatus::Unspecified => QuestProgress::NotStarted,
        })
}

fn quest_expiration_deadline(
    entry: &PlayerQuest,
    quest: &QuestDefinition,
) -> Option<u64> {
    // HeavenMS applies timeLimit2 after timeLimit, so it takes precedence when both
    // exist.
    let duration_ms = quest.info.time_limit2_ms.or(quest.info.time_limit_ms)?;
    Some(entry.accepted_at_unix_ms.saturating_add(duration_ms))
}

fn quest_is_expired(
    entry: &PlayerQuest,
    quest: &QuestDefinition,
    now_unix_ms: u64,
) -> bool {
    quest_expiration_deadline(entry, quest).is_some_and(|deadline| now_unix_ms >= deadline)
}

pub fn is_available(
    player: &PlayerState,
    effects: &PlayerEffects,
    quest: &QuestDefinition,
    item_definitions: &[ItemDefinition],
    scripts: &QuestScriptCatalog,
    environment: QuestEnvironment,
) -> bool {
    if !start_status_date_and_script_allow(
        player,
        effects,
        quest,
        item_definitions,
        scripts,
        environment,
    ) || player.level < quest.start.minimum_level.unwrap_or(1)
        || quest
            .start
            .maximum_level
            .is_some_and(|maximum| player.level > maximum)
    {
        return false;
    }
    let stats = player.stats.as_ref();
    if quest
        .start
        .minimum_fame
        .is_some_and(|minimum| stats.is_none_or(|stats| stats.fame < minimum))
    {
        return false;
    }
    let job_id = stats.map_or(0, |stats| stats.job_id);
    if !quest.start.allowed_jobs.is_empty() && !quest.start.allowed_jobs.contains(&job_id) {
        return false;
    }
    if !quest.start.allowed_map_ids.is_empty()
        && !quest.start.allowed_map_ids.contains(&player.map_id)
    {
        return false;
    }
    requirements_have_items(player, &quest.start.items, item_definitions)
        && requirements_have_quest_states(player, &quest.start.quests)
        && requirements_have_skills(player, &quest.start.skills)
        && requirements_have_records(player, &quest.start.record_conditions)
        && requirements_have_effects(effects, &quest.start.effects)
        && required_morph_is_active(effects, quest.start.required_morph_id)
}

fn start_status_date_and_script_allow(
    player: &PlayerState,
    effects: &PlayerEffects,
    quest: &QuestDefinition,
    item_definitions: &[ItemDefinition],
    scripts: &QuestScriptCatalog,
    environment: QuestEnvironment,
) -> bool {
    script_conditions_pass(
        scripts,
        player,
        quest,
        QuestScriptPhase::Start,
        item_definitions,
    ) && repeat_policy_allows(player, quest, environment.now_unix_ms)
        && requirements_have_equipped_items(player, &quest.start.equipped_items)
        && quest
            .start
            .minimum_world_id
            .is_none_or(|minimum| environment.world_id >= minimum)
        && quest
            .start
            .maximum_world_id
            .is_none_or(|maximum| environment.world_id <= maximum)
        && quest
            .start
            .available_from
            .as_ref()
            .is_none_or(|start| environment.now_unix_ms >= start.unix_ms)
        && quest
            .start
            .available_until
            .as_ref()
            .is_none_or(|end| environment.now_unix_ms <= end.unix_ms)
        && weekday_allows(quest, environment.now_unix_ms)
        && requirements_have_effects(effects, &quest.start.effects)
        && required_morph_is_active(effects, quest.start.required_morph_id)
        && requirements_have_monster_book(player, &quest.start.monster_book)
}

fn completion_window_allows(
    quest: &QuestDefinition,
    now_unix_ms: u64,
) -> bool {
    quest
        .completion
        .available_from
        .as_ref()
        .is_none_or(|start| now_unix_ms >= start.unix_ms)
        && quest
            .completion
            .available_until
            .as_ref()
            .is_none_or(|end| now_unix_ms <= end.unix_ms)
}

fn eligible_completed_quest_count(
    player: &PlayerState,
    quest_definitions: &[&QuestDefinition],
) -> u64 {
    let mut eligible_ids = quest_definitions
        .iter()
        .filter(|quest| !(9_000..=10_999).contains(&quest.id) && quest.info.area != Some(51))
        .map(|quest| quest.id)
        .collect::<BTreeSet<_>>();
    player
        .quests
        .iter()
        .filter(|entry| QuestStatus::try_from(entry.status) == Ok(QuestStatus::Completed))
        .filter(|entry| eligible_ids.remove(&entry.quest_id))
        .count() as u64
}

pub fn completion_readiness(
    player: &PlayerState,
    effects: &PlayerEffects,
    quest: &QuestDefinition,
    quest_definitions: &[&QuestDefinition],
    item_definitions: &[ItemDefinition],
    scripts: &QuestScriptCatalog,
    environment: QuestEnvironment,
) -> QuestReadiness {
    let active = player
        .quests
        .iter()
        .find(|entry| entry.quest_id == quest.id)
        .filter(|entry| QuestStatus::try_from(entry.status) == Ok(QuestStatus::Started));
    let mut objectives = Vec::new();
    for requirement in &quest.completion.effects {
        let current = effects.contains_item(requirement.item_id);
        objectives.push(QuestObjectiveProgress {
            kind: QuestObjectiveKind::Buff,
            label: if requirement.active {
                format!("Effect item {} is active", requirement.item_id)
            } else {
                format!("Effect item {} is not active", requirement.item_id)
            },
            current: u64::from(current),
            required: u64::from(requirement.active),
            complete: current == requirement.active,
        });
    }
    if let Some(morph_id) = quest.completion.required_morph_id {
        let current = effects.projected().morph_id;
        objectives.push(QuestObjectiveProgress {
            kind: QuestObjectiveKind::Buff,
            label: format!("Active morph {}", morph_id),
            current: u64::from(current.unwrap_or_default()),
            required: u64::from(morph_id.get()),
            complete: current == Some(morph_id.get()),
        });
    }
    if let Some(start) = &quest.completion.available_from {
        let complete = environment.now_unix_ms >= start.unix_ms;
        objectives.push(QuestObjectiveProgress {
            kind: QuestObjectiveKind::Availability,
            label: format!("Completion available from {}", start.source),
            current: environment.now_unix_ms,
            required: start.unix_ms,
            complete,
        });
    }
    if let Some(end) = &quest.completion.available_until {
        let complete = environment.now_unix_ms <= end.unix_ms;
        objectives.push(QuestObjectiveProgress {
            kind: QuestObjectiveKind::Availability,
            label: format!("Completion available through {}", end.source),
            current: environment.now_unix_ms,
            required: end.unix_ms,
            complete,
        });
    }
    if let Some(required) = quest.completion.minimum_mesos {
        objectives.push(QuestObjectiveProgress {
            kind: QuestObjectiveKind::Mesos,
            label: "Mesos".to_owned(),
            current: player.mesos,
            required,
            complete: player.mesos >= required,
        });
    }
    if let Some(required) = quest.completion.minimum_completed_quest_count {
        let current = eligible_completed_quest_count(player, quest_definitions);
        objectives.push(QuestObjectiveProgress {
            kind: QuestObjectiveKind::CompletedQuests,
            label: "Eligible completed quests".to_owned(),
            current,
            required: u64::from(required),
            complete: current >= u64::from(required),
        });
    }
    for requirement in &quest.completion.items {
        let current = player.inventory.as_ref().and_then(|inventory| {
            quest_item_quantity(inventory, item_definitions, requirement.item_id).ok()
        });
        let name = item_definitions
            .iter()
            .find(|definition| definition.item_id == requirement.item_id)
            .map(|definition| definition.name.as_str())
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| format!("Item {}", requirement.item_id));
        let (label, required, complete) = match requirement.condition {
            QuestItemCondition::Absent => (format!("Do not possess {name}"), 0, current == Some(0)),
            QuestItemCondition::AtLeast(count) => {
                let required = u64::from(count.get());
                (
                    name,
                    required,
                    current.is_some_and(|current| current >= required),
                )
            }
        };
        objectives.push(QuestObjectiveProgress {
            kind: QuestObjectiveKind::Item,
            label,
            current: current.unwrap_or_default(),
            required,
            complete,
        });
    }
    for requirement in &quest.completion.monster_book.cards {
        let current = u64::from(crate::monster_book::count(
            &player.monster_book_cards,
            requirement.card_item_id,
        ));
        if let Some(required) = requirement.minimum_count {
            objectives.push(QuestObjectiveProgress {
                kind: QuestObjectiveKind::MonsterBookCardMinimum,
                label: format!("Monster Book card {}", requirement.card_item_id),
                current,
                required: u64::from(required),
                complete: current >= u64::from(required),
            });
        }
        if let Some(required) = requirement.maximum_count {
            objectives.push(QuestObjectiveProgress {
                kind: QuestObjectiveKind::MonsterBookCardMaximum,
                label: format!("At most Monster Book card {}", requirement.card_item_id),
                current,
                required: u64::from(required),
                complete: current <= u64::from(required),
            });
        }
    }
    let unique_card_count = player.monster_book_cards.len() as u64;
    if let Some(required) = quest.completion.monster_book.minimum_unique_cards {
        objectives.push(QuestObjectiveProgress {
            kind: QuestObjectiveKind::MonsterBookUniqueMinimum,
            label: "Unique Monster Book cards".to_owned(),
            current: unique_card_count,
            required: u64::from(required),
            complete: unique_card_count >= u64::from(required),
        });
    }
    if let Some(required) = quest.completion.monster_book.maximum_unique_cards {
        objectives.push(QuestObjectiveProgress {
            kind: QuestObjectiveKind::MonsterBookUniqueMaximum,
            label: "Maximum unique Monster Book cards".to_owned(),
            current: unique_card_count,
            required: u64::from(required),
            complete: unique_card_count <= u64::from(required),
        });
    }
    let equipped_item_ids = player
        .inventory
        .as_ref()
        .map(|inventory| {
            inventory
                .equipment
                .iter()
                .map(|item| item.item_id)
                .collect::<BTreeSet<_>>()
        })
        .unwrap_or_default();
    for item_id in &quest.completion.equipped_items.all_of {
        let complete = equipped_item_ids.contains(item_id);
        objectives.push(QuestObjectiveProgress {
            kind: QuestObjectiveKind::Equipment,
            label: format!("Equip {}", item_name(item_definitions, *item_id)),
            current: u64::from(complete),
            required: 1,
            complete,
        });
    }
    if !quest.completion.equipped_items.any_of.is_empty() {
        let complete = quest
            .completion
            .equipped_items
            .any_of
            .iter()
            .any(|item_id| equipped_item_ids.contains(item_id));
        let names = quest
            .completion
            .equipped_items
            .any_of
            .iter()
            .map(|item_id| item_name(item_definitions, *item_id))
            .collect::<Vec<_>>()
            .join(", ");
        objectives.push(QuestObjectiveProgress {
            kind: QuestObjectiveKind::Equipment,
            label: format!("Equip one of: {names}"),
            current: u64::from(complete),
            required: 1,
            complete,
        });
    }
    for objective in &quest.completion.mobs {
        let current = active
            .and_then(|entry| {
                entry
                    .mob_progress
                    .iter()
                    .find(|progress| progress.mob_id == objective.mob_id)
            })
            .map_or(0, |progress| progress.count.min(objective.count));
        let label = quest.info.selected_skill.as_ref().map_or_else(
            || format!("Mob {}", objective.mob_id),
            |skill| match &skill.name {
                Some(name) => format!(
                    "Mob {} using {} (skill {})",
                    objective.mob_id,
                    name,
                    skill.id.get()
                ),
                None => format!("Mob {} using skill {}", objective.mob_id, skill.id.get()),
            },
        );
        objectives.push(QuestObjectiveProgress {
            kind: QuestObjectiveKind::Mob,
            label,
            current: u64::from(current),
            required: u64::from(objective.count),
            complete: current >= objective.count,
        });
    }
    for requirement in &quest.completion.quests {
        let current = progress(player, requirement.quest_id);
        objectives.push(QuestObjectiveProgress {
            kind: QuestObjectiveKind::Quest,
            label: format!(
                "Quest {}: {}",
                requirement.quest_id,
                state_label(requirement.state)
            ),
            current: u64::from(current == required_progress(requirement.state)),
            required: 1,
            complete: current == required_progress(requirement.state),
        });
    }
    for requirement in &quest.completion.record_conditions {
        let complete = record_condition_matches(player, requirement);
        let current = crate::quest_records::get(player, requirement.quest_id, requirement.index);
        objectives.push(QuestObjectiveProgress {
            kind: QuestObjectiveKind::Record,
            label: match (complete, current) {
                (true, _) => format!(
                    "Quest record {}[{}] progress",
                    requirement.quest_id, requirement.index
                ),
                (false, Some(_)) => format!(
                    "Quest record {}[{}] does not match the required progress",
                    requirement.quest_id, requirement.index
                ),
                (false, None) => format!(
                    "Quest record {}[{}] is missing",
                    requirement.quest_id, requirement.index
                ),
            },
            current: u64::from(complete),
            required: 1,
            complete,
        });
    }
    if let Some(required_level) = quest.completion.required_level {
        objectives.push(QuestObjectiveProgress {
            kind: QuestObjectiveKind::Level,
            label: format!("Level {required_level}"),
            current: u64::from(player.level),
            required: u64::from(required_level),
            complete: player.level >= required_level,
        });
    }
    if let Some(script) = &quest.completion.script {
        let resolution = resolve_quest_script(
            scripts,
            quest,
            QuestScriptPhase::Completion,
            player,
            item_definitions,
        );
        let (label, complete) = match resolution {
            QuestScriptResolution::Missing { .. } => {
                (format!("Missing quest script: {script}"), false)
            }
            QuestScriptResolution::ConditionsNotMet { .. } => {
                (format!("Script conditions not met: {script}"), false)
            }
            QuestScriptResolution::Ready(_) => (format!("Script: {script}"), true),
            QuestScriptResolution::NotReferenced => {
                (format!("Missing quest script: {script}"), false)
            }
        };
        objectives.push(QuestObjectiveProgress {
            kind: QuestObjectiveKind::Script,
            label,
            current: u64::from(complete),
            required: 1,
            complete,
        });
    }
    QuestReadiness {
        ready: active.is_some() && objectives.iter().all(|objective| objective.complete),
        objectives,
    }
}

pub fn objective_progress_text(
    player: &PlayerState,
    effects: &PlayerEffects,
    quest: &QuestDefinition,
    quest_definitions: &[&QuestDefinition],
    item_definitions: &[ItemDefinition],
    scripts: &QuestScriptCatalog,
    environment: QuestEnvironment,
) -> Vec<String> {
    completion_readiness(
        player,
        effects,
        quest,
        quest_definitions,
        item_definitions,
        scripts,
        environment,
    )
    .objectives
    .into_iter()
    .map(|objective| match objective.kind {
        QuestObjectiveKind::Script
        | QuestObjectiveKind::Record
        | QuestObjectiveKind::Availability => objective.label,
        _ => format!(
            "{}: {}/{}",
            objective.label, objective.current, objective.required
        ),
    })
    .collect()
}

pub fn incomplete_dialogue_pages(
    player: &PlayerState,
    effects: &PlayerEffects,
    quest: &QuestDefinition,
    quest_definitions: &[&QuestDefinition],
    item_definitions: &[ItemDefinition],
    scripts: &QuestScriptCatalog,
    environment: QuestEnvironment,
) -> Vec<String> {
    let readiness = completion_readiness(
        player,
        effects,
        quest,
        quest_definitions,
        item_definitions,
        scripts,
        environment,
    );
    let branch = readiness
        .objectives
        .iter()
        .find(|objective| !objective.complete)
        .map(|objective| match objective.kind {
            QuestObjectiveKind::Item => &quest.dialogue.completion.incomplete.item_pages,
            QuestObjectiveKind::Mob => &quest.dialogue.completion.incomplete.mob_pages,
            QuestObjectiveKind::Quest => &quest.dialogue.completion.incomplete.quest_pages,
            QuestObjectiveKind::Level
            | QuestObjectiveKind::Mesos
            | QuestObjectiveKind::CompletedQuests
            | QuestObjectiveKind::Equipment
            | QuestObjectiveKind::Availability
            | QuestObjectiveKind::Script
            | QuestObjectiveKind::Record => &quest.dialogue.completion.incomplete.default_pages,
            QuestObjectiveKind::Buff
            | QuestObjectiveKind::MonsterBookCardMinimum
            | QuestObjectiveKind::MonsterBookCardMaximum
            | QuestObjectiveKind::MonsterBookUniqueMinimum
            | QuestObjectiveKind::MonsterBookUniqueMaximum => {
                &quest.dialogue.completion.incomplete.default_pages
            }
        })
        .filter(|pages| !pages.is_empty())
        .cloned()
        .unwrap_or_else(|| quest.dialogue.completion.pages.clone());
    let mut pages = branch;
    if let QuestScriptResolution::ConditionsNotMet {
        incomplete_pages, ..
    } = resolve_quest_script(
        scripts,
        quest,
        QuestScriptPhase::Completion,
        player,
        item_definitions,
    ) {
        pages.extend(incomplete_pages);
    }
    let progress = readiness
        .objectives
        .iter()
        .zip(objective_progress_text(
            player,
            effects,
            quest,
            quest_definitions,
            item_definitions,
            scripts,
            environment,
        ))
        .filter_map(|(objective, text)| (!objective.complete).then_some(text))
        .collect::<Vec<_>>();
    if !progress.is_empty() {
        pages.push(progress.join("\n"));
    }
    pages
}

pub fn record_mob_kills<'a>(
    mut player: PlayerState,
    mob_kills: &[(u32, Option<u32>)],
    quest_definitions: impl IntoIterator<Item = &'a QuestDefinition>,
) -> MobKillResult {
    if mob_kills.is_empty() {
        return MobKillResult {
            player,
            changed_quest_ids: Vec::new(),
        };
    }
    let definitions = quest_definitions
        .into_iter()
        .map(|quest| (quest.id, quest))
        .collect::<BTreeMap<_, _>>();
    let mut changed_quest_ids = BTreeSet::new();
    for entry in &mut player.quests {
        if QuestStatus::try_from(entry.status) != Ok(QuestStatus::Started) {
            continue;
        }
        let Some(quest) = definitions.get(&entry.quest_id) else {
            continue;
        };
        for objective in &quest.completion.mobs {
            let increment = mob_kills
                .iter()
                .filter(|(mob_id, source_skill_id)| {
                    *mob_id == objective.mob_id
                        && quest
                            .info
                            .selected_skill
                            .as_ref()
                            .is_none_or(|required| *source_skill_id == Some(required.id.get()))
                })
                .fold(0_u32, |count, _| count.saturating_add(1));
            if increment == 0 {
                continue;
            }
            let index = entry
                .mob_progress
                .iter()
                .position(|progress| progress.mob_id == objective.mob_id);
            let old = index
                .map(|index| entry.mob_progress[index].count)
                .unwrap_or_default();
            let next = old.saturating_add(increment).min(objective.count);
            if next == old {
                continue;
            }
            if let Some(index) = index {
                entry.mob_progress[index].count = next;
            } else {
                entry.mob_progress.push(QuestMobProgress {
                    mob_id: objective.mob_id,
                    count: next,
                });
            }
            changed_quest_ids.insert(quest.id);
        }
        entry.mob_progress.sort_by_key(|progress| progress.mob_id);
    }
    MobKillResult {
        player,
        changed_quest_ids: changed_quest_ids.into_iter().collect(),
    }
}

pub fn advance_automatic_quests<'a>(
    mut player: PlayerState,
    mut effects: PlayerEffects,
    quest_definitions: impl IntoIterator<Item = &'a QuestDefinition>,
    curve: &ExperienceCurve,
    item_definitions: &[ItemDefinition],
    consume_effects: &[ConsumeEffectDefinition],
    scripts: &QuestScriptCatalog,
    environment: QuestEnvironment,
) -> AutomaticQuestAdvance {
    let definitions = quest_definitions.into_iter().collect::<Vec<_>>();
    debug_assert!(definitions.windows(2).all(|pair| pair[0].id <= pair[1].id));

    let expired_quest_ids = expire_timed_quests(&mut player, &definitions, environment.now_unix_ms);
    let expired = expired_quest_ids.iter().copied().collect::<BTreeSet<_>>();
    let transition_limit = definitions.len().saturating_mul(2);
    let mut applied = BTreeSet::new();
    let mut started_quest_ids = Vec::new();
    let mut completed_quest_ids = Vec::new();
    let mut failures = BTreeMap::new();

    while applied.len() < transition_limit {
        let mut pass_changed = false;
        for quest in &definitions {
            let quest = *quest;
            match progress(&player, quest.id) {
                QuestProgress::NotStarted | QuestProgress::Completed
                    if !expired.contains(&quest.id)
                        && !applied.contains(&(quest.id, AutomaticQuestTransition::Start)) =>
                {
                    if !starts_automatically(quest) {
                        continue;
                    }
                    if !is_available(
                        &player,
                        &effects,
                        quest,
                        item_definitions,
                        scripts,
                        environment,
                    ) {
                        continue;
                    }
                    match start_quest(
                        player.clone(),
                        &mut effects,
                        quest,
                        false,
                        curve,
                        item_definitions,
                        consume_effects,
                        scripts,
                        environment,
                    ) {
                        Ok(selection) => {
                            player = selection.player;
                            applied.insert((quest.id, AutomaticQuestTransition::Start));
                            failures.remove(&(quest.id, AutomaticQuestTransition::Start));
                            started_quest_ids.push(quest.id);
                            pass_changed = true;
                        }
                        Err(error) => {
                            failures.insert(
                                (quest.id, AutomaticQuestTransition::Start),
                                AutomaticQuestFailure {
                                    quest_id: quest.id,
                                    transition: AutomaticQuestTransition::Start,
                                    message: error.to_string(),
                                },
                            );
                        }
                    }
                }
                QuestProgress::Started
                    if !applied.contains(&(quest.id, AutomaticQuestTransition::Completion)) =>
                {
                    let Some(checks) = automatic_completion_checks(quest) else {
                        continue;
                    };
                    let ready = match checks {
                        CompletionChecks::Normal => {
                            completion_readiness(
                                &player,
                                &effects,
                                quest,
                                &definitions,
                                item_definitions,
                                scripts,
                                environment,
                            )
                            .ready
                        }
                        CompletionChecks::Automatic => {
                            completion_window_allows(quest, environment.now_unix_ms)
                                && requirements_have_equipped_items(
                                    &player,
                                    &quest.completion.equipped_items,
                                )
                                && requirements_have_effects(&effects, &quest.completion.effects)
                                && required_morph_is_active(
                                    &effects,
                                    quest.completion.required_morph_id,
                                )
                                && requirements_have_monster_book(
                                    &player,
                                    &quest.completion.monster_book,
                                )
                                && script_conditions_pass(
                                    scripts,
                                    &player,
                                    quest,
                                    QuestScriptPhase::Completion,
                                    item_definitions,
                                )
                        }
                    };
                    if !ready {
                        continue;
                    }
                    match complete_quest(
                        player.clone(),
                        &mut effects,
                        quest,
                        checks,
                        None,
                        curve,
                        item_definitions,
                        consume_effects,
                        scripts,
                        &definitions,
                        environment,
                    ) {
                        Ok(selection) => {
                            player = selection.player;
                            applied.insert((quest.id, AutomaticQuestTransition::Completion));
                            failures.remove(&(quest.id, AutomaticQuestTransition::Completion));
                            completed_quest_ids.push(quest.id);
                            pass_changed = true;
                        }
                        Err(error) => {
                            failures.insert(
                                (quest.id, AutomaticQuestTransition::Completion),
                                AutomaticQuestFailure {
                                    quest_id: quest.id,
                                    transition: AutomaticQuestTransition::Completion,
                                    message: error.to_string(),
                                },
                            );
                        }
                    }
                }
                _ => {}
            }
            if applied.len() >= transition_limit {
                break;
            }
        }
        if !pass_changed {
            break;
        }
    }

    AutomaticQuestAdvance {
        changed: !expired_quest_ids.is_empty() || !applied.is_empty(),
        player,
        effects,
        started_quest_ids,
        completed_quest_ids,
        expired_quest_ids,
        failures: failures.into_values().collect(),
    }
}

fn expire_timed_quests(
    player: &mut PlayerState,
    definitions: &[&QuestDefinition],
    now_unix_ms: u64,
) -> Vec<u32> {
    let definitions = definitions
        .iter()
        .map(|quest| (quest.id, *quest))
        .collect::<BTreeMap<_, _>>();
    let mut expired = Vec::new();
    player.quests.retain(|entry| {
        let should_expire = QuestStatus::try_from(entry.status) == Ok(QuestStatus::Started)
            && definitions
                .get(&entry.quest_id)
                .is_some_and(|quest| quest_is_expired(entry, quest, now_unix_ms));
        if should_expire {
            expired.push(entry.quest_id);
        }
        !should_expire
    });
    expired.sort_unstable();
    for quest_id in &expired {
        crate::quest_records::clear(player, *quest_id);
    }
    expired
}

fn starts_automatically(quest: &QuestDefinition) -> bool {
    if quest.dialogue.start_question.is_some() {
        return false;
    }
    quest.info.auto_accept || quest.start.normal_auto_start || quest.info.auto_start
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CompletionChecks {
    Normal,
    Automatic,
}

fn automatic_completion_checks(quest: &QuestDefinition) -> Option<CompletionChecks> {
    if quest.dialogue.question.is_some() {
        return None;
    }
    if quest.info.auto_pre_complete {
        Some(CompletionChecks::Automatic)
    } else if quest.info.auto_complete {
        Some(CompletionChecks::Normal)
    } else {
        None
    }
}

pub fn answer_choice_id(
    phase: QuestQuestionPhase,
    step_index: usize,
    choice_index: usize,
) -> Option<u32> {
    let phase_start = match phase {
        QuestQuestionPhase::Start => ANSWER_CHOICE_OFFSET,
        QuestQuestionPhase::Completion => {
            ANSWER_CHOICE_OFFSET.checked_add(ANSWER_CHOICE_PHASE_CAPACITY)?
        }
    };
    let phase_end = phase_start.checked_add(ANSWER_CHOICE_PHASE_CAPACITY)?;
    let step_index = u32::try_from(step_index).ok()?;
    let choice_index = u32::try_from(choice_index).ok()?;
    if choice_index >= ANSWER_CHOICE_STEP_STRIDE {
        return None;
    }
    phase_start
        .checked_add(step_index.checked_mul(ANSWER_CHOICE_STEP_STRIDE)?)?
        .checked_add(choice_index)
        .filter(|choice_id| *choice_id < phase_end)
}

pub fn begin_start_question(
    mut player: PlayerState,
    effects: &PlayerEffects,
    quest: &QuestDefinition,
    npc_id: u32,
    item_definitions: &[ItemDefinition],
    scripts: &QuestScriptCatalog,
    environment: QuestEnvironment,
) -> Result<QuestSelection, QuestRuleError> {
    require_npc(quest.start.npc_id, npc_id)?;
    let question = quest
        .dialogue
        .start_question
        .as_ref()
        .ok_or(QuestRuleError::InvalidChoice {
            choice_id: ACCEPT_CHOICE_ID,
        })?;
    let existing_index = player
        .quests
        .iter()
        .position(|entry| entry.quest_id == quest.id);
    if let Some(index) = existing_index
        && QuestStatus::try_from(player.quests[index].status) == Ok(QuestStatus::Unspecified)
        && start_decision_entry_is_pending(&player.quests[index], quest, question)
    {
        return Ok(QuestSelection {
            player,
            pages: question.trailing_pages.clone(),
            changed: false,
            npc_animation_action: None,
            next_interaction: Some(QuestNextInteraction::StartDecision),
            next_quest_id: None,
        });
    }
    if !is_available(
        &player,
        effects,
        quest,
        item_definitions,
        scripts,
        environment,
    ) {
        return Err(QuestRuleError::Unavailable { quest_id: quest.id });
    }
    if let Some(index) = existing_index
        && QuestStatus::try_from(player.quests[index].status) == Ok(QuestStatus::Unspecified)
        && let Some(step_index) = question_step_index(&player.quests[index], question)
    {
        return Ok(QuestSelection {
            player,
            pages: Vec::new(),
            changed: false,
            npc_animation_action: None,
            next_interaction: Some(QuestNextInteraction::Question {
                phase: QuestQuestionPhase::Start,
                step_index,
            }),
            next_quest_id: None,
        });
    }

    let mut entry = existing_index
        .map(|index| player.quests[index].clone())
        .unwrap_or_default();
    entry.quest_id = quest.id;
    entry.status = QuestStatus::Unspecified as i32;
    entry.mob_progress.clear();
    entry.dialogue_step = 1;
    entry.completion_quiz_passed = false;
    if let Some(index) = existing_index {
        player.quests[index] = entry;
    } else {
        player.quests.push(entry);
        player.quests.sort_by_key(|entry| entry.quest_id);
    }
    Ok(QuestSelection {
        player,
        pages: question.leading_pages.clone(),
        changed: true,
        npc_animation_action: None,
        next_interaction: Some(QuestNextInteraction::Question {
            phase: QuestQuestionPhase::Start,
            step_index: 0,
        }),
        next_quest_id: None,
    })
}

pub fn begin_completion_question(
    mut player: PlayerState,
    effects: &PlayerEffects,
    quest: &QuestDefinition,
    quest_definitions: &[&QuestDefinition],
    npc_id: u32,
    item_definitions: &[ItemDefinition],
    scripts: &QuestScriptCatalog,
    environment: QuestEnvironment,
) -> Result<QuestSelection, QuestRuleError> {
    require_npc(quest.completion.npc_id, npc_id)?;
    let question = quest
        .dialogue
        .question
        .as_ref()
        .ok_or(QuestRuleError::InvalidChoice {
            choice_id: COMPLETE_CHOICE_ID,
        })?;
    require_completion_ready(
        &player,
        effects,
        quest,
        quest_definitions,
        item_definitions,
        scripts,
        environment,
    )?;
    let entry = player
        .quests
        .iter_mut()
        .find(|entry| entry.quest_id == quest.id)
        .filter(|entry| QuestStatus::try_from(entry.status) == Ok(QuestStatus::Started))
        .ok_or(QuestRuleError::NotActive { quest_id: quest.id })?;
    if entry.completion_quiz_passed {
        return Ok(QuestSelection {
            player,
            pages: Vec::new(),
            changed: false,
            npc_animation_action: None,
            next_interaction: Some(QuestNextInteraction::SelectableReward),
            next_quest_id: None,
        });
    }
    let (step_index, changed, pages) = match question_step_index(entry, question) {
        Some(step_index) => (step_index, false, Vec::new()),
        None => {
            entry.dialogue_step = 1;
            (0, true, question.leading_pages.clone())
        }
    };
    Ok(QuestSelection {
        player,
        pages,
        changed,
        npc_animation_action: None,
        next_interaction: Some(QuestNextInteraction::Question {
            phase: QuestQuestionPhase::Completion,
            step_index,
        }),
        next_quest_id: None,
    })
}

pub(crate) fn eligible_selectable_reward_choices(
    player: &PlayerState,
    quest: &QuestDefinition,
) -> Vec<(u32, QuestSelectableItemReward)> {
    quest
        .completion_actions
        .selectable_items
        .iter()
        .copied()
        .enumerate()
        .filter(|(_, reward)| reward_is_eligible(player, reward.eligibility))
        .filter_map(|(index, reward)| {
            selectable_reward_choice_id(index).map(|choice_id| (choice_id, reward))
        })
        .collect()
}

pub(crate) fn lost_item_restoration_needed(
    player: &PlayerState,
    quest: &QuestDefinition,
    item_definitions: &[ItemDefinition],
    now_unix_ms: u64,
) -> bool {
    let Some(lost) = quest.dialogue.completion.lost.as_ref() else {
        return false;
    };
    missing_restoration_grants(player, quest, lost, item_definitions, now_unix_ms)
        .is_ok_and(|grants| !grants.is_empty())
}

fn missing_restoration_grants(
    player: &PlayerState,
    quest: &QuestDefinition,
    lost: &crate::content::QuestLostItemDialogue,
    item_definitions: &[ItemDefinition],
    now_unix_ms: u64,
) -> Result<Vec<(u32, u64, u64)>, QuestRuleError> {
    let inventory = player
        .inventory
        .as_ref()
        .ok_or(ItemRuleError::MissingInventory)?;
    let mut grants = Vec::new();
    let mut has_eligible_item = false;
    for item in &lost.items {
        if !restoration_item_is_eligible(player, quest, item, item_definitions, now_unix_ms)? {
            continue;
        }
        has_eligible_item = true;
        let current = quest_item_quantity(inventory, item_definitions, item.item_id)?;
        let missing = item.target_count.saturating_sub(current);
        if missing == 0 {
            continue;
        }
        let expires_at_unix_ms = resolve_item_expiration(quest.id, item.expiration, now_unix_ms)?;
        if expires_at_unix_ms != 0 && expires_at_unix_ms <= now_unix_ms {
            continue;
        }
        grants.push((item.item_id, missing, expires_at_unix_ms));
    }
    if !has_eligible_item {
        return Err(QuestRuleError::LostItemRestorationUnavailable { quest_id: quest.id });
    }
    Ok(grants)
}

fn restoration_item_is_eligible(
    player: &PlayerState,
    quest: &QuestDefinition,
    item: &crate::content::QuestRestorableItem,
    item_definitions: &[ItemDefinition],
    now_unix_ms: u64,
) -> Result<bool, QuestRuleError> {
    let eligibility = item.eligibility;
    if progress(player, quest.id) != required_progress(eligibility.owner_state) {
        return Ok(false);
    }
    if eligibility.owner_state == RequiredQuestState::Started {
        let Some(owner) = player
            .quests
            .iter()
            .find(|entry| entry.quest_id == quest.id)
        else {
            return Ok(false);
        };
        if quest_is_expired(owner, quest, now_unix_ms) {
            return Ok(false);
        }
    }
    if !requirements_have_quest_states(player, eligibility.required_quests)
        || eligibility.forbidden_quests.iter().any(|requirement| {
            progress(player, requirement.quest_id) == required_progress(requirement.state)
        })
        || eligibility.absent_skill_ids.iter().any(|skill_id| {
            player.learned_skills.iter().any(|skill| {
                skill.skill_id == *skill_id && (skill.level > 0 || skill.master_level > 0)
            })
        })
    {
        return Ok(false);
    }
    let inventory = player
        .inventory
        .as_ref()
        .ok_or(ItemRuleError::MissingInventory)?;
    for absent_item_id in eligibility
        .absent_item_ids
        .iter()
        .copied()
        .filter(|absent_item_id| *absent_item_id != item.item_id)
    {
        if quest_item_quantity(inventory, item_definitions, absent_item_id)? != 0 {
            return Ok(false);
        }
    }
    Ok(true)
}

fn selectable_reward_choice_id(index: usize) -> Option<u32> {
    SELECTABLE_REWARD_CHOICE_OFFSET.checked_add(u32::try_from(index).ok()?)
}

pub fn select_choice(
    player: PlayerState,
    effects: &mut PlayerEffects,
    quest: &QuestDefinition,
    quest_definitions: &[&QuestDefinition],
    npc_id: u32,
    choice_id: u32,
    curve: &ExperienceCurve,
    item_definitions: &[ItemDefinition],
    consume_effects: &[ConsumeEffectDefinition],
    scripts: &QuestScriptCatalog,
    environment: QuestEnvironment,
) -> Result<QuestSelection, QuestRuleError> {
    if choice_id == RESTORE_ITEMS_CHOICE_ID
        && progress(&player, quest.id) == QuestProgress::Completed
        && quest.dialogue.completion.lost.as_ref().is_some_and(|lost| {
            lost.items
                .iter()
                .any(|item| item.eligibility.owner_state == RequiredQuestState::Completed)
        })
    {
        require_npc(quest.completion.npc_id, npc_id)?;
        return restore_lost_quest_items(player, quest, item_definitions, environment.now_unix_ms);
    }
    match progress(&player, quest.id) {
        QuestProgress::Started => select_active_choice(
            player,
            effects,
            quest,
            quest_definitions,
            npc_id,
            choice_id,
            curve,
            item_definitions,
            consume_effects,
            scripts,
            environment,
        ),
        QuestProgress::NotStarted => select_offer_choice(
            player,
            effects,
            quest,
            npc_id,
            choice_id,
            curve,
            item_definitions,
            consume_effects,
            scripts,
            environment,
        ),
        QuestProgress::Completed => {
            if quest.dialogue.start_question.is_none()
                && !is_available(
                    &player,
                    effects,
                    quest,
                    item_definitions,
                    scripts,
                    environment,
                )
            {
                return Err(QuestRuleError::Unavailable { quest_id: quest.id });
            }
            select_offer_choice(
                player,
                effects,
                quest,
                npc_id,
                choice_id,
                curve,
                item_definitions,
                consume_effects,
                scripts,
                environment,
            )
        }
    }
}

fn select_offer_choice(
    player: PlayerState,
    effects: &mut PlayerEffects,
    quest: &QuestDefinition,
    npc_id: u32,
    choice_id: u32,
    curve: &ExperienceCurve,
    item_definitions: &[ItemDefinition],
    consume_effects: &[ConsumeEffectDefinition],
    scripts: &QuestScriptCatalog,
    environment: QuestEnvironment,
) -> Result<QuestSelection, QuestRuleError> {
    require_npc(quest.start.npc_id, npc_id)?;
    if let Some(question) = &quest.dialogue.start_question {
        if has_pending_start_decision(&player, quest) {
            return match choice_id {
                ACCEPT_CHOICE_ID => start_quest(
                    player,
                    effects,
                    quest,
                    false,
                    curve,
                    item_definitions,
                    consume_effects,
                    scripts,
                    environment,
                ),
                DECLINE_CHOICE_ID => Ok(decline_start_decision(player, quest)),
                _ => Err(QuestRuleError::InvalidChoice { choice_id }),
            };
        }
        if !is_available(
            &player,
            effects,
            quest,
            item_definitions,
            scripts,
            environment,
        ) {
            return Err(QuestRuleError::Unavailable { quest_id: quest.id });
        }
        let step_index = player
            .quests
            .iter()
            .find(|entry| entry.quest_id == quest.id)
            .filter(|entry| QuestStatus::try_from(entry.status) == Ok(QuestStatus::Unspecified))
            .and_then(|entry| question_step_index(entry, question))
            .ok_or(QuestRuleError::InvalidChoice { choice_id })?;
        let step = &question.steps[step_index];
        let answer_id = question_answer_id(QuestQuestionPhase::Start, step_index, step, choice_id)?;
        if answer_id != step.correct_choice_id {
            return Ok(QuestSelection {
                player,
                pages: step
                    .failure_pages
                    .get(&answer_id)
                    .cloned()
                    .unwrap_or_default(),
                changed: false,
                npc_animation_action: None,
                next_interaction: None,
                next_quest_id: None,
            });
        }
        if step_index + 1 < question.steps.len() {
            let mut player = player;
            let entry = player
                .quests
                .iter_mut()
                .find(|entry| entry.quest_id == quest.id)
                .expect("validated pending start question");
            entry.dialogue_step = question_dialogue_step(step_index + 1)
                .ok_or(QuestRuleError::QuestionStateOverflow { quest_id: quest.id })?;
            return Ok(QuestSelection {
                player,
                pages: step.continuation_pages.clone(),
                changed: true,
                npc_animation_action: None,
                next_interaction: Some(QuestNextInteraction::Question {
                    phase: QuestQuestionPhase::Start,
                    step_index: step_index + 1,
                }),
                next_quest_id: None,
            });
        }
        if quest.dialogue.has_start_decision {
            let mut player = player;
            let entry = player
                .quests
                .iter_mut()
                .find(|entry| entry.quest_id == quest.id)
                .expect("validated pending start question");
            entry.dialogue_step = pending_start_decision_step(question)
                .ok_or(QuestRuleError::QuestionStateOverflow { quest_id: quest.id })?;
            return Ok(QuestSelection {
                player,
                pages: question.trailing_pages.clone(),
                changed: true,
                npc_animation_action: None,
                next_interaction: Some(QuestNextInteraction::StartDecision),
                next_quest_id: None,
            });
        }
        return start_quest(
            player,
            effects,
            quest,
            true,
            curve,
            item_definitions,
            consume_effects,
            scripts,
            environment,
        );
    }
    if choice_id == DECLINE_CHOICE_ID {
        return Ok(QuestSelection {
            player,
            pages: quest.dialogue.declined_pages.clone(),
            changed: false,
            npc_animation_action: None,
            next_interaction: None,
            next_quest_id: None,
        });
    }
    if choice_id != ACCEPT_CHOICE_ID {
        return Err(QuestRuleError::InvalidChoice { choice_id });
    }
    start_quest(
        player,
        effects,
        quest,
        false,
        curve,
        item_definitions,
        consume_effects,
        scripts,
        environment,
    )
}

fn start_quest(
    player: PlayerState,
    effects: &mut PlayerEffects,
    quest: &QuestDefinition,
    include_question_trailing_pages: bool,
    curve: &ExperienceCurve,
    item_definitions: &[ItemDefinition],
    consume_effects: &[ConsumeEffectDefinition],
    scripts: &QuestScriptCatalog,
    environment: QuestEnvironment,
) -> Result<QuestSelection, QuestRuleError> {
    let script_plan = resolve_action_plan(
        scripts,
        &player,
        quest,
        QuestScriptPhase::Start,
        item_definitions,
    )?;
    if !is_available(
        &player,
        effects,
        quest,
        item_definitions,
        scripts,
        environment,
    ) {
        return Err(QuestRuleError::Unavailable { quest_id: quest.id });
    }
    let actions = merge_actions(quest.id, &quest.start_actions, script_plan.as_ref())?;
    let mut player = player;
    crate::quest_records::clear(&mut player, quest.id);
    let mut player = apply_actions(
        player,
        quest.id,
        environment.now_unix_ms,
        environment.now_unix_ms,
        &actions,
        None,
        curve,
        item_definitions,
        consume_effects,
        effects,
    )?;
    let entry = PlayerQuest {
        quest_id: quest.id,
        status: QuestStatus::Started as i32,
        mob_progress: Vec::new(),
        accepted_at_unix_ms: environment.now_unix_ms,
        completed_at_unix_ms: 0,
        dialogue_step: 0,
        completion_quiz_passed: false,
    };
    if let Some(existing) = player
        .quests
        .iter_mut()
        .find(|existing| existing.quest_id == quest.id)
    {
        *existing = entry;
    } else {
        player.quests.push(entry);
        player.quests.sort_by_key(|entry| entry.quest_id);
    }
    let mut pages = if include_question_trailing_pages {
        quest
            .dialogue
            .start_question
            .as_ref()
            .map(|question| question.trailing_pages.clone())
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    pages.extend(quest.dialogue.accepted_pages.iter().cloned());
    if let Some(plan) = script_plan {
        pages.extend(plan.result_pages);
    }
    Ok(QuestSelection {
        player,
        pages,
        changed: true,
        npc_animation_action: actions.npc_animation_action.clone(),
        next_interaction: None,
        next_quest_id: None,
    })
}

fn select_active_choice(
    player: PlayerState,
    effects: &mut PlayerEffects,
    quest: &QuestDefinition,
    quest_definitions: &[&QuestDefinition],
    npc_id: u32,
    choice_id: u32,
    curve: &ExperienceCurve,
    item_definitions: &[ItemDefinition],
    consume_effects: &[ConsumeEffectDefinition],
    scripts: &QuestScriptCatalog,
    environment: QuestEnvironment,
) -> Result<QuestSelection, QuestRuleError> {
    require_npc(quest.completion.npc_id, npc_id)?;
    if choice_id == RESTORE_ITEMS_CHOICE_ID {
        return restore_lost_quest_items(player, quest, item_definitions, environment.now_unix_ms);
    }
    if let Some(question) = &quest.dialogue.question {
        require_completion_ready(
            &player,
            effects,
            quest,
            quest_definitions,
            item_definitions,
            scripts,
            environment,
        )?;
        let entry = player
            .quests
            .iter()
            .find(|entry| entry.quest_id == quest.id)
            .filter(|entry| QuestStatus::try_from(entry.status) == Ok(QuestStatus::Started))
            .ok_or(QuestRuleError::NotActive { quest_id: quest.id })?;
        if entry.completion_quiz_passed {
            let selected_reward = selected_reward_for_choice(&player, quest, choice_id)?;
            return complete_quest(
                player,
                effects,
                quest,
                CompletionChecks::Normal,
                Some(selected_reward),
                curve,
                item_definitions,
                consume_effects,
                scripts,
                quest_definitions,
                environment,
            );
        }
        let step_index = question_step_index(entry, question)
            .ok_or(QuestRuleError::InvalidChoice { choice_id })?;
        let step = &question.steps[step_index];
        let answer_id =
            question_answer_id(QuestQuestionPhase::Completion, step_index, step, choice_id)?;
        if answer_id != step.correct_choice_id {
            return Ok(QuestSelection {
                player,
                pages: step
                    .failure_pages
                    .get(&answer_id)
                    .cloned()
                    .unwrap_or_default(),
                changed: false,
                npc_animation_action: None,
                next_interaction: None,
                next_quest_id: None,
            });
        }
        if step_index + 1 < question.steps.len() {
            let mut player = player;
            let entry = player
                .quests
                .iter_mut()
                .find(|entry| entry.quest_id == quest.id)
                .expect("validated pending completion question");
            entry.dialogue_step = question_dialogue_step(step_index + 1)
                .ok_or(QuestRuleError::QuestionStateOverflow { quest_id: quest.id })?;
            return Ok(QuestSelection {
                player,
                pages: step.continuation_pages.clone(),
                changed: true,
                npc_animation_action: None,
                next_interaction: Some(QuestNextInteraction::Question {
                    phase: QuestQuestionPhase::Completion,
                    step_index: step_index + 1,
                }),
                next_quest_id: None,
            });
        }
        if !quest.completion_actions.selectable_items.is_empty() {
            if eligible_selectable_reward_choices(&player, quest).is_empty() {
                return Err(QuestRuleError::NoEligibleSelectableReward { quest_id: quest.id });
            }
            let mut player = player;
            let entry = player
                .quests
                .iter_mut()
                .find(|entry| entry.quest_id == quest.id)
                .expect("validated pending completion question");
            entry.dialogue_step = 0;
            entry.completion_quiz_passed = true;
            return Ok(QuestSelection {
                player,
                pages: question.trailing_pages.clone(),
                changed: true,
                npc_animation_action: None,
                next_interaction: Some(QuestNextInteraction::SelectableReward),
                next_quest_id: None,
            });
        }
        return complete_quest(
            player,
            effects,
            quest,
            CompletionChecks::Normal,
            None,
            curve,
            item_definitions,
            consume_effects,
            scripts,
            quest_definitions,
            environment,
        );
    }
    if quest.completion_actions.selectable_items.is_empty() {
        if choice_id != COMPLETE_CHOICE_ID {
            return Err(QuestRuleError::InvalidChoice { choice_id });
        }
        return complete_quest(
            player,
            effects,
            quest,
            CompletionChecks::Normal,
            None,
            curve,
            item_definitions,
            consume_effects,
            scripts,
            quest_definitions,
            environment,
        );
    }
    let selected_reward = selected_reward_for_choice(&player, quest, choice_id)?;
    complete_quest(
        player,
        effects,
        quest,
        CompletionChecks::Normal,
        Some(selected_reward),
        curve,
        item_definitions,
        consume_effects,
        scripts,
        quest_definitions,
        environment,
    )
}

fn question_answer_id(
    phase: QuestQuestionPhase,
    step_index: usize,
    question: &QuestQuestionStep,
    choice_id: u32,
) -> Result<u32, QuestRuleError> {
    question
        .choices
        .iter()
        .enumerate()
        .find(|(choice_index, _)| {
            answer_choice_id(phase, step_index, *choice_index) == Some(choice_id)
        })
        .map(|(_, choice)| choice.id)
        .ok_or(QuestRuleError::InvalidChoice { choice_id })
}

fn question_dialogue_step(step_index: usize) -> Option<u32> {
    u32::try_from(step_index).ok()?.checked_add(1)
}

fn pending_start_decision_step(question: &QuestQuestionSequence) -> Option<u32> {
    question_dialogue_step(question.steps.len())
}

fn start_decision_entry_is_pending(
    entry: &PlayerQuest,
    quest: &QuestDefinition,
    question: &QuestQuestionSequence,
) -> bool {
    quest.dialogue.has_start_decision
        && pending_start_decision_step(question) == Some(entry.dialogue_step)
}

pub(crate) fn has_pending_start_decision(
    player: &PlayerState,
    quest: &QuestDefinition,
) -> bool {
    let Some(question) = &quest.dialogue.start_question else {
        return false;
    };
    player
        .quests
        .iter()
        .find(|entry| entry.quest_id == quest.id)
        .filter(|entry| QuestStatus::try_from(entry.status) == Ok(QuestStatus::Unspecified))
        .is_some_and(|entry| start_decision_entry_is_pending(entry, quest, question))
}

fn decline_start_decision(
    mut player: PlayerState,
    quest: &QuestDefinition,
) -> QuestSelection {
    let pending = player
        .quests
        .iter()
        .position(|entry| entry.quest_id == quest.id)
        .expect("validated pending start decision");
    if player.quests[pending].completed_at_unix_ms > 0 {
        let entry = &mut player.quests[pending];
        entry.status = QuestStatus::Completed as i32;
        entry.dialogue_step = 0;
        entry.completion_quiz_passed = false;
    } else {
        player.quests.remove(pending);
    }
    QuestSelection {
        player,
        pages: quest.dialogue.declined_pages.clone(),
        changed: true,
        npc_animation_action: None,
        next_interaction: None,
        next_quest_id: None,
    }
}

fn question_step_index(
    entry: &PlayerQuest,
    question: &QuestQuestionSequence,
) -> Option<usize> {
    entry
        .dialogue_step
        .checked_sub(1)
        .and_then(|index| usize::try_from(index).ok())
        .filter(|index| *index < question.steps.len())
}

fn require_completion_ready(
    player: &PlayerState,
    effects: &PlayerEffects,
    quest: &QuestDefinition,
    quest_definitions: &[&QuestDefinition],
    item_definitions: &[ItemDefinition],
    scripts: &QuestScriptCatalog,
    environment: QuestEnvironment,
) -> Result<(), QuestRuleError> {
    let entry = player
        .quests
        .iter()
        .find(|entry| entry.quest_id == quest.id)
        .filter(|entry| QuestStatus::try_from(entry.status) == Ok(QuestStatus::Started))
        .ok_or(QuestRuleError::NotActive { quest_id: quest.id })?;
    if quest_is_expired(entry, quest, environment.now_unix_ms) {
        return Err(QuestRuleError::Expired { quest_id: quest.id });
    }
    if !completion_readiness(
        player,
        effects,
        quest,
        quest_definitions,
        item_definitions,
        scripts,
        environment,
    )
    .ready
    {
        return Err(QuestRuleError::ObjectivesIncomplete { quest_id: quest.id });
    }
    Ok(())
}

pub(crate) fn restore_lost_quest_items(
    mut player: PlayerState,
    quest: &QuestDefinition,
    item_definitions: &[ItemDefinition],
    now_unix_ms: u64,
) -> Result<QuestSelection, QuestRuleError> {
    let lost = quest
        .dialogue
        .completion
        .lost
        .as_ref()
        .filter(|lost| !lost.prompt_pages.is_empty() && !lost.items.is_empty());
    let lost = lost.ok_or(QuestRuleError::LostItemRestorationUnavailable { quest_id: quest.id })?;
    let inventory = player
        .inventory
        .as_ref()
        .ok_or(ItemRuleError::MissingInventory)?;
    let grants = missing_restoration_grants(&player, quest, lost, item_definitions, now_unix_ms)?;
    if grants.is_empty() {
        return Err(QuestRuleError::NoMissingRestorableItems { quest_id: quest.id });
    }

    let mut restored_inventory = inventory.clone();
    for (item_id, quantity, expires_at_unix_ms) in grants {
        crate::items::apply_item_grant(
            &mut restored_inventory,
            item_definitions,
            item_id,
            quantity,
            expires_at_unix_ms,
        )?;
    }
    player.inventory = Some(restored_inventory);
    Ok(QuestSelection {
        player,
        pages: lost.success_pages.clone(),
        changed: true,
        npc_animation_action: None,
        next_interaction: None,
        next_quest_id: None,
    })
}

fn selected_reward_for_choice(
    player: &PlayerState,
    quest: &QuestDefinition,
    choice_id: u32,
) -> Result<QuestSelectableItemReward, QuestRuleError> {
    let index = choice_id
        .checked_sub(SELECTABLE_REWARD_CHOICE_OFFSET)
        .and_then(|index| usize::try_from(index).ok())
        .ok_or(QuestRuleError::InvalidChoice { choice_id })?;
    let reward = quest
        .completion_actions
        .selectable_items
        .get(index)
        .copied()
        .ok_or(QuestRuleError::InvalidChoice { choice_id })?;
    if !reward_is_eligible(player, reward.eligibility) {
        return Err(QuestRuleError::InvalidChoice { choice_id });
    }
    Ok(reward)
}

fn complete_quest(
    player: PlayerState,
    effects: &mut PlayerEffects,
    quest: &QuestDefinition,
    checks: CompletionChecks,
    selected_reward: Option<QuestSelectableItemReward>,
    curve: &ExperienceCurve,
    item_definitions: &[ItemDefinition],
    consume_effects: &[ConsumeEffectDefinition],
    scripts: &QuestScriptCatalog,
    quest_definitions: &[&QuestDefinition],
    environment: QuestEnvironment,
) -> Result<QuestSelection, QuestRuleError> {
    let active = player
        .quests
        .iter()
        .find(|entry| entry.quest_id == quest.id)
        .filter(|entry| QuestStatus::try_from(entry.status) == Ok(QuestStatus::Started))
        .ok_or(QuestRuleError::NotActive { quest_id: quest.id })?;
    if quest_is_expired(active, quest, environment.now_unix_ms) {
        return Err(QuestRuleError::Expired { quest_id: quest.id });
    }
    if !completion_window_allows(quest, environment.now_unix_ms) {
        return Err(QuestRuleError::ObjectivesIncomplete { quest_id: quest.id });
    }
    if !requirements_have_equipped_items(&player, &quest.completion.equipped_items) {
        return Err(QuestRuleError::ObjectivesIncomplete { quest_id: quest.id });
    }
    if !requirements_have_monster_book(&player, &quest.completion.monster_book) {
        return Err(QuestRuleError::ObjectivesIncomplete { quest_id: quest.id });
    }
    if selected_reward.is_none() && !quest.completion_actions.selectable_items.is_empty() {
        return Err(
            if eligible_selectable_reward_choices(&player, quest).is_empty() {
                QuestRuleError::NoEligibleSelectableReward { quest_id: quest.id }
            } else {
                QuestRuleError::RewardSelectionRequired { quest_id: quest.id }
            },
        );
    }
    let accepted_at_unix_ms = active.accepted_at_unix_ms;
    let script_plan = resolve_action_plan(
        scripts,
        &player,
        quest,
        QuestScriptPhase::Completion,
        item_definitions,
    )?;
    if checks == CompletionChecks::Normal
        && !completion_readiness(
            &player,
            effects,
            quest,
            quest_definitions,
            item_definitions,
            scripts,
            environment,
        )
        .ready
    {
        return Err(QuestRuleError::ObjectivesIncomplete { quest_id: quest.id });
    }
    let actions = merge_actions(quest.id, &quest.completion_actions, script_plan.as_ref())?;
    let mut player = apply_actions(
        player,
        quest.id,
        accepted_at_unix_ms,
        environment.now_unix_ms,
        &actions,
        selected_reward,
        curve,
        item_definitions,
        consume_effects,
        effects,
    )?;
    let entry = player
        .quests
        .iter_mut()
        .find(|entry| entry.quest_id == quest.id)
        .ok_or(QuestRuleError::NotActive { quest_id: quest.id })?;
    entry.status = QuestStatus::Completed as i32;
    entry.completed_at_unix_ms = environment.now_unix_ms;
    entry.dialogue_step = 0;
    let quiz_was_passed = entry.completion_quiz_passed;
    entry.completion_quiz_passed = false;
    let mut pages = Vec::new();
    if !quiz_was_passed && let Some(question) = &quest.dialogue.question {
        pages.extend(question.trailing_pages.iter().cloned());
    }
    pages.extend(quest.dialogue.completion.success_pages.iter().cloned());
    if let Some(plan) = script_plan {
        pages.extend(plan.result_pages);
    }
    Ok(QuestSelection {
        player,
        pages,
        changed: true,
        npc_animation_action: actions.npc_animation_action.clone(),
        next_interaction: None,
        next_quest_id: quest.completion_actions.next_quest_id,
    })
}

fn resolve_action_plan(
    scripts: &QuestScriptCatalog,
    player: &PlayerState,
    quest: &QuestDefinition,
    phase: QuestScriptPhase,
    item_definitions: &[ItemDefinition],
) -> Result<Option<QuestScriptPlan>, QuestRuleError> {
    match resolve_quest_script(scripts, quest, phase, player, item_definitions) {
        QuestScriptResolution::NotReferenced => Ok(None),
        QuestScriptResolution::Ready(plan) => Ok(Some(plan)),
        QuestScriptResolution::Missing { script } => Err(QuestRuleError::ScriptRequired {
            quest_id: quest.id,
            phase,
            script,
        }),
        QuestScriptResolution::ConditionsNotMet { .. } => Err(match phase {
            QuestScriptPhase::Start => QuestRuleError::Unavailable { quest_id: quest.id },
            QuestScriptPhase::Completion => {
                QuestRuleError::ObjectivesIncomplete { quest_id: quest.id }
            }
        }),
    }
}

fn script_conditions_pass(
    scripts: &QuestScriptCatalog,
    player: &PlayerState,
    quest: &QuestDefinition,
    phase: QuestScriptPhase,
    item_definitions: &[ItemDefinition],
) -> bool {
    matches!(
        resolve_quest_script(scripts, quest, phase, player, item_definitions),
        QuestScriptResolution::NotReferenced | QuestScriptResolution::Ready(_)
    )
}

fn merge_actions(
    quest_id: u32,
    wz: &QuestActions,
    script: Option<&QuestScriptPlan>,
) -> Result<QuestActions, QuestRuleError> {
    let Some(script) = script else {
        return Ok(wz.clone());
    };
    let mut actions = wz.clone();
    actions.fixed_items.extend_from_slice(&script.item_deltas);
    actions.money = actions
        .money
        .checked_add(script.mesos)
        .ok_or(QuestRuleError::ActionOverflow { quest_id })?;
    actions.experience = actions
        .experience
        .checked_add(script.experience)
        .ok_or(QuestRuleError::ActionOverflow { quest_id })?;
    actions.fame = actions
        .fame
        .checked_add(script.fame)
        .ok_or(QuestRuleError::ActionOverflow { quest_id })?;
    for write in &script.record_writes {
        if actions
            .record_writes
            .iter()
            .any(|existing| existing.quest_id == write.quest_id && existing.index == write.index)
        {
            return Err(QuestRuleError::ActionOverflow { quest_id });
        }
        actions.record_writes.push(write.clone());
    }
    for action in &script.quest_state_actions {
        if actions
            .quest_state_actions
            .iter()
            .any(|existing| existing.quest_id == action.quest_id)
        {
            return Err(QuestRuleError::ActionOverflow { quest_id });
        }
        actions.quest_state_actions.push(*action);
    }
    actions
        .record_writes
        .sort_by_key(|write| (write.quest_id, write.index));
    Ok(actions)
}

fn apply_actions(
    mut player: PlayerState,
    quest_id: u32,
    selection_seed_unix_ms: u64,
    now_unix_ms: u64,
    actions: &QuestActions,
    selected_reward: Option<QuestSelectableItemReward>,
    curve: &ExperienceCurve,
    item_definitions: &[ItemDefinition],
    consume_effects: &[ConsumeEffectDefinition],
    effects: &mut PlayerEffects,
) -> Result<PlayerState, QuestRuleError> {
    let action_effects = actions
        .buff_item_ids
        .iter()
        .map(|item_id| {
            consume_effects
                .iter()
                .find(|definition| definition.item_id == *item_id)
                .copied()
                .ok_or(QuestRuleError::MissingConsumeEffect {
                    quest_id,
                    item_id: *item_id,
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    apply_skill_changes(&mut player, &actions.skill_changes);
    apply_quest_state_actions(&mut player, &actions.quest_state_actions, now_unix_ms);
    let eligible_weighted = actions
        .weighted_items
        .iter()
        .copied()
        .filter(|reward| reward_is_eligible(&player, reward.eligibility))
        .collect::<Vec<_>>();
    let weighted = select_weighted_item(
        &player.id,
        quest_id,
        selection_seed_unix_ms,
        &eligible_weighted,
    );
    let mut removals = BTreeMap::<u32, u64>::new();
    let mut grants = BTreeMap::<(u32, u64), u64>::new();
    for delta in actions
        .fixed_items
        .iter()
        .copied()
        .chain(
            actions
                .conditional_items
                .iter()
                .filter(|reward| reward_is_eligible(&player, reward.eligibility))
                .map(|reward| crate::content::QuestItemDelta {
                    item_id: reward.item_id,
                    count: i64::from(reward.count),
                    expiration: reward.expiration,
                }),
        )
        .chain(weighted.map(|reward| crate::content::QuestItemDelta {
            item_id: reward.item_id,
            count: i64::from(reward.count),
            expiration: reward.expiration,
        }))
        .chain(
            selected_reward.map(|reward| crate::content::QuestItemDelta {
                item_id: reward.item_id,
                count: i64::from(reward.count),
                expiration: reward.expiration,
            }),
        )
    {
        if delta.count < 0 && delta.expiration.is_some() {
            return Err(QuestRuleError::ExpiringRemoval {
                quest_id,
                item_id: delta.item_id,
            });
        }
        let quantity = delta.count.unsigned_abs();
        let total = if delta.count < 0 {
            removals.entry(delta.item_id).or_default()
        } else {
            let expires_at_unix_ms =
                resolve_item_expiration(quest_id, delta.expiration, now_unix_ms)?;
            if expires_at_unix_ms != 0 && expires_at_unix_ms <= now_unix_ms {
                continue;
            }
            grants
                .entry((delta.item_id, expires_at_unix_ms))
                .or_default()
        };
        *total = total
            .checked_add(quantity)
            .ok_or(ItemRuleError::QuantityOverflow {
                item_id: delta.item_id,
            })?;
    }
    if !removals.is_empty() || !grants.is_empty() {
        let inventory = player
            .inventory
            .as_mut()
            .ok_or(ItemRuleError::MissingInventory)?;
        for (item_id, quantity) in removals {
            let quantity =
                i64::try_from(quantity).map_err(|_| ItemRuleError::QuantityOverflow { item_id })?;
            crate::items::apply_item_delta(inventory, item_definitions, item_id, -quantity)?;
        }
        for ((item_id, expires_at_unix_ms), quantity) in grants {
            crate::items::apply_item_grant(
                inventory,
                item_definitions,
                item_id,
                quantity,
                expires_at_unix_ms,
            )?;
        }
    }
    player.mesos = apply_signed_u64(player.mesos, actions.money)?;
    if actions.fame != 0 {
        let player_id = player.id.clone();
        let stats = player
            .stats
            .as_mut()
            .ok_or_else(|| QuestRuleError::MissingStats {
                player_id: player_id.clone(),
            })?;
        stats.fame = stats
            .fame
            .checked_add(actions.fame)
            .ok_or(QuestRuleError::FameOverflow { player_id })?;
    }
    if actions.experience != 0 {
        player = crate::experience::grant_experience(player, actions.experience, curve)?;
    }
    for write in &actions.record_writes {
        crate::quest_records::set(
            &mut player,
            write.quest_id,
            write.index,
            write.value.clone(),
        )?;
    }
    let mut staged_effects = effects.clone();
    for definition in action_effects {
        player = crate::effects::apply_consume_effect(
            player,
            &mut staged_effects,
            definition,
            now_unix_ms,
        );
    }
    *effects = staged_effects;
    Ok(player)
}

fn apply_quest_state_actions(
    player: &mut PlayerState,
    actions: &[QuestStateAction],
    now_unix_ms: u64,
) {
    for action in actions {
        let accepted_at_unix_ms = player
            .quests
            .iter()
            .find(|entry| entry.quest_id == action.quest_id)
            .filter(|entry| {
                matches!(
                    QuestStatus::try_from(entry.status),
                    Ok(QuestStatus::Started | QuestStatus::Completed)
                ) && entry.accepted_at_unix_ms > 0
                    && entry.accepted_at_unix_ms <= now_unix_ms
            })
            .map_or(now_unix_ms, |entry| entry.accepted_at_unix_ms);
        player
            .quests
            .retain(|entry| entry.quest_id != action.quest_id);
        match action.state {
            QuestStateActionState::NotStarted => {
                crate::quest_records::clear(player, action.quest_id);
            }
            QuestStateActionState::Started => player.quests.push(PlayerQuest {
                quest_id: action.quest_id,
                status: QuestStatus::Started as i32,
                mob_progress: Vec::new(),
                accepted_at_unix_ms: now_unix_ms,
                completed_at_unix_ms: 0,
                dialogue_step: 0,
                completion_quiz_passed: false,
            }),
            QuestStateActionState::Completed => player.quests.push(PlayerQuest {
                quest_id: action.quest_id,
                status: QuestStatus::Completed as i32,
                mob_progress: Vec::new(),
                accepted_at_unix_ms,
                completed_at_unix_ms: now_unix_ms,
                dialogue_step: 0,
                completion_quiz_passed: false,
            }),
        }
    }
    player.quests.sort_by_key(|entry| entry.quest_id);
}

fn apply_skill_changes(
    player: &mut PlayerState,
    changes: &[QuestSkillChange],
) {
    let job_id = player.stats.as_ref().map_or(0, |stats| stats.job_id);
    for change in changes {
        if !change.job_ids.contains(&job_id) && change.skill_id % 10_000_000 >= 10_000 {
            continue;
        }
        match change.operation {
            QuestSkillOperation::Grant {
                skill_level,
                master_level,
            } => {
                if let Some(learned) = player
                    .learned_skills
                    .iter_mut()
                    .find(|learned| learned.skill_id == change.skill_id)
                {
                    learned.level = learned.level.max(skill_level);
                    learned.master_level = learned.master_level.max(master_level);
                } else if skill_level > 0 || master_level > 0 {
                    player.learned_skills.push(LearnedSkill {
                        skill_id: change.skill_id,
                        level: skill_level,
                        master_level,
                    });
                }
            }
            QuestSkillOperation::Remove => {
                player
                    .learned_skills
                    .retain(|learned| learned.skill_id != change.skill_id);
                player
                    .key_bindings
                    .retain(|binding| binding.skill_id != change.skill_id);
            }
        }
    }
    player
        .learned_skills
        .sort_by_key(|learned| learned.skill_id);
}

fn resolve_item_expiration(
    quest_id: u32,
    expiration: Option<QuestItemExpiration>,
    now_unix_ms: u64,
) -> Result<u64, QuestRuleError> {
    match expiration {
        None => Ok(0),
        Some(QuestItemExpiration::RelativeMilliseconds(duration_ms)) => now_unix_ms
            .checked_add(duration_ms)
            .ok_or(QuestRuleError::ActionOverflow { quest_id }),
        Some(QuestItemExpiration::AbsoluteUnixMilliseconds(deadline)) => Ok(deadline),
    }
}

pub fn select_weighted_item(
    player_id: &str,
    quest_id: u32,
    accepted_at_unix_ms: u64,
    rewards: &[QuestWeightedItem],
) -> Option<QuestWeightedItem> {
    let total = rewards.iter().fold(0_u64, |total, reward| {
        total.saturating_add(u64::from(reward.weight))
    });
    if total == 0 {
        return None;
    }
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in player_id
        .bytes()
        .chain(quest_id.to_le_bytes())
        .chain(accepted_at_unix_ms.to_le_bytes())
    {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    let mut ticket = hash % total;
    rewards.iter().copied().find(|reward| {
        if ticket < u64::from(reward.weight) {
            true
        } else {
            ticket -= u64::from(reward.weight);
            false
        }
    })
}

fn apply_signed_u64(
    value: u64,
    delta: i64,
) -> Result<u64, ItemRuleError> {
    if delta >= 0 {
        value
            .checked_add(delta.unsigned_abs())
            .ok_or(ItemRuleError::MesosOverflow)
    } else {
        value
            .checked_sub(delta.unsigned_abs())
            .ok_or(ItemRuleError::InsufficientMesos)
    }
}

fn repeat_policy_allows(
    player: &PlayerState,
    quest: &QuestDefinition,
    now_unix_ms: u64,
) -> bool {
    if let Some(entry) = player.quests.iter().find(|entry| {
        entry.quest_id == quest.id
            && QuestStatus::try_from(entry.status) == Ok(QuestStatus::Unspecified)
            && entry.dialogue_step > 0
            && entry.completed_at_unix_ms > 0
    }) {
        return repeat_completion_allows(quest, now_unix_ms, entry.completed_at_unix_ms);
    }
    match progress(player, quest.id) {
        QuestProgress::NotStarted => true,
        QuestProgress::Started => false,
        QuestProgress::Completed => {
            let Some(completed_at) = player
                .quests
                .iter()
                .find(|entry| entry.quest_id == quest.id)
                .map(|entry| entry.completed_at_unix_ms)
                .filter(|completed_at| *completed_at > 0)
            else {
                return false;
            };
            repeat_completion_allows(quest, now_unix_ms, completed_at)
        }
    }
}

fn repeat_completion_allows(
    quest: &QuestDefinition,
    now_unix_ms: u64,
    completed_at: u64,
) -> bool {
    let repeat = &quest.start.repeat;
    if repeat.interval_ms.is_none() && !repeat.day_by_day {
        return false;
    }
    repeat.interval_ms.is_none_or(|interval| {
        completed_at
            .checked_add(interval)
            .is_some_and(|available_at| now_unix_ms >= available_at)
    }) && (!repeat.day_by_day
        || utc_datetime(now_unix_ms)
            .zip(utc_datetime(completed_at))
            .is_some_and(|(now, completed)| now.date() > completed.date()))
}

fn weekday_allows(
    quest: &QuestDefinition,
    now_unix_ms: u64,
) -> bool {
    let days = &quest.start.repeat.days_of_week;
    days.is_empty() || weekday(now_unix_ms).is_some_and(|weekday| days.contains(&weekday))
}

fn weekday(now_unix_ms: u64) -> Option<QuestWeekday> {
    match utc_datetime(now_unix_ms).map(|datetime| datetime.weekday()) {
        Some(Weekday::Monday) => Some(QuestWeekday::Monday),
        Some(Weekday::Tuesday) => Some(QuestWeekday::Tuesday),
        Some(Weekday::Wednesday) => Some(QuestWeekday::Wednesday),
        Some(Weekday::Thursday) => Some(QuestWeekday::Thursday),
        Some(Weekday::Friday) => Some(QuestWeekday::Friday),
        Some(Weekday::Saturday) => Some(QuestWeekday::Saturday),
        Some(Weekday::Sunday) => Some(QuestWeekday::Sunday),
        None => None,
    }
}

fn utc_datetime(unix_ms: u64) -> Option<jiff::civil::DateTime> {
    let unix_ms = i64::try_from(unix_ms).ok()?;
    let timestamp = Timestamp::from_millisecond(unix_ms).ok()?;
    Some(Offset::UTC.to_datetime(timestamp))
}

fn requirements_have_items(
    player: &PlayerState,
    requirements: &[QuestItemRequirement],
    item_definitions: &[ItemDefinition],
) -> bool {
    requirements.iter().all(|requirement| {
        player.inventory.as_ref().is_some_and(|inventory| {
            quest_item_quantity(inventory, item_definitions, requirement.item_id).is_ok_and(
                |count| match requirement.condition {
                    QuestItemCondition::Absent => count == 0,
                    QuestItemCondition::AtLeast(required) => count >= u64::from(required.get()),
                },
            )
        })
    })
}

fn requirements_have_monster_book(
    player: &PlayerState,
    requirements: &QuestMonsterBookRequirements,
) -> bool {
    let cards_match = requirements.cards.iter().all(|requirement| {
        let count =
            crate::monster_book::count(&player.monster_book_cards, requirement.card_item_id);
        requirement
            .minimum_count
            .is_none_or(|minimum| count >= minimum)
            && requirement
                .maximum_count
                .is_none_or(|maximum| count <= maximum)
    });
    let unique_count = player.monster_book_cards.len() as u64;
    cards_match
        && requirements
            .minimum_unique_cards
            .is_none_or(|minimum| unique_count >= u64::from(minimum))
        && requirements
            .maximum_unique_cards
            .is_none_or(|maximum| unique_count <= u64::from(maximum))
}

fn requirements_have_equipped_items(
    player: &PlayerState,
    requirements: &QuestEquippedItemRequirements,
) -> bool {
    if requirements.all_of.is_empty() && requirements.any_of.is_empty() {
        return true;
    }
    let Some(inventory) = player.inventory.as_ref() else {
        return false;
    };
    let is_equipped = |item_id| {
        inventory
            .equipment
            .iter()
            .any(|equipped| equipped.item_id == item_id)
    };
    requirements.all_of.iter().copied().all(is_equipped)
        && (requirements.any_of.is_empty() || requirements.any_of.iter().copied().any(is_equipped))
}

fn item_name(
    item_definitions: &[ItemDefinition],
    item_id: u32,
) -> String {
    item_definitions
        .iter()
        .find(|definition| definition.item_id == item_id)
        .map(|definition| definition.name.as_str())
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| format!("Item {item_id}"))
}

fn quest_item_quantity(
    inventory: &oozems_proto::v1::InventoryState,
    item_definitions: &[ItemDefinition],
    item_id: u32,
) -> Result<u64, ItemRuleError> {
    let inventory_count = crate::items::count_inventory_item(inventory, item_definitions, item_id)?;
    let equipped_count = inventory
        .equipment
        .iter()
        .filter(|item| item.item_id == item_id)
        .count() as u64;
    inventory_count
        .checked_add(equipped_count)
        .ok_or(ItemRuleError::QuantityOverflow { item_id })
}

fn requirements_have_quest_states(
    player: &PlayerState,
    requirements: &[QuestStateRequirement],
) -> bool {
    requirements.iter().all(|requirement| {
        progress(player, requirement.quest_id) == required_progress(requirement.state)
    })
}

fn requirements_have_skills(
    player: &PlayerState,
    requirements: &[QuestSkillRequirement],
) -> bool {
    requirements.iter().all(|requirement| {
        let acquired = player
            .learned_skills
            .iter()
            .any(|skill| skill.skill_id == requirement.skill_id && skill.level > 0);
        acquired == requirement.acquired
    })
}

fn requirements_have_records(
    player: &PlayerState,
    requirements: &[QuestRecordCondition],
) -> bool {
    requirements
        .iter()
        .all(|requirement| record_condition_matches(player, requirement))
}

fn requirements_have_effects(
    effects: &PlayerEffects,
    requirements: &[crate::content::QuestEffectRequirement],
) -> bool {
    requirements
        .iter()
        .all(|requirement| effects.contains_item(requirement.item_id) == requirement.active)
}

fn required_morph_is_active(
    effects: &PlayerEffects,
    required: Option<std::num::NonZeroU32>,
) -> bool {
    required.is_none_or(|required| effects.projected().morph_id == Some(required.get()))
}

fn record_condition_matches(
    player: &PlayerState,
    requirement: &QuestRecordCondition,
) -> bool {
    let Some(current) = crate::quest_records::get(player, requirement.quest_id, requirement.index)
    else {
        return false;
    };
    requirement
        .alternatives
        .iter()
        .any(|predicate| match predicate {
            QuestRecordPredicate::Equal(expected) => current == expected,
            QuestRecordPredicate::AtLeast(expected) => {
                crate::quest_records::strict_decimal(current)
                    .is_some_and(|current| current >= *expected)
            }
            QuestRecordPredicate::AtMost(expected) => crate::quest_records::strict_decimal(current)
                .is_some_and(|current| current <= *expected),
        })
}

fn reward_is_eligible(
    player: &PlayerState,
    eligibility: QuestRewardEligibility,
) -> bool {
    let job_matches = eligibility.job_mask.is_none_or(|required| {
        player
            .stats
            .as_ref()
            .and_then(|stats| job_family_mask(stats.job_id))
            .is_some_and(|actual| required & actual != 0)
    });
    let gender_matches = eligibility.gender.is_none_or(|required| {
        player
            .appearance
            .as_ref()
            .and_then(|appearance| CharacterGender::try_from(appearance.gender).ok())
            .is_some_and(|actual| {
                matches!(
                    (required, actual),
                    (QuestRewardGender::Male, CharacterGender::Male)
                        | (QuestRewardGender::Female, CharacterGender::Female)
                )
            })
    });
    job_matches && gender_matches
}

fn job_family_mask(job_id: u32) -> Option<u32> {
    // WZ assigns one reward-mask bit to each hundreds-level job family.
    1_u32.checked_shl(job_id / 100)
}

fn required_progress(state: RequiredQuestState) -> QuestProgress {
    match state {
        RequiredQuestState::NotStarted => QuestProgress::NotStarted,
        RequiredQuestState::Started => QuestProgress::Started,
        RequiredQuestState::Completed => QuestProgress::Completed,
    }
}

fn state_label(state: RequiredQuestState) -> &'static str {
    match state {
        RequiredQuestState::NotStarted => "not started",
        RequiredQuestState::Started => "started",
        RequiredQuestState::Completed => "completed",
    }
}

fn require_npc(
    expected: Option<u32>,
    actual: u32,
) -> Result<(), QuestRuleError> {
    if expected == Some(actual) {
        Ok(())
    } else {
        Err(QuestRuleError::WrongNpc { npc_id: actual })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::collections::HashMap;
    use std::fs;
    use std::num::NonZeroU32;
    use std::path::Path;

    use oozems_proto::v1::CharacterAppearance;
    use oozems_proto::v1::CharacterGender;
    use oozems_proto::v1::CharacterStats;
    use oozems_proto::v1::EquipmentSlot;
    use oozems_proto::v1::EquippedItem;
    use oozems_proto::v1::InventoryItemStack;
    use oozems_proto::v1::InventoryState;
    use oozems_proto::v1::ItemDefinition;
    use oozems_proto::v1::KeyAction;
    use oozems_proto::v1::KeyBinding;
    use oozems_proto::v1::LearnedSkill;
    use oozems_proto::v1::MonsterBookCard;
    use oozems_proto::v1::PlayerQuest;
    use oozems_proto::v1::PlayerState;
    use oozems_proto::v1::QuestMobProgress;
    use oozems_proto::v1::QuestStatus;

    use super::ACCEPT_CHOICE_ID;
    use super::COMPLETE_CHOICE_ID;
    use super::DECLINE_CHOICE_ID;
    use super::QuestNextInteraction;
    use super::QuestProgress;
    use super::QuestQuestionPhase;
    use super::QuestRuleError;
    use super::RESTORE_ITEMS_CHOICE_ID;
    use super::advance_automatic_quests as advance_automatic_quests_with_effects;
    use super::answer_choice_id;
    use super::begin_completion_question as begin_completion_question_with_effects;
    use super::begin_start_question as begin_start_question_with_effects;
    use super::completion_readiness as completion_readiness_with_effects;
    use super::eligible_selectable_reward_choices;
    use super::incomplete_dialogue_pages as incomplete_dialogue_pages_with_effects;
    use super::is_available as is_available_with_effects;
    use super::lost_item_restoration_needed;
    use super::progress;
    use super::record_mob_kills;
    use super::select_choice as select_choice_with_effects;
    use super::select_weighted_item;
    use super::selectable_reward_choice_id;
    use crate::content::ConsumeEffectDefinition;
    use crate::content::QuestActions;
    use crate::content::QuestCalendar;
    use crate::content::QuestChoice;
    use crate::content::QuestCompletionDialogue;
    use crate::content::QuestCompletionRequirements;
    use crate::content::QuestConditionalItemReward;
    use crate::content::QuestDefinition;
    use crate::content::QuestDialogue;
    use crate::content::QuestEffectRequirement;
    use crate::content::QuestEquippedItemRequirements;
    use crate::content::QuestInfo;
    use crate::content::QuestItemCondition;
    use crate::content::QuestItemDelta;
    use crate::content::QuestItemExpiration;
    use crate::content::QuestItemRequirement;
    use crate::content::QuestLostItemDialogue;
    use crate::content::QuestMobObjective;
    use crate::content::QuestMonsterBookCardRequirement;
    use crate::content::QuestMonsterBookRequirements;
    use crate::content::QuestQuestionSequence;
    use crate::content::QuestQuestionStep;
    use crate::content::QuestRecordCondition;
    use crate::content::QuestRecordPredicate;
    use crate::content::QuestRecordWrite;
    use crate::content::QuestRepeatMetadata;
    use crate::content::QuestRestorableItem;
    use crate::content::QuestRestorationEligibility;
    use crate::content::QuestRestorationProvenance;
    use crate::content::QuestRewardEligibility;
    use crate::content::QuestRewardGender;
    use crate::content::QuestSelectableItemReward;
    use crate::content::QuestSelectedSkill;
    use crate::content::QuestSkillChange;
    use crate::content::QuestSkillOperation;
    use crate::content::QuestSkillRequirement;
    use crate::content::QuestStartRequirements;
    use crate::content::QuestStateAction;
    use crate::content::QuestStateActionState;
    use crate::content::QuestStateRequirement;
    use crate::content::QuestWeightedItem;
    use crate::content::RequiredQuestState;
    use crate::effects::PlayerEffects;
    use crate::experience::ExperienceCurves;
    use crate::items::ItemRuleError;
    use crate::quest_scripts::QuestScriptCatalog;

    const ITEM_A: u32 = 4_000_000;
    const ITEM_B: u32 = 1_332_005;
    const ITEM_C: u32 = 1_332_007;
    const EFFECT_ITEM: u32 = 2_210_003;
    const OTHER_EFFECT_ITEM: u32 = 2_022_070;
    const MOB_A: u32 = 100_100;
    const CARD_A: u32 = 2_380_000;
    const CARD_B: u32 = 2_380_001;

    fn environment(now_unix_ms: u64) -> super::QuestEnvironment {
        super::QuestEnvironment {
            now_unix_ms,
            world_id: 0,
        }
    }

    fn is_available_in_environment(
        player: &PlayerState,
        quest: &QuestDefinition,
        item_definitions: &[ItemDefinition],
        scripts: &QuestScriptCatalog,
        environment: super::QuestEnvironment,
    ) -> bool {
        is_available_with_effects(
            player,
            &PlayerEffects::default(),
            quest,
            item_definitions,
            scripts,
            environment,
        )
    }

    fn completion_readiness_in_environment(
        player: &PlayerState,
        quest: &QuestDefinition,
        quest_definitions: &[&QuestDefinition],
        item_definitions: &[ItemDefinition],
        scripts: &QuestScriptCatalog,
        environment: super::QuestEnvironment,
    ) -> super::QuestReadiness {
        completion_readiness_with_effects(
            player,
            &PlayerEffects::default(),
            quest,
            quest_definitions,
            item_definitions,
            scripts,
            environment,
        )
    }

    fn incomplete_dialogue_pages_in_environment(
        player: &PlayerState,
        quest: &QuestDefinition,
        quest_definitions: &[&QuestDefinition],
        item_definitions: &[ItemDefinition],
        scripts: &QuestScriptCatalog,
        environment: super::QuestEnvironment,
    ) -> Vec<String> {
        incomplete_dialogue_pages_with_effects(
            player,
            &PlayerEffects::default(),
            quest,
            quest_definitions,
            item_definitions,
            scripts,
            environment,
        )
    }

    fn advance_automatic_quests_in_environment<'a>(
        player: PlayerState,
        quest_definitions: impl IntoIterator<Item = &'a QuestDefinition>,
        curve: &crate::experience::ExperienceCurve,
        item_definitions: &[ItemDefinition],
        scripts: &QuestScriptCatalog,
        environment: super::QuestEnvironment,
    ) -> super::AutomaticQuestAdvance {
        advance_automatic_quests_with_effects(
            player,
            PlayerEffects::default(),
            quest_definitions,
            curve,
            item_definitions,
            &[],
            scripts,
            environment,
        )
    }

    fn select_choice_in_environment(
        player: PlayerState,
        quest: &QuestDefinition,
        quest_definitions: &[&QuestDefinition],
        npc_id: u32,
        choice_id: u32,
        curve: &crate::experience::ExperienceCurve,
        item_definitions: &[ItemDefinition],
        scripts: &QuestScriptCatalog,
        environment: super::QuestEnvironment,
    ) -> Result<super::QuestSelection, super::QuestRuleError> {
        select_choice_with_effects(
            player,
            &mut PlayerEffects::default(),
            quest,
            quest_definitions,
            npc_id,
            choice_id,
            curve,
            item_definitions,
            &[],
            scripts,
            environment,
        )
    }

    fn begin_start_question(
        player: PlayerState,
        quest: &QuestDefinition,
        npc_id: u32,
        item_definitions: &[ItemDefinition],
        scripts: &QuestScriptCatalog,
        environment: super::QuestEnvironment,
    ) -> Result<super::QuestSelection, super::QuestRuleError> {
        begin_start_question_with_effects(
            player,
            &PlayerEffects::default(),
            quest,
            npc_id,
            item_definitions,
            scripts,
            environment,
        )
    }

    fn begin_completion_question(
        player: PlayerState,
        quest: &QuestDefinition,
        quest_definitions: &[&QuestDefinition],
        npc_id: u32,
        item_definitions: &[ItemDefinition],
        scripts: &QuestScriptCatalog,
        environment: super::QuestEnvironment,
    ) -> Result<super::QuestSelection, super::QuestRuleError> {
        begin_completion_question_with_effects(
            player,
            &PlayerEffects::default(),
            quest,
            quest_definitions,
            npc_id,
            item_definitions,
            scripts,
            environment,
        )
    }

    fn is_available(
        player: &PlayerState,
        quest: &QuestDefinition,
        item_definitions: &[ItemDefinition],
        scripts: &QuestScriptCatalog,
        now_unix_ms: u64,
    ) -> bool {
        is_available_in_environment(
            player,
            quest,
            item_definitions,
            scripts,
            environment(now_unix_ms),
        )
    }

    fn completion_readiness(
        player: &PlayerState,
        quest: &QuestDefinition,
        item_definitions: &[ItemDefinition],
        scripts: &QuestScriptCatalog,
    ) -> super::QuestReadiness {
        completion_readiness_in_environment(
            player,
            quest,
            &[quest],
            item_definitions,
            scripts,
            environment(0),
        )
    }

    fn incomplete_dialogue_pages(
        player: &PlayerState,
        quest: &QuestDefinition,
        item_definitions: &[ItemDefinition],
        scripts: &QuestScriptCatalog,
    ) -> Vec<String> {
        incomplete_dialogue_pages_in_environment(
            player,
            quest,
            &[quest],
            item_definitions,
            scripts,
            environment(0),
        )
    }

    fn advance_automatic_quests<'a>(
        player: PlayerState,
        quest_definitions: impl IntoIterator<Item = &'a QuestDefinition>,
        curve: &crate::experience::ExperienceCurve,
        item_definitions: &[ItemDefinition],
        scripts: &QuestScriptCatalog,
        now_unix_ms: u64,
    ) -> super::AutomaticQuestAdvance {
        advance_automatic_quests_in_environment(
            player,
            quest_definitions,
            curve,
            item_definitions,
            scripts,
            environment(now_unix_ms),
        )
    }

    fn select_choice(
        player: PlayerState,
        quest: &QuestDefinition,
        npc_id: u32,
        choice_id: u32,
        curve: &crate::experience::ExperienceCurve,
        item_definitions: &[ItemDefinition],
        scripts: &QuestScriptCatalog,
        now_unix_ms: u64,
    ) -> Result<super::QuestSelection, super::QuestRuleError> {
        select_choice_in_environment(
            player,
            quest,
            &[quest],
            npc_id,
            choice_id,
            curve,
            item_definitions,
            scripts,
            environment(now_unix_ms),
        )
    }

    fn open_start_question(
        player: PlayerState,
        quest: &QuestDefinition,
        item_definitions: &[ItemDefinition],
        scripts: &QuestScriptCatalog,
        now_unix_ms: u64,
    ) -> PlayerState {
        begin_start_question(
            player,
            quest,
            1,
            item_definitions,
            scripts,
            environment(now_unix_ms),
        )
        .expect("open start question")
        .player
    }

    fn open_completion_question(
        player: PlayerState,
        quest: &QuestDefinition,
        item_definitions: &[ItemDefinition],
        scripts: &QuestScriptCatalog,
        now_unix_ms: u64,
    ) -> PlayerState {
        begin_completion_question(
            player,
            quest,
            &[quest],
            2,
            item_definitions,
            scripts,
            environment(now_unix_ms),
        )
        .expect("open completion question")
        .player
    }

    #[test]
    fn availability_enforces_item_job_level_and_quest_prerequisites() {
        let definitions = item_definitions();
        let mut quest = quest(100);
        quest.start.minimum_level = Some(5);
        quest.start.maximum_level = Some(10);
        quest.start.allowed_jobs = vec![0];
        quest.start.items = vec![item_requirement(ITEM_A, 2)];
        quest.start.quests = vec![QuestStateRequirement {
            quest_id: 99,
            state: RequiredQuestState::Completed,
        }];
        let mut player = player(vec![(ITEM_A, 1)], 4);
        player.level = 5;
        player.quests.push(player_quest(99, QuestStatus::Started));

        assert!(!is_available(
            &player,
            &quest,
            &definitions,
            scripts(),
            1_000
        ));
        player.inventory.as_mut().expect("inventory").stacks[0].quantity = 2;
        assert!(!is_available(
            &player,
            &quest,
            &definitions,
            scripts(),
            1_000
        ));
        player.quests[0].status = QuestStatus::Completed as i32;
        assert!(is_available(
            &player,
            &quest,
            &definitions,
            scripts(),
            1_000
        ));
        player.level = 11;
        assert!(!is_available(
            &player,
            &quest,
            &definitions,
            scripts(),
            1_000
        ));
        player.level = 5;
        player.stats.as_mut().expect("stats").job_id = 100;
        assert!(!is_available(
            &player,
            &quest,
            &definitions,
            scripts(),
            1_000
        ));
    }

    #[test]
    fn fame_availability_is_inclusive_and_requires_character_stats() {
        let definitions = item_definitions();
        let mut quest = quest(100);
        quest.start.minimum_fame = Some(3);
        let mut player = player(Vec::new(), 1);
        player.stats.as_mut().expect("stats").fame = 2;

        assert!(!is_available(
            &player,
            &quest,
            &definitions,
            scripts(),
            1_000,
        ));
        player.stats.as_mut().expect("stats").fame = 3;
        assert!(is_available(
            &player,
            &quest,
            &definitions,
            scripts(),
            1_000,
        ));
        player.stats.as_mut().expect("stats").fame = 4;
        assert!(is_available(
            &player,
            &quest,
            &definitions,
            scripts(),
            1_000,
        ));
        player.stats = None;
        assert!(!is_available(
            &player,
            &quest,
            &definitions,
            scripts(),
            1_000,
        ));

        quest.start.minimum_fame = Some(0);
        assert!(!is_available(
            &player,
            &quest,
            &definitions,
            scripts(),
            1_000,
        ));
    }

    #[test]
    fn world_availability_uses_inclusive_configured_bounds_for_manual_and_automatic_start() {
        let definitions = item_definitions();
        let mut quest = quest(100);
        quest.start.minimum_world_id = Some(2);
        quest.start.maximum_world_id = Some(4);
        let mut player = player(Vec::new(), 1);
        player.stats = None;

        for (world_id, expected) in [(1, false), (2, true), (3, true), (4, true), (5, false)] {
            assert_eq!(
                is_available_in_environment(
                    &player,
                    &quest,
                    &definitions,
                    scripts(),
                    super::QuestEnvironment {
                        now_unix_ms: 1_000,
                        world_id,
                    },
                ),
                expected,
                "world {world_id}",
            );
        }

        quest.info.auto_start = true;
        quest.start.minimum_world_id = Some(3);
        quest.start.maximum_world_id = Some(3);
        let blocked = advance_automatic_quests_in_environment(
            player.clone(),
            [&quest],
            curve(),
            &definitions,
            scripts(),
            super::QuestEnvironment {
                now_unix_ms: 1_000,
                world_id: 2,
            },
        );
        assert!(blocked.started_quest_ids.is_empty());
        let exact = advance_automatic_quests_in_environment(
            player,
            [&quest],
            curve(),
            &definitions,
            scripts(),
            super::QuestEnvironment {
                now_unix_ms: 1_000,
                world_id: 3,
            },
        );
        assert_eq!(exact.started_quest_ids, vec![quest.id]);
    }

    #[test]
    fn availability_enforces_item_absence_in_inventory_and_equipment() {
        let definitions = item_definitions();
        let mut quest = quest(100);
        quest.start.items.push(QuestItemRequirement {
            item_id: ITEM_A,
            condition: QuestItemCondition::Absent,
        });
        let mut player = player(Vec::new(), 1);

        assert!(is_available(
            &player,
            &quest,
            &definitions,
            scripts(),
            1_000
        ));
        player
            .inventory
            .as_mut()
            .expect("inventory")
            .stacks
            .push(InventoryItemStack {
                item_id: ITEM_A,
                quantity: 1,
                expires_at_unix_ms: 0,
            });
        assert!(!is_available(
            &player,
            &quest,
            &definitions,
            scripts(),
            1_000
        ));
        let inventory = player.inventory.as_mut().expect("inventory");
        inventory.stacks.clear();
        inventory.equipment.push(EquippedItem {
            item_id: ITEM_A,
            ..EquippedItem::default()
        });
        assert!(!is_available(
            &player,
            &quest,
            &definitions,
            scripts(),
            1_000
        ));
    }

    #[test]
    fn availability_requires_equipped_items_in_all_of_and_any_of_lists() {
        let definitions = item_definitions();
        let mut quest = quest(100);
        quest.start.equipped_items = QuestEquippedItemRequirements {
            all_of: vec![ITEM_B, ITEM_C],
            any_of: Vec::new(),
        };
        let mut player = player(vec![(ITEM_C, 1)], 2);
        let inventory = player.inventory.as_mut().expect("inventory");
        inventory.equipment = vec![
            EquippedItem {
                slot: EquipmentSlot::Top as i32,
                item_id: ITEM_B,
                expires_at_unix_ms: 0,
            },
            EquippedItem {
                slot: EquipmentSlot::Bottom as i32,
                item_id: ITEM_C,
                expires_at_unix_ms: 0,
            },
        ];

        assert!(is_available(
            &player,
            &quest,
            &definitions,
            scripts(),
            1_000,
        ));
        player
            .inventory
            .as_mut()
            .expect("inventory")
            .equipment
            .retain(|item| item.item_id != ITEM_C);
        assert!(
            !is_available(&player, &quest, &definitions, scripts(), 1_000),
            "an item in the bag does not satisfy an equipped-item requirement",
        );

        quest.start.equipped_items = QuestEquippedItemRequirements {
            all_of: Vec::new(),
            any_of: vec![ITEM_B, ITEM_C],
        };
        assert!(is_available(
            &player,
            &quest,
            &definitions,
            scripts(),
            1_000,
        ));
        player
            .inventory
            .as_mut()
            .expect("inventory")
            .equipment
            .clear();
        assert!(!is_available(
            &player,
            &quest,
            &definitions,
            scripts(),
            1_000,
        ));
        player.inventory = None;
        assert!(!is_available(
            &player,
            &quest,
            &definitions,
            scripts(),
            1_000,
        ));
    }

    #[test]
    fn completion_readiness_reports_equipped_item_objectives() {
        let definitions = item_definitions();
        let mut quest = quest(100);
        quest.completion.equipped_items = QuestEquippedItemRequirements {
            all_of: vec![ITEM_B],
            any_of: vec![ITEM_C],
        };
        let mut player = player(vec![(ITEM_C, 1)], 2);
        player.quests.push(player_quest(100, QuestStatus::Started));
        player
            .inventory
            .as_mut()
            .expect("inventory")
            .equipment
            .push(EquippedItem {
                slot: EquipmentSlot::Top as i32,
                item_id: ITEM_B,
                expires_at_unix_ms: 0,
            });

        let missing = completion_readiness(&player, &quest, &definitions, scripts());
        assert!(!missing.ready);
        assert_eq!(missing.objectives.len(), 2);
        assert!(
            missing
                .objectives
                .iter()
                .all(|objective| { objective.kind == super::QuestObjectiveKind::Equipment })
        );
        assert!(missing.objectives[0].complete);
        assert!(!missing.objectives[1].complete);
        assert!(missing.objectives[0].label.contains("Dagger A"));
        assert!(missing.objectives[1].label.contains("Dagger B"));

        player
            .inventory
            .as_mut()
            .expect("inventory")
            .equipment
            .push(EquippedItem {
                slot: EquipmentSlot::Bottom as i32,
                item_id: ITEM_C,
                expires_at_unix_ms: 0,
            });
        assert!(completion_readiness(&player, &quest, &definitions, scripts()).ready);

        player.inventory = None;
        let missing_inventory = completion_readiness(&player, &quest, &definitions, scripts());
        assert!(!missing_inventory.ready);
        assert!(
            missing_inventory
                .objectives
                .iter()
                .all(|objective| !objective.complete)
        );
    }

    #[test]
    fn automatic_transitions_enforce_equipped_item_checks() {
        let definitions = item_definitions();
        let mut automatic_start = quest(100);
        automatic_start.info.auto_start = true;
        automatic_start.start.equipped_items.all_of.push(ITEM_B);
        let bag_only = player(vec![(ITEM_B, 1)], 2);

        let blocked_start = advance_automatic_quests(
            bag_only.clone(),
            [&automatic_start],
            curve(),
            &definitions,
            scripts(),
            1_000,
        );
        assert!(blocked_start.started_quest_ids.is_empty());
        let mut equipped = bag_only;
        equipped
            .inventory
            .as_mut()
            .expect("inventory")
            .equipment
            .push(EquippedItem {
                slot: EquipmentSlot::Top as i32,
                item_id: ITEM_B,
                expires_at_unix_ms: 0,
            });
        let started = advance_automatic_quests(
            equipped,
            [&automatic_start],
            curve(),
            &definitions,
            scripts(),
            1_000,
        );
        assert_eq!(started.started_quest_ids, vec![automatic_start.id]);

        let mut automatic_completion = quest(200);
        automatic_completion.info.auto_pre_complete = true;
        automatic_completion
            .completion
            .equipped_items
            .all_of
            .push(ITEM_B);
        let mut active = player(vec![(ITEM_B, 1)], 2);
        active
            .quests
            .push(player_quest(automatic_completion.id, QuestStatus::Started));
        let blocked_completion = advance_automatic_quests(
            active.clone(),
            [&automatic_completion],
            curve(),
            &definitions,
            scripts(),
            1_000,
        );
        assert!(blocked_completion.completed_quest_ids.is_empty());
        active
            .inventory
            .as_mut()
            .expect("inventory")
            .equipment
            .push(EquippedItem {
                slot: EquipmentSlot::Top as i32,
                item_id: ITEM_B,
                expires_at_unix_ms: 0,
            });
        let completed = advance_automatic_quests(
            active,
            [&automatic_completion],
            curve(),
            &definitions,
            scripts(),
            1_000,
        );
        assert_eq!(completed.completed_quest_ids, vec![automatic_completion.id]);
    }

    #[test]
    fn monster_book_requirements_gate_start_and_report_typed_completion_objectives() {
        let definitions = item_definitions();
        let requirements = QuestMonsterBookRequirements {
            cards: vec![QuestMonsterBookCardRequirement {
                card_item_id: CARD_A,
                minimum_count: Some(2),
                maximum_count: Some(3),
            }],
            minimum_unique_cards: Some(2),
            maximum_unique_cards: Some(2),
        };
        let mut quest = quest(100);
        quest.start.monster_book = requirements.clone();
        quest.completion.monster_book = requirements;

        let mut player = player(Vec::new(), 1);
        player.monster_book_cards = vec![
            MonsterBookCard {
                card_item_id: CARD_A,
                count: 1,
            },
            MonsterBookCard {
                card_item_id: CARD_B,
                count: 1,
            },
        ];
        assert!(!is_available(
            &player,
            &quest,
            &definitions,
            scripts(),
            1_000
        ));
        player.monster_book_cards[0].count = 2;
        assert!(is_available(
            &player,
            &quest,
            &definitions,
            scripts(),
            1_000
        ));
        player.monster_book_cards[0].count = 4;
        assert!(!is_available(
            &player,
            &quest,
            &definitions,
            scripts(),
            1_000
        ));

        player.monster_book_cards[0].count = 1;
        player
            .quests
            .push(player_quest(quest.id, QuestStatus::Started));
        let incomplete = completion_readiness(&player, &quest, &definitions, scripts());
        assert!(!incomplete.ready);
        assert_eq!(incomplete.objectives.len(), 4);
        assert_eq!(
            incomplete
                .objectives
                .iter()
                .map(|objective| objective.kind)
                .collect::<Vec<_>>(),
            vec![
                super::QuestObjectiveKind::MonsterBookCardMinimum,
                super::QuestObjectiveKind::MonsterBookCardMaximum,
                super::QuestObjectiveKind::MonsterBookUniqueMinimum,
                super::QuestObjectiveKind::MonsterBookUniqueMaximum,
            ]
        );
        assert!(!incomplete.objectives[0].complete);
        assert!(
            incomplete.objectives[1..]
                .iter()
                .all(|objective| objective.complete)
        );

        player.monster_book_cards[0].count = 2;
        assert!(completion_readiness(&player, &quest, &definitions, scripts()).ready);
    }

    #[test]
    fn automatic_transitions_enforce_monster_book_checks_without_consuming_cards() {
        let definitions = item_definitions();
        let requirement = QuestMonsterBookCardRequirement {
            card_item_id: CARD_A,
            minimum_count: Some(1),
            maximum_count: None,
        };
        let mut automatic_start = quest(100);
        automatic_start.info.auto_start = true;
        automatic_start.start.monster_book.cards.push(requirement);
        let missing = player(Vec::new(), 1);

        let blocked_start = advance_automatic_quests(
            missing.clone(),
            [&automatic_start],
            curve(),
            &definitions,
            scripts(),
            1_000,
        );
        assert!(blocked_start.started_quest_ids.is_empty());
        let mut collected = missing;
        collected.monster_book_cards.push(MonsterBookCard {
            card_item_id: CARD_A,
            count: 1,
        });
        let started = advance_automatic_quests(
            collected,
            [&automatic_start],
            curve(),
            &definitions,
            scripts(),
            1_000,
        );
        assert_eq!(started.started_quest_ids, vec![automatic_start.id]);

        let mut automatic_completion = quest(200);
        automatic_completion.info.auto_pre_complete = true;
        automatic_completion
            .completion
            .monster_book
            .cards
            .push(requirement);
        let mut active = player(Vec::new(), 1);
        active
            .quests
            .push(player_quest(automatic_completion.id, QuestStatus::Started));
        let blocked_completion = advance_automatic_quests(
            active.clone(),
            [&automatic_completion],
            curve(),
            &definitions,
            scripts(),
            1_000,
        );
        assert!(blocked_completion.completed_quest_ids.is_empty());
        active.monster_book_cards.push(MonsterBookCard {
            card_item_id: CARD_A,
            count: 1,
        });
        let completed = advance_automatic_quests(
            active,
            [&automatic_completion],
            curve(),
            &definitions,
            scripts(),
            1_000,
        );
        assert_eq!(completed.completed_quest_ids, vec![automatic_completion.id]);
        assert_eq!(
            completed.player.monster_book_cards,
            vec![MonsterBookCard {
                card_item_id: CARD_A,
                count: 1,
            }]
        );
    }

    #[test]
    fn availability_enforces_map_and_skill_requirements() {
        let definitions = item_definitions();
        let mut quest = quest(100);
        quest.start.allowed_map_ids = vec![100, 200];
        quest.start.skills = vec![
            QuestSkillRequirement {
                skill_id: 1_001_004,
                acquired: true,
            },
            QuestSkillRequirement {
                skill_id: 1_001_005,
                acquired: false,
            },
        ];
        let mut player = player(Vec::new(), 1);
        player.map_id = 100;
        player.learned_skills.push(LearnedSkill {
            skill_id: 1_001_004,
            level: 2,
            master_level: 0,
        });

        assert!(is_available(
            &player,
            &quest,
            &definitions,
            scripts(),
            1_000
        ));
        player.map_id = 300;
        assert!(!is_available(
            &player,
            &quest,
            &definitions,
            scripts(),
            1_000
        ));
        player.map_id = 100;
        player.learned_skills.push(LearnedSkill {
            skill_id: 1_001_005,
            level: 1,
            master_level: 0,
        });
        assert!(!is_available(
            &player,
            &quest,
            &definitions,
            scripts(),
            1_000
        ));
    }

    #[test]
    fn every_automatic_start_flag_enforces_normal_requirements() {
        let definitions = item_definitions();
        let mut auto_start = quest(100);
        auto_start.info.auto_start = true;
        auto_start.start.minimum_level = Some(10);
        let mut auto_accept = quest(200);
        auto_accept.info.auto_accept = true;
        auto_accept.start.minimum_level = Some(10);
        let mut normal_auto_start = quest(300);
        normal_auto_start.start.normal_auto_start = true;
        normal_auto_start.start.minimum_level = Some(10);
        let quests = [&auto_start, &auto_accept, &normal_auto_start];

        let low_level = advance_automatic_quests(
            player(Vec::new(), 1),
            quests,
            curve(),
            &definitions,
            scripts(),
            1_000,
        );
        assert!(low_level.started_quest_ids.is_empty());
        assert_eq!(progress(&low_level.player, 100), QuestProgress::NotStarted);
        assert_eq!(progress(&low_level.player, 200), QuestProgress::NotStarted);
        assert_eq!(progress(&low_level.player, 300), QuestProgress::NotStarted);

        let mut eligible = player(Vec::new(), 1);
        eligible.level = 10;
        let eligible =
            advance_automatic_quests(eligible, quests, curve(), &definitions, scripts(), 1_000);
        assert_eq!(eligible.started_quest_ids, vec![100, 200, 300]);
    }

    #[test]
    fn combined_automatic_start_flags_enforce_normal_requirements() {
        let definitions = item_definitions();
        let mut normal_auto_start = quest(100);
        normal_auto_start.info.auto_start = true;
        normal_auto_start.start.normal_auto_start = true;
        normal_auto_start.start.minimum_level = Some(10);
        let mut auto_accept = quest(200);
        auto_accept.info.auto_start = true;
        auto_accept.info.auto_accept = true;
        auto_accept.start.allowed_jobs = vec![100];
        let quests = [&normal_auto_start, &auto_accept];

        let ineligible = advance_automatic_quests(
            player(Vec::new(), 1),
            quests,
            curve(),
            &definitions,
            scripts(),
            1_000,
        );
        assert!(ineligible.started_quest_ids.is_empty());

        let mut eligible = player(Vec::new(), 1);
        eligible.level = 10;
        eligible.stats.as_mut().expect("stats").job_id = 100;
        let eligible =
            advance_automatic_quests(eligible, quests, curve(), &definitions, scripts(), 1_000);
        assert_eq!(eligible.started_quest_ids, vec![100, 200]);
    }

    #[test]
    fn automatic_start_does_not_restart_a_completed_nonrepeatable_quest() {
        let definitions = item_definitions();
        let mut quest = quest(100);
        quest.info.auto_start = true;
        let mut player = player(Vec::new(), 1);
        player
            .quests
            .push(player_quest(quest.id, QuestStatus::Completed));

        let advanced =
            advance_automatic_quests(player, [&quest], curve(), &definitions, scripts(), 1_000);

        assert!(!advanced.changed);
        assert_eq!(
            progress(&advanced.player, quest.id),
            QuestProgress::Completed
        );
    }

    #[test]
    fn timed_quests_expire_without_rewards() {
        let definitions = item_definitions();
        let mut quest = quest(100);
        quest.info.time_limit_ms = Some(100);
        quest.completion_actions.money = 500;
        let mut player = player(Vec::new(), 1);
        player.quests.push(PlayerQuest {
            accepted_at_unix_ms: 1_000,
            ..player_quest(quest.id, QuestStatus::Started)
        });
        crate::quest_records::set(&mut player, 100, 0, "owned".to_owned()).expect("owned record");
        crate::quest_records::set(&mut player, 999, 0, "redirected".to_owned())
            .expect("redirected record");

        let before_deadline = advance_automatic_quests(
            player.clone(),
            [&quest],
            curve(),
            &definitions,
            scripts(),
            1_099,
        );
        assert!(!before_deadline.changed);
        assert_eq!(
            crate::quest_records::get(&before_deadline.player, 100, 0),
            Some("owned")
        );
        let expired =
            advance_automatic_quests(player, [&quest], curve(), &definitions, scripts(), 1_100);

        assert!(expired.changed);
        assert_eq!(expired.expired_quest_ids, vec![100]);
        assert_eq!(progress(&expired.player, 100), QuestProgress::NotStarted);
        assert_eq!(expired.player.mesos, 0);
        assert_eq!(crate::quest_records::get(&expired.player, 100, 0), None);
        assert_eq!(
            crate::quest_records::get(&expired.player, 999, 0),
            Some("redirected")
        );
    }

    #[test]
    fn expired_automatic_quest_is_not_reaccepted_in_the_same_pass() {
        let definitions = item_definitions();
        let mut quest = quest(100);
        quest.info.time_limit2_ms = Some(100);
        quest.info.auto_accept = true;
        let mut player = player(Vec::new(), 1);
        player.quests.push(PlayerQuest {
            accepted_at_unix_ms: 1_000,
            ..player_quest(quest.id, QuestStatus::Started)
        });

        let advanced =
            advance_automatic_quests(player, [&quest], curve(), &definitions, scripts(), 1_100);

        assert_eq!(advanced.expired_quest_ids, vec![100]);
        assert!(advanced.started_quest_ids.is_empty());
        assert_eq!(progress(&advanced.player, 100), QuestProgress::NotStarted);
    }

    #[test]
    fn automatic_completion_uses_normal_readiness_and_rewards() {
        let definitions = item_definitions();
        let mut quest = quest(100);
        quest.info.auto_complete = true;
        quest.completion.items.push(item_requirement(ITEM_A, 1));
        quest.completion_actions.money = 100;
        let mut player = player(vec![(ITEM_A, 1)], 1);
        player.quests.push(PlayerQuest {
            accepted_at_unix_ms: 100,
            ..player_quest(quest.id, QuestStatus::Started)
        });

        let advanced =
            advance_automatic_quests(player, [&quest], curve(), &definitions, scripts(), 200);

        assert_eq!(advanced.completed_quest_ids, vec![100]);
        assert_eq!(progress(&advanced.player, 100), QuestProgress::Completed);
        assert_eq!(advanced.player.mesos, 100);
    }

    #[test]
    fn automatic_precompletion_bypasses_ordinary_objectives() {
        let definitions = item_definitions();
        let mut quest = quest(100);
        quest.info.auto_pre_complete = true;
        quest.completion.required_level = Some(99);
        quest.completion.items.push(item_requirement(ITEM_A, 10));
        quest.completion_actions.money = 100;
        let mut player = player(Vec::new(), 1);
        player.quests.push(PlayerQuest {
            accepted_at_unix_ms: 100,
            ..player_quest(quest.id, QuestStatus::Started)
        });

        let advanced =
            advance_automatic_quests(player, [&quest], curve(), &definitions, scripts(), 200);

        assert_eq!(advanced.completed_quest_ids, vec![100]);
        assert_eq!(progress(&advanced.player, 100), QuestProgress::Completed);
        assert_eq!(advanced.player.mesos, 100);
    }

    #[test]
    fn automatic_transitions_repeat_until_dependency_chains_are_stable() {
        let definitions = item_definitions();
        let mut dependent = quest(100);
        dependent.info.auto_accept = true;
        dependent.info.auto_complete = true;
        dependent.start.quests.push(QuestStateRequirement {
            quest_id: 200,
            state: RequiredQuestState::Completed,
        });
        let mut first = quest(200);
        first.info.auto_accept = true;
        first.info.auto_complete = true;
        first.completion_actions.next_quest_id = Some(100);

        let advanced = advance_automatic_quests(
            player(Vec::new(), 1),
            [&dependent, &first],
            curve(),
            &definitions,
            scripts(),
            1_000,
        );

        assert_eq!(advanced.started_quest_ids, vec![200, 100]);
        assert_eq!(advanced.completed_quest_ids, vec![200, 100]);
        assert_eq!(progress(&advanced.player, 100), QuestProgress::Completed);
        assert_eq!(progress(&advanced.player, 200), QuestProgress::Completed);
    }

    #[test]
    fn record_writes_unlock_automatic_quests_on_the_next_fixed_point_pass() {
        let definitions = item_definitions();
        let mut dependent = quest(100);
        dependent.info.auto_accept = true;
        dependent.start.record_conditions.push(record_condition(
            300,
            vec![QuestRecordPredicate::Equal("ready".to_owned())],
        ));
        let mut producer = quest(200);
        producer.info.auto_accept = true;
        producer.start_actions.record_writes.push(QuestRecordWrite {
            quest_id: 300,
            index: 0,
            value: "ready".to_owned(),
        });

        let advanced = advance_automatic_quests(
            player(Vec::new(), 1),
            [&dependent, &producer],
            curve(),
            &definitions,
            scripts(),
            1_000,
        );

        assert_eq!(advanced.started_quest_ids, vec![200, 100]);
        assert_eq!(progress(&advanced.player, 100), QuestProgress::Started);
        assert_eq!(
            crate::quest_records::get(&advanced.player, 300, 0),
            Some("ready")
        );
    }

    #[test]
    fn quest_state_actions_unlock_automatic_prerequisite_chains_in_one_advance() {
        let definitions = item_definitions();
        let mut dependent = quest(100);
        dependent.info.auto_accept = true;
        dependent.start.quests.push(QuestStateRequirement {
            quest_id: 300,
            state: RequiredQuestState::Completed,
        });
        let mut producer = quest(200);
        producer.info.auto_accept = true;
        producer
            .start_actions
            .quest_state_actions
            .push(QuestStateAction {
                quest_id: 300,
                state: QuestStateActionState::Completed,
            });

        let advanced = advance_automatic_quests(
            player(Vec::new(), 1),
            [&dependent, &producer],
            curve(),
            &definitions,
            scripts(),
            1_000,
        );

        assert_eq!(advanced.started_quest_ids, vec![200, 100]);
        assert_eq!(progress(&advanced.player, 100), QuestProgress::Started);
        assert_eq!(progress(&advanced.player, 300), QuestProgress::Completed);
    }

    #[test]
    fn automatic_start_flags_enforce_records_while_auto_pre_complete_bypasses_them() {
        let definitions = item_definitions();
        let missing = record_condition(900, vec![QuestRecordPredicate::Equal("ready".to_owned())]);
        let mut auto_start = quest(100);
        auto_start.info.auto_start = true;
        auto_start.start.record_conditions.push(missing.clone());
        let mut auto_accept = quest(200);
        auto_accept.info.auto_accept = true;
        auto_accept.start.record_conditions.push(missing.clone());
        let mut auto_complete = quest(300);
        auto_complete.info.auto_complete = true;
        auto_complete
            .completion
            .record_conditions
            .push(missing.clone());
        let mut auto_pre_complete = quest(400);
        auto_pre_complete.info.auto_pre_complete = true;
        auto_pre_complete.completion.record_conditions.push(missing);
        let mut player = player(Vec::new(), 1);
        player.quests.push(player_quest(300, QuestStatus::Started));
        player.quests.push(player_quest(400, QuestStatus::Started));

        let advanced = advance_automatic_quests(
            player,
            [
                &auto_start,
                &auto_accept,
                &auto_complete,
                &auto_pre_complete,
            ],
            curve(),
            &definitions,
            scripts(),
            1_000,
        );

        assert!(advanced.started_quest_ids.is_empty());
        assert_eq!(advanced.completed_quest_ids, vec![400]);
        assert_eq!(progress(&advanced.player, 200), QuestProgress::NotStarted);
        assert_eq!(progress(&advanced.player, 300), QuestProgress::Started);
    }

    #[test]
    fn record_predicates_are_or_alternatives_and_missing_records_fail_closed() {
        let definitions = item_definitions();
        let mut quest = quest(100);
        quest.start.record_conditions.push(record_condition(
            200,
            vec![
                QuestRecordPredicate::Equal("007".to_owned()),
                QuestRecordPredicate::AtLeast(10),
                QuestRecordPredicate::AtMost(2),
            ],
        ));
        let mut player = player(Vec::new(), 1);

        assert!(!is_available(
            &player,
            &quest,
            &definitions,
            scripts(),
            1_000
        ));
        crate::quest_records::set(&mut player, 200, 0, "007".to_owned()).expect("exact record");
        assert!(is_available(
            &player,
            &quest,
            &definitions,
            scripts(),
            1_000
        ));
        let mut exact_quest = quest.clone();
        exact_quest.start.record_conditions[0].alternatives =
            vec![QuestRecordPredicate::Equal("007".to_owned())];
        assert!(is_available(
            &player,
            &exact_quest,
            &definitions,
            scripts(),
            1_000
        ));
        crate::quest_records::set(&mut player, 200, 0, "0070".to_owned())
            .expect("different leading zeros");
        assert!(!is_available(
            &player,
            &exact_quest,
            &definitions,
            scripts(),
            1_000
        ));
        assert!(is_available(
            &player,
            &quest,
            &definitions,
            scripts(),
            1_000
        ));
        crate::quest_records::set(&mut player, 200, 0, "AbC".to_owned()).expect("case record");
        assert!(!is_available(
            &player,
            &quest,
            &definitions,
            scripts(),
            1_000
        ));
        crate::quest_records::set(&mut player, 200, 0, "2".to_owned()).expect("upper-bound record");
        assert!(is_available(
            &player,
            &quest,
            &definitions,
            scripts(),
            1_000
        ));
    }

    #[test]
    fn completion_record_progress_is_reported_and_completion_preserves_records() {
        let definitions = item_definitions();
        let mut quest = quest(100);
        quest.completion.record_conditions.push(record_condition(
            200,
            vec![QuestRecordPredicate::Equal("Done".to_owned())],
        ));
        let mut player = player(Vec::new(), 1);
        player
            .quests
            .push(player_quest(quest.id, QuestStatus::Started));

        let missing = completion_readiness(&player, &quest, &definitions, scripts());
        assert!(!missing.ready);
        assert_eq!(
            missing.objectives[0].kind,
            super::QuestObjectiveKind::Record
        );
        assert!(missing.objectives[0].label.contains("is missing"));
        crate::quest_records::set(&mut player, 200, 0, "done".to_owned())
            .expect("wrong-case record");
        assert!(!completion_readiness(&player, &quest, &definitions, scripts()).ready);
        crate::quest_records::set(&mut player, 200, 0, "Done".to_owned()).expect("ready record");
        assert!(completion_readiness(&player, &quest, &definitions, scripts()).ready);

        let completed = select_choice(
            player,
            &quest,
            2,
            COMPLETE_CHOICE_ID,
            curve(),
            &definitions,
            scripts(),
            2_000,
        )
        .expect("complete record quest");
        assert_eq!(
            crate::quest_records::get(&completed.player, 200, 0),
            Some("Done")
        );
    }

    #[test]
    fn completion_mesos_is_inclusive_and_does_not_require_character_stats() {
        let definitions = item_definitions();
        let mut quest = quest(100);
        quest.completion.minimum_mesos = Some(100);
        let mut player = player(Vec::new(), 1);
        player.stats = None;
        player.quests.push(player_quest(100, QuestStatus::Started));

        player.mesos = 99;
        let below = completion_readiness(&player, &quest, &definitions, scripts());
        assert!(!below.ready);
        assert_eq!(below.objectives[0].kind, super::QuestObjectiveKind::Mesos);
        assert_eq!(below.objectives[0].current, 99);
        player.mesos = 100;
        assert!(completion_readiness(&player, &quest, &definitions, scripts()).ready);
        player.mesos = 101;
        assert!(completion_readiness(&player, &quest, &definitions, scripts()).ready);
    }

    #[test]
    fn completed_quest_count_uses_only_known_eligible_completed_definitions() {
        let definitions = item_definitions();
        let mut gate = quest(100);
        gate.completion.minimum_completed_quest_count = Some(2);
        let known = quest(200);
        let second_known = quest(300);
        let excluded_low = quest(9_000);
        let excluded_high = quest(10_999);
        let mut excluded_area = quest(11_000);
        excluded_area.info.area = Some(51);
        let quest_definitions = [
            &gate,
            &known,
            &second_known,
            &excluded_low,
            &excluded_high,
            &excluded_area,
        ];
        let mut player = player(Vec::new(), 1);
        player.stats = None;
        player.quests = vec![
            player_quest(gate.id, QuestStatus::Started),
            player_quest(known.id, QuestStatus::Completed),
            PlayerQuest {
                quest_id: second_known.id,
                status: 99,
                ..PlayerQuest::default()
            },
            player_quest(excluded_low.id, QuestStatus::Completed),
            player_quest(excluded_high.id, QuestStatus::Completed),
            player_quest(excluded_area.id, QuestStatus::Completed),
            player_quest(777, QuestStatus::Completed),
        ];

        let incomplete = completion_readiness_in_environment(
            &player,
            &gate,
            &quest_definitions,
            &definitions,
            scripts(),
            environment(1_000),
        );
        assert!(!incomplete.ready);
        assert_eq!(
            incomplete.objectives[0],
            super::QuestObjectiveProgress {
                kind: super::QuestObjectiveKind::CompletedQuests,
                label: "Eligible completed quests".to_owned(),
                current: 1,
                required: 2,
                complete: false,
            }
        );

        player
            .quests
            .iter_mut()
            .find(|entry| entry.quest_id == second_known.id)
            .expect("second known quest")
            .status = QuestStatus::Completed as i32;
        assert!(
            completion_readiness_in_environment(
                &player,
                &gate,
                &quest_definitions,
                &definitions,
                scripts(),
                environment(1_000),
            )
            .ready
        );
    }

    #[test]
    fn completion_window_is_inclusive_for_readiness_manual_and_automatic_completion() {
        let definitions = item_definitions();
        let initial_automatic_player = player(Vec::new(), 1);
        let mut automatic = quest(200);
        let mut quest = quest(100);
        quest.completion.available_from = Some(QuestCalendar {
            source: "start".to_owned(),
            unix_ms: 100,
        });
        quest.completion.available_until = Some(QuestCalendar {
            source: "end".to_owned(),
            unix_ms: 200,
        });
        let quest_definitions = [&quest];
        let mut player = player(Vec::new(), 1);
        player.stats = None;
        player
            .quests
            .push(player_quest(quest.id, QuestStatus::Started));

        for (now_unix_ms, expected) in [(99, false), (100, true), (200, true), (201, false)] {
            let readiness = completion_readiness_in_environment(
                &player,
                &quest,
                &quest_definitions,
                &definitions,
                scripts(),
                environment(now_unix_ms),
            );
            assert_eq!(readiness.ready, expected, "timestamp {now_unix_ms}");
            assert_eq!(
                readiness
                    .objectives
                    .iter()
                    .filter(|objective| {
                        objective.kind == super::QuestObjectiveKind::Availability
                    })
                    .count(),
                2
            );
        }

        assert!(matches!(
            select_choice_in_environment(
                player.clone(),
                &quest,
                &quest_definitions,
                2,
                COMPLETE_CHOICE_ID,
                curve(),
                &definitions,
                scripts(),
                environment(99),
            ),
            Err(QuestRuleError::ObjectivesIncomplete { quest_id: 100 })
        ));
        let completed = select_choice_in_environment(
            player,
            &quest,
            &quest_definitions,
            2,
            COMPLETE_CHOICE_ID,
            curve(),
            &definitions,
            scripts(),
            environment(100),
        )
        .expect("completion at inclusive start");
        assert_eq!(completed.player.quests[0].completed_at_unix_ms, 100);

        automatic.info.auto_accept = true;
        automatic.info.auto_complete = true;
        automatic.start.available_from = Some(QuestCalendar {
            source: "same".to_owned(),
            unix_ms: 500,
        });
        automatic.start.available_until = Some(QuestCalendar {
            source: "same".to_owned(),
            unix_ms: 500,
        });
        automatic.completion.available_from = Some(QuestCalendar {
            source: "same".to_owned(),
            unix_ms: 500,
        });
        automatic.completion.available_until = Some(QuestCalendar {
            source: "same".to_owned(),
            unix_ms: 500,
        });
        let advanced = advance_automatic_quests_in_environment(
            initial_automatic_player,
            [&automatic],
            curve(),
            &definitions,
            scripts(),
            environment(500),
        );
        assert_eq!(advanced.started_quest_ids, vec![automatic.id]);
        assert_eq!(advanced.completed_quest_ids, vec![automatic.id]);
        let entry = advanced
            .player
            .quests
            .iter()
            .find(|entry| entry.quest_id == automatic.id)
            .expect("automatic quest record");
        assert_eq!(entry.accepted_at_unix_ms, 500);
        assert_eq!(entry.completed_at_unix_ms, 500);
    }

    #[test]
    fn presentation_npc_metadata_does_not_replace_the_required_start_npc() {
        let definitions = item_definitions();
        let mut quest = quest(100);
        quest.start_actions.presentation_npc_id = Some(9_201_142);

        assert!(matches!(
            select_choice(
                player(Vec::new(), 1),
                &quest,
                9_201_142,
                ACCEPT_CHOICE_ID,
                curve(),
                &definitions,
                scripts(),
                1_000,
            ),
            Err(QuestRuleError::WrongNpc { npc_id: 9_201_142 })
        ));
        let accepted = select_choice(
            player(Vec::new(), 1),
            &quest,
            1,
            ACCEPT_CHOICE_ID,
            curve(),
            &definitions,
            scripts(),
            1_000,
        )
        .expect("required Check NPC starts quest");
        assert_eq!(progress(&accepted.player, quest.id), QuestProgress::Started);
        assert_eq!(quest.start.npc_id, Some(1));
        assert_eq!(quest.start_actions.presentation_npc_id, Some(9_201_142));
    }

    #[test]
    fn start_record_actions_are_atomic_and_only_successful_acceptance_writes() {
        let definitions = item_definitions();
        let mut quest = quest(100);
        quest.start.record_conditions.push(record_condition(
            100,
            vec![QuestRecordPredicate::Equal("gate".to_owned())],
        ));
        quest.start_actions.record_writes.push(QuestRecordWrite {
            quest_id: 100,
            index: 0,
            value: "started".to_owned(),
        });
        let mut player = player(Vec::new(), 1);
        crate::quest_records::set(&mut player, 100, 0, "gate".to_owned()).expect("gate record");
        crate::quest_records::set(&mut player, 100, 7, "stale".to_owned()).expect("stale record");

        let declined = select_choice(
            player.clone(),
            &quest,
            1,
            DECLINE_CHOICE_ID,
            curve(),
            &definitions,
            scripts(),
            1_000,
        )
        .expect("decline quest");
        assert_eq!(
            crate::quest_records::get(&declined.player, 100, 0),
            Some("gate")
        );

        let mut unavailable = player.clone();
        crate::quest_records::set(&mut unavailable, 100, 0, "closed".to_owned())
            .expect("closed gate");
        let unchanged_unavailable = unavailable.clone();
        assert!(matches!(
            select_choice(
                unavailable,
                &quest,
                1,
                ACCEPT_CHOICE_ID,
                curve(),
                &definitions,
                scripts(),
                1_000,
            ),
            Err(QuestRuleError::Unavailable { quest_id: 100 })
        ));
        assert_eq!(
            crate::quest_records::get(&unchanged_unavailable, 100, 0),
            Some("closed")
        );

        let mut blocked = quest.clone();
        blocked.start_actions.fixed_items.push(QuestItemDelta {
            item_id: ITEM_B,
            count: 1,
            expiration: None,
        });
        blocked.start_actions.fixed_items.push(QuestItemDelta {
            item_id: ITEM_C,
            count: 1,
            expiration: None,
        });
        let original = player.clone();
        assert!(
            select_choice(
                player,
                &blocked,
                1,
                ACCEPT_CHOICE_ID,
                curve(),
                &definitions,
                scripts(),
                1_000,
            )
            .is_err()
        );
        assert_eq!(crate::quest_records::get(&original, 100, 0), Some("gate"));
        assert_eq!(crate::quest_records::get(&original, 100, 7), Some("stale"));

        let accepted = select_choice(
            original,
            &quest,
            1,
            ACCEPT_CHOICE_ID,
            curve(),
            &definitions,
            scripts(),
            1_000,
        )
        .expect("accept record quest");
        assert_eq!(
            crate::quest_records::get(&accepted.player, 100, 0),
            Some("started")
        );
        assert_eq!(crate::quest_records::get(&accepted.player, 100, 7), None);
    }

    #[test]
    fn wrong_start_answer_does_not_apply_a_record_action() {
        let definitions = item_definitions();
        let mut quest = quest(100);
        configure_start_question(&mut quest);
        quest.start_actions.record_writes.push(QuestRecordWrite {
            quest_id: 100,
            index: 0,
            value: "started".to_owned(),
        });

        let pending = open_start_question(
            player(Vec::new(), 1),
            &quest,
            &definitions,
            scripts(),
            1_000,
        );
        let wrong = select_choice(
            pending,
            &quest,
            1,
            answer_choice_id(QuestQuestionPhase::Start, 0, 0).expect("first start answer"),
            curve(),
            &definitions,
            scripts(),
            1_000,
        )
        .expect("wrong answer");

        assert!(!wrong.changed);
        assert_eq!(crate::quest_records::get(&wrong.player, 100, 0), None);
    }

    #[test]
    fn script_record_action_rolls_back_with_a_failed_merged_action() {
        let definitions = item_definitions();
        let mut quest = quest(100);
        quest.start.script = Some("record_rollback".to_owned());
        let scripts = script_catalog(
            r#"
                [[scripts]]
                name = "record_rollback"

                [[scripts.actions]]
                type = "item_delta"
                item_id = 1332005
                delta = 1

                [[scripts.actions]]
                type = "set_record"
                quest_id = 900
                index = 7
                value = "written"
            "#,
            &quest,
            &definitions,
        );
        let original = player(Vec::new(), 0);

        assert!(
            select_choice(
                original.clone(),
                &quest,
                1,
                ACCEPT_CHOICE_ID,
                curve(),
                &definitions,
                &scripts,
                1_000,
            )
            .is_err()
        );
        assert_eq!(crate::quest_records::get(&original, 900, 7), None);
        assert_eq!(progress(&original, 100), QuestProgress::NotStarted);
    }

    #[test]
    fn blocked_automatic_action_is_atomic_and_does_not_stop_other_quests() {
        let definitions = item_definitions();
        let mut blocked = quest(100);
        blocked.info.auto_complete = true;
        blocked.completion_actions.fixed_items.push(QuestItemDelta {
            item_id: ITEM_B,
            count: 1,
            expiration: None,
        });
        blocked.completion_actions.skill_changes = vec![skill_change(1_003, 1, 1, Vec::new())];
        let mut startable = quest(200);
        startable.info.auto_accept = true;
        startable.start_actions.skill_changes = vec![skill_change(1_004, 1, 1, Vec::new())];
        let mut player = player(vec![(ITEM_A, 10)], 1);
        player.quests.push(PlayerQuest {
            accepted_at_unix_ms: 100,
            ..player_quest(blocked.id, QuestStatus::Started)
        });

        let advanced = advance_automatic_quests(
            player,
            [&blocked, &startable],
            curve(),
            &definitions,
            scripts(),
            200,
        );

        assert_eq!(
            progress(&advanced.player, blocked.id),
            QuestProgress::Started
        );
        assert_eq!(item_count(&advanced.player, ITEM_B), 0);
        assert_eq!(
            progress(&advanced.player, startable.id),
            QuestProgress::Started
        );
        assert_eq!(advanced.failures.len(), 1);
        assert_eq!(advanced.failures[0].quest_id, blocked.id);
        assert!(
            !advanced
                .player
                .learned_skills
                .iter()
                .any(|skill| skill.skill_id == 1_003)
        );
        assert!(
            advanced
                .player
                .learned_skills
                .iter()
                .any(|skill| skill.skill_id == 1_004)
        );
    }

    #[test]
    fn quest_skill_changes_use_independent_maxima_without_sp_or_downgrades() {
        let definitions = item_definitions();
        let mut quest = quest(100);
        quest.start_actions.skill_changes = vec![
            skill_change(1_121_010, 3, 30, vec![112]),
            skill_change(1_121_002, 10, 15, vec![112]),
        ];
        let mut player = player(Vec::new(), 1);
        player.stats.as_mut().expect("stats").job_id = 112;
        player.skill_points = 7;
        player.learned_skills = vec![
            LearnedSkill {
                skill_id: 1_121_002,
                level: 20,
                master_level: 10,
            },
            LearnedSkill {
                skill_id: 1_121_010,
                level: 5,
                master_level: 20,
            },
        ];

        let accepted = select_choice(
            player,
            &quest,
            1,
            ACCEPT_CHOICE_ID,
            curve(),
            &definitions,
            scripts(),
            100,
        )
        .expect("quest skill changes");

        assert_eq!(accepted.player.skill_points, 7);
        assert_eq!(
            accepted
                .player
                .learned_skills
                .iter()
                .map(|skill| (skill.skill_id, skill.level, skill.master_level))
                .collect::<Vec<_>>(),
            vec![(1_121_002, 20, 15), (1_121_010, 5, 30)]
        );
    }

    #[test]
    fn quest_skill_eligibility_uses_exact_jobs_and_beginner_family_bypass() {
        let definitions = item_definitions();
        let mut quest = quest(100);
        quest.start_actions.skill_changes = vec![
            skill_change(2_321_003, 1, 10, vec![232]),
            skill_change(1_003, 1, 1, Vec::new()),
        ];
        let mut player = player(Vec::new(), 1);
        player.stats.as_mut().expect("stats").job_id = 112;

        let accepted = select_choice(
            player,
            &quest,
            1,
            ACCEPT_CHOICE_ID,
            curve(),
            &definitions,
            scripts(),
            100,
        )
        .expect("eligible beginner skill");

        assert_eq!(accepted.player.learned_skills.len(), 1);
        assert_eq!(accepted.player.learned_skills[0].skill_id, 1_003);
        assert_eq!(progress(&accepted.player, quest.id), QuestProgress::Started);
    }

    #[test]
    fn quest_skill_removal_is_exact_idempotent_and_clears_only_its_binding() {
        let removed_skill_id = 1_007;
        let retained_skill_id = 1_008;
        let mut player = player(Vec::new(), 1);
        player.skill_points = 7;
        player.learned_skills = vec![
            LearnedSkill {
                skill_id: removed_skill_id,
                level: 3,
                master_level: 3,
            },
            LearnedSkill {
                skill_id: retained_skill_id,
                level: 1,
                master_level: 2,
            },
        ];
        player.key_bindings = vec![
            KeyBinding {
                code: "KeyA".to_owned(),
                action: KeyAction::Unspecified as i32,
                skill_id: removed_skill_id,
            },
            KeyBinding {
                code: "KeyB".to_owned(),
                action: KeyAction::Unspecified as i32,
                skill_id: retained_skill_id,
            },
            KeyBinding {
                code: "Space".to_owned(),
                action: KeyAction::Jump as i32,
                skill_id: 0,
            },
        ];
        let removal = skill_removal(removed_skill_id, vec![0, 100]);

        super::apply_skill_changes(&mut player, std::slice::from_ref(&removal));
        let once = player.clone();
        super::apply_skill_changes(&mut player, &[removal]);

        assert_eq!(player, once);
        assert_eq!(player.skill_points, 7);
        assert_eq!(
            player
                .learned_skills
                .iter()
                .map(|skill| skill.skill_id)
                .collect::<Vec<_>>(),
            vec![retained_skill_id]
        );
        assert_eq!(
            player
                .key_bindings
                .iter()
                .map(|binding| (binding.code.as_str(), binding.skill_id, binding.action))
                .collect::<Vec<_>>(),
            vec![
                ("KeyB", retained_skill_id, KeyAction::Unspecified as i32),
                ("Space", 0, KeyAction::Jump as i32),
            ]
        );
    }

    #[test]
    fn marker_skill_is_acquired_for_quest_checks() {
        let definitions = item_definitions();
        let mut grant = quest(100);
        grant.start_actions.skill_changes = vec![skill_change(9_999, 1, 0, Vec::new())];
        let accepted = select_choice(
            player(Vec::new(), 1),
            &grant,
            1,
            ACCEPT_CHOICE_ID,
            curve(),
            &definitions,
            scripts(),
            100,
        )
        .expect("marker grant");
        let mut gated = quest(200);
        gated.start.skills.push(QuestSkillRequirement {
            skill_id: 9_999,
            acquired: true,
        });

        assert!(is_available(
            &accepted.player,
            &gated,
            &definitions,
            scripts(),
            100,
        ));
    }

    #[test]
    fn later_item_and_meso_failures_roll_back_quest_skill_changes() {
        let definitions = item_definitions();
        let original = player(Vec::new(), 1);
        for failure in ["item", "mesos"] {
            let mut quest = quest(100);
            quest.start_actions.skill_changes = vec![skill_change(1_003, 1, 1, Vec::new())];
            if failure == "item" {
                quest.start_actions.fixed_items.push(QuestItemDelta {
                    item_id: ITEM_A,
                    count: -1,
                    expiration: None,
                });
            } else {
                quest.start_actions.money = -1;
            }

            assert!(
                select_choice(
                    original.clone(),
                    &quest,
                    1,
                    ACCEPT_CHOICE_ID,
                    curve(),
                    &definitions,
                    scripts(),
                    100,
                )
                .is_err()
            );
            assert!(original.learned_skills.is_empty());
            assert_eq!(progress(&original, quest.id), QuestProgress::NotStarted);
        }
    }

    #[test]
    fn later_item_and_meso_failures_roll_back_skill_and_binding_removal() {
        let definitions = item_definitions();
        let mut original = player(Vec::new(), 1);
        original.learned_skills.push(LearnedSkill {
            skill_id: 1_007,
            level: 3,
            master_level: 3,
        });
        original.key_bindings.push(KeyBinding {
            code: "KeyA".to_owned(),
            action: KeyAction::Unspecified as i32,
            skill_id: 1_007,
        });
        for failure in ["item", "mesos"] {
            let mut quest = quest(100);
            quest.start_actions.skill_changes = vec![skill_removal(1_007, vec![0])];
            if failure == "item" {
                quest.start_actions.fixed_items.push(QuestItemDelta {
                    item_id: ITEM_A,
                    count: -1,
                    expiration: None,
                });
            } else {
                quest.start_actions.money = -1;
            }

            assert!(
                select_choice(
                    original.clone(),
                    &quest,
                    1,
                    ACCEPT_CHOICE_ID,
                    curve(),
                    &definitions,
                    scripts(),
                    100,
                )
                .is_err()
            );
            assert_eq!(original.learned_skills[0].skill_id, 1_007);
            assert_eq!(original.key_bindings[0].skill_id, 1_007);
            assert_eq!(progress(&original, quest.id), QuestProgress::NotStarted);
        }
    }

    #[test]
    fn item_objectives_track_current_authoritative_possession() {
        let definitions = item_definitions();
        let mut quest = quest(100);
        quest.completion.items.push(item_requirement(ITEM_A, 2));
        let mut player = player(vec![(ITEM_A, 1)], 4);
        player.quests.push(player_quest(100, QuestStatus::Started));

        let incomplete = completion_readiness(&player, &quest, &definitions, scripts());
        assert!(!incomplete.ready);
        assert_eq!(incomplete.objectives[0].current, 1);
        player.inventory.as_mut().expect("inventory").stacks[0].quantity = 2;
        assert!(completion_readiness(&player, &quest, &definitions, scripts()).ready);
        player.inventory.as_mut().expect("inventory").stacks[0].quantity = 1;
        assert!(!completion_readiness(&player, &quest, &definitions, scripts()).ready);
    }

    #[test]
    fn item_absence_objectives_require_an_authoritative_empty_inventory() {
        let definitions = item_definitions();
        let mut quest = quest(100);
        quest.completion.items.push(QuestItemRequirement {
            item_id: ITEM_A,
            condition: QuestItemCondition::Absent,
        });
        let mut player = player(Vec::new(), 1);
        player.quests.push(player_quest(100, QuestStatus::Started));

        assert!(completion_readiness(&player, &quest, &definitions, scripts()).ready);
        player.inventory = None;
        assert!(!completion_readiness(&player, &quest, &definitions, scripts()).ready);
    }

    #[test]
    fn completion_script_conditions_use_authored_incomplete_pages() {
        let definitions = item_definitions();
        let mut quest = quest(100);
        quest.completion.script = Some("completion_check".to_owned());
        let scripts = script_catalog(
            r#"
                [[scripts]]
                name = "completion_check"
                incomplete_pages = ["Return at level 2."]

                [[scripts.conditions]]
                type = "minimum_level"
                level = 2
            "#,
            &quest,
            &definitions,
        );
        let mut player = player(Vec::new(), 1);
        player.quests.push(player_quest(100, QuestStatus::Started));

        let readiness = completion_readiness(&player, &quest, &definitions, &scripts);
        assert!(!readiness.ready);
        assert_eq!(
            readiness.objectives[0].kind,
            super::QuestObjectiveKind::Script
        );
        let pages = incomplete_dialogue_pages(&player, &quest, &definitions, &scripts);
        assert_eq!(pages[0], "Ready");
        assert_eq!(pages[1], "Return at level 2.");
        assert!(pages[2].contains("Script conditions not met: completion_check"));

        player.level = 2;
        assert!(completion_readiness(&player, &quest, &definitions, &scripts).ready);
    }

    #[test]
    fn acceptance_applies_costs_and_grants_before_starting() {
        let definitions = item_definitions();
        let mut quest = quest(100);
        quest.start.items.push(item_requirement(ITEM_A, 1));
        quest.start_actions = QuestActions {
            fixed_items: vec![
                QuestItemDelta {
                    item_id: ITEM_A,
                    count: -1,
                    expiration: None,
                },
                QuestItemDelta {
                    item_id: ITEM_B,
                    count: 2,
                    expiration: None,
                },
            ],
            money: -50,
            experience: 2,
            fame: 3,
            next_quest_id: None,
            conditional_items: Vec::new(),
            weighted_items: Vec::new(),
            selectable_items: Vec::new(),
            quest_state_actions: Vec::new(),
            record_writes: Vec::new(),
            skill_changes: Vec::new(),
            buff_item_ids: Vec::new(),
            presentation_npc_id: None,
            npc_animation_action: None,
        };
        let mut player = player(vec![(ITEM_A, 2)], 4);
        player.mesos = 100;

        let accepted = select_choice(
            player,
            &quest,
            1,
            ACCEPT_CHOICE_ID,
            curve(),
            &definitions,
            scripts(),
            123_456,
        )
        .expect("accept quest");

        assert_eq!(accepted.player.mesos, 50);
        assert_eq!(item_count(&accepted.player, ITEM_A), 1);
        assert_eq!(item_count(&accepted.player, ITEM_B), 2);
        let stats = accepted.player.stats.expect("stats");
        assert_eq!(stats.experience, 2);
        assert_eq!(stats.fame, 3);
        let entry = &accepted.player.quests[0];
        assert_eq!(entry.status, QuestStatus::Started as i32);
        assert_eq!(entry.accepted_at_unix_ms, 123_456);
        assert_eq!(entry.completed_at_unix_ms, 0);
        assert!(entry.mob_progress.is_empty());
    }

    #[test]
    fn quest_state_action_reset_removes_progress_timestamps_and_owning_record() {
        let definitions = item_definitions();
        let mut producer = quest(100);
        producer
            .start_actions
            .quest_state_actions
            .push(QuestStateAction {
                quest_id: 200,
                state: QuestStateActionState::NotStarted,
            });
        let mut original = player(Vec::new(), 1);
        original.quests.push(PlayerQuest {
            quest_id: 200,
            status: QuestStatus::Completed as i32,
            mob_progress: vec![QuestMobProgress {
                mob_id: MOB_A,
                count: 9,
            }],
            accepted_at_unix_ms: 100,
            completed_at_unix_ms: 200,
            dialogue_step: 3,
            completion_quiz_passed: true,
        });
        crate::quest_records::set(&mut original, 200, 0, "owned".to_owned())
            .expect("target-owned record");
        crate::quest_records::set(&mut original, 900, 0, "redirected".to_owned())
            .expect("redirected helper record");

        let accepted = select_choice(
            original,
            &producer,
            1,
            ACCEPT_CHOICE_ID,
            curve(),
            &definitions,
            scripts(),
            1_000,
        )
        .expect("accept producer");

        assert_eq!(progress(&accepted.player, 200), QuestProgress::NotStarted);
        assert!(
            !accepted
                .player
                .quests
                .iter()
                .any(|entry| entry.quest_id == 200)
        );
        assert_eq!(crate::quest_records::get(&accepted.player, 200, 0), None);
        assert_eq!(
            crate::quest_records::get(&accepted.player, 900, 0),
            Some("redirected")
        );
    }

    #[test]
    fn quest_state_action_start_replaces_stale_progress_and_timestamps() {
        let definitions = item_definitions();
        let mut producer = quest(100);
        producer
            .start_actions
            .quest_state_actions
            .push(QuestStateAction {
                quest_id: 200,
                state: QuestStateActionState::Started,
            });
        let mut original = player(Vec::new(), 1);
        original.quests.push(PlayerQuest {
            quest_id: 200,
            status: QuestStatus::Completed as i32,
            mob_progress: vec![QuestMobProgress {
                mob_id: MOB_A,
                count: 9,
            }],
            accepted_at_unix_ms: 100,
            completed_at_unix_ms: 200,
            dialogue_step: 3,
            completion_quiz_passed: true,
        });
        crate::quest_records::set(&mut original, 200, 0, "preserved".to_owned())
            .expect("target record");

        let accepted = select_choice(
            original,
            &producer,
            1,
            ACCEPT_CHOICE_ID,
            curve(),
            &definitions,
            scripts(),
            1_000,
        )
        .expect("accept producer");
        let target = accepted
            .player
            .quests
            .iter()
            .find(|entry| entry.quest_id == 200)
            .expect("started target");

        assert_eq!(target.status, QuestStatus::Started as i32);
        assert!(target.mob_progress.is_empty());
        assert_eq!(target.accepted_at_unix_ms, 1_000);
        assert_eq!(target.completed_at_unix_ms, 0);
        assert_eq!(
            crate::quest_records::get(&accepted.player, 200, 0),
            Some("preserved")
        );
    }

    #[test]
    fn quest_state_action_completion_preserves_only_a_valid_acceptance_timestamp() {
        let definitions = item_definitions();
        let mut producer = quest(100);
        producer.start_actions.quest_state_actions = vec![
            QuestStateAction {
                quest_id: 200,
                state: QuestStateActionState::Completed,
            },
            QuestStateAction {
                quest_id: 300,
                state: QuestStateActionState::Completed,
            },
            QuestStateAction {
                quest_id: 400,
                state: QuestStateActionState::Completed,
            },
        ];
        let mut original = player(Vec::new(), 1);
        original.quests.push(PlayerQuest {
            quest_id: 200,
            status: QuestStatus::Started as i32,
            mob_progress: vec![QuestMobProgress {
                mob_id: MOB_A,
                count: 9,
            }],
            accepted_at_unix_ms: 123,
            completed_at_unix_ms: 0,
            dialogue_step: 3,
            completion_quiz_passed: true,
        });
        original.quests.push(PlayerQuest {
            quest_id: 300,
            status: QuestStatus::Completed as i32,
            mob_progress: vec![QuestMobProgress {
                mob_id: MOB_A,
                count: 5,
            }],
            accepted_at_unix_ms: 456,
            completed_at_unix_ms: 789,
            dialogue_step: 3,
            completion_quiz_passed: true,
        });

        let accepted = select_choice(
            original,
            &producer,
            1,
            ACCEPT_CHOICE_ID,
            curve(),
            &definitions,
            scripts(),
            1_000,
        )
        .expect("accept producer");
        let existing = accepted
            .player
            .quests
            .iter()
            .find(|entry| entry.quest_id == 200)
            .expect("completed existing target");
        let completed = accepted
            .player
            .quests
            .iter()
            .find(|entry| entry.quest_id == 300)
            .expect("replaced completed target");
        let new = accepted
            .player
            .quests
            .iter()
            .find(|entry| entry.quest_id == 400)
            .expect("completed new target");

        assert_eq!(existing.status, QuestStatus::Completed as i32);
        assert!(existing.mob_progress.is_empty());
        assert_eq!(existing.accepted_at_unix_ms, 123);
        assert_eq!(existing.completed_at_unix_ms, 1_000);
        assert!(completed.mob_progress.is_empty());
        assert_eq!(completed.accepted_at_unix_ms, 456);
        assert_eq!(completed.completed_at_unix_ms, 1_000);
        assert_eq!(new.status, QuestStatus::Completed as i32);
        assert_eq!(new.accepted_at_unix_ms, 1_000);
        assert_eq!(new.completed_at_unix_ms, 1_000);
    }

    #[test]
    fn quest_state_action_does_not_run_target_rewards() {
        let definitions = item_definitions();
        let mut producer = quest(100);
        producer
            .start_actions
            .quest_state_actions
            .push(QuestStateAction {
                quest_id: 200,
                state: QuestStateActionState::Completed,
            });
        let mut target = quest(200);
        target.completion_actions.money = 500;
        target.completion.script = Some("target_script_must_not_run".to_owned());
        target
            .completion_actions
            .selectable_items
            .push(QuestSelectableItemReward {
                item_id: ITEM_A,
                count: 1,
                expiration: None,
                eligibility: QuestRewardEligibility::default(),
            });

        let accepted = select_choice_in_environment(
            player(Vec::new(), 1),
            &producer,
            &[&producer, &target],
            1,
            ACCEPT_CHOICE_ID,
            curve(),
            &definitions,
            scripts(),
            environment(1_000),
        )
        .expect("accept producer");

        assert_eq!(progress(&accepted.player, 100), QuestProgress::Started);
        assert_eq!(progress(&accepted.player, 200), QuestProgress::Completed);
        assert_eq!(accepted.player.mesos, 0);
    }

    #[test]
    fn quest_state_action_rolls_back_when_a_later_resource_action_fails() {
        let definitions = item_definitions();
        let mut producer = quest(100);
        producer.start_actions.quest_state_actions = vec![QuestStateAction {
            quest_id: 200,
            state: QuestStateActionState::Completed,
        }];
        producer.start_actions.money = -1;
        let mut original = player(Vec::new(), 1);
        original.quests.push(PlayerQuest {
            accepted_at_unix_ms: 100,
            ..player_quest(200, QuestStatus::Started)
        });

        assert!(
            select_choice(
                original.clone(),
                &producer,
                1,
                ACCEPT_CHOICE_ID,
                curve(),
                &definitions,
                scripts(),
                1_000,
            )
            .is_err()
        );
        assert_eq!(progress(&original, 100), QuestProgress::NotStarted);
        assert_eq!(progress(&original, 200), QuestProgress::Started);
    }

    #[test]
    fn script_quest_status_action_uses_the_same_state_transform() {
        let definitions = item_definitions();
        let mut producer = quest(100);
        producer.start.script = Some("set_target_started".to_owned());
        let target = quest(200);
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("quest-scripts.toml");
        fs::write(
            &path,
            r#"
                [[scripts]]
                name = "set_target_started"

                [[scripts.actions]]
                type = "set_quest_status"
                quest_id = 200
                state = "started"
            "#,
        )
        .expect("write quest script");
        let scripts =
            QuestScriptCatalog::load(&path, [&producer, &target], &BTreeSet::new(), &definitions)
                .expect("quest status script catalog");
        let mut original = player(Vec::new(), 1);
        original.quests.push(PlayerQuest {
            quest_id: 200,
            status: QuestStatus::Completed as i32,
            mob_progress: vec![QuestMobProgress {
                mob_id: MOB_A,
                count: 9,
            }],
            accepted_at_unix_ms: 100,
            completed_at_unix_ms: 200,
            dialogue_step: 3,
            completion_quiz_passed: true,
        });

        let accepted = select_choice(
            original,
            &producer,
            1,
            ACCEPT_CHOICE_ID,
            curve(),
            &definitions,
            &scripts,
            1_000,
        )
        .expect("accept scripted producer");
        let target = accepted
            .player
            .quests
            .iter()
            .find(|entry| entry.quest_id == 200)
            .expect("script-started target");

        assert_eq!(target.status, QuestStatus::Started as i32);
        assert!(target.mob_progress.is_empty());
        assert_eq!(target.accepted_at_unix_ms, 1_000);
        assert_eq!(target.completed_at_unix_ms, 0);
    }

    #[test]
    fn relative_item_expiration_and_completion_use_action_execution_time() {
        let definitions = item_definitions();
        let mut quest = quest(100);
        quest.completion_actions.fixed_items.push(QuestItemDelta {
            item_id: ITEM_A,
            count: 1,
            expiration: Some(QuestItemExpiration::RelativeMilliseconds(3_600_000)),
        });
        let mut player = player(Vec::new(), 1);
        player.quests.push(PlayerQuest {
            accepted_at_unix_ms: 100,
            ..player_quest(quest.id, QuestStatus::Started)
        });

        let completed = select_choice(
            player,
            &quest,
            2,
            COMPLETE_CHOICE_ID,
            curve(),
            &definitions,
            scripts(),
            10_000,
        )
        .expect("complete with a relative expiration reward");

        assert_eq!(completed.player.quests[0].completed_at_unix_ms, 10_000);
        assert_eq!(
            completed.player.inventory.expect("inventory").stacks[0].expires_at_unix_ms,
            3_610_000
        );
    }

    #[test]
    fn expired_absolute_reward_does_not_block_quest_completion() {
        let definitions = item_definitions();
        let mut quest = quest(100);
        quest.completion_actions.fixed_items.push(QuestItemDelta {
            item_id: ITEM_A,
            count: 1,
            expiration: Some(QuestItemExpiration::AbsoluteUnixMilliseconds(200)),
        });
        quest.completion_actions.money = 50;
        let mut player = player(Vec::new(), 1);
        player
            .quests
            .push(player_quest(quest.id, QuestStatus::Started));

        let completed = select_choice(
            player,
            &quest,
            2,
            COMPLETE_CHOICE_ID,
            curve(),
            &definitions,
            scripts(),
            200,
        )
        .expect("an expired item grant is skipped");

        assert_eq!(
            progress(&completed.player, quest.id),
            QuestProgress::Completed
        );
        assert_eq!(completed.player.mesos, 50);
        assert_eq!(item_count(&completed.player, ITEM_A), 0);
    }

    #[test]
    fn relative_item_expiration_overflow_is_atomic() {
        let definitions = item_definitions();
        let mut quest = quest(100);
        quest.completion_actions.fixed_items.push(QuestItemDelta {
            item_id: ITEM_A,
            count: 1,
            expiration: Some(QuestItemExpiration::RelativeMilliseconds(2)),
        });
        let mut player = player(Vec::new(), 1);
        player
            .quests
            .push(player_quest(quest.id, QuestStatus::Started));

        assert!(matches!(
            select_choice(
                player,
                &quest,
                2,
                COMPLETE_CHOICE_ID,
                curve(),
                &definitions,
                scripts(),
                u64::MAX - 1,
            ),
            Err(QuestRuleError::ActionOverflow { quest_id: 100 })
        ));
    }

    #[test]
    fn failed_acceptance_preflight_returns_no_mutated_state() {
        let definitions = item_definitions();
        let mut quest = quest(100);
        quest.start_actions.money = -101;
        let mut player = player(Vec::new(), 1);
        player.mesos = 100;
        let unchanged = player.clone();

        assert!(matches!(
            select_choice(
                player,
                &quest,
                1,
                ACCEPT_CHOICE_ID,
                curve(),
                &definitions,
                scripts(),
                100,
            ),
            Err(QuestRuleError::Item(ItemRuleError::InsufficientMesos))
        ));
        assert_eq!(unchanged.mesos, 100);
        assert!(unchanged.quests.is_empty());
    }

    #[test]
    fn start_question_answers_control_atomic_acceptance_and_page_order() {
        let definitions = item_definitions();
        let mut quest = quest(100);
        configure_start_question(&mut quest);
        quest.start.script = Some("question_start".to_owned());
        quest.start_actions.money = 5;
        let scripts = script_catalog(
            r#"
                [[scripts]]
                name = "question_start"
                result_pages = ["Script result"]
            "#,
            &quest,
            &definitions,
        );
        let initial =
            open_start_question(player(Vec::new(), 1), &quest, &definitions, &scripts, 100);

        let wrong = select_choice(
            initial.clone(),
            &quest,
            1,
            answer_choice_id(QuestQuestionPhase::Start, 0, 0).expect("first start answer"),
            curve(),
            &definitions,
            &scripts,
            100,
        )
        .expect("authored wrong-answer result");
        assert!(!wrong.changed);
        assert_eq!(wrong.pages, vec!["Wrong answer"]);
        assert_eq!(wrong.player, initial);

        let correct = select_choice(
            initial,
            &quest,
            1,
            answer_choice_id(QuestQuestionPhase::Start, 0, 1).expect("second start answer"),
            curve(),
            &definitions,
            &scripts,
            100,
        )
        .expect("correct start answer");
        assert!(correct.changed);
        assert_eq!(progress(&correct.player, quest.id), QuestProgress::Started);
        assert_eq!(correct.player.mesos, 5);
        assert_eq!(
            correct.pages,
            vec!["Question continuation", "Accepted", "Script result"]
        );
    }

    #[test]
    fn quest_10000_style_start_question_waits_for_an_explicit_decision() {
        let definitions = item_definitions();
        let mut quest = quest(10_000);
        configure_start_question(&mut quest);
        quest.dialogue.has_start_decision = true;
        let pending =
            open_start_question(player(Vec::new(), 1), &quest, &definitions, scripts(), 100);

        let passed = select_choice(
            pending,
            &quest,
            1,
            answer_choice_id(QuestQuestionPhase::Start, 0, 1).expect("correct start answer"),
            curve(),
            &definitions,
            scripts(),
            100,
        )
        .expect("pass start question");
        assert_eq!(passed.pages, vec!["Question continuation"]);
        assert_eq!(
            progress(&passed.player, quest.id),
            QuestProgress::NotStarted
        );
        assert_eq!(
            passed.player.quests[0].status,
            QuestStatus::Unspecified as i32
        );
        assert_eq!(passed.player.quests[0].dialogue_step, 2);
        assert_eq!(
            passed.next_interaction,
            Some(QuestNextInteraction::StartDecision)
        );

        let mut unavailable_quest = quest.clone();
        unavailable_quest.start.minimum_level = Some(2);
        let resumed = begin_start_question(
            passed.player.clone(),
            &unavailable_quest,
            1,
            &definitions,
            scripts(),
            environment(100),
        )
        .expect("resume pending decision");
        assert!(!resumed.changed);
        assert_eq!(resumed.pages, vec!["Question continuation"]);
        assert_eq!(
            resumed.next_interaction,
            Some(QuestNextInteraction::StartDecision)
        );

        let declined = select_choice(
            passed.player.clone(),
            &unavailable_quest,
            1,
            DECLINE_CHOICE_ID,
            curve(),
            &definitions,
            scripts(),
            100,
        )
        .expect("decline after passing question");
        assert!(declined.changed);
        assert_eq!(declined.pages, vec!["Declined"]);
        assert!(declined.player.quests.is_empty());

        let accepted = select_choice(
            passed.player,
            &quest,
            1,
            ACCEPT_CHOICE_ID,
            curve(),
            &definitions,
            scripts(),
            100,
        )
        .expect("accept after passing question");
        assert_eq!(progress(&accepted.player, quest.id), QuestProgress::Started);
        assert_eq!(accepted.pages, vec!["Accepted"]);
        assert_eq!(accepted.player.quests[0].accepted_at_unix_ms, 100);
    }

    #[test]
    fn one_choice_start_question_without_a_decision_branch_starts_immediately() {
        let definitions = item_definitions();
        let mut quest = quest(100);
        quest.dialogue.start_question = Some(QuestQuestionSequence {
            leading_pages: Vec::new(),
            steps: vec![QuestQuestionStep {
                archive_index: 0,
                prompt: "Continue?".to_owned(),
                choices: vec![QuestChoice {
                    id: 7,
                    label: "Continue".to_owned(),
                }],
                correct_choice_id: 7,
                continuation_pages: Vec::new(),
                failure_pages: HashMap::new(),
            }],
            trailing_pages: vec!["Question continuation".to_owned()],
        });
        let pending =
            open_start_question(player(Vec::new(), 1), &quest, &definitions, scripts(), 100);

        let accepted = select_choice(
            pending,
            &quest,
            1,
            answer_choice_id(QuestQuestionPhase::Start, 0, 0).expect("only start answer"),
            curve(),
            &definitions,
            scripts(),
            100,
        )
        .expect("one-choice question starts immediately");

        assert_eq!(progress(&accepted.player, quest.id), QuestProgress::Started);
        assert_eq!(accepted.pages, vec!["Question continuation", "Accepted"]);
    }

    #[test]
    fn replayed_answer_id_cannot_answer_the_next_identical_step() {
        let definitions = item_definitions();
        let mut quest = quest(100);
        let choices = vec![
            QuestChoice {
                id: 7,
                label: "Wrong".to_owned(),
            },
            QuestChoice {
                id: 42,
                label: "Right".to_owned(),
            },
        ];
        quest.dialogue.start_question = Some(QuestQuestionSequence {
            leading_pages: Vec::new(),
            steps: vec![
                QuestQuestionStep {
                    archive_index: 1,
                    prompt: "First".to_owned(),
                    choices: choices.clone(),
                    correct_choice_id: 42,
                    continuation_pages: vec!["Next".to_owned()],
                    failure_pages: HashMap::from([(7, vec!["Wrong".to_owned()])]),
                },
                QuestQuestionStep {
                    archive_index: 3,
                    prompt: "Second".to_owned(),
                    choices,
                    correct_choice_id: 42,
                    continuation_pages: Vec::new(),
                    failure_pages: HashMap::from([(7, vec!["Wrong".to_owned()])]),
                },
            ],
            trailing_pages: Vec::new(),
        });
        let pending =
            open_start_question(player(Vec::new(), 1), &quest, &definitions, scripts(), 100);
        let first_answer =
            answer_choice_id(QuestQuestionPhase::Start, 0, 1).expect("first-step answer");
        let second_answer =
            answer_choice_id(QuestQuestionPhase::Start, 1, 1).expect("second-step answer");
        assert_ne!(first_answer, second_answer);

        let second_step = select_choice(
            pending,
            &quest,
            1,
            first_answer,
            curve(),
            &definitions,
            scripts(),
            100,
        )
        .expect("advance to second question");
        assert_eq!(second_step.player.quests[0].dialogue_step, 2);
        assert!(matches!(
            select_choice(
                second_step.player.clone(),
                &quest,
                1,
                first_answer,
                curve(),
                &definitions,
                scripts(),
                100,
            ),
            Err(QuestRuleError::InvalidChoice { choice_id }) if choice_id == first_answer
        ));

        let accepted = select_choice(
            second_step.player,
            &quest,
            1,
            second_answer,
            curve(),
            &definitions,
            scripts(),
            100,
        )
        .expect("answer authoritative second step");
        assert_eq!(progress(&accepted.player, quest.id), QuestProgress::Started);
    }

    #[test]
    fn answer_choice_encoding_is_checked_and_disjoint_from_other_choice_ranges() {
        let start = answer_choice_id(QuestQuestionPhase::Start, 0, 0).expect("start answer");
        let completion =
            answer_choice_id(QuestQuestionPhase::Completion, 0, 0).expect("completion answer");

        assert_ne!(start, completion);
        assert!(start > COMPLETE_CHOICE_ID && start < RESTORE_ITEMS_CHOICE_ID);
        assert!(completion > start && completion < RESTORE_ITEMS_CHOICE_ID);
        assert!(answer_choice_id(QuestQuestionPhase::Start, 0, 0x1_0000).is_none());
        assert!(answer_choice_id(QuestQuestionPhase::Completion, usize::MAX, 0).is_none());
        assert!(
            selectable_reward_choice_id(0).expect("first reward choice") > RESTORE_ITEMS_CHOICE_ID
        );
    }

    #[test]
    fn forged_and_unavailable_start_answers_do_not_start_the_quest() {
        let definitions = item_definitions();
        let mut quest = quest(100);
        configure_start_question(&mut quest);
        let player = player(Vec::new(), 1);

        for choice_id in [
            ACCEPT_CHOICE_ID,
            DECLINE_CHOICE_ID,
            answer_choice_id(QuestQuestionPhase::Start, 0, 2).expect("third start answer"),
            RESTORE_ITEMS_CHOICE_ID,
            selectable_reward_choice_id(0).expect("selectable reward choice"),
        ] {
            assert!(matches!(
                select_choice(
                    player.clone(),
                    &quest,
                    1,
                    choice_id,
                    curve(),
                    &definitions,
                    scripts(),
                    100,
                ),
                Err(QuestRuleError::InvalidChoice { choice_id: rejected }) if rejected == choice_id
            ));
        }
        assert_eq!(progress(&player, quest.id), QuestProgress::NotStarted);

        let pending = open_start_question(player.clone(), &quest, &definitions, scripts(), 100);
        quest.start.minimum_level = Some(2);
        assert!(matches!(
            select_choice(
                pending.clone(),
                &quest,
                1,
                answer_choice_id(QuestQuestionPhase::Start, 0, 0).expect("first start answer"),
                curve(),
                &definitions,
                scripts(),
                100,
            ),
            Err(QuestRuleError::Unavailable { quest_id: 100 })
        ));
        assert!(matches!(
            select_choice(
                pending,
                &quest,
                1,
                answer_choice_id(QuestQuestionPhase::Start, 0, 1).expect("second start answer"),
                curve(),
                &definitions,
                scripts(),
                100,
            ),
            Err(QuestRuleError::Unavailable { quest_id: 100 })
        ));
        assert_eq!(progress(&player, quest.id), QuestProgress::NotStarted);

        let mut completed = player.clone();
        completed
            .quests
            .push(player_quest(quest.id, QuestStatus::Completed));
        assert!(matches!(
            select_choice(
                completed,
                &quest,
                1,
                answer_choice_id(QuestQuestionPhase::Start, 0, 2).expect("third start answer"),
                curve(),
                &definitions,
                scripts(),
                100,
            ),
            Err(QuestRuleError::Unavailable { quest_id: 100 })
        ));
    }

    #[test]
    fn ordinary_accept_decline_and_question_automation_semantics_are_preserved() {
        let definitions = item_definitions();
        let mut quest = quest(100);
        quest.start_actions.npc_animation_action = Some("accept".to_owned());
        let initial = player(Vec::new(), 1);

        let declined = select_choice(
            initial.clone(),
            &quest,
            1,
            DECLINE_CHOICE_ID,
            curve(),
            &definitions,
            scripts(),
            100,
        )
        .expect("decline ordinary quest");
        assert!(!declined.changed);
        assert_eq!(declined.npc_animation_action, None);
        assert_eq!(declined.pages, vec!["Declined"]);
        assert_eq!(declined.player, initial);

        let accepted = select_choice(
            initial.clone(),
            &quest,
            1,
            ACCEPT_CHOICE_ID,
            curve(),
            &definitions,
            scripts(),
            100,
        )
        .expect("accept ordinary quest");
        assert_eq!(progress(&accepted.player, quest.id), QuestProgress::Started);
        assert_eq!(accepted.pages, vec!["Accepted"]);
        assert_eq!(accepted.npc_animation_action.as_deref(), Some("accept"));

        let mut automatic_question = quest;
        configure_start_question(&mut automatic_question);
        automatic_question.info.auto_accept = true;
        let advanced = advance_automatic_quests(
            initial,
            [&automatic_question],
            curve(),
            &definitions,
            scripts(),
            100,
        );
        assert!(!advanced.changed);
        assert_eq!(
            progress(&advanced.player, automatic_question.id),
            QuestProgress::NotStarted
        );
    }

    #[test]
    fn script_and_wz_actions_share_atomic_quest_selection() {
        let definitions = item_definitions();
        let mut quest = quest(100);
        quest.start.script = Some("replace_item".to_owned());
        quest.start_actions.fixed_items.push(QuestItemDelta {
            item_id: ITEM_B,
            count: 1,
            expiration: None,
        });
        quest.start_actions.npc_animation_action = Some("scripted".to_owned());
        let scripts = script_catalog(
            r#"
                [[scripts]]
                name = "replace_item"
                result_pages = ["Script result"]

                [[scripts.actions]]
                type = "item_delta"
                item_id = 4000000
                delta = -1

                [[scripts.actions]]
                type = "mesos"
                delta = -101
            "#,
            &quest,
            &definitions,
        );
        let mut insufficient = player(vec![(ITEM_A, 1)], 1);
        insufficient.mesos = 100;
        let unchanged = insufficient.clone();

        assert!(matches!(
            select_choice(
                insufficient,
                &quest,
                1,
                ACCEPT_CHOICE_ID,
                curve(),
                &definitions,
                &scripts,
                100,
            ),
            Err(QuestRuleError::Item(ItemRuleError::InsufficientMesos))
        ));
        assert_eq!(item_count(&unchanged, ITEM_A), 1);
        assert_eq!(item_count(&unchanged, ITEM_B), 0);
        assert_eq!(unchanged.mesos, 100);
        assert!(unchanged.quests.is_empty());

        let mut sufficient = unchanged;
        sufficient.mesos = 101;
        let selected = select_choice(
            sufficient,
            &quest,
            1,
            ACCEPT_CHOICE_ID,
            curve(),
            &definitions,
            &scripts,
            100,
        )
        .expect("combined script and WZ actions");
        assert_eq!(item_count(&selected.player, ITEM_A), 0);
        assert_eq!(item_count(&selected.player, ITEM_B), 1);
        assert_eq!(selected.player.mesos, 0);
        assert_eq!(selected.pages, vec!["Accepted", "Script result"]);
        assert_eq!(selected.npc_animation_action.as_deref(), Some("scripted"));
        assert_eq!(progress(&selected.player, quest.id), QuestProgress::Started);
    }

    #[test]
    fn mob_kills_cap_across_every_matching_active_quest() {
        let mut first = quest(100);
        first.completion.mobs.push(QuestMobObjective {
            mob_id: MOB_A,
            count: 2,
        });
        let mut second = quest(200);
        second.completion.mobs.push(QuestMobObjective {
            mob_id: MOB_A,
            count: 5,
        });
        let mut inactive = quest(300);
        inactive.completion.mobs.push(QuestMobObjective {
            mob_id: MOB_A,
            count: 10,
        });
        let mut player = player(Vec::new(), 1);
        player.quests = vec![
            player_quest(100, QuestStatus::Started),
            player_quest(200, QuestStatus::Started),
            player_quest(300, QuestStatus::Completed),
        ];
        let quests = vec![first, second, inactive];

        let first = record_mob_kills(
            player,
            &[(MOB_A, None), (MOB_A, None), (MOB_A, None)],
            &quests,
        );
        assert_eq!(first.changed_quest_ids, vec![100, 200]);
        assert_eq!(mob_count(&first.player, 100, MOB_A), 2);
        assert_eq!(mob_count(&first.player, 200, MOB_A), 3);
        assert_eq!(mob_count(&first.player, 300, MOB_A), 0);
        let capped = record_mob_kills(first.player, &[(MOB_A, None); 10], &quests);
        assert_eq!(capped.changed_quest_ids, vec![200]);
        assert_eq!(mob_count(&capped.player, 100, MOB_A), 2);
        assert_eq!(mob_count(&capped.player, 200, MOB_A), 5);
        let unchanged = capped.player.clone();
        let empty = record_mob_kills(capped.player, &[], &quests);
        assert!(empty.changed_quest_ids.is_empty());
        assert_eq!(empty.player, unchanged);
    }

    #[test]
    fn selected_skill_mob_credit_requires_exact_authoritative_provenance() {
        const REQUIRED_SKILL: u32 = 1_001_004;
        let mut ordinary = quest(100);
        ordinary.completion.mobs.push(QuestMobObjective {
            mob_id: MOB_A,
            count: 5,
        });
        let mut selected = quest(200);
        selected.completion.mobs.push(QuestMobObjective {
            mob_id: MOB_A,
            count: 3,
        });
        selected.info.selected_skill = Some(QuestSelectedSkill {
            id: NonZeroU32::new(REQUIRED_SKILL).expect("positive skill"),
            name: Some("Power Strike".to_owned()),
        });
        let quests = vec![ordinary, selected];
        let mut player = player(Vec::new(), 1);
        player.quests = vec![
            player_quest(100, QuestStatus::Started),
            player_quest(200, QuestStatus::Started),
        ];

        let credited = record_mob_kills(
            player,
            &[
                (MOB_A, None),
                (MOB_A, Some(REQUIRED_SKILL + 1)),
                (MOB_A, Some(REQUIRED_SKILL)),
                (MOB_A, Some(REQUIRED_SKILL)),
            ],
            &quests,
        );

        assert_eq!(mob_count(&credited.player, 100, MOB_A), 4);
        assert_eq!(mob_count(&credited.player, 200, MOB_A), 2);
        assert_eq!(credited.changed_quest_ids, vec![100, 200]);
        let capped = record_mob_kills(
            credited.player,
            &[(MOB_A, Some(REQUIRED_SKILL)); 10],
            &quests,
        );
        assert_eq!(mob_count(&capped.player, 100, MOB_A), 5);
        assert_eq!(mob_count(&capped.player, 200, MOB_A), 3);
        let readiness = completion_readiness(&capped.player, &quests[1], &[], scripts());
        assert!(readiness.objectives[0].label.contains("Power Strike"));
        assert!(readiness.objectives[0].label.contains("1001004"));
    }

    #[test]
    fn incomplete_completion_rejects_every_reward() {
        let definitions = item_definitions();
        let mut quest = quest(100);
        quest.completion.items.push(item_requirement(ITEM_A, 2));
        quest.completion_actions = reward_actions();
        quest.completion_actions.npc_animation_action = Some("quest".to_owned());
        let mut player = player(vec![(ITEM_A, 1)], 4);
        player.quests.push(player_quest(100, QuestStatus::Started));
        let unchanged = player.clone();

        assert!(matches!(
            select_choice(
                player,
                &quest,
                2,
                COMPLETE_CHOICE_ID,
                curve(),
                &definitions,
                scripts(),
                200,
            ),
            Err(QuestRuleError::ObjectivesIncomplete { quest_id: 100 })
        ));
        assert_eq!(unchanged.mesos, 0);
        assert_eq!(unchanged.stats.expect("stats").fame, 0);
        assert_eq!(unchanged.quests[0].status, QuestStatus::Started as i32);
    }

    #[test]
    fn manual_completion_rejects_a_quest_at_its_deadline() {
        let definitions = item_definitions();
        let mut quest = quest(100);
        quest.info.time_limit_ms = Some(100);
        quest.completion_actions.money = 100;
        let mut player = player(Vec::new(), 1);
        player.quests.push(PlayerQuest {
            accepted_at_unix_ms: 100,
            ..player_quest(quest.id, QuestStatus::Started)
        });

        assert!(matches!(
            select_choice(
                player,
                &quest,
                2,
                COMPLETE_CHOICE_ID,
                curve(),
                &definitions,
                scripts(),
                200,
            ),
            Err(QuestRuleError::Expired { quest_id: 100 })
        ));
    }

    #[test]
    fn reward_capacity_failure_is_atomic() {
        let definitions = item_definitions();
        let mut quest = quest(100);
        quest.completion_actions.fixed_items.push(QuestItemDelta {
            item_id: ITEM_B,
            count: 1,
            expiration: None,
        });
        quest.completion_actions.money = 100;
        quest.completion_actions.npc_animation_action = Some("quest".to_owned());
        let mut player = player(vec![(ITEM_A, 10)], 1);
        player.quests.push(player_quest(100, QuestStatus::Started));
        let unchanged = player.clone();

        assert!(matches!(
            select_choice(
                player,
                &quest,
                2,
                COMPLETE_CHOICE_ID,
                curve(),
                &definitions,
                scripts(),
                200,
            ),
            Err(QuestRuleError::Item(ItemRuleError::InventoryFull))
        ));
        assert_eq!(item_count(&unchanged, ITEM_A), 10);
        assert_eq!(unchanged.mesos, 0);
        assert_eq!(unchanged.quests[0].status, QuestStatus::Started as i32);
    }

    #[test]
    fn deadline_separated_reward_stacks_are_preflighted_atomically() {
        let definitions = item_definitions();
        let mut quest = quest(100);
        quest.completion_actions.fixed_items = vec![
            QuestItemDelta {
                item_id: ITEM_A,
                count: 1,
                expiration: Some(QuestItemExpiration::AbsoluteUnixMilliseconds(10_000)),
            },
            QuestItemDelta {
                item_id: ITEM_A,
                count: 1,
                expiration: Some(QuestItemExpiration::AbsoluteUnixMilliseconds(20_000)),
            },
        ];
        quest.completion_actions.money = 100;
        let mut player = player(Vec::new(), 1);
        player
            .quests
            .push(player_quest(quest.id, QuestStatus::Started));
        let unchanged = player.clone();

        assert!(matches!(
            select_choice(
                player,
                &quest,
                2,
                COMPLETE_CHOICE_ID,
                curve(),
                &definitions,
                scripts(),
                200,
            ),
            Err(QuestRuleError::Item(ItemRuleError::InventoryFull))
        ));
        assert_eq!(item_count(&unchanged, ITEM_A), 0);
        assert_eq!(unchanged.mesos, 0);
        assert_eq!(progress(&unchanged, quest.id), QuestProgress::Started);
    }

    #[test]
    fn objective_consumption_creates_reward_room() {
        let definitions = item_definitions();
        let mut quest = quest(100);
        quest.completion.items.push(item_requirement(ITEM_A, 1));
        quest.completion_actions.fixed_items = vec![
            QuestItemDelta {
                item_id: ITEM_A,
                count: -1,
                expiration: None,
            },
            QuestItemDelta {
                item_id: ITEM_B,
                count: 1,
                expiration: None,
            },
        ];
        let mut player = player(vec![(ITEM_A, 1)], 1);
        player.quests.push(player_quest(100, QuestStatus::Started));

        let completed = select_choice(
            player,
            &quest,
            2,
            COMPLETE_CHOICE_ID,
            curve(),
            &definitions,
            scripts(),
            200,
        )
        .expect("complete after removal");

        assert_eq!(item_count(&completed.player, ITEM_A), 0);
        assert_eq!(item_count(&completed.player, ITEM_B), 1);
        assert_eq!(progress(&completed.player, 100), QuestProgress::Completed);
    }

    #[test]
    fn completion_grants_mesos_fame_and_experience_once() {
        let definitions = item_definitions();
        let mut quest = quest(100);
        quest.completion_actions = reward_actions();
        quest.completion_actions.npc_animation_action = Some("quest".to_owned());
        let mut player = player(Vec::new(), 1);
        player.quests.push(player_quest(100, QuestStatus::Started));

        let completed = select_choice(
            player,
            &quest,
            2,
            COMPLETE_CHOICE_ID,
            curve(),
            &definitions,
            scripts(),
            200,
        )
        .expect("complete quest");
        assert_eq!(completed.player.mesos, 100);
        let stats = completed.player.stats.as_ref().expect("stats");
        assert_eq!(stats.fame, 2);
        assert_eq!(stats.experience, 5);
        assert_eq!(completed.player.quests[0].completed_at_unix_ms, 200);
        assert_eq!(completed.npc_animation_action.as_deref(), Some("quest"));
        let unchanged = completed.player.clone();

        assert!(matches!(
            select_choice(
                completed.player,
                &quest,
                2,
                COMPLETE_CHOICE_ID,
                curve(),
                &definitions,
                scripts(),
                300,
            ),
            Err(QuestRuleError::Unavailable { quest_id: 100 })
        ));
        assert_eq!(unchanged.mesos, 100);
        assert_eq!(unchanged.stats.expect("stats").experience, 5);
    }

    #[test]
    fn repeat_acceptance_resets_timestamps_and_progress() {
        let definitions = item_definitions();
        let mut quest = quest(100);
        quest.start.repeat = QuestRepeatMetadata {
            interval_ms: Some(100),
            ..QuestRepeatMetadata::default()
        };
        let mut player = player(Vec::new(), 1);
        player.quests.push(PlayerQuest {
            quest_id: 100,
            status: QuestStatus::Completed as i32,
            mob_progress: vec![QuestMobProgress {
                mob_id: MOB_A,
                count: 5,
            }],
            accepted_at_unix_ms: 10,
            completed_at_unix_ms: 100,
            dialogue_step: 0,
            completion_quiz_passed: false,
        });

        assert!(!is_available(&player, &quest, &definitions, scripts(), 199));
        assert!(is_available(&player, &quest, &definitions, scripts(), 200));
        let accepted = select_choice(
            player,
            &quest,
            1,
            ACCEPT_CHOICE_ID,
            curve(),
            &definitions,
            scripts(),
            200,
        )
        .expect("repeat quest");
        let entry = &accepted.player.quests[0];
        assert_eq!(entry.status, QuestStatus::Started as i32);
        assert_eq!(entry.accepted_at_unix_ms, 200);
        assert_eq!(entry.completed_at_unix_ms, 0);
        assert!(entry.mob_progress.is_empty());
    }

    #[test]
    fn zero_interval_allows_immediate_repeat_acceptance() {
        let definitions = item_definitions();
        let mut quest = quest(100);
        quest.start.repeat.interval_ms = Some(0);
        let mut player = player(Vec::new(), 1);
        player.quests.push(PlayerQuest {
            quest_id: 100,
            status: QuestStatus::Completed as i32,
            completed_at_unix_ms: 100,
            ..PlayerQuest::default()
        });

        assert!(is_available(&player, &quest, &definitions, scripts(), 100));
    }

    #[test]
    fn weighted_selection_is_deterministic_and_grants_one_alternative() {
        let rewards = vec![
            QuestWeightedItem {
                item_id: ITEM_B,
                count: 1,
                expiration: None,
                weight: 1,
                eligibility: Default::default(),
            },
            QuestWeightedItem {
                item_id: ITEM_C,
                count: 1,
                expiration: None,
                weight: 3,
                eligibility: Default::default(),
            },
        ];
        let selected = select_weighted_item("player", 100, 123, &rewards).expect("selection");
        assert_eq!(
            select_weighted_item("player", 100, 123, &rewards),
            Some(selected)
        );
        let definitions = item_definitions();
        let mut quest = quest(100);
        quest.completion_actions.weighted_items = rewards;
        let mut player = player(Vec::new(), 2);
        player.quests.push(PlayerQuest {
            accepted_at_unix_ms: 123,
            ..player_quest(100, QuestStatus::Started)
        });

        let completed = select_choice(
            player,
            &quest,
            2,
            COMPLETE_CHOICE_ID,
            curve(),
            &definitions,
            scripts(),
            200,
        )
        .expect("weighted reward");

        assert_eq!(
            item_count(&completed.player, ITEM_B) + item_count(&completed.player, ITEM_C),
            1
        );
    }

    #[test]
    fn weighted_and_selectable_rewards_preserve_expiration() {
        let definitions = item_definitions();
        let mut weighted = quest(100);
        weighted.completion_actions.weighted_items = vec![QuestWeightedItem {
            item_id: ITEM_A,
            count: 1,
            expiration: Some(QuestItemExpiration::RelativeMilliseconds(1_000)),
            weight: 1,
            eligibility: QuestRewardEligibility::default(),
        }];
        let mut weighted_player = player(Vec::new(), 1);
        weighted_player
            .quests
            .push(player_quest(weighted.id, QuestStatus::Started));
        let weighted = select_choice(
            weighted_player,
            &weighted,
            2,
            COMPLETE_CHOICE_ID,
            curve(),
            &definitions,
            scripts(),
            200,
        )
        .expect("weighted expiring reward");
        assert_eq!(
            weighted.player.inventory.expect("inventory").stacks[0].expires_at_unix_ms,
            1_200
        );

        let mut selectable = quest(200);
        selectable
            .completion_actions
            .selectable_items
            .push(QuestSelectableItemReward {
                item_id: ITEM_A,
                count: 1,
                expiration: Some(QuestItemExpiration::AbsoluteUnixMilliseconds(5_000)),
                eligibility: QuestRewardEligibility::default(),
            });
        let mut player = player(Vec::new(), 1);
        player
            .quests
            .push(player_quest(selectable.id, QuestStatus::Started));
        let choice = eligible_selectable_reward_choices(&player, &selectable)[0].0;
        let selected = select_choice(
            player,
            &selectable,
            2,
            choice,
            curve(),
            &definitions,
            scripts(),
            200,
        )
        .expect("selectable expiring reward");
        assert_eq!(
            selected.player.inventory.expect("inventory").stacks[0].expires_at_unix_ms,
            5_000
        );
    }

    #[test]
    fn item_rewards_filter_by_job_family_and_gender_before_selection() {
        let definitions = item_definitions();
        let warrior_mask = 1 << 1;
        let magician_mask = 1 << 2;
        let mut quest = quest(100);
        quest.completion_actions.conditional_items = vec![
            QuestConditionalItemReward {
                item_id: ITEM_B,
                count: 1,
                expiration: None,
                eligibility: QuestRewardEligibility {
                    job_mask: Some(warrior_mask),
                    gender: Some(QuestRewardGender::Male),
                },
            },
            QuestConditionalItemReward {
                item_id: ITEM_C,
                count: 1,
                expiration: None,
                eligibility: QuestRewardEligibility {
                    job_mask: Some(warrior_mask),
                    gender: Some(QuestRewardGender::Female),
                },
            },
        ];
        quest.completion_actions.weighted_items = vec![
            QuestWeightedItem {
                item_id: ITEM_A,
                count: 1,
                expiration: None,
                weight: 1,
                eligibility: QuestRewardEligibility {
                    job_mask: Some(warrior_mask),
                    gender: None,
                },
            },
            QuestWeightedItem {
                item_id: ITEM_C,
                count: 1,
                expiration: None,
                weight: u32::MAX,
                eligibility: QuestRewardEligibility {
                    job_mask: Some(magician_mask),
                    gender: None,
                },
            },
        ];
        let mut player = player(Vec::new(), 2);
        player.stats.as_mut().expect("stats").job_id = 112;
        player.appearance = Some(CharacterAppearance {
            gender: CharacterGender::Male as i32,
            ..CharacterAppearance::default()
        });
        player.quests.push(player_quest(100, QuestStatus::Started));

        let completed = select_choice(
            player,
            &quest,
            2,
            COMPLETE_CHOICE_ID,
            curve(),
            &definitions,
            scripts(),
            200,
        )
        .expect("filtered quest reward");

        assert_eq!(item_count(&completed.player, ITEM_A), 1);
        assert_eq!(item_count(&completed.player, ITEM_B), 1);
        assert_eq!(item_count(&completed.player, ITEM_C), 0);
    }

    #[test]
    fn selectable_completion_grants_exactly_the_chosen_reward_with_other_actions() {
        let definitions = item_definitions();
        let mut quest = quest(100);
        quest.completion_actions.fixed_items = vec![
            QuestItemDelta {
                item_id: ITEM_A,
                count: -1,
                expiration: None,
            },
            QuestItemDelta {
                item_id: ITEM_A,
                count: 1,
                expiration: None,
            },
        ];
        quest.completion_actions.weighted_items = vec![QuestWeightedItem {
            item_id: ITEM_A,
            count: 1,
            expiration: None,
            weight: 1,
            eligibility: QuestRewardEligibility::default(),
        }];
        quest.completion_actions.selectable_items = vec![
            selectable_reward(ITEM_B, 1, QuestRewardEligibility::default()),
            selectable_reward(ITEM_C, 1, QuestRewardEligibility::default()),
        ];
        quest.completion_actions.money = 100;
        quest.completion_actions.experience = 5;
        quest.completion_actions.fame = 2;
        quest.completion_actions.npc_animation_action = Some("reward".to_owned());
        quest.completion.script = Some("selected_reward_script".to_owned());
        let scripts = script_catalog(
            r#"
                [[scripts]]
                name = "selected_reward_script"

                [[scripts.actions]]
                type = "item_delta"
                item_id = 4000000
                delta = 1

                [[scripts.actions]]
                type = "mesos"
                delta = 7
            "#,
            &quest,
            &definitions,
        );
        let mut player = player(vec![(ITEM_A, 1)], 3);
        player.quests.push(PlayerQuest {
            accepted_at_unix_ms: 123,
            ..player_quest(quest.id, QuestStatus::Started)
        });
        let choices = eligible_selectable_reward_choices(&player, &quest);
        assert_eq!(choices.len(), 2);
        assert_ne!(choices[0].0, ACCEPT_CHOICE_ID);
        assert_ne!(choices[0].0, COMPLETE_CHOICE_ID);
        assert_ne!(choices[0].0, RESTORE_ITEMS_CHOICE_ID);
        assert_ne!(
            Some(choices[0].0),
            answer_choice_id(QuestQuestionPhase::Completion, 0, 0)
        );

        let completed = select_choice(
            player,
            &quest,
            2,
            choices[1].0,
            curve(),
            &definitions,
            &scripts,
            200,
        )
        .expect("selected completion reward");

        assert_eq!(item_count(&completed.player, ITEM_A), 3);
        assert_eq!(item_count(&completed.player, ITEM_B), 0);
        assert_eq!(item_count(&completed.player, ITEM_C), 1);
        assert_eq!(completed.player.mesos, 107);
        let stats = completed.player.stats.as_ref().expect("stats");
        assert_eq!(stats.experience, 5);
        assert_eq!(stats.fame, 2);
        let entry = &completed.player.quests[0];
        assert_eq!(entry.status, QuestStatus::Completed as i32);
        assert_eq!(entry.completed_at_unix_ms, 200);
        assert_eq!(completed.npc_animation_action.as_deref(), Some("reward"));
    }

    #[test]
    fn forged_ineligible_and_out_of_range_reward_choices_are_atomic() {
        let definitions = item_definitions();
        let mut quest = quest(100);
        quest.completion_actions.fixed_items.push(QuestItemDelta {
            item_id: ITEM_A,
            count: -1,
            expiration: None,
        });
        quest.completion_actions.selectable_items = vec![
            selectable_reward(
                ITEM_B,
                1,
                QuestRewardEligibility {
                    job_mask: Some(1 << 1),
                    gender: Some(QuestRewardGender::Female),
                },
            ),
            selectable_reward(ITEM_C, 1, QuestRewardEligibility::default()),
        ];
        let mut player = player(vec![(ITEM_A, 1)], 2);
        player.stats.as_mut().expect("stats").job_id = 112;
        player.appearance = Some(CharacterAppearance {
            gender: CharacterGender::Male as i32,
            ..CharacterAppearance::default()
        });
        player
            .quests
            .push(player_quest(quest.id, QuestStatus::Started));

        for choice_id in [
            COMPLETE_CHOICE_ID,
            selectable_reward_choice_id(0).expect("first selectable choice"),
            selectable_reward_choice_id(2).expect("out-of-range selectable choice"),
        ] {
            assert!(matches!(
                select_choice(
                    player.clone(),
                    &quest,
                    2,
                    choice_id,
                    curve(),
                    &definitions,
                    scripts(),
                    200,
                ),
                Err(QuestRuleError::InvalidChoice { choice_id: rejected }) if rejected == choice_id
            ));
        }
        assert_eq!(item_count(&player, ITEM_A), 1);
        assert_eq!(item_count(&player, ITEM_B), 0);
        assert_eq!(item_count(&player, ITEM_C), 0);
        assert_eq!(progress(&player, quest.id), QuestProgress::Started);
    }

    #[test]
    fn selectable_reward_capacity_failure_is_atomic() {
        let definitions = item_definitions();
        let mut quest = quest(100);
        quest
            .completion_actions
            .selectable_items
            .push(selectable_reward(
                ITEM_B,
                1,
                QuestRewardEligibility::default(),
            ));
        quest.completion_actions.money = 100;
        quest.completion_actions.npc_animation_action = Some("reward".to_owned());
        let mut player = player(vec![(ITEM_A, 10)], 1);
        player
            .quests
            .push(player_quest(quest.id, QuestStatus::Started));
        let choice_id = eligible_selectable_reward_choices(&player, &quest)[0].0;

        assert!(matches!(
            select_choice(
                player.clone(),
                &quest,
                2,
                choice_id,
                curve(),
                &definitions,
                scripts(),
                200,
            ),
            Err(QuestRuleError::Item(ItemRuleError::InventoryFull))
        ));
        assert_eq!(item_count(&player, ITEM_A), 10);
        assert_eq!(item_count(&player, ITEM_B), 0);
        assert_eq!(player.mesos, 0);
        assert_eq!(progress(&player, quest.id), QuestProgress::Started);
    }

    #[test]
    fn lost_item_restoration_grants_only_the_partial_missing_quantity_once() {
        let definitions = item_definitions();
        let mut quest = quest(100);
        configure_lost_items(&mut quest, &[(ITEM_A, 3)]);
        quest.completion.items.clear();
        let expiration = Some(QuestItemExpiration::RelativeMilliseconds(1_000));
        quest.start_actions.fixed_items[0].expiration = expiration;
        quest
            .dialogue
            .completion
            .lost
            .as_mut()
            .expect("lost interaction")
            .items[0]
            .expiration = expiration;
        quest.completion_actions.money = 500;
        let mut player = player(vec![(ITEM_A, 1)], 2);
        player.quests.push(PlayerQuest {
            mob_progress: vec![QuestMobProgress {
                mob_id: MOB_A,
                count: 4,
            }],
            accepted_at_unix_ms: 123,
            ..player_quest(quest.id, QuestStatus::Started)
        });

        assert!(lost_item_restoration_needed(
            &player,
            &quest,
            &definitions,
            999
        ));
        let restored = select_choice(
            player,
            &quest,
            2,
            RESTORE_ITEMS_CHOICE_ID,
            curve(),
            &definitions,
            scripts(),
            999,
        )
        .expect("restore partial missing quantity");

        assert_eq!(item_count(&restored.player, ITEM_A), 3);
        assert_eq!(
            restored
                .player
                .inventory
                .as_ref()
                .expect("inventory")
                .stacks[1]
                .expires_at_unix_ms,
            1_999
        );
        assert_eq!(restored.pages, vec!["Replacement items"]);
        assert_eq!(restored.player.mesos, 0);
        let entry = &restored.player.quests[0];
        assert_eq!(entry.status, QuestStatus::Started as i32);
        assert_eq!(entry.accepted_at_unix_ms, 123);
        assert_eq!(entry.completed_at_unix_ms, 0);
        assert_eq!(entry.mob_progress[0].count, 4);

        let unchanged = restored.player.clone();
        assert!(matches!(
            select_choice(
                restored.player,
                &quest,
                2,
                RESTORE_ITEMS_CHOICE_ID,
                curve(),
                &definitions,
                scripts(),
                1_000,
            ),
            Err(QuestRuleError::NoMissingRestorableItems { quest_id: 100 })
        ));
        assert_eq!(item_count(&unchanged, ITEM_A), 3);
        assert_eq!(progress(&unchanged, quest.id), QuestProgress::Started);
    }

    #[test]
    fn completed_restoration_requires_completion_and_every_downstream_guard() {
        const REQUIRED_QUEST: u32 = 200;
        const FORBIDDEN_QUEST: u32 = 300;
        const ABSENT_SKILL: u32 = 2_221_003;

        let definitions = item_definitions();
        let mut quest = quest(100);
        quest.dialogue.completion.lost = Some(QuestLostItemDialogue {
            prompt_pages: vec!["Did you lose the completed quest item?".to_owned()],
            success_pages: vec!["Completed quest replacement".to_owned()],
            items: vec![QuestRestorableItem {
                item_id: ITEM_A,
                target_count: 1,
                expiration: None,
                provenance: QuestRestorationProvenance::AuditedCompletionGrant,
                eligibility: QuestRestorationEligibility {
                    owner_state: RequiredQuestState::Completed,
                    required_quests: &[QuestStateRequirement {
                        quest_id: REQUIRED_QUEST,
                        state: RequiredQuestState::Started,
                    }],
                    forbidden_quests: &[QuestStateRequirement {
                        quest_id: FORBIDDEN_QUEST,
                        state: RequiredQuestState::Completed,
                    }],
                    absent_skill_ids: &[ABSENT_SKILL],
                    absent_item_ids: &[ITEM_A, ITEM_B],
                },
            }],
        });
        let first_time = player(Vec::new(), 2);
        assert!(!lost_item_restoration_needed(
            &first_time,
            &quest,
            &definitions,
            200
        ));
        assert!(matches!(
            select_choice(
                first_time,
                &quest,
                1,
                RESTORE_ITEMS_CHOICE_ID,
                curve(),
                &definitions,
                scripts(),
                200,
            ),
            Err(QuestRuleError::InvalidChoice {
                choice_id: RESTORE_ITEMS_CHOICE_ID
            })
        ));

        let mut player = player(Vec::new(), 2);
        player
            .quests
            .push(player_quest(quest.id, QuestStatus::Started));
        player
            .quests
            .push(player_quest(REQUIRED_QUEST, QuestStatus::Started));
        assert!(!lost_item_restoration_needed(
            &player,
            &quest,
            &definitions,
            200
        ));
        assert!(matches!(
            select_choice(
                player.clone(),
                &quest,
                2,
                RESTORE_ITEMS_CHOICE_ID,
                curve(),
                &definitions,
                scripts(),
                200,
            ),
            Err(QuestRuleError::LostItemRestorationUnavailable { quest_id: 100 })
        ));
        player.quests[0] = PlayerQuest {
            completed_at_unix_ms: 123,
            ..player_quest(quest.id, QuestStatus::Completed)
        };
        assert!(lost_item_restoration_needed(
            &player,
            &quest,
            &definitions,
            200
        ));

        let mut blocked = player.clone();
        blocked.quests[1].status = QuestStatus::Completed as i32;
        assert!(!lost_item_restoration_needed(
            &blocked,
            &quest,
            &definitions,
            200
        ));
        blocked = player.clone();
        blocked
            .quests
            .push(player_quest(FORBIDDEN_QUEST, QuestStatus::Completed));
        assert!(!lost_item_restoration_needed(
            &blocked,
            &quest,
            &definitions,
            200
        ));
        blocked = player.clone();
        blocked.learned_skills.push(LearnedSkill {
            skill_id: ABSENT_SKILL,
            level: 1,
            master_level: 0,
        });
        assert!(!lost_item_restoration_needed(
            &blocked,
            &quest,
            &definitions,
            200
        ));
        blocked = player.clone();
        blocked
            .inventory
            .as_mut()
            .expect("inventory")
            .stacks
            .push(InventoryItemStack {
                item_id: ITEM_B,
                quantity: 1,
                expires_at_unix_ms: 0,
            });
        assert!(!lost_item_restoration_needed(
            &blocked,
            &quest,
            &definitions,
            200
        ));

        assert!(matches!(
            select_choice(
                player.clone(),
                &quest,
                999,
                RESTORE_ITEMS_CHOICE_ID,
                curve(),
                &definitions,
                scripts(),
                200,
            ),
            Err(QuestRuleError::WrongNpc { npc_id: 999 })
        ));
        let restored = select_choice(
            player,
            &quest,
            2,
            RESTORE_ITEMS_CHOICE_ID,
            curve(),
            &definitions,
            scripts(),
            200,
        )
        .expect("eligible completed restoration");
        assert_eq!(item_count(&restored.player, ITEM_A), 1);
        assert_eq!(
            progress(&restored.player, quest.id),
            QuestProgress::Completed
        );
        assert_eq!(restored.player.quests[0].completed_at_unix_ms, 123);
        assert!(matches!(
            select_choice(
                restored.player,
                &quest,
                2,
                RESTORE_ITEMS_CHOICE_ID,
                curve(),
                &definitions,
                scripts(),
                200,
            ),
            Err(QuestRuleError::NoMissingRestorableItems { quest_id: 100 })
        ));
    }

    #[test]
    fn completed_restoration_capacity_failure_is_atomic() {
        let definitions = item_definitions();
        let mut quest = quest(100);
        quest.dialogue.completion.lost = Some(QuestLostItemDialogue {
            prompt_pages: vec!["Restore?".to_owned()],
            success_pages: vec!["Restored".to_owned()],
            items: vec![QuestRestorableItem {
                item_id: ITEM_A,
                target_count: 1,
                expiration: None,
                provenance: QuestRestorationProvenance::AuditedCompletionGrant,
                eligibility: QuestRestorationEligibility {
                    owner_state: RequiredQuestState::Completed,
                    required_quests: &[],
                    forbidden_quests: &[],
                    absent_skill_ids: &[],
                    absent_item_ids: &[ITEM_A],
                },
            }],
        });
        let mut player = player(vec![(ITEM_B, 1)], 1);
        player
            .quests
            .push(player_quest(quest.id, QuestStatus::Completed));
        let unchanged = player.clone();

        assert!(matches!(
            select_choice(
                player.clone(),
                &quest,
                2,
                RESTORE_ITEMS_CHOICE_ID,
                curve(),
                &[],
                scripts(),
                200,
            ),
            Err(QuestRuleError::Item(ItemRuleError::UnknownItem { .. }))
        ));
        assert!(matches!(
            select_choice(
                player,
                &quest,
                2,
                RESTORE_ITEMS_CHOICE_ID,
                curve(),
                &definitions,
                scripts(),
                200,
            ),
            Err(QuestRuleError::Item(ItemRuleError::InventoryFull))
        ));
        assert_eq!(item_count(&unchanged, ITEM_A), 0);
        assert_eq!(item_count(&unchanged, ITEM_B), 1);
        assert_eq!(progress(&unchanged, quest.id), QuestProgress::Completed);
    }

    #[test]
    fn repeat_start_changes_only_normal_quest_state_for_completed_restoration() {
        let definitions = item_definitions();
        let mut quest = quest(100);
        quest.start.repeat.interval_ms = Some(0);
        quest.dialogue.completion.lost = Some(QuestLostItemDialogue {
            prompt_pages: vec!["Restore?".to_owned()],
            success_pages: vec!["Restored".to_owned()],
            items: vec![QuestRestorableItem {
                item_id: ITEM_A,
                target_count: 1,
                expiration: None,
                provenance: QuestRestorationProvenance::AuditedCompletionGrant,
                eligibility: QuestRestorationEligibility {
                    owner_state: RequiredQuestState::Completed,
                    required_quests: &[],
                    forbidden_quests: &[],
                    absent_skill_ids: &[],
                    absent_item_ids: &[ITEM_A],
                },
            }],
        });
        let mut player = player(Vec::new(), 1);
        player.quests.push(PlayerQuest {
            completed_at_unix_ms: 100,
            ..player_quest(quest.id, QuestStatus::Completed)
        });
        assert!(lost_item_restoration_needed(
            &player,
            &quest,
            &definitions,
            200
        ));

        let repeated = select_choice(
            player,
            &quest,
            1,
            ACCEPT_CHOICE_ID,
            curve(),
            &definitions,
            scripts(),
            200,
        )
        .expect("repeat quest start");

        assert_eq!(progress(&repeated.player, quest.id), QuestProgress::Started);
        assert_eq!(item_count(&repeated.player, ITEM_A), 0);
        assert!(!lost_item_restoration_needed(
            &repeated.player,
            &quest,
            &definitions,
            200
        ));
        assert!(matches!(
            select_choice(
                repeated.player,
                &quest,
                2,
                RESTORE_ITEMS_CHOICE_ID,
                curve(),
                &definitions,
                scripts(),
                200,
            ),
            Err(QuestRuleError::LostItemRestorationUnavailable { quest_id: 100 })
        ));
    }

    #[test]
    fn quest_3310_reactor_device_restoration_is_active_only_while_started() {
        const DEVICE: u32 = 4_031_698;

        let definitions = vec![ItemDefinition {
            item_id: DEVICE,
            name: "Reactor device".to_owned(),
            stack_max: 1,
            ..ItemDefinition::default()
        }];
        let mut quest = quest(3_310);
        quest.dialogue.completion.lost = Some(QuestLostItemDialogue {
            prompt_pages: vec!["Did you lose the reactor device?".to_owned()],
            success_pages: vec!["Take another device.".to_owned()],
            items: vec![QuestRestorableItem {
                item_id: DEVICE,
                target_count: 1,
                expiration: None,
                provenance: QuestRestorationProvenance::AuditedReactorDevice,
                eligibility: QuestRestorationEligibility {
                    owner_state: RequiredQuestState::Started,
                    required_quests: &[],
                    forbidden_quests: &[],
                    absent_skill_ids: &[],
                    absent_item_ids: &[DEVICE],
                },
            }],
        });
        let mut player = player(Vec::new(), 1);
        player
            .quests
            .push(player_quest(quest.id, QuestStatus::Started));

        let restored = select_choice(
            player,
            &quest,
            2,
            RESTORE_ITEMS_CHOICE_ID,
            curve(),
            &definitions,
            scripts(),
            200,
        )
        .expect("3310 active device restoration");
        assert_eq!(item_count(&restored.player, DEVICE), 1);
        assert_eq!(progress(&restored.player, quest.id), QuestProgress::Started);

        let mut completed = restored.player;
        completed
            .inventory
            .as_mut()
            .expect("inventory")
            .stacks
            .clear();
        completed.quests[0].status = QuestStatus::Completed as i32;
        assert!(!lost_item_restoration_needed(
            &completed,
            &quest,
            &definitions,
            200
        ));
        assert!(matches!(
            select_choice(
                completed,
                &quest,
                2,
                RESTORE_ITEMS_CHOICE_ID,
                curve(),
                &definitions,
                scripts(),
                200,
            ),
            Err(QuestRuleError::Unavailable { quest_id: 3_310 })
        ));
    }

    #[test]
    fn lost_item_restoration_rejects_expired_absolute_grants() {
        let definitions = item_definitions();
        let mut quest = quest(100);
        configure_lost_items(&mut quest, &[(ITEM_A, 1)]);
        quest
            .dialogue
            .completion
            .lost
            .as_mut()
            .expect("lost interaction")
            .items[0]
            .expiration = Some(QuestItemExpiration::AbsoluteUnixMilliseconds(200));
        let mut player = player(Vec::new(), 1);
        player.revision = 7;
        player
            .quests
            .push(player_quest(quest.id, QuestStatus::Started));

        assert!(!lost_item_restoration_needed(
            &player,
            &quest,
            &definitions,
            200
        ));
        assert!(matches!(
            select_choice(
                player.clone(),
                &quest,
                2,
                RESTORE_ITEMS_CHOICE_ID,
                curve(),
                &definitions,
                scripts(),
                200,
            ),
            Err(QuestRuleError::NoMissingRestorableItems { quest_id: 100 })
        ));
        assert_eq!(player.revision, 7);
        assert_eq!(item_count(&player, ITEM_A), 0);
    }

    #[test]
    fn lost_item_restoration_grants_only_live_items_from_a_mixed_branch() {
        let definitions = item_definitions();
        let mut quest = quest(100);
        configure_lost_items(&mut quest, &[(ITEM_A, 1), (ITEM_B, 1)]);
        let items = &mut quest
            .dialogue
            .completion
            .lost
            .as_mut()
            .expect("lost interaction")
            .items;
        items[0].expiration = Some(QuestItemExpiration::AbsoluteUnixMilliseconds(200));
        items[1].expiration = Some(QuestItemExpiration::AbsoluteUnixMilliseconds(201));
        let mut player = player(Vec::new(), 2);
        player
            .quests
            .push(player_quest(quest.id, QuestStatus::Started));

        assert!(lost_item_restoration_needed(
            &player,
            &quest,
            &definitions,
            200
        ));
        let restored = select_choice(
            player,
            &quest,
            2,
            RESTORE_ITEMS_CHOICE_ID,
            curve(),
            &definitions,
            scripts(),
            200,
        )
        .expect("restore the live grant");

        assert!(restored.changed);
        assert_eq!(item_count(&restored.player, ITEM_A), 0);
        assert_eq!(item_count(&restored.player, ITEM_B), 1);
        assert_eq!(
            restored
                .player
                .inventory
                .as_ref()
                .expect("inventory")
                .stacks[0]
                .expires_at_unix_ms,
            201
        );
        assert!(!lost_item_restoration_needed(
            &restored.player,
            &quest,
            &definitions,
            200
        ));
        assert!(matches!(
            select_choice(
                restored.player,
                &quest,
                2,
                RESTORE_ITEMS_CHOICE_ID,
                curve(),
                &definitions,
                scripts(),
                200,
            ),
            Err(QuestRuleError::NoMissingRestorableItems { quest_id: 100 })
        ));
    }

    #[test]
    fn lost_item_restoration_restores_multiple_items_and_counts_equipment() {
        let definitions = item_definitions();
        let mut quest = quest(100);
        configure_lost_items(&mut quest, &[(ITEM_A, 3), (ITEM_B, 1)]);
        quest.completion.mobs.push(QuestMobObjective {
            mob_id: MOB_A,
            count: 1,
        });
        let mut player = player(vec![(ITEM_A, 1)], 3);
        player
            .inventory
            .as_mut()
            .expect("inventory")
            .equipment
            .push(EquippedItem {
                item_id: ITEM_A,
                ..EquippedItem::default()
            });
        player
            .quests
            .push(player_quest(quest.id, QuestStatus::Started));

        let restored = select_choice(
            player,
            &quest,
            2,
            RESTORE_ITEMS_CHOICE_ID,
            curve(),
            &definitions,
            scripts(),
            200,
        )
        .expect("restore every missing quest item");

        assert_eq!(item_count(&restored.player, ITEM_A), 2);
        assert_eq!(item_count(&restored.player, ITEM_B), 1);
        assert_eq!(progress(&restored.player, quest.id), QuestProgress::Started);
        assert!(!completion_readiness(&restored.player, &quest, &definitions, scripts()).ready);
    }

    #[test]
    fn lost_item_restoration_capacity_failure_is_atomic() {
        let definitions = item_definitions();
        let mut quest = quest(100);
        configure_lost_items(&mut quest, &[(ITEM_A, 5), (ITEM_B, 1)]);
        let mut player = player(vec![(ITEM_A, 1)], 1);
        player
            .quests
            .push(player_quest(quest.id, QuestStatus::Started));
        let unchanged = player.clone();

        assert!(matches!(
            select_choice(
                player,
                &quest,
                2,
                RESTORE_ITEMS_CHOICE_ID,
                curve(),
                &definitions,
                scripts(),
                200,
            ),
            Err(QuestRuleError::Item(ItemRuleError::InventoryFull))
        ));
        assert_eq!(item_count(&unchanged, ITEM_A), 1);
        assert_eq!(item_count(&unchanged, ITEM_B), 0);
        assert_eq!(progress(&unchanged, quest.id), QuestProgress::Started);
    }

    #[test]
    fn forged_lost_item_choices_reject_wrong_npc_inactive_and_unmapped_quests() {
        let definitions = item_definitions();
        let mut quest = quest(100);
        configure_lost_items(&mut quest, &[(ITEM_A, 1)]);
        let mut player = player(Vec::new(), 1);
        player
            .quests
            .push(player_quest(quest.id, QuestStatus::Started));

        assert!(matches!(
            select_choice(
                player.clone(),
                &quest,
                999,
                RESTORE_ITEMS_CHOICE_ID,
                curve(),
                &definitions,
                scripts(),
                200,
            ),
            Err(QuestRuleError::WrongNpc { npc_id: 999 })
        ));

        let mut inactive = player.clone();
        inactive.quests[0].status = QuestStatus::Completed as i32;
        assert!(
            select_choice(
                inactive,
                &quest,
                2,
                RESTORE_ITEMS_CHOICE_ID,
                curve(),
                &definitions,
                scripts(),
                200,
            )
            .is_err()
        );

        let mut ordinary = quest.clone();
        ordinary.dialogue.completion.lost = None;
        assert!(matches!(
            select_choice(
                player.clone(),
                &ordinary,
                2,
                RESTORE_ITEMS_CHOICE_ID,
                curve(),
                &definitions,
                scripts(),
                200,
            ),
            Err(QuestRuleError::LostItemRestorationUnavailable { quest_id: 100 })
        ));
        assert_eq!(item_count(&player, ITEM_A), 0);
        assert_eq!(progress(&player, quest.id), QuestProgress::Started);

        let mut satisfied = player.clone();
        satisfied
            .inventory
            .as_mut()
            .expect("inventory")
            .stacks
            .push(InventoryItemStack {
                item_id: ITEM_A,
                quantity: 2,
                expires_at_unix_ms: 0,
            });
        quest
            .dialogue
            .completion
            .lost
            .as_mut()
            .expect("lost interaction")
            .items[0]
            .target_count = 2;
        assert!(matches!(
            select_choice(
                satisfied,
                &quest,
                2,
                RESTORE_ITEMS_CHOICE_ID,
                curve(),
                &definitions,
                scripts(),
                200,
            ),
            Err(QuestRuleError::NoMissingRestorableItems { quest_id: 100 })
        ));
    }

    #[test]
    fn automatic_completion_reports_selectable_reward_blocks_and_keeps_advancing() {
        let definitions = item_definitions();
        let mut no_eligible_reward = quest(100);
        no_eligible_reward.info.auto_complete = true;
        no_eligible_reward
            .completion_actions
            .selectable_items
            .push(selectable_reward(
                ITEM_B,
                1,
                QuestRewardEligibility {
                    job_mask: None,
                    gender: Some(QuestRewardGender::Female),
                },
            ));
        let mut selection_required = quest(200);
        selection_required.info.auto_pre_complete = true;
        selection_required
            .completion_actions
            .selectable_items
            .push(selectable_reward(
                ITEM_C,
                1,
                QuestRewardEligibility::default(),
            ));
        let mut unrelated = quest(300);
        unrelated.info.auto_accept = true;
        let mut player = player(Vec::new(), 2);
        player.appearance = Some(CharacterAppearance {
            gender: CharacterGender::Male as i32,
            ..CharacterAppearance::default()
        });
        player.quests = vec![
            player_quest(no_eligible_reward.id, QuestStatus::Started),
            player_quest(selection_required.id, QuestStatus::Started),
        ];

        let advanced = advance_automatic_quests(
            player,
            [&no_eligible_reward, &selection_required, &unrelated],
            curve(),
            &definitions,
            scripts(),
            200,
        );

        assert!(advanced.completed_quest_ids.is_empty());
        assert_eq!(advanced.started_quest_ids, vec![unrelated.id]);
        assert_eq!(
            progress(&advanced.player, no_eligible_reward.id),
            QuestProgress::Started
        );
        assert_eq!(
            progress(&advanced.player, selection_required.id),
            QuestProgress::Started
        );
        assert_eq!(
            progress(&advanced.player, unrelated.id),
            QuestProgress::Started
        );
        assert_eq!(advanced.failures.len(), 2);
        assert!(advanced.failures.iter().any(|failure| {
            failure.quest_id == no_eligible_reward.id
                && failure
                    .message
                    .contains("no selectable completion reward eligible")
        }));
        assert!(advanced.failures.iter().any(|failure| {
            failure.quest_id == selection_required.id
                && failure.message.contains("requires the player to select")
        }));
    }

    #[test]
    fn correct_quiz_answer_is_required_for_completion() {
        let definitions = item_definitions();
        let mut quest = quest(1_009);
        quest.dialogue.question = Some(QuestQuestionSequence {
            leading_pages: Vec::new(),
            steps: vec![QuestQuestionStep {
                archive_index: 0,
                prompt: "Question".to_owned(),
                choices: vec![
                    QuestChoice {
                        id: 7,
                        label: "Wrong".to_owned(),
                    },
                    QuestChoice {
                        id: 42,
                        label: "Right".to_owned(),
                    },
                ],
                correct_choice_id: 42,
                continuation_pages: Vec::new(),
                failure_pages: HashMap::from([(7, vec!["Try again".to_owned()])]),
            }],
            trailing_pages: vec!["Correct".to_owned()],
        });
        quest.completion_actions.experience = 2;
        quest.completion_actions.npc_animation_action = Some("quiz".to_owned());
        let mut player = player(Vec::new(), 1);
        player
            .quests
            .push(player_quest(1_009, QuestStatus::Started));
        let player = open_completion_question(player, &quest, &definitions, scripts(), 200);

        let wrong = select_choice(
            player,
            &quest,
            2,
            answer_choice_id(QuestQuestionPhase::Completion, 0, 0)
                .expect("first completion answer"),
            curve(),
            &definitions,
            scripts(),
            200,
        )
        .expect("wrong answer dialogue");
        assert!(!wrong.changed);
        assert_eq!(wrong.npc_animation_action, None);
        assert_eq!(progress(&wrong.player, 1_009), QuestProgress::Started);
        let correct = select_choice(
            wrong.player,
            &quest,
            2,
            answer_choice_id(QuestQuestionPhase::Completion, 0, 1)
                .expect("second completion answer"),
            curve(),
            &definitions,
            scripts(),
            200,
        )
        .expect("correct answer");
        assert_eq!(progress(&correct.player, 1_009), QuestProgress::Completed);
        assert_eq!(correct.player.stats.expect("stats").experience, 2);
        assert_eq!(correct.npc_animation_action.as_deref(), Some("quiz"));
    }

    #[test]
    fn quest_6034_no_closes_and_yes_continues_to_completion() {
        let definitions = item_definitions();
        let mut quest = quest(6_034);
        quest.dialogue.question = Some(QuestQuestionSequence {
            leading_pages: Vec::new(),
            steps: vec![QuestQuestionStep {
                archive_index: 0,
                prompt: "Would you like to pick up the crumbled piece of paper?".to_owned(),
                choices: vec![
                    QuestChoice {
                        id: 0,
                        label: "Yes".to_owned(),
                    },
                    QuestChoice {
                        id: 1,
                        label: "No".to_owned(),
                    },
                ],
                correct_choice_id: 0,
                continuation_pages: Vec::new(),
                failure_pages: HashMap::new(),
            }],
            trailing_pages: vec!["The writing on the paper is illegible.".to_owned()],
        });
        quest.completion_actions.next_quest_id = Some(6_035);
        let mut player = player(Vec::new(), 1);
        player
            .quests
            .push(player_quest(quest.id, QuestStatus::Started));
        let player = open_completion_question(player, &quest, &definitions, scripts(), 200);

        let no = select_choice(
            player,
            &quest,
            2,
            answer_choice_id(QuestQuestionPhase::Completion, 0, 1).expect("quest 6034 no answer"),
            curve(),
            &definitions,
            scripts(),
            200,
        )
        .expect("quest 6034 no closes cleanly");
        assert!(!no.changed);
        assert!(no.pages.is_empty());
        assert_eq!(no.next_interaction, None);
        assert_eq!(progress(&no.player, quest.id), QuestProgress::Started);
        assert_eq!(no.player.quests[0].dialogue_step, 1);

        let reopened = begin_completion_question(
            no.player,
            &quest,
            &[&quest],
            2,
            &definitions,
            scripts(),
            environment(201),
        )
        .expect("quest 6034 question reopens");
        assert!(!reopened.changed);
        assert_eq!(
            reopened.next_interaction,
            Some(QuestNextInteraction::Question {
                phase: QuestQuestionPhase::Completion,
                step_index: 0,
            })
        );
        assert_eq!(reopened.player.quests[0].dialogue_step, 1);

        let yes = select_choice(
            reopened.player,
            &quest,
            2,
            answer_choice_id(QuestQuestionPhase::Completion, 0, 0).expect("quest 6034 yes answer"),
            curve(),
            &definitions,
            scripts(),
            201,
        )
        .expect("quest 6034 yes completes");
        assert_eq!(progress(&yes.player, quest.id), QuestProgress::Completed);
        assert_eq!(
            yes.pages,
            vec!["The writing on the paper is illegible.", "Complete"]
        );
        assert_eq!(yes.next_quest_id, Some(6_035));
    }

    #[test]
    fn selectable_quiz_emits_animation_only_when_reward_completes_quest() {
        let definitions = item_definitions();
        let mut quest = quest(100);
        quest.dialogue.question = Some(QuestQuestionSequence {
            leading_pages: Vec::new(),
            steps: vec![QuestQuestionStep {
                archive_index: 0,
                prompt: "Question".to_owned(),
                choices: vec![QuestChoice {
                    id: 42,
                    label: "Right".to_owned(),
                }],
                correct_choice_id: 42,
                continuation_pages: Vec::new(),
                failure_pages: HashMap::new(),
            }],
            trailing_pages: vec!["Choose a reward".to_owned()],
        });
        quest
            .completion_actions
            .selectable_items
            .push(selectable_reward(
                ITEM_B,
                1,
                QuestRewardEligibility::default(),
            ));
        quest.completion_actions.npc_animation_action = Some("quizReward".to_owned());
        let mut player = player(Vec::new(), 1);
        player
            .quests
            .push(player_quest(quest.id, QuestStatus::Started));
        let player = open_completion_question(player, &quest, &definitions, scripts(), 200);

        let passed = select_choice(
            player,
            &quest,
            2,
            answer_choice_id(QuestQuestionPhase::Completion, 0, 0).expect("completion answer"),
            curve(),
            &definitions,
            scripts(),
            200,
        )
        .expect("pass completion quiz");
        assert_eq!(passed.npc_animation_action, None);
        assert_eq!(progress(&passed.player, quest.id), QuestProgress::Started);
        assert!(passed.player.quests[0].completion_quiz_passed);

        let reward_choice = eligible_selectable_reward_choices(&passed.player, &quest)[0].0;
        let completed = select_choice(
            passed.player,
            &quest,
            2,
            reward_choice,
            curve(),
            &definitions,
            scripts(),
            201,
        )
        .expect("select completion reward");
        assert_eq!(
            completed.npc_animation_action.as_deref(),
            Some("quizReward")
        );
        assert_eq!(
            progress(&completed.player, quest.id),
            QuestProgress::Completed
        );
        assert!(matches!(
            select_choice(
                completed.player,
                &quest,
                2,
                reward_choice,
                curve(),
                &definitions,
                scripts(),
                202,
            ),
            Err(QuestRuleError::Unavailable { quest_id: 100 })
        ));
    }

    #[test]
    fn next_quest_becomes_available_through_its_prerequisite() {
        let definitions = item_definitions();
        let mut current = quest(100);
        current.completion_actions.next_quest_id = Some(200);
        let mut next = quest(200);
        next.start.quests.push(QuestStateRequirement {
            quest_id: 100,
            state: RequiredQuestState::Completed,
        });
        let mut player = player(Vec::new(), 1);
        player.quests.push(player_quest(100, QuestStatus::Started));
        assert!(!is_available(&player, &next, &definitions, scripts(), 200));

        let completed = select_choice(
            player,
            &current,
            2,
            COMPLETE_CHOICE_ID,
            curve(),
            &definitions,
            scripts(),
            200,
        )
        .expect("complete current quest");

        assert_eq!(completed.next_quest_id, Some(200));
        assert!(is_available(
            &completed.player,
            &next,
            &definitions,
            scripts(),
            200
        ));
        assert_eq!(progress(&completed.player, 200), QuestProgress::NotStarted);
    }

    #[test]
    fn scripted_transitions_expose_names_without_succeeding() {
        let definitions = item_definitions();
        let mut start_scripted = quest(100);
        start_scripted.start.script = Some("start_quest".to_owned());
        let initial_player = player(Vec::new(), 1);
        assert!(!is_available(
            &initial_player,
            &start_scripted,
            &definitions,
            scripts(),
            100
        ));
        assert!(matches!(
            select_choice(
                initial_player,
                &start_scripted,
                1,
                ACCEPT_CHOICE_ID,
                curve(),
                &definitions,
                scripts(),
                100,
            ),
            Err(QuestRuleError::ScriptRequired {
                quest_id: 100,
                script,
                ..
            }) if script == "start_quest"
        ));

        let mut completion_scripted = quest(200);
        completion_scripted.completion.script = Some("end_quest".to_owned());
        let mut player = player(Vec::new(), 1);
        player.quests.push(player_quest(200, QuestStatus::Started));
        assert!(matches!(
            select_choice(
                player,
                &completion_scripted,
                2,
                COMPLETE_CHOICE_ID,
                curve(),
                &definitions,
                scripts(),
                200,
            ),
            Err(QuestRuleError::ScriptRequired {
                quest_id: 200,
                script,
                ..
            }) if script == "end_quest"
        ));
    }

    #[test]
    fn availability_requires_active_effects_absent_effects_and_the_exact_morph() {
        let definitions = item_definitions();
        let mut quest = quest(100);
        quest.start.effects = vec![
            QuestEffectRequirement {
                item_id: EFFECT_ITEM,
                active: true,
            },
            QuestEffectRequirement {
                item_id: OTHER_EFFECT_ITEM,
                active: false,
            },
        ];
        quest.start.required_morph_id = NonZeroU32::new(40);
        let player = player(Vec::new(), 1);
        let mut effects = PlayerEffects::default();

        assert!(!is_available_with_effects(
            &player,
            &effects,
            &quest,
            &definitions,
            scripts(),
            environment(100),
        ));
        crate::effects::apply_consume_effect(
            PlayerState::default(),
            &mut effects,
            consume_effect(EFFECT_ITEM, Some(40)),
            100,
        );
        assert!(is_available_with_effects(
            &player,
            &effects,
            &quest,
            &definitions,
            scripts(),
            environment(100),
        ));
        crate::effects::apply_consume_effect(
            PlayerState::default(),
            &mut effects,
            consume_effect(OTHER_EFFECT_ITEM, None),
            100,
        );
        assert!(!is_available_with_effects(
            &player,
            &effects,
            &quest,
            &definitions,
            scripts(),
            environment(100),
        ));
    }

    #[test]
    fn quest_buff_actions_apply_hp_and_effects_only_after_other_actions_succeed() {
        let definitions = item_definitions();
        let definition = ConsumeEffectDefinition {
            hp: 50,
            speed: 10,
            ..consume_effect(EFFECT_ITEM, None)
        };
        let actions = QuestActions {
            fixed_items: vec![QuestItemDelta {
                item_id: ITEM_A,
                count: 1,
                expiration: None,
            }],
            buff_item_ids: vec![EFFECT_ITEM],
            ..QuestActions::default()
        };
        let mut effects = PlayerEffects::default();
        let initial_effects = effects.clone();
        let mut invalid_player = player(Vec::new(), 1);
        invalid_player.inventory = None;

        let error = super::apply_actions(
            invalid_player,
            100,
            100,
            100,
            &actions,
            None,
            curve(),
            &definitions,
            &[definition],
            &mut effects,
        )
        .expect_err("missing inventory must reject all actions");
        assert!(matches!(
            error,
            QuestRuleError::Item(ItemRuleError::MissingInventory)
        ));
        assert_eq!(effects, initial_effects);

        let mut valid_player = player(Vec::new(), 1);
        let stats = valid_player.stats.as_mut().expect("stats");
        stats.hp = 80;
        stats.max_hp = 100;
        let updated = super::apply_actions(
            valid_player,
            100,
            100,
            100,
            &actions,
            None,
            curve(),
            &definitions,
            &[definition],
            &mut effects,
        )
        .expect("valid actions");

        assert_eq!(updated.stats.expect("stats").hp, 100);
        assert_eq!(item_count(&updated, ITEM_A), 1);
        assert!(effects.contains_item(EFFECT_ITEM));
        assert_eq!(effects.projected().modifiers.speed, 10);
    }

    #[test]
    fn automatic_quests_recheck_effect_requirements_to_a_fixed_point() {
        let definitions = item_definitions();
        let mut dependent = quest(100);
        dependent.info.auto_accept = true;
        dependent.start.effects.push(QuestEffectRequirement {
            item_id: EFFECT_ITEM,
            active: true,
        });
        let mut producer = quest(200);
        producer.info.auto_accept = true;
        producer.start_actions.buff_item_ids.push(EFFECT_ITEM);

        let advanced = advance_automatic_quests_with_effects(
            player(Vec::new(), 1),
            PlayerEffects::default(),
            [&dependent, &producer],
            curve(),
            &definitions,
            &[consume_effect(EFFECT_ITEM, None)],
            scripts(),
            environment(100),
        );

        assert!(advanced.failures.is_empty());
        assert_eq!(advanced.started_quest_ids, vec![200, 100]);
        assert_eq!(progress(&advanced.player, 100), QuestProgress::Started);
        assert_eq!(progress(&advanced.player, 200), QuestProgress::Started);
        assert!(advanced.effects.contains_item(EFFECT_ITEM));
    }

    fn quest(id: u32) -> QuestDefinition {
        QuestDefinition {
            id,
            name: format!("Quest {id}"),
            start: QuestStartRequirements {
                npc_id: Some(1),
                ..QuestStartRequirements::default()
            },
            completion: QuestCompletionRequirements {
                npc_id: Some(2),
                ..QuestCompletionRequirements::default()
            },
            start_actions: QuestActions::default(),
            completion_actions: QuestActions::default(),
            dialogue: QuestDialogue {
                offer_pages: vec!["Offer".to_owned()],
                accepted_pages: vec!["Accepted".to_owned()],
                declined_pages: vec!["Declined".to_owned()],
                completion: QuestCompletionDialogue {
                    pages: vec!["Ready".to_owned()],
                    success_pages: vec!["Complete".to_owned()],
                    ..QuestCompletionDialogue::default()
                },
                ..QuestDialogue::default()
            },
            info: QuestInfo::default(),
        }
    }

    fn reward_actions() -> QuestActions {
        QuestActions {
            money: 100,
            experience: 5,
            fame: 2,
            ..QuestActions::default()
        }
    }

    fn consume_effect(
        item_id: u32,
        morph_id: Option<u32>,
    ) -> ConsumeEffectDefinition {
        ConsumeEffectDefinition {
            item_id,
            morph_id,
            duration_ms: 1_000,
            ..ConsumeEffectDefinition::default()
        }
    }

    fn player(
        stacks: Vec<(u32, u32)>,
        capacity: u32,
    ) -> PlayerState {
        PlayerState {
            id: "player".to_owned(),
            level: 1,
            stats: Some(CharacterStats {
                job_id: 0,
                experience_required: 100,
                ..CharacterStats::default()
            }),
            inventory: Some(InventoryState {
                capacity,
                stacks: stacks
                    .into_iter()
                    .map(|(item_id, quantity)| InventoryItemStack {
                        item_id,
                        quantity,
                        expires_at_unix_ms: 0,
                    })
                    .collect(),
                ..InventoryState::default()
            }),
            ..PlayerState::default()
        }
    }

    fn player_quest(
        quest_id: u32,
        status: QuestStatus,
    ) -> PlayerQuest {
        PlayerQuest {
            quest_id,
            status: status as i32,
            ..PlayerQuest::default()
        }
    }

    fn item_definitions() -> Vec<ItemDefinition> {
        vec![
            ItemDefinition {
                item_id: ITEM_A,
                name: "Blue Snail Shell".to_owned(),
                stack_max: 10,
                ..ItemDefinition::default()
            },
            ItemDefinition {
                item_id: ITEM_B,
                name: "Dagger A".to_owned(),
                stack_max: 1,
                ..ItemDefinition::default()
            },
            ItemDefinition {
                item_id: ITEM_C,
                name: "Dagger B".to_owned(),
                stack_max: 1,
                ..ItemDefinition::default()
            },
        ]
    }

    fn item_requirement(
        item_id: u32,
        count: u32,
    ) -> QuestItemRequirement {
        QuestItemRequirement {
            item_id,
            condition: QuestItemCondition::AtLeast(
                std::num::NonZeroU32::new(count).expect("positive item requirement"),
            ),
        }
    }

    fn record_condition(
        quest_id: u32,
        alternatives: Vec<QuestRecordPredicate>,
    ) -> QuestRecordCondition {
        QuestRecordCondition {
            quest_id,
            index: 0,
            alternatives,
        }
    }

    fn selectable_reward(
        item_id: u32,
        count: u32,
        eligibility: QuestRewardEligibility,
    ) -> QuestSelectableItemReward {
        QuestSelectableItemReward {
            item_id,
            count,
            expiration: None,
            eligibility,
        }
    }

    fn skill_change(
        skill_id: u32,
        skill_level: u32,
        master_level: u32,
        job_ids: Vec<u32>,
    ) -> QuestSkillChange {
        QuestSkillChange {
            skill_id,
            operation: QuestSkillOperation::Grant {
                skill_level,
                master_level,
            },
            job_ids,
        }
    }

    fn skill_removal(
        skill_id: u32,
        job_ids: Vec<u32>,
    ) -> QuestSkillChange {
        QuestSkillChange {
            skill_id,
            operation: QuestSkillOperation::Remove,
            job_ids,
        }
    }

    fn configure_lost_items(
        quest: &mut QuestDefinition,
        items: &[(u32, u64)],
    ) {
        quest.start_actions.fixed_items = items
            .iter()
            .map(|(item_id, count)| QuestItemDelta {
                item_id: *item_id,
                count: i64::try_from(*count).expect("test restoration count fits i64"),
                expiration: None,
            })
            .collect();
        quest.completion.items = items
            .iter()
            .map(|(item_id, count)| {
                item_requirement(
                    *item_id,
                    u32::try_from(*count).expect("test restoration count fits u32"),
                )
            })
            .collect();
        quest.dialogue.completion.lost = Some(QuestLostItemDialogue {
            prompt_pages: vec!["Did you lose the quest items?".to_owned()],
            success_pages: vec!["Replacement items".to_owned()],
            items: items
                .iter()
                .map(|(item_id, target_count)| QuestRestorableItem {
                    item_id: *item_id,
                    target_count: *target_count,
                    expiration: None,
                    provenance: crate::content::QuestRestorationProvenance::InferredStartGrant,
                    eligibility: crate::content::QuestRestorationEligibility {
                        owner_state: RequiredQuestState::Started,
                        required_quests: &[],
                        forbidden_quests: &[],
                        absent_skill_ids: &[],
                        absent_item_ids: &[],
                    },
                })
                .collect(),
        });
    }

    fn configure_start_question(quest: &mut QuestDefinition) {
        quest.dialogue.start_question = Some(QuestQuestionSequence {
            leading_pages: Vec::new(),
            steps: vec![QuestQuestionStep {
                archive_index: 0,
                prompt: "Question".to_owned(),
                choices: vec![
                    QuestChoice {
                        id: 7,
                        label: "Wrong".to_owned(),
                    },
                    QuestChoice {
                        id: 42,
                        label: "Right".to_owned(),
                    },
                ],
                correct_choice_id: 42,
                continuation_pages: Vec::new(),
                failure_pages: HashMap::from([(7, vec!["Wrong answer".to_owned()])]),
            }],
            trailing_pages: vec!["Question continuation".to_owned()],
        });
    }

    fn script_catalog(
        source: &str,
        quest: &QuestDefinition,
        item_definitions: &[ItemDefinition],
    ) -> QuestScriptCatalog {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("quest-scripts.toml");
        fs::write(&path, source).expect("write quest scripts");
        QuestScriptCatalog::load(&path, [quest], &BTreeSet::new(), item_definitions)
            .expect("quest script catalog")
    }

    fn item_count(
        player: &PlayerState,
        item_id: u32,
    ) -> u64 {
        crate::items::count_item_quantity(
            &player.inventory.as_ref().expect("inventory").stacks,
            item_id,
        )
        .expect("valid inventory")
    }

    fn mob_count(
        player: &PlayerState,
        quest_id: u32,
        mob_id: u32,
    ) -> u32 {
        player
            .quests
            .iter()
            .find(|quest| quest.quest_id == quest_id)
            .and_then(|quest| {
                quest
                    .mob_progress
                    .iter()
                    .find(|progress| progress.mob_id == mob_id)
            })
            .map_or(0, |progress| progress.count)
    }

    fn curve() -> &'static crate::experience::ExperienceCurve {
        use std::sync::OnceLock;

        static CURVES: OnceLock<ExperienceCurves> = OnceLock::new();
        CURVES
            .get_or_init(|| {
                ExperienceCurves::load(
                    &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/xp-curves.toml"),
                )
                .expect("XP curves")
            })
            .default_curve()
    }

    fn scripts() -> &'static QuestScriptCatalog {
        use std::sync::OnceLock;

        static SCRIPTS: OnceLock<QuestScriptCatalog> = OnceLock::new();
        SCRIPTS.get_or_init(QuestScriptCatalog::default)
    }
}
