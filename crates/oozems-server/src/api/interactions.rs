use axum::body::Bytes;
use axum::extract::State;
use axum::http::HeaderMap;
use oozems_proto::v1::Npc;
use oozems_proto::v1::NpcDialogChoice;
use oozems_proto::v1::NpcDialogChoiceKind;
use oozems_proto::v1::NpcDialogView;
use oozems_proto::v1::NpcInteraction;
use oozems_proto::v1::NpcInteractionRequest;
use oozems_proto::v1::NpcInteractionResponse;
use oozems_proto::v1::NpcShopOffer;
use oozems_proto::v1::NpcShopView;
use oozems_proto::v1::NpcTaxiDestination;
use oozems_proto::v1::NpcTaxiView;
use oozems_proto::v1::PlayerState;
use oozems_proto::v1::npc_interaction;
use oozems_proto::v1::npc_interaction_request;

use super::ApiError;
use super::Protobuf;
use super::decode_request;
use super::item_rule_error;
use super::load_map;
use super::lock_player;
use super::parse_player_id;
use super::record_recovery_activity;
use super::require_player;
use super::unix_time_ms;
use crate::app::AppState;
use crate::content::QuestDefinition;
use crate::quests::QuestProgress;

const NPC_HORIZONTAL_REACH: f32 = 320.0;
const NPC_VERTICAL_REACH: f32 = 180.0;

pub async fn interact(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Protobuf<NpcInteractionResponse>, ApiError> {
    let request: NpcInteractionRequest = decode_request(&headers, body)?;
    let player_id = parse_player_id(&request.player_id)?;
    let _player_guard = lock_player(&state, &player_id).await?;
    let current = require_player(&state, &player_id).await?;
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

    let response = match action {
        npc_interaction_request::Action::Open(_) => NpcInteractionResponse {
            player: Some(current.clone()),
            interaction: Some(open_interaction(&state, &current, &npc)),
            authoritative: None,
            map: None,
        },
        npc_interaction_request::Action::SelectChoice(action) => {
            select_choice(&state, current, &npc, action.quest_id, action.choice_id).await?
        }
        npc_interaction_request::Action::Buy(action) => {
            buy_item(&state, current, &npc, action.item_id).await?
        }
        npc_interaction_request::Action::Sell(action) => {
            sell_item(&state, current, &npc, action.inventory_index).await?
        }
        npc_interaction_request::Action::TakeTaxi(action) => {
            take_taxi(&state, current, &map, &npc, action.target_map_id).await?
        }
    };
    Ok(Protobuf(response))
}

async fn select_choice(
    state: &AppState,
    current: PlayerState,
    npc: &Npc,
    quest_id: u32,
    choice_id: u32,
) -> Result<NpcInteractionResponse, ApiError> {
    let quest = state
        .catalog
        .quest(quest_id)
        .ok_or_else(|| invalid("the selected quest is not available"))?;
    let selection = crate::quests::select_choice(
        current,
        quest,
        npc.npc_id,
        choice_id,
        state.experience.default_curve(),
    )
    .map_err(quest_rule_error)?;
    let player = if selection.changed {
        crate::database::save_player(&state.database, &selection.player).await?
    } else {
        selection.player
    };
    if selection.changed {
        record_recovery_activity(state, &player.id, unix_time_ms()?);
    }
    Ok(NpcInteractionResponse {
        interaction: Some(interaction(
            player.map_id,
            npc,
            npc_interaction::View::Dialog(NpcDialogView {
                quest_id,
                title: quest.name.clone(),
                pages: selection.pages,
                choices: Vec::new(),
            }),
        )),
        player: Some(player),
        authoritative: None,
        map: None,
    })
}

async fn buy_item(
    state: &AppState,
    current: PlayerState,
    npc: &Npc,
    item_id: u32,
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
        &state.catalog.item_definitions(),
    )
    .map_err(item_rule_error)?;
    let player = crate::database::save_player(&state.database, &player).await?;
    record_recovery_activity(state, &player.id, unix_time_ms()?);
    Ok(shop_response(state, player, npc, shop))
}

async fn sell_item(
    state: &AppState,
    current: PlayerState,
    npc: &Npc,
    inventory_index: u32,
) -> Result<NpcInteractionResponse, ApiError> {
    let shop = state
        .interactions
        .shop(current.map_id, npc.spawn_id)
        .ok_or_else(|| invalid("this NPC does not operate a shop"))?;
    let player = crate::items::sell_inventory_item(
        current,
        inventory_index,
        &state.catalog.item_definitions(),
    )
    .map_err(item_rule_error)?;
    let player = crate::database::save_player(&state.database, &player).await?;
    record_recovery_activity(state, &player.id, unix_time_ms()?);
    Ok(shop_response(state, player, npc, shop))
}

async fn take_taxi(
    state: &AppState,
    current: PlayerState,
    source_map: &oozems_proto::v1::Map,
    npc: &Npc,
    target_map_id: u32,
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
    let now_ms = unix_time_ms()?;
    let (decision, rollback) = crate::movement::relocate_player(
        &state.movement,
        &current,
        source_map,
        &target_map,
        &destination.portal_name,
        state.gameplay.movement,
        now_ms,
    )?;
    let player = match crate::database::save_player(&state.database, &player).await {
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
    if let Err(error) = crate::movement::mark_persisted(&state.movement, &player.id, now_ms) {
        tracing::error!(%error, "taxi movement was persisted but could not be marked clean");
    }
    record_recovery_activity(state, &player.id, now_ms);
    Ok(NpcInteractionResponse {
        player: Some(player),
        interaction: None,
        authoritative: Some(decision.authoritative),
        map: Some(target_map),
    })
}

fn open_interaction(
    state: &AppState,
    player: &PlayerState,
    npc: &Npc,
) -> NpcInteraction {
    let mut quests = state.catalog.quests_for_npc(npc.npc_id);
    quests.sort_by_key(|quest| quest.id);
    if let Some(quest) = quests.iter().copied().find(|quest| {
        crate::quests::progress(player, quest.id) == QuestProgress::Started
            && quest.completion_npc_id == npc.npc_id
    }) {
        return interaction(
            player.map_id,
            npc,
            npc_interaction::View::Dialog(active_quest_dialog(quest)),
        );
    }
    if let Some(quest) = quests.iter().copied().find(|quest| {
        quest.start_npc_id == npc.npc_id && crate::quests::is_available(player, quest)
    }) {
        return interaction(
            player.map_id,
            npc,
            npc_interaction::View::Dialog(quest_offer_dialog(quest)),
        );
    }
    if let Some(shop) = state.interactions.shop(player.map_id, npc.spawn_id) {
        return interaction(
            player.map_id,
            npc,
            npc_interaction::View::Shop(shop_view(shop)),
        );
    }
    if let Some(taxi) = state.interactions.taxi(player.map_id, npc.spawn_id) {
        return interaction(
            player.map_id,
            npc,
            npc_interaction::View::Taxi(taxi_view(taxi)),
        );
    }
    interaction(
        player.map_id,
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
    )
}

fn quest_offer_dialog(quest: &QuestDefinition) -> NpcDialogView {
    NpcDialogView {
        quest_id: quest.id,
        title: quest.name.clone(),
        pages: quest.offer_pages.clone(),
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

fn active_quest_dialog(quest: &QuestDefinition) -> NpcDialogView {
    let Some(question) = &quest.question else {
        return NpcDialogView {
            quest_id: quest.id,
            title: quest.name.clone(),
            pages: vec!["This quest cannot be completed here yet.".to_owned()],
            choices: Vec::new(),
        };
    };
    NpcDialogView {
        quest_id: quest.id,
        title: quest.name.clone(),
        pages: vec![question.prompt.clone()],
        choices: question
            .choices
            .iter()
            .map(|choice| NpcDialogChoice {
                choice_id: crate::quests::answer_choice_id(choice.id),
                label: choice.label.clone(),
                kind: NpcDialogChoiceKind::Answer as i32,
            })
            .collect(),
    }
}

fn shop_response(
    _state: &AppState,
    player: PlayerState,
    npc: &Npc,
    shop: &crate::interactions::ShopDefinition,
) -> NpcInteractionResponse {
    NpcInteractionResponse {
        interaction: Some(interaction(
            player.map_id,
            npc,
            npc_interaction::View::Shop(shop_view(shop)),
        )),
        player: Some(player),
        authoritative: None,
        map: None,
    }
}

fn shop_view(shop: &crate::interactions::ShopDefinition) -> NpcShopView {
    NpcShopView {
        offers: shop
            .offers
            .iter()
            .map(|offer| NpcShopOffer {
                item_id: offer.item_id,
                buy_price: offer.buy_price,
            })
            .collect(),
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
        _ => invalid(error.to_string()),
    }
}

fn invalid(message: impl Into<String>) -> ApiError {
    ApiError::bad_request("invalid_npc_interaction", message)
}

#[cfg(test)]
mod tests {
    use oozems_proto::v1::Npc;
    use oozems_proto::v1::PlayerState;
    use oozems_proto::v1::Vec2;

    use super::validate_reach;

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
}
