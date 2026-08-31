use jiff::Timestamp;
use oozems_proto::v1::Npc;
use oozems_proto::v1::NpcAnimationEvent;
use oozems_proto::v1::NpcDialogChoice;
use oozems_proto::v1::NpcDialogChoiceKind;
use oozems_proto::v1::NpcDialogView;
use oozems_proto::v1::NpcInteractionResponse;
use oozems_proto::v1::PlayerState;
use oozems_proto::v1::QuestStatus;
use oozems_proto::v1::npc_interaction;

use super::interaction;
use super::invalid;
use super::shop::shop_view;
use super::taxi::taxi_view;
use crate::api::ApiError;
use crate::api::PlayerMutation;
use crate::api::advance_automatic_player;
use crate::api::item_rule_error;
use crate::api::prepare_player_mutation;
use crate::app::AppState;
use crate::content::QuestDefinition;
use crate::content::QuestItemExpiration;
use crate::player_lock::PlayerGuard;
use crate::quests::QuestProgress;

const GMS_TIME_ZONE: &str = "America/Los_Angeles";

pub(super) async fn select_choice(
    state: &AppState,
    guard: &PlayerGuard,
    mutation: PlayerMutation,
    npc: &Npc,
    quest_definitions: &[&QuestDefinition],
    quest_id: u32,
    choice_id: u32,
    now_unix_ms: u64,
) -> Result<NpcInteractionResponse, ApiError> {
    let quest = state
        .catalog
        .quest(quest_id)
        .ok_or_else(|| invalid("the selected quest is not available"))?;
    let original = mutation.original;
    let original_effects = mutation.original_effects;
    let current = mutation.player;
    let mut effects = mutation.effects;
    let consume_effects = state.catalog.consume_effect_definitions();
    validate_quest_npc_animation(&current, quest, npc)?;
    let environment = quest_environment(state, &current, now_unix_ms)?;
    let selection = crate::quests::select_choice(
        current,
        &mut effects,
        quest,
        quest_definitions,
        npc.npc_id,
        choice_id,
        state.experience.default_curve(),
        state.catalog.item_definition_slice(),
        &consume_effects,
        &state.quest_scripts,
        environment,
    )
    .map_err(quest_rule_error)?;
    let is_restoration = choice_id == crate::quests::RESTORE_ITEMS_CHOICE_ID;
    let selection_changed = selection.changed;
    let npc_animation_action = selection.npc_animation_action;
    let pages = selection.pages;
    let next_interaction = selection.next_interaction;
    let (updated, automatic_changed) = if is_restoration {
        (selection.player, false)
    } else {
        let automatic = advance_automatic_player(state, selection.player, effects, now_unix_ms);
        effects = automatic.effects;
        (automatic.player, automatic.changed)
    };
    let persistence = if selection_changed {
        QuestSelectionPlan::PersistTransition(Box::new(QuestTransitionPlan {
            original,
            player: updated,
            original_effects,
            effects,
            animation_action: npc_animation_action,
            activity: true,
        }))
    } else if automatic_changed {
        QuestSelectionPlan::PersistTransition(Box::new(QuestTransitionPlan {
            original,
            player: updated,
            original_effects,
            effects,
            animation_action: None,
            activity: false,
        }))
    } else {
        QuestSelectionPlan::Unchanged(Box::new(updated))
    };
    let persisted = persist_quest_selection(
        &state.database,
        state.active_effects.clone(),
        state.recovery_timers.clone(),
        guard,
        persistence,
        npc,
        now_unix_ms,
    )
    .await?;
    let (player, npc_animation) = persisted.into_parts();
    let interaction = (!pages.is_empty() || next_interaction.is_some()).then(|| {
        interaction(
            player.map_id,
            npc,
            npc_interaction::View::Dialog(selection_dialog(
                &player,
                quest,
                state.catalog.item_definition_slice(),
                pages,
                next_interaction,
            )),
        )
    });
    Ok(NpcInteractionResponse {
        interaction,
        player: Some(player),
        authoritative: None,
        map: None,
        npc_animation,
        active_buffs: None,
        quest_indicators: Vec::new(),
    })
}

pub(super) enum QuestSelectionPlan {
    Unchanged(Box<PlayerState>),
    PersistTransition(Box<QuestTransitionPlan>),
}

pub(super) struct QuestTransitionPlan {
    pub(super) original: PlayerState,
    pub(super) player: PlayerState,
    pub(super) original_effects: crate::effects::PlayerEffects,
    pub(super) effects: crate::effects::PlayerEffects,
    pub(super) animation_action: Option<String>,
    pub(super) activity: bool,
}

pub(super) enum PersistedQuestSelection {
    Unchanged {
        player: PlayerState,
    },
    Transition {
        player: PlayerState,
        animation: Option<NpcAnimationEvent>,
    },
}

impl PersistedQuestSelection {
    fn into_parts(self) -> (PlayerState, Option<NpcAnimationEvent>) {
        match self {
            Self::Unchanged { player } => (player, None),
            Self::Transition { player, animation } => (player, animation),
        }
    }
}

pub(super) async fn persist_quest_selection(
    database: &crate::database::Database,
    active_effects: std::sync::Arc<crate::effects::ActiveEffects>,
    recovery_timers: std::sync::Arc<crate::recovery::RecoveryTimers>,
    guard: &PlayerGuard,
    plan: QuestSelectionPlan,
    npc: &Npc,
    now_unix_ms: u64,
) -> Result<PersistedQuestSelection, ApiError> {
    let (original, player, original_effects, effects, animation_action, activity) = match plan {
        QuestSelectionPlan::Unchanged(player) => {
            return Ok(PersistedQuestSelection::Unchanged { player: *player });
        }
        QuestSelectionPlan::PersistTransition(plan) => {
            let QuestTransitionPlan {
                original,
                player,
                original_effects,
                effects,
                animation_action,
                activity,
            } = *plan;
            (
                original,
                player,
                original_effects,
                effects,
                animation_action,
                activity,
            )
        }
    };
    let mut transaction = crate::player_transaction::new_player_transaction(
        original,
        player,
        crate::player_transaction::PlayerPersistence::Full,
    );
    crate::player_transaction::stage_effects(
        &mut transaction,
        active_effects,
        original_effects,
        effects,
    );
    if activity {
        let player_id = crate::player_transaction::staged_player(&transaction)
            .id
            .clone();
        crate::player_transaction::stage_activity(
            &mut transaction,
            recovery_timers,
            player_id,
            now_unix_ms,
        );
    }
    let player = crate::player_transaction::commit_player_transaction(database, guard, transaction)
        .await?
        .player;
    let animation = npc_animation_event(animation_action, &player, npc);
    Ok(PersistedQuestSelection::Transition { player, animation })
}

pub(super) fn npc_animation_event(
    action_name: Option<String>,
    player: &PlayerState,
    npc: &Npc,
) -> Option<NpcAnimationEvent> {
    action_name.map(|action_name| NpcAnimationEvent {
        map_id: player.map_id,
        npc_spawn_id: npc.spawn_id,
        npc_id: npc.npc_id,
        action_name,
        player_revision: player.revision,
    })
}

pub(super) fn validate_quest_npc_animation(
    player: &PlayerState,
    quest: &QuestDefinition,
    npc: &Npc,
) -> Result<(), ApiError> {
    let (authoritative_npc_id, action_name) = match crate::quests::progress(player, quest.id) {
        QuestProgress::Started => (
            quest.completion.npc_id,
            quest.completion_actions.npc_animation_action.as_deref(),
        ),
        QuestProgress::NotStarted | QuestProgress::Completed => (
            quest.start.npc_id,
            quest.start_actions.npc_animation_action.as_deref(),
        ),
    };
    let Some(action_name) = action_name.filter(|_| authoritative_npc_id == Some(npc.npc_id)) else {
        return Ok(());
    };
    if npc
        .animations
        .iter()
        .any(|animation| animation.name == action_name && !animation.frames.is_empty())
    {
        Ok(())
    } else {
        Err(invalid(format!(
            "NPC {} does not provide quest animation {action_name:?}",
            npc.npc_id
        )))
    }
}

pub(super) async fn open_interaction(
    state: &AppState,
    guard: &PlayerGuard,
    mutation: PlayerMutation,
    npc: &Npc,
    quest_definitions: &[&QuestDefinition],
    now_unix_ms: u64,
) -> Result<NpcInteractionResponse, ApiError> {
    let mut player = mutation.player.clone();
    let environment = quest_environment(state, &player, now_unix_ms)?;
    let effects = mutation.effects.clone();
    let quests = state.catalog.quests_for_npc(npc.npc_id);
    if let Some(quest) = player.quests.iter().find_map(|entry| {
        (QuestStatus::try_from(entry.status) == Ok(QuestStatus::Unspecified)
            && entry.dialogue_step > 0)
            .then(|| state.catalog.quest(entry.quest_id))
            .flatten()
            .filter(|quest| quest.start.npc_id == Some(npc.npc_id))
    }) && (crate::quests::has_pending_start_decision(&player, quest)
        || crate::quests::is_available(
            &player,
            &effects,
            quest,
            state.catalog.item_definition_slice(),
            &state.quest_scripts,
            environment,
        ))
    {
        let selection = crate::quests::begin_start_question(
            player,
            &effects,
            quest,
            npc.npc_id,
            state.catalog.item_definition_slice(),
            &state.quest_scripts,
            environment,
        )
        .map_err(quest_rule_error)?;
        let dialog = selection_dialog(
            &selection.player,
            quest,
            state.catalog.item_definition_slice(),
            selection.pages,
            selection.next_interaction,
        );
        player =
            persist_dialog_selection(state, guard, mutation, selection.player, selection.changed)
                .await?;
        return Ok(dialog_response(&player, npc, dialog));
    }
    if let Some(quest) = active_quest_for_npc(&player, &quests, npc.npc_id) {
        let mob_ids = quest
            .completion
            .mobs
            .iter()
            .map(|objective| objective.mob_id)
            .collect();
        let mob_definitions = state.catalog.mob_definitions(&mob_ids);
        let ready = crate::quests::completion_readiness(
            &player,
            &effects,
            quest,
            quest_definitions,
            state.catalog.item_definition_slice(),
            &state.quest_scripts,
            environment,
        )
        .ready;
        let restoration_needed = crate::quests::lost_item_restoration_needed(
            &player,
            quest,
            state.catalog.item_definition_slice(),
            now_unix_ms,
        );
        let mut changed = false;
        let dialog = if quest.dialogue.question.is_some() && ready && !restoration_needed {
            let selection = crate::quests::begin_completion_question(
                player,
                &effects,
                quest,
                quest_definitions,
                npc.npc_id,
                state.catalog.item_definition_slice(),
                &state.quest_scripts,
                environment,
            )
            .map_err(quest_rule_error)?;
            let dialog = selection_dialog(
                &selection.player,
                quest,
                state.catalog.item_definition_slice(),
                selection.pages,
                selection.next_interaction,
            );
            player = selection.player;
            changed = selection.changed;
            dialog
        } else {
            active_quest_dialog(
                &player,
                &effects,
                quest,
                quest_definitions,
                state.catalog.item_definition_slice(),
                &mob_definitions,
                &state.quest_scripts,
                environment,
            )
        };
        player = persist_dialog_selection(state, guard, mutation, player, changed).await?;
        return Ok(dialog_response(&player, npc, dialog));
    }
    if let Some(quest) = completed_restoration_for_npc(
        &player,
        &quests,
        npc.npc_id,
        state.catalog.item_definition_slice(),
        now_unix_ms,
    ) {
        let dialog = lost_item_restoration_dialog(
            &player,
            quest,
            state.catalog.item_definition_slice(),
            now_unix_ms,
        )
        .expect("selected completed restoration remains eligible");
        player = persist_dialog_selection(state, guard, mutation, player, false).await?;
        return Ok(dialog_response(&player, npc, dialog));
    }
    if let Some(quest) = quests.iter().copied().find(|quest| {
        quest.start.npc_id == Some(npc.npc_id)
            && crate::quests::is_available(
                &player,
                &effects,
                quest,
                state.catalog.item_definition_slice(),
                &state.quest_scripts,
                environment,
            )
    }) {
        let mut changed = false;
        let dialog = if quest.dialogue.start_question.is_some() {
            let selection = crate::quests::begin_start_question(
                player,
                &effects,
                quest,
                npc.npc_id,
                state.catalog.item_definition_slice(),
                &state.quest_scripts,
                environment,
            )
            .map_err(quest_rule_error)?;
            let dialog = selection_dialog(
                &selection.player,
                quest,
                state.catalog.item_definition_slice(),
                selection.pages,
                selection.next_interaction,
            );
            player = selection.player;
            changed = selection.changed;
            dialog
        } else {
            quest_offer_dialog(quest)
        };
        player = persist_dialog_selection(state, guard, mutation, player, changed).await?;
        return Ok(dialog_response(&player, npc, dialog));
    }
    if let Some(shop) = state.interactions.shop(player.map_id, npc.spawn_id) {
        player = persist_dialog_selection(state, guard, mutation, player, false).await?;
        return Ok(interaction_response(
            &player,
            npc,
            npc_interaction::View::Shop(shop_view(shop, state.cash_shop.currency_name())),
        ));
    }
    if let Some(taxi) = state.interactions.taxi(player.map_id, npc.spawn_id) {
        player = persist_dialog_selection(state, guard, mutation, player, false).await?;
        return Ok(interaction_response(
            &player,
            npc,
            npc_interaction::View::Taxi(taxi_view(taxi)),
        ));
    }
    player = persist_dialog_selection(state, guard, mutation, player, false).await?;
    Ok(interaction_response(
        &player,
        npc,
        npc_interaction::View::Dialog(NpcDialogView {
            quest_id: 0,
            title: npc.function.clone(),
            pages: if npc.ambient_lines.is_empty() {
                vec![format!("{} has nothing to say right now.", npc.name)]
            } else {
                npc.ambient_lines.clone()
            },
            choices: Vec::new(),
        }),
    ))
}

fn interaction_response(
    player: &PlayerState,
    npc: &Npc,
    view: npc_interaction::View,
) -> NpcInteractionResponse {
    NpcInteractionResponse {
        player: Some(player.clone()),
        interaction: Some(interaction(player.map_id, npc, view)),
        authoritative: None,
        map: None,
        npc_animation: None,
        active_buffs: None,
        quest_indicators: Vec::new(),
    }
}

fn dialog_response(
    player: &PlayerState,
    npc: &Npc,
    dialog: NpcDialogView,
) -> NpcInteractionResponse {
    interaction_response(player, npc, npc_interaction::View::Dialog(dialog))
}

pub(super) fn quest_offer_dialog(quest: &QuestDefinition) -> NpcDialogView {
    if let Some(question) = &quest.dialogue.start_question {
        return question_dialog(
            quest,
            crate::quests::QuestQuestionPhase::Start,
            0,
            &question.steps[0],
            question.leading_pages.clone(),
        );
    }
    start_decision_dialog(quest, quest.dialogue.offer_pages.clone())
}

fn start_decision_dialog(
    quest: &QuestDefinition,
    pages: Vec<String>,
) -> NpcDialogView {
    NpcDialogView {
        quest_id: quest.id,
        title: quest.name.clone(),
        pages,
        choices: vec![
            NpcDialogChoice {
                choice_id: crate::quests::ACCEPT_CHOICE_ID,
                label: "Accept".to_owned(),
                kind: NpcDialogChoiceKind::AcceptQuest as i32,
            },
            NpcDialogChoice {
                choice_id: crate::quests::DECLINE_CHOICE_ID,
                label: "Decline".to_owned(),
                kind: NpcDialogChoiceKind::DeclineQuest as i32,
            },
        ],
    }
}

pub(super) fn active_quest_dialog(
    player: &PlayerState,
    effects: &crate::effects::PlayerEffects,
    quest: &QuestDefinition,
    quest_definitions: &[&QuestDefinition],
    item_definitions: &[oozems_proto::v1::ItemDefinition],
    mob_definitions: &[oozems_proto::v1::MobDefinition],
    scripts: &crate::quest_scripts::QuestScriptCatalog,
    environment: crate::quests::QuestEnvironment,
) -> NpcDialogView {
    let readiness = crate::quests::completion_readiness(
        player,
        effects,
        quest,
        quest_definitions,
        item_definitions,
        scripts,
        environment,
    );
    if let Some(dialog) =
        lost_item_restoration_dialog(player, quest, item_definitions, environment.now_unix_ms)
    {
        return dialog;
    }
    if !readiness.ready {
        return NpcDialogView {
            quest_id: quest.id,
            title: quest.name.clone(),
            pages: crate::quests::incomplete_dialogue_pages(
                player,
                effects,
                quest,
                quest_definitions,
                item_definitions,
                mob_definitions,
                scripts,
                environment,
            ),
            choices: Vec::new(),
        };
    }
    let Some(question) = &quest.dialogue.question else {
        let pages = if quest.dialogue.completion.pages.is_empty() {
            vec!["The quest objectives are complete.".to_owned()]
        } else {
            quest.dialogue.completion.pages.clone()
        };
        if !quest.completion_actions.selectable_items.is_empty() {
            let rewards = crate::quests::eligible_selectable_reward_choices(player, quest);
            if rewards.is_empty() {
                let mut pages = pages;
                pages
                    .push("No selectable quest reward is available for your character.".to_owned());
                return NpcDialogView {
                    quest_id: quest.id,
                    title: quest.name.clone(),
                    pages,
                    choices: Vec::new(),
                };
            }
            return NpcDialogView {
                quest_id: quest.id,
                title: quest.name.clone(),
                pages,
                choices: rewards
                    .into_iter()
                    .map(|(choice_id, reward)| {
                        let name = item_definitions
                            .iter()
                            .find(|definition| definition.item_id == reward.item_id)
                            .map(|definition| definition.name.as_str())
                            .unwrap_or("Unknown item");
                        NpcDialogChoice {
                            choice_id,
                            label: selectable_reward_label(name, reward.count, reward.expiration),
                            kind: NpcDialogChoiceKind::SelectQuestReward as i32,
                        }
                    })
                    .collect(),
            };
        }
        return NpcDialogView {
            quest_id: quest.id,
            title: quest.name.clone(),
            pages,
            choices: vec![NpcDialogChoice {
                choice_id: crate::quests::COMPLETE_CHOICE_ID,
                label: "Complete".to_owned(),
                kind: NpcDialogChoiceKind::CompleteQuest as i32,
            }],
        };
    };
    let entry = player
        .quests
        .iter()
        .find(|entry| entry.quest_id == quest.id);
    if entry.is_some_and(|entry| entry.completion_quiz_passed) {
        return selectable_reward_dialog(
            player,
            quest,
            item_definitions,
            quest.dialogue.completion.pages.clone(),
        );
    }
    let step_index = entry
        .and_then(|entry| entry.dialogue_step.checked_sub(1))
        .and_then(|index| usize::try_from(index).ok())
        .filter(|index| *index < question.steps.len())
        .unwrap_or_default();
    let pages = if step_index == 0 {
        question.leading_pages.clone()
    } else {
        Vec::new()
    };
    question_dialog(
        quest,
        crate::quests::QuestQuestionPhase::Completion,
        step_index,
        &question.steps[step_index],
        pages,
    )
}

pub(super) fn lost_item_restoration_dialog(
    player: &PlayerState,
    quest: &QuestDefinition,
    item_definitions: &[oozems_proto::v1::ItemDefinition],
    now_unix_ms: u64,
) -> Option<NpcDialogView> {
    let lost = quest.dialogue.completion.lost.as_ref().filter(|_| {
        crate::quests::lost_item_restoration_needed(player, quest, item_definitions, now_unix_ms)
    })?;
    Some(NpcDialogView {
        quest_id: quest.id,
        title: quest.name.clone(),
        pages: lost.prompt_pages.clone(),
        choices: vec![NpcDialogChoice {
            choice_id: crate::quests::RESTORE_ITEMS_CHOICE_ID,
            label: "Restore items".to_owned(),
            kind: NpcDialogChoiceKind::RestoreQuestItems as i32,
        }],
    })
}

pub(super) fn active_quest_for_npc<'a>(
    player: &PlayerState,
    quests: &[&'a QuestDefinition],
    npc_id: u32,
) -> Option<&'a QuestDefinition> {
    let is_active_here = |quest: &&QuestDefinition| {
        crate::quests::progress(player, quest.id) == QuestProgress::Started
            && quest.completion.npc_id == Some(npc_id)
    };
    quests
        .iter()
        .copied()
        .find(|quest| {
            is_active_here(quest)
                && player.quests.iter().any(|entry| {
                    entry.quest_id == quest.id
                        && (entry.dialogue_step > 0 || entry.completion_quiz_passed)
                })
        })
        .or_else(|| quests.iter().copied().find(is_active_here))
}

pub(super) fn completed_restoration_for_npc<'a>(
    player: &PlayerState,
    quests: &[&'a QuestDefinition],
    npc_id: u32,
    item_definitions: &[oozems_proto::v1::ItemDefinition],
    now_unix_ms: u64,
) -> Option<&'a QuestDefinition> {
    quests.iter().copied().find(|quest| {
        crate::quests::progress(player, quest.id) == QuestProgress::Completed
            && quest.completion.npc_id == Some(npc_id)
            && crate::quests::lost_item_restoration_needed(
                player,
                quest,
                item_definitions,
                now_unix_ms,
            )
    })
}

fn quest_environment(
    state: &AppState,
    player: &PlayerState,
    now_unix_ms: u64,
) -> Result<crate::quests::QuestEnvironment, ApiError> {
    let context = state.catalog.skill_book_context(player)?;
    let learned_skill_modifiers = crate::skills::learned_skill_modifiers(&context, player)
        .map_err(crate::api::skill_rule_error)?;
    Ok(crate::quests::QuestEnvironment {
        now_unix_ms,
        world_id: state.gameplay.world_id,
        learned_skill_modifiers,
    })
}

async fn persist_dialog_selection(
    state: &AppState,
    guard: &PlayerGuard,
    mutation: PlayerMutation,
    player: PlayerState,
    changed: bool,
) -> Result<PlayerState, ApiError> {
    if !changed {
        return Ok(player);
    }
    let (transaction, _) = prepare_player_mutation(state, mutation, player, true, false);
    Ok(
        crate::player_transaction::commit_player_transaction(&state.database, guard, transaction)
            .await?
            .player,
    )
}

fn selectable_reward_label(
    name: &str,
    count: u32,
    expiration: Option<QuestItemExpiration>,
) -> String {
    let label = format!("{name} x{count}");
    let Some(expiration) = expiration else {
        return label;
    };
    let expiration = match expiration {
        QuestItemExpiration::RelativeMilliseconds(duration_ms) => {
            let minutes = duration_ms.div_ceil(60_000);
            let unit = if minutes == 1 { "minute" } else { "minutes" };
            format!("expires {minutes} {unit} after receipt")
        }
        QuestItemExpiration::AbsoluteUnixMilliseconds(deadline) => {
            let deadline = format_gms_deadline(deadline).unwrap_or_else(|| {
                format!("Unix millisecond {deadline} outside the GMS display range")
            });
            format!("expires {deadline}")
        }
    };
    format!("{label} ({expiration})")
}

pub(super) fn format_gms_deadline(deadline_unix_ms: u64) -> Option<String> {
    let deadline_unix_ms = i64::try_from(deadline_unix_ms).ok()?;
    let timestamp = Timestamp::from_millisecond(deadline_unix_ms).ok()?;
    let deadline = timestamp.in_tz(GMS_TIME_ZONE).ok()?;
    Some(format!("{} (GMS)", deadline.strftime("%Y-%m-%d %H:%M %Z")))
}

fn question_dialog(
    quest: &QuestDefinition,
    phase: crate::quests::QuestQuestionPhase,
    step_index: usize,
    question: &crate::content::QuestQuestionStep,
    mut pages: Vec<String>,
) -> NpcDialogView {
    pages.push(question.prompt.clone());
    NpcDialogView {
        quest_id: quest.id,
        title: quest.name.clone(),
        pages,
        choices: question
            .choices
            .iter()
            .enumerate()
            .map(|(index, choice)| NpcDialogChoice {
                choice_id: crate::quests::answer_choice_id(phase, step_index, index)
                    .expect("quest question step and choice counts are choice-ID bounded"),
                label: choice.label.clone(),
                kind: NpcDialogChoiceKind::Answer as i32,
            })
            .collect(),
    }
}

pub(super) fn selection_dialog(
    player: &PlayerState,
    quest: &QuestDefinition,
    item_definitions: &[oozems_proto::v1::ItemDefinition],
    pages: Vec<String>,
    next: Option<crate::quests::QuestNextInteraction>,
) -> NpcDialogView {
    match next {
        Some(crate::quests::QuestNextInteraction::Question { phase, step_index }) => {
            let question = match phase {
                crate::quests::QuestQuestionPhase::Start => quest.dialogue.start_question.as_ref(),
                crate::quests::QuestQuestionPhase::Completion => quest.dialogue.question.as_ref(),
            }
            .and_then(|question| question.steps.get(step_index))
            .expect("quest selection references an imported question step");
            question_dialog(quest, phase, step_index, question, pages)
        }
        Some(crate::quests::QuestNextInteraction::StartDecision) => {
            start_decision_dialog(quest, pages)
        }
        Some(crate::quests::QuestNextInteraction::SelectableReward) => {
            selectable_reward_dialog(player, quest, item_definitions, pages)
        }
        None => NpcDialogView {
            quest_id: quest.id,
            title: quest.name.clone(),
            pages,
            choices: Vec::new(),
        },
    }
}

fn selectable_reward_dialog(
    player: &PlayerState,
    quest: &QuestDefinition,
    item_definitions: &[oozems_proto::v1::ItemDefinition],
    mut pages: Vec<String>,
) -> NpcDialogView {
    let rewards = crate::quests::eligible_selectable_reward_choices(player, quest);
    if rewards.is_empty() {
        pages.push("No selectable quest reward is available for your character.".to_owned());
    }
    NpcDialogView {
        quest_id: quest.id,
        title: quest.name.clone(),
        pages,
        choices: rewards
            .into_iter()
            .map(|(choice_id, reward)| {
                let name = item_definitions
                    .iter()
                    .find(|definition| definition.item_id == reward.item_id)
                    .map(|definition| definition.name.as_str())
                    .unwrap_or("Unknown item");
                NpcDialogChoice {
                    choice_id,
                    label: selectable_reward_label(name, reward.count, reward.expiration),
                    kind: NpcDialogChoiceKind::SelectQuestReward as i32,
                }
            })
            .collect(),
    }
}

fn quest_rule_error(error: crate::quests::QuestRuleError) -> ApiError {
    match error {
        crate::quests::QuestRuleError::Experience(error) => error.into(),
        crate::quests::QuestRuleError::Item(error) => item_rule_error(error),
        _ => invalid(error.to_string()),
    }
}
