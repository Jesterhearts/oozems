use axum::body::Bytes;
use axum::extract::State;
use axum::http::HeaderMap;
use jiff::Timestamp;
use oozems_proto::v1::Npc;
use oozems_proto::v1::NpcAnimationEvent;
use oozems_proto::v1::NpcDialogChoice;
use oozems_proto::v1::NpcDialogChoiceKind;
use oozems_proto::v1::NpcDialogView;
use oozems_proto::v1::NpcInteraction;
use oozems_proto::v1::NpcInteractionRequest;
use oozems_proto::v1::NpcInteractionResponse;
use oozems_proto::v1::NpcShopCurrency;
use oozems_proto::v1::NpcShopOffer;
use oozems_proto::v1::NpcShopView;
use oozems_proto::v1::NpcTaxiDestination;
use oozems_proto::v1::NpcTaxiView;
use oozems_proto::v1::PlayerState;
use oozems_proto::v1::QuestStatus;
use oozems_proto::v1::npc_interaction;
use oozems_proto::v1::npc_interaction_request;

use super::ApiError;
use super::Protobuf;
use super::advance_automatic_player;
use super::decode_request;
use super::item_rule_error;
use super::load_map;
use super::lock_player;
use super::parse_player_id;
use super::record_recovery_activity;
use super::require_player_at;
use super::unix_time_ms;
use crate::app::AppState;
use crate::content::QuestDefinition;
use crate::content::QuestItemExpiration;
use crate::interactions::ShopCurrency;
use crate::quests::QuestProgress;

const NPC_HORIZONTAL_REACH: f32 = 320.0;
const NPC_VERTICAL_REACH: f32 = 180.0;
const GMS_TIME_ZONE: &str = "America/Los_Angeles";

pub async fn interact(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Protobuf<NpcInteractionResponse>, ApiError> {
    let request: NpcInteractionRequest = decode_request(&headers, body)?;
    let player_id = parse_player_id(&request.player_id)?;
    let _player_guard = lock_player(&state, &player_id).await?;
    let now_unix_ms = unix_time_ms()?;
    let current = require_player_at(&state, &player_id, now_unix_ms).await?;
    if request.map_id != current.map_id {
        return Err(invalid(
            "the requested NPC is not on the player's current map",
        ));
    }
    let map = load_map(&state, current.map_id)
        .await?
        .ok_or_else(|| ApiError::not_found("map_not_found", "the current map does not exist"))?;
    let npc = map
        .npcs
        .iter()
        .find(|npc| npc.spawn_id == request.npc_spawn_id)
        .cloned()
        .ok_or_else(|| invalid("the requested NPC spawn does not exist"))?;
    validate_reach(&current, &npc)?;
    let action = request
        .action
        .ok_or_else(|| invalid("the request does not contain an NPC action"))?;

    let mut response = match action {
        npc_interaction_request::Action::Open(_) => {
            open_interaction(&state, current, &npc, now_unix_ms).await?
        }
        npc_interaction_request::Action::SelectChoice(action) => {
            select_choice(
                &state,
                current,
                &npc,
                action.quest_id,
                action.choice_id,
                now_unix_ms,
            )
            .await?
        }
        npc_interaction_request::Action::Buy(action) => {
            buy_item(&state, current, &npc, action.item_id, now_unix_ms).await?
        }
        npc_interaction_request::Action::Sell(action) => {
            sell_item(
                &state,
                current,
                &npc,
                action.inventory_index,
                action.expected_item_id,
                action.expected_expires_at_unix_ms,
                now_unix_ms,
            )
            .await?
        }
        npc_interaction_request::Action::TakeTaxi(action) => {
            take_taxi(
                &state,
                current,
                &map,
                &npc,
                action.target_map_id,
                now_unix_ms,
            )
            .await?
        }
    };
    let effects = crate::effects::snapshot(&state.active_effects, player_id.as_str(), now_unix_ms)?;
    response.active_buffs = Some(crate::effects::state(&effects, now_unix_ms));
    Ok(Protobuf(response))
}

async fn select_choice(
    state: &AppState,
    current: PlayerState,
    npc: &Npc,
    quest_id: u32,
    choice_id: u32,
    now_unix_ms: u64,
) -> Result<NpcInteractionResponse, ApiError> {
    let quest = state
        .catalog
        .quest(quest_id)
        .ok_or_else(|| invalid("the selected quest is not available"))?;
    let mut quest_definitions = state.catalog.quest_definitions().collect::<Vec<_>>();
    quest_definitions.sort_by_key(|quest| quest.id);
    let mut effects = crate::effects::snapshot(&state.active_effects, &current.id, now_unix_ms)?;
    let consume_effects = state.catalog.consume_effect_definitions();
    validate_quest_npc_animation(&current, quest, npc)?;
    let selection = crate::quests::select_choice(
        current,
        &mut effects,
        quest,
        &quest_definitions,
        npc.npc_id,
        choice_id,
        state.experience.default_curve(),
        state.catalog.item_definition_slice(),
        &consume_effects,
        &state.quest_scripts,
        quest_environment(state, now_unix_ms),
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
    let (player, npc_animation) = persist_quest_selection(
        &state.database,
        updated,
        selection_changed || automatic_changed,
        npc_animation_action,
        npc,
    )
    .await?;
    crate::effects::commit(&state.active_effects, &player.id, effects)?;
    if selection_changed {
        record_recovery_activity(state, &player.id, now_unix_ms);
    }
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
    })
}

async fn persist_quest_selection(
    database: &crate::database::Database,
    updated: PlayerState,
    save_required: bool,
    npc_animation_action: Option<String>,
    npc: &Npc,
) -> Result<(PlayerState, Option<NpcAnimationEvent>), ApiError> {
    if npc_animation_action.is_some() && !save_required {
        return Err(invalid(
            "an NPC animation cannot be emitted without a persisted quest transition",
        ));
    }
    let player = if save_required {
        crate::database::save_player(database, &updated).await?
    } else {
        updated
    };
    let event = npc_animation_event(npc_animation_action, &player, npc);
    Ok((player, event))
}

fn npc_animation_event(
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

fn validate_quest_npc_animation(
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

async fn buy_item(
    state: &AppState,
    current: PlayerState,
    npc: &Npc,
    item_id: u32,
    now_unix_ms: u64,
) -> Result<NpcInteractionResponse, ApiError> {
    let shop = state
        .interactions
        .shop(current.map_id, npc.spawn_id)
        .ok_or_else(|| invalid("this NPC does not operate a shop"))?;
    let offer = shop
        .offers
        .iter()
        .find(|offer| offer.item_id == item_id)
        .ok_or_else(|| invalid("the selected item is not sold by this shop"))?;
    let player = crate::items::buy_shop_item(
        current,
        item_id,
        offer.buy_price,
        shop.currency,
        state.catalog.as_ref(),
    )
    .map_err(|error| shop_item_rule_error(error, shop, state.cash_shop.currency_name()))?;
    let effects = crate::effects::snapshot(&state.active_effects, &player.id, now_unix_ms)?;
    let advanced = advance_automatic_player(state, player, effects, now_unix_ms);
    let player = crate::database::save_player(&state.database, &advanced.player).await?;
    crate::effects::commit(&state.active_effects, &player.id, advanced.effects)?;
    record_recovery_activity(state, &player.id, now_unix_ms);
    Ok(shop_response(state, player, npc, shop))
}

async fn sell_item(
    state: &AppState,
    current: PlayerState,
    npc: &Npc,
    inventory_index: u32,
    expected_item_id: u32,
    expected_expires_at_unix_ms: u64,
    now_unix_ms: u64,
) -> Result<NpcInteractionResponse, ApiError> {
    let shop = state
        .interactions
        .shop(current.map_id, npc.spawn_id)
        .ok_or_else(|| invalid("this NPC does not operate a shop"))?;
    validate_shop_sale(shop)?;
    crate::items::validate_inventory_selection(
        &current,
        inventory_index,
        expected_item_id,
        expected_expires_at_unix_ms,
    )
    .map_err(item_rule_error)?;
    let player =
        crate::items::sell_inventory_item(current, inventory_index, state.catalog.as_ref())
            .map_err(item_rule_error)?;
    let effects = crate::effects::snapshot(&state.active_effects, &player.id, now_unix_ms)?;
    let advanced = advance_automatic_player(state, player, effects, now_unix_ms);
    let player = crate::database::save_player(&state.database, &advanced.player).await?;
    crate::effects::commit(&state.active_effects, &player.id, advanced.effects)?;
    record_recovery_activity(state, &player.id, now_unix_ms);
    Ok(shop_response(state, player, npc, shop))
}

fn validate_shop_sale(shop: &crate::interactions::ShopDefinition) -> Result<(), ApiError> {
    if shop.currency == ShopCurrency::CashPoints {
        Err(invalid("cash-point shops do not buy items"))
    } else {
        Ok(())
    }
}

fn shop_item_rule_error(
    error: crate::items::ItemRuleError,
    shop: &crate::interactions::ShopDefinition,
    premium_currency_name: &str,
) -> ApiError {
    match error {
        crate::items::ItemRuleError::InsufficientCashPoints
            if shop.currency == ShopCurrency::CashPoints =>
        {
            invalid(format!(
                "the player does not have enough {premium_currency_name}"
            ))
        }
        error => item_rule_error(error),
    }
}

async fn take_taxi(
    state: &AppState,
    current: PlayerState,
    source_map: &oozems_proto::v1::Map,
    npc: &Npc,
    target_map_id: u32,
    now_unix_ms: u64,
) -> Result<NpcInteractionResponse, ApiError> {
    let taxi = state
        .interactions
        .taxi(current.map_id, npc.spawn_id)
        .ok_or_else(|| invalid("this NPC does not operate a taxi"))?;
    let destination = taxi
        .destinations
        .iter()
        .find(|destination| destination.map_id == target_map_id)
        .ok_or_else(|| invalid("the selected taxi destination is not available"))?;
    if current.mesos < destination.fare {
        return Err(invalid(
            "the player does not have enough mesos for that taxi",
        ));
    }
    let mut target_map = load_map(state, destination.map_id)
        .await?
        .ok_or_else(|| invalid("the taxi destination map does not exist"))?;
    let position = crate::movement::authorized_destination(&target_map, &destination.portal_name)?;
    target_map.dropped_items = crate::items::map_drops(&state.drops, target_map.id)?;
    let simulation = crate::mobs::map_snapshot(&state.mobs, &target_map)?;
    target_map.mobs = simulation.mobs;
    target_map.mob_projectiles = simulation.mob_projectiles;
    target_map.simulation_sequence = simulation.sequence;
    let mut player = current.clone();
    player.mesos -= destination.fare;
    player.map_id = target_map.id;
    player.position = Some(position);
    let (decision, rollback) = crate::movement::relocate_player(
        &state.movement,
        &current,
        source_map,
        &target_map,
        &destination.portal_name,
        state.gameplay.movement,
        now_unix_ms,
    )?;
    let effects = crate::effects::snapshot(&state.active_effects, &player.id, now_unix_ms)?;
    let advanced = advance_automatic_player(state, player, effects, now_unix_ms);
    let player = match crate::database::save_player(&state.database, &advanced.player).await {
        Ok(player) => player,
        Err(error) => {
            if let Err(restore_error) =
                crate::movement::restore_relocation(&state.movement, &current.id, rollback)
            {
                tracing::error!(%restore_error, "failed to roll back taxi movement after persistence failure");
            }
            return Err(error.into());
        }
    };
    crate::effects::commit(&state.active_effects, &player.id, advanced.effects)?;
    if let Err(error) = crate::movement::mark_persisted(&state.movement, &player.id, now_unix_ms) {
        tracing::error!(%error, "taxi movement was persisted but could not be marked clean");
    }
    record_recovery_activity(state, &player.id, now_unix_ms);
    Ok(NpcInteractionResponse {
        player: Some(player),
        interaction: None,
        authoritative: Some(decision.authoritative),
        map: Some(target_map),
        npc_animation: None,
        active_buffs: None,
    })
}

async fn open_interaction(
    state: &AppState,
    mut player: PlayerState,
    npc: &Npc,
    now_unix_ms: u64,
) -> Result<NpcInteractionResponse, ApiError> {
    let environment = quest_environment(state, now_unix_ms);
    let effects = crate::effects::snapshot(&state.active_effects, &player.id, now_unix_ms)?;
    let mut quest_definitions = state.catalog.quest_definitions().collect::<Vec<_>>();
    quest_definitions.sort_by_key(|quest| quest.id);
    let mut quests = state.catalog.quests_for_npc(npc.npc_id);
    quests.sort_by_key(|quest| quest.id);
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
        player = if selection.changed {
            crate::database::save_player(&state.database, &selection.player).await?
        } else {
            selection.player
        };
        return Ok(dialog_response(&player, npc, dialog));
    }
    if let Some(quest) = active_quest_for_npc(&player, &quests, npc.npc_id) {
        let ready = crate::quests::completion_readiness(
            &player,
            &effects,
            quest,
            &quest_definitions,
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
        let dialog = if quest.dialogue.question.is_some() && ready && !restoration_needed {
            let selection = crate::quests::begin_completion_question(
                player,
                &effects,
                quest,
                &quest_definitions,
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
            player = if selection.changed {
                crate::database::save_player(&state.database, &selection.player).await?
            } else {
                selection.player
            };
            dialog
        } else {
            active_quest_dialog(
                &player,
                &effects,
                quest,
                &quest_definitions,
                state.catalog.item_definition_slice(),
                &state.quest_scripts,
                environment,
            )
        };
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
            player = if selection.changed {
                crate::database::save_player(&state.database, &selection.player).await?
            } else {
                selection.player
            };
            dialog
        } else {
            quest_offer_dialog(quest)
        };
        return Ok(dialog_response(&player, npc, dialog));
    }
    if let Some(shop) = state.interactions.shop(player.map_id, npc.spawn_id) {
        return Ok(interaction_response(
            &player,
            npc,
            npc_interaction::View::Shop(shop_view(shop, state.cash_shop.currency_name())),
        ));
    }
    if let Some(taxi) = state.interactions.taxi(player.map_id, npc.spawn_id) {
        return Ok(interaction_response(
            &player,
            npc,
            npc_interaction::View::Taxi(taxi_view(taxi)),
        ));
    }
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
    }
}

fn dialog_response(
    player: &PlayerState,
    npc: &Npc,
    dialog: NpcDialogView,
) -> NpcInteractionResponse {
    interaction_response(player, npc, npc_interaction::View::Dialog(dialog))
}

fn quest_offer_dialog(quest: &QuestDefinition) -> NpcDialogView {
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

fn active_quest_dialog(
    player: &PlayerState,
    effects: &crate::effects::PlayerEffects,
    quest: &QuestDefinition,
    quest_definitions: &[&QuestDefinition],
    item_definitions: &[oozems_proto::v1::ItemDefinition],
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

fn lost_item_restoration_dialog(
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

fn active_quest_for_npc<'a>(
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

fn completed_restoration_for_npc<'a>(
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
    now_unix_ms: u64,
) -> crate::quests::QuestEnvironment {
    crate::quests::QuestEnvironment {
        now_unix_ms,
        world_id: state.gameplay.world_id,
    }
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

fn format_gms_deadline(deadline_unix_ms: u64) -> Option<String> {
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

fn selection_dialog(
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

fn shop_response(
    state: &AppState,
    player: PlayerState,
    npc: &Npc,
    shop: &crate::interactions::ShopDefinition,
) -> NpcInteractionResponse {
    NpcInteractionResponse {
        interaction: Some(interaction(
            player.map_id,
            npc,
            npc_interaction::View::Shop(shop_view(shop, state.cash_shop.currency_name())),
        )),
        player: Some(player),
        authoritative: None,
        map: None,
        npc_animation: None,
        active_buffs: None,
    }
}

fn shop_view(
    shop: &crate::interactions::ShopDefinition,
    premium_currency_name: &str,
) -> NpcShopView {
    NpcShopView {
        offers: shop
            .offers
            .iter()
            .map(|offer| NpcShopOffer {
                item_id: offer.item_id,
                buy_price: offer.buy_price,
            })
            .collect(),
        currency: match shop.currency {
            ShopCurrency::Mesos => NpcShopCurrency::Mesos as i32,
            ShopCurrency::CashPoints => NpcShopCurrency::CashPoints as i32,
        },
        currency_name: match shop.currency {
            ShopCurrency::Mesos => "mesos",
            ShopCurrency::CashPoints => premium_currency_name,
        }
        .to_owned(),
    }
}

fn taxi_view(taxi: &crate::interactions::TaxiDefinition) -> NpcTaxiView {
    NpcTaxiView {
        destinations: taxi
            .destinations
            .iter()
            .map(|destination| NpcTaxiDestination {
                map_id: destination.map_id,
                label: destination.label.clone(),
                fare: destination.fare,
            })
            .collect(),
    }
}

fn interaction(
    map_id: u32,
    npc: &Npc,
    view: npc_interaction::View,
) -> NpcInteraction {
    NpcInteraction {
        map_id,
        npc_spawn_id: npc.spawn_id,
        npc_id: npc.npc_id,
        npc_name: npc.name.clone(),
        view: Some(view),
    }
}

fn validate_reach(
    player: &PlayerState,
    npc: &Npc,
) -> Result<(), ApiError> {
    let player_position = player
        .position
        .as_ref()
        .ok_or_else(|| invalid("the player does not have an authoritative position"))?;
    let npc_position = npc
        .position
        .as_ref()
        .ok_or_else(|| invalid("the NPC does not have a valid position"))?;
    if (player_position.x - npc_position.x).abs() > NPC_HORIZONTAL_REACH
        || (player_position.y - npc_position.y).abs() > NPC_VERTICAL_REACH
    {
        return Err(invalid("the player is too far away from that NPC"));
    }
    Ok(())
}

fn quest_rule_error(error: crate::quests::QuestRuleError) -> ApiError {
    match error {
        crate::quests::QuestRuleError::Experience(error) => error.into(),
        crate::quests::QuestRuleError::Item(error) => item_rule_error(error),
        _ => invalid(error.to_string()),
    }
}

fn invalid(message: impl Into<String>) -> ApiError {
    ApiError::bad_request("invalid_npc_interaction", message)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use oozems_proto::v1::CharacterAppearance;
    use oozems_proto::v1::CharacterGender;
    use oozems_proto::v1::CharacterStats;
    use oozems_proto::v1::InventoryItemStack;
    use oozems_proto::v1::InventoryState;
    use oozems_proto::v1::ItemDefinition;
    use oozems_proto::v1::Npc;
    use oozems_proto::v1::NpcAnimation;
    use oozems_proto::v1::NpcDialogChoiceKind;
    use oozems_proto::v1::NpcShopCurrency;
    use oozems_proto::v1::PlayerQuest;
    use oozems_proto::v1::PlayerState;
    use oozems_proto::v1::QuestStatus;
    use oozems_proto::v1::Vec2;

    use super::active_quest_dialog;
    use super::active_quest_for_npc;
    use super::completed_restoration_for_npc;
    use super::format_gms_deadline;
    use super::npc_animation_event;
    use super::persist_quest_selection;
    use super::quest_offer_dialog;
    use super::selection_dialog;
    use super::shop_view;
    use super::validate_quest_npc_animation;
    use super::validate_reach;
    use super::validate_shop_sale;
    use crate::content::QuestActions;
    use crate::content::QuestChoice;
    use crate::content::QuestCompletionDialogue;
    use crate::content::QuestCompletionRequirements;
    use crate::content::QuestDefinition;
    use crate::content::QuestDialogue;
    use crate::content::QuestIncompleteDialogue;
    use crate::content::QuestInfo;
    use crate::content::QuestItemCondition;
    use crate::content::QuestItemExpiration;
    use crate::content::QuestItemRequirement;
    use crate::content::QuestLostItemDialogue;
    use crate::content::QuestQuestionSequence;
    use crate::content::QuestQuestionStep;
    use crate::content::QuestRestorableItem;
    use crate::content::QuestRewardEligibility;
    use crate::content::QuestRewardGender;
    use crate::content::QuestSelectableItemReward;
    use crate::content::QuestStartRequirements;
    use crate::interactions::ShopCurrency;
    use crate::interactions::ShopDefinition;
    use crate::interactions::ShopOffer;

    fn environment(now_unix_ms: u64) -> crate::quests::QuestEnvironment {
        crate::quests::QuestEnvironment {
            now_unix_ms,
            world_id: 0,
        }
    }

    fn active_dialog(
        player: &PlayerState,
        quest: &QuestDefinition,
        item_definitions: &[ItemDefinition],
        scripts: &crate::quest_scripts::QuestScriptCatalog,
        now_unix_ms: u64,
    ) -> oozems_proto::v1::NpcDialogView {
        active_quest_dialog(
            player,
            &crate::effects::PlayerEffects::default(),
            quest,
            &[quest],
            item_definitions,
            scripts,
            environment(now_unix_ms),
        )
    }

    #[test]
    fn npc_interaction_uses_the_authoritative_player_position() {
        let npc = Npc {
            position: Some(Vec2 { x: 100.0, y: 200.0 }),
            ..Npc::default()
        };
        let nearby = PlayerState {
            position: Some(Vec2 { x: 180.0, y: 250.0 }),
            ..PlayerState::default()
        };
        let far_away = PlayerState {
            position: Some(Vec2 { x: 421.0, y: 200.0 }),
            ..PlayerState::default()
        };

        assert!(validate_reach(&nearby, &npc).is_ok());
        assert!(validate_reach(&far_away, &npc).is_err());
    }

    #[test]
    fn cash_point_shop_view_is_buy_only_on_the_server() {
        let shop = ShopDefinition {
            currency: ShopCurrency::CashPoints,
            offers: vec![ShopOffer {
                item_id: 5_000_001,
                buy_price: 250,
            }],
        };

        let view = shop_view(&shop, "Ooze");

        assert_eq!(
            NpcShopCurrency::try_from(view.currency),
            Ok(NpcShopCurrency::CashPoints)
        );
        assert_eq!(view.offers[0].buy_price, 250);
        assert_eq!(view.currency_name, "Ooze");
        assert!(validate_shop_sale(&shop).is_err());
        assert!(
            validate_shop_sale(&ShopDefinition {
                currency: ShopCurrency::Mesos,
                offers: Vec::new(),
            })
            .is_ok()
        );
        assert_eq!(
            NpcShopCurrency::try_from(
                shop_view(
                    &ShopDefinition {
                        currency: ShopCurrency::Mesos,
                        offers: Vec::new(),
                    },
                    "Ooze",
                )
                .currency
            ),
            Ok(NpcShopCurrency::Mesos)
        );
    }

    #[test]
    fn quest_npc_animation_is_bound_to_the_interacted_spawn_and_saved_revision() {
        let mut quest = QuestDefinition {
            id: 100,
            name: "Animation".to_owned(),
            start: QuestStartRequirements::default(),
            completion: QuestCompletionRequirements {
                npc_id: Some(10),
                ..QuestCompletionRequirements::default()
            },
            start_actions: QuestActions::default(),
            completion_actions: QuestActions {
                npc_animation_action: Some("quest".to_owned()),
                ..QuestActions::default()
            },
            dialogue: QuestDialogue::default(),
            info: QuestInfo::default(),
        };
        let player = PlayerState {
            map_id: 1,
            revision: 9,
            quests: vec![PlayerQuest {
                quest_id: quest.id,
                status: QuestStatus::Started as i32,
                ..PlayerQuest::default()
            }],
            ..PlayerState::default()
        };
        let npc = Npc {
            spawn_id: 7,
            npc_id: 10,
            animations: vec![NpcAnimation {
                name: "quest".to_owned(),
                frames: vec![oozems_proto::v1::NpcFrame {
                    delay_ms: 100,
                    ..oozems_proto::v1::NpcFrame::default()
                }],
            }],
            ..Npc::default()
        };

        validate_quest_npc_animation(&player, &quest, &npc).expect("matching animation");
        let event = npc_animation_event(
            quest.completion_actions.npc_animation_action.clone(),
            &player,
            &npc,
        )
        .expect("NPC animation event");
        assert_eq!(
            (
                event.map_id,
                event.npc_spawn_id,
                event.npc_id,
                event.action_name.as_str(),
                event.player_revision,
            ),
            (1, 7, 10, "quest", 9)
        );

        quest.completion_actions.npc_animation_action = Some("missing".to_owned());
        assert!(validate_quest_npc_animation(&player, &quest, &npc).is_err());
    }

    #[tokio::test]
    async fn failed_quest_persistence_cannot_produce_an_animation_event() {
        let directory = tempfile::tempdir().expect("temporary database directory");
        let database = crate::database::open_surreal_kv(directory.path(), 0)
            .await
            .expect("open database");
        let player = PlayerState {
            id: "animation-save-failure".to_owned(),
            map_id: 1,
            revision: i64::MAX as u64,
            ..PlayerState::default()
        };
        let npc = Npc {
            spawn_id: 7,
            npc_id: 10,
            ..Npc::default()
        };

        let result =
            persist_quest_selection(&database, player, true, Some("quest".to_owned()), &npc).await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn failed_restoration_persistence_keeps_the_saved_inventory_unchanged() {
        let directory = tempfile::tempdir().expect("temporary database directory");
        let database = crate::database::open_surreal_kv(directory.path(), 0)
            .await
            .expect("open database");
        let quest = QuestDefinition {
            id: 100,
            name: "Completed restoration".to_owned(),
            start: QuestStartRequirements::default(),
            completion: QuestCompletionRequirements {
                npc_id: Some(10),
                ..QuestCompletionRequirements::default()
            },
            start_actions: QuestActions::default(),
            completion_actions: QuestActions::default(),
            dialogue: QuestDialogue {
                completion: QuestCompletionDialogue {
                    lost: Some(QuestLostItemDialogue {
                        prompt_pages: vec!["Restore?".to_owned()],
                        success_pages: vec!["Restored".to_owned()],
                        items: vec![QuestRestorableItem {
                            item_id: 4_000_000,
                            target_count: 1,
                            expiration: None,
                            provenance:
                                crate::content::QuestRestorationProvenance::AuditedCompletionGrant,
                            eligibility: crate::content::QuestRestorationEligibility {
                                owner_state: crate::content::RequiredQuestState::Completed,
                                required_quests: &[],
                                forbidden_quests: &[],
                                absent_skill_ids: &[],
                                absent_item_ids: &[4_000_000],
                            },
                        }],
                    }),
                    ..QuestCompletionDialogue::default()
                },
                ..QuestDialogue::default()
            },
            info: QuestInfo::default(),
        };
        let definitions = [ItemDefinition {
            item_id: 4_000_000,
            name: "Quest item".to_owned(),
            stack_max: 10,
            ..ItemDefinition::default()
        }];
        let original = PlayerState {
            id: "restoration-save-failure".to_owned(),
            name: "Mina".to_owned(),
            level: 1,
            map_id: 100,
            position: Some(Vec2 { x: 10.0, y: 20.0 }),
            appearance: Some(CharacterAppearance {
                gender: CharacterGender::Female as i32,
                skin_id: 2_000,
                face_id: 21_000,
                hair_id: 31_000,
            }),
            stats: Some(CharacterStats {
                hp: 1,
                max_hp: 1,
                mp: 1,
                max_mp: 1,
                experience_required: 1,
                ..CharacterStats::default()
            }),
            inventory: Some(InventoryState {
                capacity: 1,
                ..InventoryState::default()
            }),
            key_bindings: crate::keymap::default_bindings(),
            quests: vec![PlayerQuest {
                quest_id: quest.id,
                status: QuestStatus::Completed as i32,
                ..PlayerQuest::default()
            }],
            ..PlayerState::default()
        };
        let saved = crate::database::save_player(&database, &original)
            .await
            .expect("save original player");
        let mut restored =
            crate::quests::restore_lost_quest_items(saved, &quest, &definitions, 200)
                .expect("stage completed restoration")
                .player;
        restored.revision = i64::MAX as u64;

        assert!(
            persist_quest_selection(&database, restored, true, None, &Npc::default())
                .await
                .is_err()
        );
        let player_id =
            crate::database::PlayerId::parse("restoration-save-failure").expect("valid player ID");
        let loaded = crate::database::load_player(&database, &player_id)
            .await
            .expect("load saved player")
            .expect("saved player");
        assert_eq!(
            crate::items::count_inventory_item(
                loaded.inventory.as_ref().expect("inventory"),
                &definitions,
                4_000_000,
            )
            .expect("count quest item"),
            0
        );
    }

    #[test]
    fn offer_dialog_projects_start_questions_without_changing_ordinary_offers() {
        let mut quest = QuestDefinition {
            id: 100,
            name: "Question offer".to_owned(),
            start: QuestStartRequirements::default(),
            completion: QuestCompletionRequirements::default(),
            start_actions: QuestActions::default(),
            completion_actions: QuestActions::default(),
            dialogue: QuestDialogue {
                offer_pages: vec!["Ordinary offer".to_owned()],
                ..QuestDialogue::default()
            },
            info: QuestInfo::default(),
        };

        let ordinary = quest_offer_dialog(&quest);
        assert_eq!(ordinary.pages, vec!["Ordinary offer"]);
        assert_eq!(ordinary.choices.len(), 2);
        assert_eq!(
            ordinary
                .choices
                .iter()
                .map(|choice| NpcDialogChoiceKind::try_from(choice.kind))
                .collect::<Result<Vec<_>, _>>()
                .expect("ordinary choice kinds"),
            vec![
                NpcDialogChoiceKind::AcceptQuest,
                NpcDialogChoiceKind::DeclineQuest,
            ]
        );

        quest.dialogue.start_question = Some(QuestQuestionSequence {
            leading_pages: Vec::new(),
            steps: vec![QuestQuestionStep {
                archive_index: 0,
                prompt: "Pick one".to_owned(),
                choices: vec![
                    QuestChoice {
                        id: 0,
                        label: "First".to_owned(),
                    },
                    QuestChoice {
                        id: 1,
                        label: "Second".to_owned(),
                    },
                ],
                correct_choice_id: 1,
                continuation_pages: Vec::new(),
                failure_pages: HashMap::from([(0, vec!["Wrong".to_owned()])]),
            }],
            trailing_pages: Vec::new(),
        });
        let question = quest_offer_dialog(&quest);

        assert_eq!(question.pages, vec!["Pick one"]);
        assert_eq!(
            question
                .choices
                .iter()
                .map(|choice| (choice.choice_id, choice.label.as_str()))
                .collect::<Vec<_>>(),
            vec![
                (
                    crate::quests::answer_choice_id(
                        crate::quests::QuestQuestionPhase::Start,
                        0,
                        0,
                    )
                    .expect("first answer choice"),
                    "First",
                ),
                (
                    crate::quests::answer_choice_id(
                        crate::quests::QuestQuestionPhase::Start,
                        0,
                        1,
                    )
                    .expect("second answer choice"),
                    "Second",
                ),
            ]
        );
        assert!(question.choices.iter().all(|choice| {
            NpcDialogChoiceKind::try_from(choice.kind) == Ok(NpcDialogChoiceKind::Answer)
                && choice.choice_id != crate::quests::ACCEPT_CHOICE_ID
                && choice.choice_id != crate::quests::DECLINE_CHOICE_ID
                && choice.choice_id != crate::quests::COMPLETE_CHOICE_ID
                && choice.choice_id != crate::quests::RESTORE_ITEMS_CHOICE_ID
                && choice.choice_id < 0x8000_0000
        }));
    }

    #[test]
    fn pending_start_decision_projects_normal_accept_and_decline_choices() {
        let quest = QuestDefinition {
            id: 100,
            name: "Question offer".to_owned(),
            start: QuestStartRequirements::default(),
            completion: QuestCompletionRequirements::default(),
            start_actions: QuestActions::default(),
            completion_actions: QuestActions::default(),
            dialogue: QuestDialogue::default(),
            info: QuestInfo::default(),
        };

        let dialog = selection_dialog(
            &PlayerState::default(),
            &quest,
            &[],
            vec!["Passed".to_owned()],
            Some(crate::quests::QuestNextInteraction::StartDecision),
        );

        assert_eq!(dialog.pages, vec!["Passed"]);
        assert_eq!(
            dialog
                .choices
                .iter()
                .map(|choice| (choice.choice_id, NpcDialogChoiceKind::try_from(choice.kind)))
                .collect::<Vec<_>>(),
            vec![
                (
                    crate::quests::ACCEPT_CHOICE_ID,
                    Ok(NpcDialogChoiceKind::AcceptQuest),
                ),
                (
                    crate::quests::DECLINE_CHOICE_ID,
                    Ok(NpcDialogChoiceKind::DeclineQuest),
                ),
            ]
        );
    }

    #[test]
    fn pending_active_interaction_has_priority_over_the_lowest_quest_id() {
        let ordinary = QuestDefinition {
            id: 100,
            name: "Ordinary".to_owned(),
            start: QuestStartRequirements::default(),
            completion: QuestCompletionRequirements {
                npc_id: Some(2),
                ..QuestCompletionRequirements::default()
            },
            start_actions: QuestActions::default(),
            completion_actions: QuestActions::default(),
            dialogue: QuestDialogue::default(),
            info: QuestInfo::default(),
        };
        let pending = QuestDefinition {
            id: 200,
            name: "Pending".to_owned(),
            ..ordinary.clone()
        };
        let mut player = PlayerState {
            quests: vec![
                PlayerQuest {
                    quest_id: ordinary.id,
                    status: QuestStatus::Started as i32,
                    ..PlayerQuest::default()
                },
                PlayerQuest {
                    quest_id: pending.id,
                    status: QuestStatus::Started as i32,
                    dialogue_step: 2,
                    ..PlayerQuest::default()
                },
            ],
            ..PlayerState::default()
        };
        let quests = [&ordinary, &pending];

        assert_eq!(
            active_quest_for_npc(&player, &quests, 2).map(|quest| quest.id),
            Some(pending.id)
        );
        player.quests[1].dialogue_step = 0;
        assert_eq!(
            active_quest_for_npc(&player, &quests, 2).map(|quest| quest.id),
            Some(ordinary.id)
        );
        player.quests[1].completion_quiz_passed = true;
        assert_eq!(
            active_quest_for_npc(&player, &quests, 2).map(|quest| quest.id),
            Some(pending.id)
        );
    }

    #[test]
    fn completed_restoration_is_npc_bound_and_follows_active_quest_priority() {
        let active = QuestDefinition {
            id: 100,
            name: "Active".to_owned(),
            start: QuestStartRequirements::default(),
            completion: QuestCompletionRequirements {
                npc_id: Some(2),
                ..QuestCompletionRequirements::default()
            },
            start_actions: QuestActions::default(),
            completion_actions: QuestActions::default(),
            dialogue: QuestDialogue::default(),
            info: QuestInfo::default(),
        };
        let completed = QuestDefinition {
            id: 200,
            name: "Completed restoration".to_owned(),
            dialogue: QuestDialogue {
                completion: QuestCompletionDialogue {
                    lost: Some(QuestLostItemDialogue {
                        prompt_pages: vec!["Restore the lost item?".to_owned()],
                        success_pages: vec!["Restored".to_owned()],
                        items: vec![QuestRestorableItem {
                            item_id: 4_000_000,
                            target_count: 1,
                            expiration: None,
                            provenance:
                                crate::content::QuestRestorationProvenance::AuditedCompletionGrant,
                            eligibility: crate::content::QuestRestorationEligibility {
                                owner_state: crate::content::RequiredQuestState::Completed,
                                required_quests: &[],
                                forbidden_quests: &[],
                                absent_skill_ids: &[],
                                absent_item_ids: &[4_000_000],
                            },
                        }],
                    }),
                    ..QuestCompletionDialogue::default()
                },
                ..QuestDialogue::default()
            },
            ..active.clone()
        };
        let definitions = [ItemDefinition {
            item_id: 4_000_000,
            name: "Quest item".to_owned(),
            stack_max: 10,
            ..ItemDefinition::default()
        }];
        let player = PlayerState {
            inventory: Some(InventoryState {
                capacity: 1,
                ..InventoryState::default()
            }),
            quests: vec![
                PlayerQuest {
                    quest_id: active.id,
                    status: QuestStatus::Started as i32,
                    ..PlayerQuest::default()
                },
                PlayerQuest {
                    quest_id: completed.id,
                    status: QuestStatus::Completed as i32,
                    ..PlayerQuest::default()
                },
            ],
            ..PlayerState::default()
        };
        let quests = [&active, &completed];

        assert_eq!(
            active_quest_for_npc(&player, &quests, 2).map(|quest| quest.id),
            Some(active.id),
            "an active quest remains the interaction selected first"
        );
        assert_eq!(
            completed_restoration_for_npc(&player, &quests, 2, &definitions, 200)
                .map(|quest| quest.id),
            Some(completed.id)
        );
        assert!(
            completed_restoration_for_npc(&player, &quests, 3, &definitions, 200).is_none(),
            "the completed dialogue is available only at its completion NPC"
        );

        let completed_only = [&completed];
        let selected =
            completed_restoration_for_npc(&player, &completed_only, 2, &definitions, 200)
                .expect("completed restoration before repeat/start offers");
        let dialog = super::lost_item_restoration_dialog(&player, selected, &definitions, 200)
            .expect("completed restoration dialog");
        assert_eq!(dialog.quest_id, completed.id);
        assert_eq!(
            dialog.choices[0].choice_id,
            crate::quests::RESTORE_ITEMS_CHOICE_ID
        );
    }

    #[test]
    fn active_ordinary_quest_shows_progress_then_complete_choice() {
        let quest = QuestDefinition {
            id: 100,
            name: "Collection".to_owned(),
            start: QuestStartRequirements::default(),
            completion: QuestCompletionRequirements {
                items: vec![QuestItemRequirement {
                    item_id: 4_000_000,
                    condition: QuestItemCondition::AtLeast(
                        std::num::NonZeroU32::new(2).expect("positive requirement"),
                    ),
                }],
                ..QuestCompletionRequirements::default()
            },
            start_actions: QuestActions::default(),
            completion_actions: QuestActions::default(),
            dialogue: QuestDialogue {
                completion: QuestCompletionDialogue {
                    pages: vec!["You have everything.".to_owned()],
                    incomplete: QuestIncompleteDialogue {
                        item_pages: vec!["Bring me the shells.".to_owned()],
                        ..QuestIncompleteDialogue::default()
                    },
                    ..QuestCompletionDialogue::default()
                },
                ..QuestDialogue::default()
            },
            info: QuestInfo::default(),
        };
        let definitions = vec![ItemDefinition {
            item_id: 4_000_000,
            name: "Blue Snail Shell".to_owned(),
            stack_max: 10,
            ..ItemDefinition::default()
        }];
        let mut player = PlayerState {
            inventory: Some(InventoryState {
                capacity: 1,
                stacks: vec![InventoryItemStack {
                    item_id: 4_000_000,
                    quantity: 1,
                    expires_at_unix_ms: 0,
                }],
                ..InventoryState::default()
            }),
            quests: vec![PlayerQuest {
                quest_id: 100,
                status: QuestStatus::Started as i32,
                ..PlayerQuest::default()
            }],
            ..PlayerState::default()
        };
        let scripts = crate::quest_scripts::QuestScriptCatalog::default();

        let incomplete = active_dialog(&player, &quest, &definitions, &scripts, 200);
        assert!(incomplete.choices.is_empty());
        assert_eq!(incomplete.pages[0], "Bring me the shells.");
        assert!(incomplete.pages[1].contains("Blue Snail Shell: 1/2"));

        player.inventory.as_mut().expect("inventory").stacks[0].quantity = 2;
        let ready = active_dialog(&player, &quest, &definitions, &scripts, 200);
        assert_eq!(ready.pages, vec!["You have everything."]);
        assert_eq!(ready.choices.len(), 1);
        assert_eq!(
            NpcDialogChoiceKind::try_from(ready.choices[0].kind),
            Ok(NpcDialogChoiceKind::CompleteQuest)
        );
    }

    #[test]
    fn selectable_reward_dialog_uses_authoritative_names_counts_and_eligibility() {
        let warrior_mask = 1 << 1;
        let quest = QuestDefinition {
            id: 100,
            name: "Choice".to_owned(),
            start: QuestStartRequirements::default(),
            completion: QuestCompletionRequirements::default(),
            start_actions: QuestActions::default(),
            completion_actions: QuestActions {
                selectable_items: vec![
                    QuestSelectableItemReward {
                        item_id: 1_332_005,
                        count: 2,
                        expiration: Some(QuestItemExpiration::RelativeMilliseconds(3_600_000)),
                        eligibility: QuestRewardEligibility {
                            job_mask: Some(warrior_mask),
                            gender: Some(QuestRewardGender::Male),
                        },
                    },
                    QuestSelectableItemReward {
                        item_id: 1_332_007,
                        count: 1,
                        expiration: None,
                        eligibility: QuestRewardEligibility {
                            job_mask: Some(warrior_mask),
                            gender: Some(QuestRewardGender::Female),
                        },
                    },
                    QuestSelectableItemReward {
                        item_id: 4_000_000,
                        count: 3,
                        expiration: Some(QuestItemExpiration::AbsoluteUnixMilliseconds(
                            1_893_484_800_000,
                        )),
                        eligibility: QuestRewardEligibility {
                            job_mask: Some(warrior_mask),
                            gender: Some(QuestRewardGender::Male),
                        },
                    },
                ],
                ..QuestActions::default()
            },
            dialogue: QuestDialogue {
                completion: QuestCompletionDialogue {
                    pages: vec!["Choose one.".to_owned()],
                    ..QuestCompletionDialogue::default()
                },
                ..QuestDialogue::default()
            },
            info: QuestInfo::default(),
        };
        let definitions = vec![
            ItemDefinition {
                item_id: 1_332_005,
                name: "Dagger A".to_owned(),
                stack_max: 1,
                ..ItemDefinition::default()
            },
            ItemDefinition {
                item_id: 1_332_007,
                name: "Dagger B".to_owned(),
                stack_max: 1,
                ..ItemDefinition::default()
            },
            ItemDefinition {
                item_id: 4_000_000,
                name: "Blue Snail Shell".to_owned(),
                stack_max: 10,
                ..ItemDefinition::default()
            },
        ];
        let mut player = PlayerState {
            stats: Some(CharacterStats {
                job_id: 112,
                ..CharacterStats::default()
            }),
            appearance: Some(CharacterAppearance {
                gender: CharacterGender::Male as i32,
                ..CharacterAppearance::default()
            }),
            quests: vec![PlayerQuest {
                quest_id: quest.id,
                status: QuestStatus::Started as i32,
                ..PlayerQuest::default()
            }],
            ..PlayerState::default()
        };
        let scripts = crate::quest_scripts::QuestScriptCatalog::default();

        let dialog = active_dialog(&player, &quest, &definitions, &scripts, 200);

        assert_eq!(
            dialog
                .choices
                .iter()
                .map(|choice| choice.label.as_str())
                .collect::<Vec<_>>(),
            vec![
                "Dagger A x2 (expires 60 minutes after receipt)",
                "Blue Snail Shell x3 (expires 2030-01-01 00:00 PST (GMS))",
            ]
        );
        assert!(dialog.choices.iter().all(|choice| {
            NpcDialogChoiceKind::try_from(choice.kind) == Ok(NpcDialogChoiceKind::SelectQuestReward)
        }));
        assert!(dialog.choices.iter().all(|choice| {
            choice.choice_id != crate::quests::ACCEPT_CHOICE_ID
                && choice.choice_id != crate::quests::DECLINE_CHOICE_ID
                && choice.choice_id != crate::quests::COMPLETE_CHOICE_ID
                && Some(choice.choice_id)
                    != crate::quests::answer_choice_id(
                        crate::quests::QuestQuestionPhase::Start,
                        0,
                        0,
                    )
        }));

        player.stats.as_mut().expect("stats").job_id = 212;
        let blocked = active_dialog(&player, &quest, &definitions, &scripts, 200);
        assert!(blocked.choices.is_empty());
        assert!(blocked.pages.last().is_some_and(|page| {
            page == "No selectable quest reward is available for your character."
        }));
    }

    #[test]
    fn absolute_reward_deadlines_use_the_gms_timezone() {
        assert_eq!(
            format_gms_deadline(1_893_484_800_000).as_deref(),
            Some("2030-01-01 00:00 PST (GMS)")
        );
        assert_eq!(
            format_gms_deadline(1_909_119_600_000).as_deref(),
            Some("2030-07-01 00:00 PDT (GMS)")
        );
    }

    #[test]
    fn active_quest_with_a_missing_start_item_shows_the_lost_interaction() {
        let mut quest = QuestDefinition {
            id: 100,
            name: "Lost item".to_owned(),
            start: QuestStartRequirements::default(),
            completion: QuestCompletionRequirements::default(),
            start_actions: QuestActions::default(),
            completion_actions: QuestActions::default(),
            dialogue: QuestDialogue {
                completion: QuestCompletionDialogue {
                    lost: Some(QuestLostItemDialogue {
                        prompt_pages: vec!["Did you lose the shells?".to_owned()],
                        success_pages: vec!["Take these replacements.".to_owned()],
                        items: vec![QuestRestorableItem {
                            item_id: 4_000_000,
                            target_count: 2,
                            expiration: None,
                            provenance:
                                crate::content::QuestRestorationProvenance::InferredStartGrant,
                            eligibility: crate::content::QuestRestorationEligibility {
                                owner_state: crate::content::RequiredQuestState::Started,
                                required_quests: &[],
                                forbidden_quests: &[],
                                absent_skill_ids: &[],
                                absent_item_ids: &[],
                            },
                        }],
                    }),
                    ..QuestCompletionDialogue::default()
                },
                ..QuestDialogue::default()
            },
            info: QuestInfo::default(),
        };
        let definitions = vec![ItemDefinition {
            item_id: 4_000_000,
            name: "Blue Snail Shell".to_owned(),
            stack_max: 10,
            ..ItemDefinition::default()
        }];
        let player = PlayerState {
            inventory: Some(InventoryState {
                capacity: 1,
                stacks: vec![InventoryItemStack {
                    item_id: 4_000_000,
                    quantity: 1,
                    expires_at_unix_ms: 0,
                }],
                ..InventoryState::default()
            }),
            quests: vec![PlayerQuest {
                quest_id: quest.id,
                status: QuestStatus::Started as i32,
                ..PlayerQuest::default()
            }],
            ..PlayerState::default()
        };

        let dialog = active_dialog(
            &player,
            &quest,
            &definitions,
            &crate::quest_scripts::QuestScriptCatalog::default(),
            200,
        );

        assert_eq!(dialog.pages, vec!["Did you lose the shells?"]);
        assert_eq!(dialog.choices.len(), 1);
        assert_eq!(
            dialog.choices[0].choice_id,
            crate::quests::RESTORE_ITEMS_CHOICE_ID
        );
        assert_eq!(dialog.choices[0].label, "Restore items");
        assert_eq!(
            NpcDialogChoiceKind::try_from(dialog.choices[0].kind),
            Ok(NpcDialogChoiceKind::RestoreQuestItems)
        );

        quest
            .dialogue
            .completion
            .lost
            .as_mut()
            .expect("lost interaction")
            .items[0]
            .expiration = Some(QuestItemExpiration::AbsoluteUnixMilliseconds(200));
        let expired = active_dialog(
            &player,
            &quest,
            &definitions,
            &crate::quest_scripts::QuestScriptCatalog::default(),
            200,
        );
        assert_eq!(expired.choices.len(), 1);
        assert_eq!(
            expired.choices[0].choice_id,
            crate::quests::COMPLETE_CHOICE_ID
        );
    }
}
