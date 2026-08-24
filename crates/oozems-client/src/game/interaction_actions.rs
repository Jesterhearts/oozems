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
                let Some(stack) = game
                    .player
                    .inventory
                    .as_ref()
                    .and_then(|inventory| inventory.stacks.get(index))
                else {
                    show_status("The selected inventory item is no longer available.", true);
                    return;
                };
                npc_interaction_request::Action::Sell(SellShopItemAction {
                    inventory_index: u32::try_from(index).unwrap_or(u32::MAX),
                    expected_item_id: stack.item_id,
                    expected_expires_at_unix_ms: stack.expires_at_unix_ms,
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
            | InteractionUiAction::PreviousChoicePage
            | InteractionUiAction::NextChoicePage
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
    let (in_flight, generation, source_map_id, learned_skills) = {
        let game = game.borrow();
        (
            game.interaction.in_flight.clone(),
            game.interaction.generation,
            game.map.id,
            game.player.learned_skills.clone(),
        )
    };
    if in_flight.replace(true) {
        show_status("An NPC action is already in progress.", true);
        return;
    }
    spawn_local(async move {
        let request_started_ms = super::monotonic_time_ms();
        let result = request_and_prepare(
            request,
            generation,
            source_map_id,
            learned_skills,
            request_started_ms,
        )
        .await;
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
    skill_book: Option<(api::LoadedSkillBook, f64)>,
    generation: u64,
    source_map_id: u32,
    request_started_ms: f64,
}

async fn request_and_prepare(
    request: NpcInteractionRequest,
    generation: u64,
    source_map_id: u32,
    learned_skills: Vec<oozems_proto::v1::LearnedSkill>,
    request_started_ms: f64,
) -> Result<InteractionUpdate, String> {
    let response = api::interact_npc(request)
        .await
        .map_err(|error| error.to_string())?;
    let skill_book = match response.player.as_ref() {
        Some(player) if player.learned_skills != learned_skills => {
            let skill_requested_at_ms = super::monotonic_time_ms();
            api::get_skill_book(&player.id)
                .await
                .ok()
                .map(|loaded| (loaded, skill_requested_at_ms))
        }
        _ => None,
    };
    Ok(InteractionUpdate {
        response,
        skill_book,
        generation,
        source_map_id,
        request_started_ms,
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
    let response_player_revision = player.revision;
    let npc_animation = update.response.npc_animation.take();
    let context_is_current =
        game.interaction.generation == update.generation && game.map.id == update.source_map_id;
    let installed = super::install_full_player_update(game, player);
    super::install_active_buffs(
        game,
        update.response.active_buffs.take().unwrap_or_default(),
        update.request_started_ms,
    );
    if installed.domains.skills {
        if let Some((mut loaded, skill_requested_at_ms)) = update.skill_book.take() {
            game.skill_book = loaded.skill_book;
            super::install_active_buffs(
                game,
                std::mem::take(&mut loaded.active_buffs),
                skill_requested_at_ms,
            );
        }
    }
    let relocation_requested =
        update.response.map.is_some() || update.response.authoritative.is_some();
    let relocated = match (
        update.response.map.take(),
        update.response.authoritative.take(),
    ) {
        (Some(map), Some(authoritative)) => {
            super::movement_actions::install_relocation(game, map, authoritative)?
        }
        (None, None) => false,
        _ => return Err("NPC response contains an incomplete map transition".to_owned()),
    };
    if context_is_current && !relocation_requested {
        game.interaction.install(update.response.interaction);
        if let Some(event) = npc_animation
            && let Err(error) = crate::render::npc::install_event(
                &mut game.npc_animations,
                &game.map,
                event,
                response_player_revision,
                game.player.revision,
                game.frame_time_ms,
            )
        {
            return Err(error);
        }
    }
    Ok(if relocated {
        "Travel complete."
    } else if relocation_requested {
        "Travel response was superseded by newer movement."
    } else {
        "NPC interaction updated."
    })
}
