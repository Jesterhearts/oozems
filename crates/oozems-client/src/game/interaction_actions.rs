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

use super::Game;
use crate::api;
use crate::interaction_ui::InteractionUiAction;
use crate::show_status;

pub(super) fn begin_open(
    game: Rc<RefCell<Game>>,
    npc_spawn_id: u32,
    permit: super::requests::RequestPermit,
) {
    let request = {
        let game = game.borrow();
        NpcInteractionRequest {
            player_id: game.player.id.clone(),
            map_id: game.world.map.id,
            npc_spawn_id,
            action: Some(npc_interaction_request::Action::Open(OpenNpcAction {})),
        }
    };
    begin_request(game, request, "NPC interaction", permit);
}

pub(super) fn begin_action(
    game: Rc<RefCell<Game>>,
    action: InteractionUiAction,
    permit: super::requests::RequestPermit,
) {
    let request = {
        let game = game.borrow();
        let Some(interaction) = game.ui.interaction.interaction.as_ref() else {
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
                    .ui
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
                let Some(npc_interaction::View::Shop(shop)) = interaction.view.as_ref() else {
                    return;
                };
                if crate::interaction_ui::is_cash_point_shop(shop) {
                    show_status("Cash-point shops do not buy items.", true);
                    return;
                }
                let Some(index) = game.ui.interaction.selected_inventory else {
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
    begin_request(game, request, "NPC action", permit);
}

fn begin_request(
    game: Rc<RefCell<Game>>,
    request: NpcInteractionRequest,
    context: &'static str,
    permit: super::requests::RequestPermit,
) {
    let (generation, source_map_id, learned_skills) = {
        let game = game.borrow();
        (
            game.ui.interaction.generation,
            game.world.map.id,
            game.player.learned_skills.clone(),
        )
    };
    super::requests::spawn_request(
        game,
        permit,
        move || async move {
            let request_started_ms = super::monotonic_time_ms();
            request_and_prepare(
                request,
                generation,
                source_map_id,
                learned_skills,
                request_started_ms,
            )
            .await
        },
        move |game, result, _| match result {
            Ok(update) => match install_response(game, update) {
                Ok(message) => super::requests::RequestStatus::success(message),
                Err(error) => super::requests::RequestStatus::error(format!(
                    "{context} could not finish: {error}"
                )),
            },
            Err(error) => {
                super::requests::RequestStatus::error(format!("{context} failed: {error}"))
            }
        },
    );
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
    let (player, active_buffs) =
        super::responses::take_player_and_active_buffs(&mut update.response)?;
    let skill_active_buffs = update
        .skill_book
        .as_mut()
        .map(|(loaded, _)| super::buffs::validate_state(std::mem::take(&mut loaded.active_buffs)))
        .transpose()?;
    let response_player_revision = player.revision;
    let npc_animation = update.response.npc_animation.take();
    let context_is_current = game.ui.interaction.generation == update.generation
        && game.world.map.id == update.source_map_id;
    let installed = super::install_full_player_update(game, player);
    super::install_active_buffs(game, active_buffs, update.request_started_ms);
    if installed.domains.skills
        && let Some(((loaded, skill_requested_at_ms), skill_active_buffs)) =
            update.skill_book.take().zip(skill_active_buffs)
    {
        game.player.skill_book = loaded.skill_book;
        super::install_active_buffs(game, skill_active_buffs, skill_requested_at_ms);
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
    if (context_is_current && !relocation_requested) || relocated {
        super::install_quest_indicators(game, &update.response.quest_indicators);
    }
    if context_is_current && !relocation_requested {
        game.ui.interaction.install(update.response.interaction);
        if let Some(event) = npc_animation
            && let Err(error) = crate::render::npc::install_event(
                &mut game.world.npc_animations,
                &game.world.map,
                event,
                response_player_revision,
                game.player.revision,
                game.clock.now_ms,
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
