pub(super) use std::collections::BTreeSet;
pub(super) use std::collections::HashMap;
pub(super) use std::fs;
pub(super) use std::num::NonZeroU32;
pub(super) use std::path::Path;

pub(super) use oozems_proto::v1::CharacterAppearance;
pub(super) use oozems_proto::v1::CharacterGender;
pub(super) use oozems_proto::v1::CharacterStats;
pub(super) use oozems_proto::v1::EquipmentSlot;
pub(super) use oozems_proto::v1::EquippedItem;
pub(super) use oozems_proto::v1::InventoryItemStack;
pub(super) use oozems_proto::v1::InventoryState;
pub(super) use oozems_proto::v1::ItemDefinition;
pub(super) use oozems_proto::v1::KeyAction;
pub(super) use oozems_proto::v1::KeyBinding;
pub(super) use oozems_proto::v1::LearnedSkill;
pub(super) use oozems_proto::v1::MonsterBookCard;
pub(super) use oozems_proto::v1::PlayerQuest;
pub(super) use oozems_proto::v1::PlayerState;
pub(super) use oozems_proto::v1::QuestMobProgress;
pub(super) use oozems_proto::v1::QuestStatus;

pub(super) use super::ACCEPT_CHOICE_ID;
pub(super) use super::COMPLETE_CHOICE_ID;
pub(super) use super::DECLINE_CHOICE_ID;
pub(super) use super::QuestNextInteraction;
pub(super) use super::QuestProgress;
pub(super) use super::QuestQuestionPhase;
pub(super) use super::QuestRuleError;
pub(super) use super::RESTORE_ITEMS_CHOICE_ID;
pub(super) use super::advance_automatic_quests as advance_automatic_quests_with_effects;
pub(super) use super::answer_choice_id;
pub(super) use super::begin_completion_question as begin_completion_question_with_effects;
pub(super) use super::begin_start_question as begin_start_question_with_effects;
pub(super) use super::completion_readiness as completion_readiness_with_effects;
pub(super) use super::eligible_selectable_reward_choices;
pub(super) use super::incomplete_dialogue_pages as incomplete_dialogue_pages_with_effects;
pub(super) use super::is_available as is_available_with_effects;
pub(super) use super::lost_item_restoration_needed;
pub(super) use super::progress;
pub(super) use super::record_mob_kills;
pub(super) use super::select_choice as select_choice_with_effects;
pub(super) use super::select_weighted_item;
pub(super) use super::selectable_reward_choice_id;
pub(super) use crate::content::ConsumeEffectDefinition;
pub(super) use crate::content::QuestActions;
pub(super) use crate::content::QuestCalendar;
pub(super) use crate::content::QuestChoice;
pub(super) use crate::content::QuestCompletionDialogue;
pub(super) use crate::content::QuestCompletionRequirements;
pub(super) use crate::content::QuestConditionalItemReward;
pub(super) use crate::content::QuestDefinition;
pub(super) use crate::content::QuestDialogue;
pub(super) use crate::content::QuestEffectRequirement;
pub(super) use crate::content::QuestEquippedItemRequirements;
pub(super) use crate::content::QuestInfo;
pub(super) use crate::content::QuestItemCondition;
pub(super) use crate::content::QuestItemDelta;
pub(super) use crate::content::QuestItemExpiration;
pub(super) use crate::content::QuestItemRequirement;
pub(super) use crate::content::QuestLostItemDialogue;
pub(super) use crate::content::QuestMobObjective;
pub(super) use crate::content::QuestMonsterBookCardRequirement;
pub(super) use crate::content::QuestMonsterBookRequirements;
pub(super) use crate::content::QuestQuestionSequence;
pub(super) use crate::content::QuestQuestionStep;
pub(super) use crate::content::QuestRecordCondition;
pub(super) use crate::content::QuestRecordPredicate;
pub(super) use crate::content::QuestRecordWrite;
pub(super) use crate::content::QuestRepeatMetadata;
pub(super) use crate::content::QuestRestorableItem;
pub(super) use crate::content::QuestRestorationEligibility;
pub(super) use crate::content::QuestRestorationProvenance;
pub(super) use crate::content::QuestRewardEligibility;
pub(super) use crate::content::QuestRewardGender;
pub(super) use crate::content::QuestSelectableItemReward;
pub(super) use crate::content::QuestSelectedSkill;
pub(super) use crate::content::QuestSkillChange;
pub(super) use crate::content::QuestSkillOperation;
pub(super) use crate::content::QuestSkillRequirement;
pub(super) use crate::content::QuestStartRequirements;
pub(super) use crate::content::QuestStateAction;
pub(super) use crate::content::QuestStateActionState;
pub(super) use crate::content::QuestStateRequirement;
pub(super) use crate::content::QuestWeightedItem;
pub(super) use crate::content::RequiredQuestState;
pub(super) use crate::effects::PlayerEffects;
pub(super) use crate::experience::ExperienceCurves;
pub(super) use crate::items::ItemRuleError;
pub(super) use crate::quest_scripts::QuestScriptCatalog;

pub(super) const ITEM_A: u32 = 4_000_000;
pub(super) const ITEM_B: u32 = 1_332_005;
pub(super) const ITEM_C: u32 = 1_332_007;
pub(super) const EFFECT_ITEM: u32 = 2_210_003;
pub(super) const OTHER_EFFECT_ITEM: u32 = 2_022_070;
pub(super) const MOB_A: u32 = 100_100;
pub(super) const CARD_A: u32 = 2_380_000;
pub(super) const CARD_B: u32 = 2_380_001;

pub(super) fn environment(now_unix_ms: u64) -> super::QuestEnvironment {
    super::QuestEnvironment {
        now_unix_ms,
        world_id: 0,
    }
}

pub(super) fn is_available_in_environment(
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

pub(super) fn completion_readiness_in_environment(
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

pub(super) fn incomplete_dialogue_pages_in_environment(
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

pub(super) fn advance_automatic_quests_in_environment<'a>(
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

pub(super) fn select_choice_in_environment(
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

pub(super) fn begin_start_question(
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

pub(super) fn begin_completion_question(
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

pub(super) fn is_available(
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

pub(super) fn completion_readiness(
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

pub(super) fn incomplete_dialogue_pages(
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

pub(super) fn advance_automatic_quests<'a>(
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

pub(super) fn select_choice(
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

pub(super) fn open_start_question(
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

pub(super) fn open_completion_question(
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

pub(super) fn quest(id: u32) -> QuestDefinition {
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

pub(super) fn reward_actions() -> QuestActions {
    QuestActions {
        money: 100,
        experience: 5,
        fame: 2,
        ..QuestActions::default()
    }
}

pub(super) fn consume_effect(
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

pub(super) fn player(
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

pub(super) fn player_quest(
    quest_id: u32,
    status: QuestStatus,
) -> PlayerQuest {
    PlayerQuest {
        quest_id,
        status: status as i32,
        ..PlayerQuest::default()
    }
}

pub(super) fn item_definitions() -> Vec<ItemDefinition> {
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

pub(super) fn item_requirement(
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

pub(super) fn record_condition(
    quest_id: u32,
    alternatives: Vec<QuestRecordPredicate>,
) -> QuestRecordCondition {
    QuestRecordCondition {
        quest_id,
        index: 0,
        alternatives,
    }
}

pub(super) fn selectable_reward(
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

pub(super) fn skill_change(
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

pub(super) fn skill_removal(
    skill_id: u32,
    job_ids: Vec<u32>,
) -> QuestSkillChange {
    QuestSkillChange {
        skill_id,
        operation: QuestSkillOperation::Remove,
        job_ids,
    }
}

pub(super) fn configure_lost_items(
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

pub(super) fn configure_start_question(quest: &mut QuestDefinition) {
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

pub(super) fn script_catalog(
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

pub(super) fn item_count(
    player: &PlayerState,
    item_id: u32,
) -> u64 {
    crate::items::count_item_quantity(
        &player.inventory.as_ref().expect("inventory").stacks,
        item_id,
    )
    .expect("valid inventory")
}

pub(super) fn mob_count(
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

pub(super) fn curve() -> &'static crate::experience::ExperienceCurve {
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

pub(super) fn scripts() -> &'static QuestScriptCatalog {
    use std::sync::OnceLock;

    static SCRIPTS: OnceLock<QuestScriptCatalog> = OnceLock::new();
    SCRIPTS.get_or_init(QuestScriptCatalog::default)
}
