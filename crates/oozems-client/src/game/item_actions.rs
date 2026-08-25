use std::cell::RefCell;
use std::rc::Rc;

use oozems_proto::v1::DroppedItem;
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
    if game.borrow().transition_in_flight.get() {
        show_status("A map transition is already in progress.", true);
        return;
    }
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
                action,
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
    if game.borrow().transition_in_flight.get() {
        return;
    }
    if super::recovery_actions::is_in_flight(&game.borrow().recovery_state) {
        return;
    }
    super::recovery_actions::reset(&mut game.borrow_mut().recovery_state);
    let in_flight = game.borrow().item_action_in_flight.clone();
    if in_flight.replace(true) {
        return;
    }
    let player_id = game.borrow().player.id.clone();
    spawn_local(async move {
        let request_started_ms = super::monotonic_time_ms();
        match api::pick_up_item(&player_id).await {
            Ok(mut response) => {
                let result = api::require_data(response.player.take(), "player")
                    .and_then(|player| {
                        api::require_data(response.active_buffs.take(), "active buffs").and_then(
                            |active_buffs| {
                                super::validate_active_buffs(active_buffs)
                                    .map(|active_buffs| (player, active_buffs))
                                    .map_err(api::ClientError::InvalidResponse)
                            },
                        )
                    })
                    .and_then(|(player, active_buffs)| {
                        (!response.picked_up_drop_id.is_empty())
                            .then(|| {
                                (
                                    player,
                                    active_buffs,
                                    std::mem::take(&mut response.picked_up_drop_id),
                                )
                            })
                            .ok_or(api::ClientError::MissingData("picked-up drop ID"))
                    });
                match result {
                    Ok((player, active_buffs, drop_id)) => {
                        let mut game = game.borrow_mut();
                        super::install_full_player_update(&mut game, player);
                        super::install_active_buffs(&mut game, active_buffs, request_started_ms);
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
        GuiAction::OpenCashShop
        | GuiAction::ToggleStats
        | GuiAction::ToggleEquipment
        | GuiAction::ToggleInventory
        | GuiAction::ToggleKeyConfig
        | GuiAction::ToggleSkills
        | GuiAction::PreviousSkillPage
        | GuiAction::NextSkillPage
        | GuiAction::CloseStats
        | GuiAction::CloseEquipment
        | GuiAction::CloseInventory
        | GuiAction::SelectInventoryTab { .. }
        | GuiAction::CloseKeyConfig
        | GuiAction::CloseSkills
        | GuiAction::AllocateSkill { .. }
        | GuiAction::UseSkill { .. } => unreachable!("non-item GUI action reached the item server"),
    }
}

fn install_item_action_update(
    game: &mut Game,
    mut response: ItemActionResponse,
    action: GuiAction,
    request_started_ms: f64,
) -> Result<(), String> {
    let player =
        api::require_data(response.player.take(), "player").map_err(|error| error.to_string())?;
    let active_buffs = api::require_data(response.active_buffs.take(), "active buffs")
        .map_err(|error| error.to_string())?;
    let active_buffs = super::validate_active_buffs(active_buffs)?;
    let dropped_item = take_dropped_item(action, response.dropped_item.take())?;
    let player_map_id = player.map_id;
    super::install_full_player_update(game, player);
    super::install_active_buffs(game, active_buffs, request_started_ms);
    if let Some(drop) = dropped_item
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

fn take_dropped_item(
    action: GuiAction,
    dropped_item: Option<DroppedItem>,
) -> Result<Option<DroppedItem>, String> {
    match (action, dropped_item) {
        (GuiAction::Drop { .. }, Some(drop)) => Ok(Some(drop)),
        (GuiAction::Drop { .. }, None) => {
            Err("response does not contain the dropped item".to_owned())
        }
        (GuiAction::Equip { .. } | GuiAction::Unequip { .. }, None) => Ok(None),
        (GuiAction::Equip { .. } | GuiAction::Unequip { .. }, Some(_)) => {
            Err("response contains an unexpected dropped item".to_owned())
        }
        _ => unreachable!("non-item GUI action reached the item response installer"),
    }
}

fn item_action_message(action: GuiAction) -> &'static str {
    match action {
        GuiAction::Equip { .. } => "Item equipped.",
        GuiAction::Unequip { .. } => "Item moved to inventory.",
        GuiAction::Drop { .. } => "Item dropped.",
        GuiAction::OpenCashShop
        | GuiAction::ToggleStats
        | GuiAction::ToggleEquipment
        | GuiAction::ToggleInventory
        | GuiAction::ToggleKeyConfig
        | GuiAction::ToggleSkills
        | GuiAction::PreviousSkillPage
        | GuiAction::NextSkillPage
        | GuiAction::CloseStats
        | GuiAction::CloseEquipment
        | GuiAction::CloseInventory
        | GuiAction::SelectInventoryTab { .. }
        | GuiAction::CloseKeyConfig
        | GuiAction::CloseSkills
        | GuiAction::AllocateSkill { .. }
        | GuiAction::UseSkill { .. } => "GUI updated.",
    }
}

#[cfg(test)]
mod tests {
    use super::take_dropped_item;
    use crate::game_gui::GuiAction;

    #[test]
    fn drop_responses_require_a_dropped_item() {
        let error = take_dropped_item(GuiAction::Drop { inventory_index: 0 }, None)
            .expect_err("drop response must contain a dropped item");

        assert_eq!(error, "response does not contain the dropped item");
    }

    #[test]
    fn non_drop_responses_reject_a_dropped_item() {
        let error = take_dropped_item(
            GuiAction::Equip { inventory_index: 0 },
            Some(oozems_proto::v1::DroppedItem::default()),
        )
        .expect_err("equip response must not contain a dropped item");

        assert_eq!(error, "response contains an unexpected dropped item");
    }
}
