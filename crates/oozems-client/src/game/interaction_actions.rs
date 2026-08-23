use std::cell::RefCell;
use std::rc::Rc;

use oozems_proto::v1::BuyShopItemAction;
use oozems_proto::v1::NpcInteractionRequest;
use oozems_proto::v1::NpcInteractionResponse;
use oozems_proto::v1::OpenNpcAction;
use oozems_proto::v1::SelectNpcChoiceAction;
use oozems_proto::v1::SellShopItemAction;
use oozems_proto::v1::TakeTaxiAction;
use oozems_proto::v1::npc_interaction;
use oozems_proto::v1::npc_interaction_request;
use wasm_bindgen_futures::spawn_local;

use super::Game;
use crate::api;
use crate::interaction_ui::InteractionUiAction;
use crate::show_status;

pub(super) fn begin_open(
    game: Rc<RefCell<Game>>,
    npc_spawn_id: u32,
) {
    let request = {
        let game = game.borrow();
        NpcInteractionRequest {
            player_id: game.player.id.clone(),
            map_id: game.map.id,
            npc_spawn_id,
            action: Some(npc_interaction_request::Action::Open(OpenNpcAction {})),
        }
    };
    begin_request(game, request, "NPC interaction");
}

pub(super) fn begin_action(
    game: Rc<RefCell<Game>>,
    action: InteractionUiAction,
) {
    let request = {
        let game = game.borrow();
        let Some(interaction) = game.interaction.interaction.as_ref() else {
            return;
        };
        let action = match action {
            InteractionUiAction::SelectChoice {
                quest_id,
                choice_id,
            } => npc_interaction_request::Action::SelectChoice(SelectNpcChoiceAction {
                quest_id,
                choice_id,
            }),
            InteractionUiAction::Buy => {
                let Some(npc_interaction::View::Shop(shop)) = interaction.view.as_ref() else {
                    return;
                };
                let Some(offer) = game
                    .interaction
                    .selected_offer
                    .and_then(|index| shop.offers.get(index))
                else {
                    show_status("Select a shop item first.", true);
                    return;
                };
                npc_interaction_request::Action::Buy(BuyShopItemAction {
                    item_id: offer.item_id,
                })
            }
            InteractionUiAction::Sell => {
                let Some(index) = game.interaction.selected_inventory else {
                    show_status("Select an inventory item first.", true);
                    return;
                };
                npc_interaction_request::Action::Sell(SellShopItemAction {
                    inventory_index: u32::try_from(index).unwrap_or(u32::MAX),
                })
            }
            InteractionUiAction::TakeTaxi { map_id } => {
                npc_interaction_request::Action::TakeTaxi(TakeTaxiAction {
                    target_map_id: map_id,
                })
            }
            InteractionUiAction::Consume
            | InteractionUiAction::Close
            | InteractionUiAction::PreviousPage
            | InteractionUiAction::NextPage
            | InteractionUiAction::SelectOffer { .. }
            | InteractionUiAction::SelectInventory { .. }
            | InteractionUiAction::PreviousInventoryPage
            | InteractionUiAction::NextInventoryPage => return,
        };
        NpcInteractionRequest {
            player_id: game.player.id.clone(),
            map_id: interaction.map_id,
            npc_spawn_id: interaction.npc_spawn_id,
            action: Some(action),
        }
    };
    begin_request(game, request, "NPC action");
}

fn begin_request(
    game: Rc<RefCell<Game>>,
    request: NpcInteractionRequest,
    context: &'static str,
) {
    let (in_flight, generation, source_map_id) = {
        let game = game.borrow();
        (
            game.interaction.in_flight.clone(),
            game.interaction.generation,
            game.map.id,
        )
    };
    if in_flight.replace(true) {
        show_status("An NPC action is already in progress.", true);
        return;
    }
    spawn_local(async move {
        let result = request_and_prepare(request, generation, source_map_id).await;
        match result {
            Ok(update) => match install_response(&mut game.borrow_mut(), update) {
                Ok(message) => show_status(message, false),
                Err(error) => show_status(&format!("{context} could not finish: {error}"), true),
            },
            Err(error) => show_status(&format!("{context} failed: {error}"), true),
        }
        in_flight.set(false);
    });
}

struct InteractionUpdate {
    response: NpcInteractionResponse,
    generation: u64,
    source_map_id: u32,
}

async fn request_and_prepare(
    request: NpcInteractionRequest,
    generation: u64,
    source_map_id: u32,
) -> Result<InteractionUpdate, String> {
    let response = api::interact_npc(request)
        .await
        .map_err(|error| error.to_string())?;
    Ok(InteractionUpdate {
        response,
        generation,
        source_map_id,
    })
}

fn install_response(
    game: &mut Game,
    mut update: InteractionUpdate,
) -> Result<&'static str, String> {
    let player = update
        .response
        .player
        .take()
        .ok_or("NPC response did not contain a player")?;
    let context_is_current =
        game.interaction.generation == update.generation && game.map.id == update.source_map_id;
    let relocated = match (
        update.response.map.take(),
        update.response.authoritative.take(),
    ) {
        (Some(map), Some(authoritative)) => {
            super::movement_actions::install_relocation(game, map, authoritative)?;
            true
        }
        (None, None) => false,
        _ => return Err("NPC response contains an incomplete map transition".to_owned()),
    };
    if relocated {
        game.player.map_id = player.map_id;
        game.player.position = player.position;
    }
    game.player.level = player.level;
    game.player.stats = player.stats;
    game.player.inventory = player.inventory;
    game.player.mesos = player.mesos;
    game.player.quests = player.quests;
    if context_is_current && !relocated {
        game.interaction.install(update.response.interaction);
    }
    Ok(if relocated {
        "Travel complete."
    } else {
        "NPC interaction updated."
    })
}
