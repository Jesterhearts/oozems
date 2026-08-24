use std::cell::RefCell;
use std::rc::Rc;

use oozems_proto::v1::InventoryItemStack;
use oozems_proto::v1::ItemActionResponse;
use wasm_bindgen_futures::spawn_local;

use super::Game;
use crate::api;
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
    let (player_id, expected_stack) = {
        let game = game.borrow();
        let expected_stack = match action {
            GuiAction::Equip { inventory_index } | GuiAction::Drop { inventory_index } => game
                .player
                .inventory
                .as_ref()
                .and_then(|inventory| inventory.stacks.get(inventory_index as usize))
                .cloned(),
            GuiAction::Unequip { .. } => None,
            _ => unreachable!("non-item GUI action reached the item server"),
        };
        if matches!(action, GuiAction::Equip { .. } | GuiAction::Drop { .. })
            && expected_stack.is_none()
        {
            in_flight.set(false);
            show_status("The selected inventory item is no longer available.", true);
            return;
        }
        (game.player.id.clone(), expected_stack)
    };
    spawn_local(async move {
        let request_started_ms = super::monotonic_time_ms();
        let result = request_item_action(&player_id, action, expected_stack.as_ref()).await;
        match result {
            Ok(response) => match install_item_action_update(
                &mut game.borrow_mut(),
                response,
                request_started_ms,
            ) {
                Ok(()) => show_status(item_action_message(action), false),
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
        let request_started_ms = super::monotonic_time_ms();
        match api::pick_up_item(&request.0, request.1).await {
            Ok(mut response) => {
                let result = response
                    .player
                    .take()
                    .ok_or("server pickup response did not contain a player")
                    .and_then(|player| {
                        (!response.picked_up_drop_id.is_empty())
                            .then_some((player, std::mem::take(&mut response.picked_up_drop_id)))
                            .ok_or("server pickup response did not identify the dropped item")
                    });
                match result {
                    Ok((player, drop_id)) => {
                        let mut game = game.borrow_mut();
                        super::install_full_player_update(&mut game, player);
                        super::install_active_buffs(
                            &mut game,
                            response.active_buffs.take().unwrap_or_default(),
                            request_started_ms,
                        );
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
    expected_stack: Option<&InventoryItemStack>,
) -> Result<ItemActionResponse, api::ClientError> {
    match action {
        GuiAction::Equip { inventory_index } => {
            let stack = expected_stack.expect("equip captures its selected stack");
            api::equip_item(
                player_id,
                inventory_index,
                stack.item_id,
                stack.expires_at_unix_ms,
            )
            .await
        }
        GuiAction::Unequip { slot } => api::unequip_item(player_id, slot).await,
        GuiAction::Drop { inventory_index } => {
            let stack = expected_stack.expect("drop captures its selected stack");
            api::drop_item(
                player_id,
                inventory_index,
                stack.item_id,
                stack.expires_at_unix_ms,
            )
            .await
        }
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

fn install_item_action_update(
    game: &mut Game,
    mut response: ItemActionResponse,
    request_started_ms: f64,
) -> Result<(), &'static str> {
    let player = response
        .player
        .take()
        .ok_or("server item response did not contain a player")?;
    let player_map_id = player.map_id;
    super::install_full_player_update(game, player);
    super::install_active_buffs(
        game,
        response.active_buffs.take().unwrap_or_default(),
        request_started_ms,
    );
    if let Some(drop) = response.dropped_item
        && player_map_id == game.map.id
        && {
            let now_ms = js_sys::Date::now().max(0.0) as u64;
            drop.despawn_at_unix_ms > now_ms
                && (drop.expires_at_unix_ms == 0 || drop.expires_at_unix_ms > now_ms)
        }
    {
        game.map.dropped_items.push(drop);
    }
    Ok(())
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
