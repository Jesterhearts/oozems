use axum::body::Bytes;
use axum::extract::State;
use axum::http::HeaderMap;
use oozems_proto::v1::Npc;
use oozems_proto::v1::NpcInteraction;
use oozems_proto::v1::NpcInteractionRequest;
use oozems_proto::v1::NpcInteractionResponse;
use oozems_proto::v1::PlayerState;
use oozems_proto::v1::npc_interaction;
use oozems_proto::v1::npc_interaction_request;

use super::ApiError;
use super::Protobuf;
use super::begin_player_mutation;
use super::decode_request;
use super::load_map;
use super::lock_player;
use super::parse_player_id;
use super::unix_time_ms;
use crate::app::AppState;

mod quest;
mod shop;
mod taxi;

#[cfg(test)]
mod tests;

const NPC_HORIZONTAL_REACH: f32 = 320.0;
const NPC_VERTICAL_REACH: f32 = 180.0;

pub async fn interact(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Protobuf<NpcInteractionResponse>, ApiError> {
    let request: NpcInteractionRequest = decode_request(&headers, body)?;
    let player_id = parse_player_id(&request.player_id)?;
    let player_guard = lock_player(&state, &player_id, &headers).await?;
    let now_unix_ms = unix_time_ms()?;
    let mutation = begin_player_mutation(&state, &player_guard, &player_id, now_unix_ms).await?;
    super::require_living_player(&mutation.player, "interact with NPCs")?;
    if request.map_id != mutation.player.map_id {
        return Err(invalid(
            "the requested NPC is not on the player's current map",
        ));
    }
    let mut map = load_map(&state, mutation.player.map_id)
        .await?
        .ok_or_else(|| ApiError::not_found("map_not_found", "the current map does not exist"))?;
    let npc = map
        .npcs
        .iter()
        .find(|npc| npc.spawn_id == request.npc_spawn_id)
        .cloned()
        .ok_or_else(|| invalid("the requested NPC spawn does not exist"))?;
    validate_reach(&mutation.player, &npc)?;
    let action = request
        .action
        .ok_or_else(|| invalid("the request does not contain an NPC action"))?;
    let quest_definitions = state.catalog.quest_definitions().collect::<Vec<_>>();

    let mut response = match action {
        npc_interaction_request::Action::Open(_) => {
            quest::open_interaction(
                &state,
                &player_guard,
                mutation,
                &npc,
                &quest_definitions,
                now_unix_ms,
            )
            .await?
        }
        npc_interaction_request::Action::SelectChoice(action) => {
            quest::select_choice(
                &state,
                &player_guard,
                mutation,
                &npc,
                &quest_definitions,
                action.quest_id,
                action.choice_id,
                now_unix_ms,
            )
            .await?
        }
        npc_interaction_request::Action::Buy(action) => {
            shop::buy_item(&state, &player_guard, mutation, &npc, action.item_id).await?
        }
        npc_interaction_request::Action::Sell(action) => {
            shop::sell_item(
                &state,
                &player_guard,
                mutation,
                &npc,
                action.inventory_index,
                action.expected_item_id,
                action.expected_expires_at_unix_ms,
            )
            .await?
        }
        npc_interaction_request::Action::TakeTaxi(action) => {
            taxi::take_taxi(
                &state,
                &player_guard,
                mutation,
                &map,
                &npc,
                action.target_map_id,
                now_unix_ms,
            )
            .await?
        }
    };
    let effects = crate::effects::snapshot(&state.active_effects, player_id.as_str(), now_unix_ms)?;
    let response_player = response
        .player
        .as_ref()
        .ok_or_else(|| ApiError::PlayerData("NPC response does not contain a player".to_owned()))?;
    let environment = crate::quests::QuestEnvironment {
        now_unix_ms,
        world_id: state.gameplay.world_id,
    };
    let indicator_map = response.map.as_mut().unwrap_or(&mut map);
    response.quest_indicators = crate::quests::project_npc_quest_indicators(
        indicator_map,
        response_player,
        &effects,
        &quest_definitions,
        state.catalog.item_definition_slice(),
        &state.quest_scripts,
        environment,
    );
    response.active_buffs = Some(crate::effects::state(&effects, now_unix_ms));
    Ok(Protobuf(response))
}

pub(super) fn interaction(
    map_id: u32,
    npc: &Npc,
    mut view: npc_interaction::View,
) -> NpcInteraction {
    if let npc_interaction::View::Dialog(dialog) = &mut view {
        dialog.title = normalize_dialog_text(&dialog.title);
        for page in &mut dialog.pages {
            *page = normalize_dialog_text(page);
        }
        for choice in &mut dialog.choices {
            choice.label = normalize_dialog_text(&choice.label);
        }
    }
    NpcInteraction {
        map_id,
        npc_spawn_id: npc.spawn_id,
        npc_id: npc.npc_id,
        npc_name: normalize_dialog_text(&npc.name),
        view: Some(view),
    }
}

fn normalize_dialog_text(source: &str) -> String {
    source
        .replace("\\r", "\r")
        .replace("\\n", "\n")
        .replace("\r\n", "\n")
        .replace('\r', "\n")
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

pub(super) fn invalid(message: impl Into<String>) -> ApiError {
    ApiError::bad_request("invalid_npc_interaction", message)
}
