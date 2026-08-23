use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use oozems_proto::v1::CharacterSpriteSet;
use oozems_proto::v1::ItemActionResponse;
use oozems_proto::v1::PlayerState;
use wasm_bindgen_futures::spawn_local;

use super::Game;
use crate::api;
use crate::assets;
use crate::assets::BrowserAsset;
use crate::game_gui::GuiAction;
use crate::show_status;

pub(super) fn begin(
    game: Rc<RefCell<Game>>,
    action: GuiAction,
) {
    if super::recovery_actions::is_in_flight(&game.borrow().recovery_state) {
        show_status("Recovery is still being saved.", true);
        return;
    }
    super::recovery_actions::reset(&mut game.borrow_mut().recovery_state);
    let in_flight = game.borrow().item_action_in_flight.clone();
    if in_flight.replace(true) {
        show_status("An item action is already in progress.", true);
        return;
    }
    let player_id = game.borrow().player.id.clone();
    spawn_local(async move {
        let result = request_item_action(&player_id, action).await;
        match result {
            Ok(response) => match prepare_item_action_update(action, response).await {
                Ok(update) => {
                    let warning = update.warning.clone();
                    install_item_action_update(&mut game.borrow_mut(), update);
                    match warning {
                        Some(warning) => show_status(&warning, true),
                        None => show_status(item_action_message(action), false),
                    }
                }
                Err(error) => show_status(&format!("Item action could not finish: {error}"), true),
            },
            Err(error) => show_status(&format!("Item action failed: {error}"), true),
        }
        in_flight.set(false);
    });
}

pub(super) fn begin_pick_up(game: Rc<RefCell<Game>>) {
    if super::recovery_actions::is_in_flight(&game.borrow().recovery_state) {
        return;
    }
    super::recovery_actions::reset(&mut game.borrow_mut().recovery_state);
    let in_flight = game.borrow().item_action_in_flight.clone();
    if in_flight.replace(true) {
        return;
    }
    let request = {
        let game = game.borrow();
        (game.player.id.clone(), game.map.id)
    };
    spawn_local(async move {
        match api::pick_up_item(&request.0, request.1).await {
            Ok(response) => {
                let result = response
                    .player
                    .ok_or("server pickup response did not contain a player")
                    .and_then(|player| {
                        (!response.picked_up_drop_id.is_empty())
                            .then_some((player, response.picked_up_drop_id))
                            .ok_or("server pickup response did not identify the dropped item")
                    });
                match result {
                    Ok((player, drop_id)) => {
                        let mut game = game.borrow_mut();
                        game.player.inventory = player.inventory;
                        game.map.dropped_items.retain(|drop| drop.id != drop_id);
                        show_status("Item picked up.", false);
                    }
                    Err(error) => show_status(&format!("Pickup could not finish: {error}"), true),
                }
            }
            Err(error) => show_status(&format!("Pickup failed: {error}"), true),
        }
        in_flight.set(false);
    });
}

async fn request_item_action(
    player_id: &str,
    action: GuiAction,
) -> Result<ItemActionResponse, api::ClientError> {
    match action {
        GuiAction::Equip { inventory_index } => api::equip_item(player_id, inventory_index).await,
        GuiAction::Unequip { slot } => api::unequip_item(player_id, slot).await,
        GuiAction::Drop { inventory_index } => api::drop_item(player_id, inventory_index).await,
        GuiAction::ToggleStats
        | GuiAction::ToggleEquipment
        | GuiAction::ToggleInventory
        | GuiAction::ToggleKeyConfig
        | GuiAction::ToggleSkills
        | GuiAction::PreviousSkillPage
        | GuiAction::NextSkillPage
        | GuiAction::CloseStats
        | GuiAction::CloseEquipment
        | GuiAction::CloseInventory
        | GuiAction::CloseKeyConfig
        | GuiAction::CloseSkills
        | GuiAction::AllocateSkill { .. }
        | GuiAction::UseSkill { .. } => unreachable!("non-item GUI action reached the item server"),
    }
}

struct ItemActionUpdate {
    player: PlayerState,
    dropped_item: Option<oozems_proto::v1::DroppedItem>,
    sprites: Option<CharacterSpriteSet>,
    images: Option<HashMap<String, BrowserAsset>>,
    warning: Option<String>,
}

async fn prepare_item_action_update(
    action: GuiAction,
    response: ItemActionResponse,
) -> Result<ItemActionUpdate, String> {
    let player = response
        .player
        .ok_or("server item response did not contain a player")?;
    let mut sprites = None;
    let mut images = None;
    let mut warning = None;
    if matches!(action, GuiAction::Equip { .. } | GuiAction::Unequip { .. }) {
        let appearance = player
            .appearance
            .ok_or("server item response did not contain an appearance")?;
        let equipment = player
            .inventory
            .as_ref()
            .map(|inventory| inventory.equipment.as_slice())
            .unwrap_or_default();
        match api::get_character_sprites(appearance, Some(equipment)).await {
            Ok(next_sprites) => match assets::prepare_assets(next_sprites.assets.iter()) {
                Ok(next_images) => {
                    sprites = Some(next_sprites);
                    images = Some(next_images);
                }
                Err(error) => {
                    warning = Some(format!(
                        "Item change was saved, but character assets could not refresh: {error}"
                    ));
                }
            },
            Err(error) => {
                warning = Some(format!(
                    "Item change was saved, but character sprites could not refresh: {error}"
                ));
            }
        }
    }
    Ok(ItemActionUpdate {
        player,
        dropped_item: response.dropped_item,
        sprites,
        images,
        warning,
    })
}

fn install_item_action_update(
    game: &mut Game,
    update: ItemActionUpdate,
) {
    game.player.inventory = update.player.inventory;
    if let (Some(sprites), Some(images)) = (update.sprites, update.images) {
        assets::merge_assets(&mut game.images, images);
        game.character_sprites = sprites;
        super::restart_character_animation(&mut game.character_animation, game.frame_time_ms);
    }
    if let Some(drop) = update.dropped_item
        && update.player.map_id == game.map.id
        && drop.despawn_at_unix_ms > js_sys::Date::now().max(0.0) as u64
    {
        game.map.dropped_items.push(drop);
    }
}

fn item_action_message(action: GuiAction) -> &'static str {
    match action {
        GuiAction::Equip { .. } => "Item equipped.",
        GuiAction::Unequip { .. } => "Item moved to inventory.",
        GuiAction::Drop { .. } => "Item dropped.",
        GuiAction::ToggleStats
        | GuiAction::ToggleEquipment
        | GuiAction::ToggleInventory
        | GuiAction::ToggleKeyConfig
        | GuiAction::ToggleSkills
        | GuiAction::PreviousSkillPage
        | GuiAction::NextSkillPage
        | GuiAction::CloseStats
        | GuiAction::CloseEquipment
        | GuiAction::CloseInventory
        | GuiAction::CloseKeyConfig
        | GuiAction::CloseSkills
        | GuiAction::AllocateSkill { .. }
        | GuiAction::UseSkill { .. } => "GUI updated.",
    }
}
