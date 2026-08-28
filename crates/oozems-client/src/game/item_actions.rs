use std::cell::RefCell;
use std::rc::Rc;

use oozems_proto::v1::DroppedItem;
use oozems_proto::v1::InventoryItemStack;
use oozems_proto::v1::ItemActionResponse;

use super::Game;
use crate::api;
use crate::audio::MapSound;
use crate::game_gui::GuiAction;
use crate::show_status;

pub(super) fn begin(
    game: Rc<RefCell<Game>>,
    action: GuiAction,
    permit: super::requests::RequestPermit,
) {
    super::recovery_actions::reset(&mut game.borrow_mut().requests.recovery);
    let (player_id, expected_stack, request_position) = {
        let game = game.borrow();
        let expected_stack = match action {
            GuiAction::Equip { inventory_index }
            | GuiAction::Drop { inventory_index }
            | GuiAction::UseItem { inventory_index } => game
                .player
                .inventory
                .as_ref()
                .and_then(|inventory| inventory.stacks.get(inventory_index as usize))
                .cloned(),
            GuiAction::Unequip { .. } => None,
            _ => unreachable!("non-item GUI action reached the item server"),
        };
        if matches!(
            action,
            GuiAction::Equip { .. } | GuiAction::Drop { .. } | GuiAction::UseItem { .. }
        ) && expected_stack.is_none()
        {
            show_status("The selected inventory item is no longer available.", true);
            return;
        }
        (game.player.id.clone(), expected_stack, game.player.position)
    };
    super::requests::spawn_request(
        game,
        permit,
        move || async move {
            request_item_action(&player_id, action, expected_stack.as_ref())
                .await
                .map_err(|error| error.to_string())
        },
        move |game, result, request_started_ms| match result {
            Ok(response) => {
                match install_item_action_update(
                    game,
                    response,
                    action,
                    request_position,
                    request_started_ms,
                ) {
                    Ok(()) => super::requests::RequestStatus::success(item_action_message(action)),
                    Err(error) => super::requests::RequestStatus::error(format!(
                        "Item action could not finish: {error}"
                    )),
                }
            }
            Err(error) => {
                super::requests::RequestStatus::error(format!("Item action failed: {error}"))
            }
        },
    );
}

pub(super) fn begin_pick_up(
    game: Rc<RefCell<Game>>,
    permit: super::requests::RequestPermit,
) {
    super::recovery_actions::reset(&mut game.borrow_mut().requests.recovery);
    let player_id = game.borrow().player.id.clone();
    super::requests::spawn_request(
        game,
        permit,
        move || async move {
            api::pick_up_item(&player_id)
                .await
                .map_err(|error| error.to_string())
        },
        |game, result, request_started_ms| match result {
            Ok(mut response) => {
                let update = super::responses::take_player_and_active_buffs(&mut response)
                    .and_then(|(player, active_buffs)| {
                        (!response.picked_up_drop_id.is_empty())
                            .then(|| {
                                (
                                    player,
                                    active_buffs,
                                    std::mem::take(&mut response.picked_up_drop_id),
                                )
                            })
                            .ok_or_else(|| "response did not contain picked-up drop ID".to_owned())
                    });
                match update {
                    Ok((player, active_buffs, drop_id)) => {
                        super::install_full_player_update(game, player);
                        super::install_active_buffs(game, active_buffs, request_started_ms);
                        game.world
                            .map
                            .dropped_items
                            .retain(|drop| drop.id != drop_id);
                        super::play_map_sound(game, MapSound::PickUpItem);
                        super::requests::RequestStatus::success("Item picked up.")
                    }
                    Err(error) => super::requests::RequestStatus::error(format!(
                        "Pickup could not finish: {error}"
                    )),
                }
            }
            Err(error) => super::requests::RequestStatus::error(format!("Pickup failed: {error}")),
        },
    );
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
        GuiAction::UseItem { inventory_index } => {
            let stack = expected_stack.expect("item use captures its selected stack");
            api::use_item(
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
        | GuiAction::AllocateAbility { .. }
        | GuiAction::AllocateSkill { .. }
        | GuiAction::UseSkill { .. } => unreachable!("non-item GUI action reached the item server"),
    }
}

fn install_item_action_update(
    game: &mut Game,
    mut response: ItemActionResponse,
    action: GuiAction,
    request_position: Option<oozems_proto::v1::Vec2>,
    request_started_ms: f64,
) -> Result<(), String> {
    let (player, active_buffs) = super::responses::take_player_and_active_buffs(&mut response)?;
    let dropped_item = take_dropped_item(action, response.dropped_item.take())?;
    let used_setup_item = take_used_setup_item(action, response.used_setup_item_id)?;
    let player_map_id = player.map_id;
    super::install_full_player_update(game, player);
    super::install_active_buffs(game, active_buffs, request_started_ms);
    if let Some(item_id) = used_setup_item
        && !super::runtime::player_is_dead(&game.player.state)
        && game.player.position == request_position
        && game.world.motion.on_ground
        && game.world.motion.climbing.is_none()
    {
        game.world.active_setup_item_id = Some(item_id);
        game.world.character_animation = super::runtime::new_character_animation_state(
            crate::character_render::CharacterAnimation::Sit,
            true,
            game.clock.now_ms,
        );
    }
    if let Some(drop) = dropped_item
        && player_map_id == game.world.map.id
        && {
            let now_ms = js_sys::Date::now().max(0.0) as u64;
            drop.despawn_at_unix_ms > now_ms
                && (drop.expires_at_unix_ms == 0 || drop.expires_at_unix_ms > now_ms)
        }
    {
        game.world.map.dropped_items.push(drop);
    }
    match action {
        GuiAction::Drop { .. } => super::play_map_sound(game, MapSound::DropItem),
        GuiAction::UseItem { .. } => super::play_map_sound(game, MapSound::UseItem),
        _ => {}
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
        (GuiAction::Equip { .. } | GuiAction::Unequip { .. } | GuiAction::UseItem { .. }, None) => {
            Ok(None)
        }
        (
            GuiAction::Equip { .. } | GuiAction::Unequip { .. } | GuiAction::UseItem { .. },
            Some(_),
        ) => Err("response contains an unexpected dropped item".to_owned()),
        _ => unreachable!("non-item GUI action reached the item response installer"),
    }
}

fn take_used_setup_item(
    action: GuiAction,
    item_id: u32,
) -> Result<Option<u32>, String> {
    match (action, item_id) {
        (GuiAction::UseItem { .. }, 0) => Ok(None),
        (GuiAction::UseItem { .. }, item_id) => Ok(Some(item_id)),
        (_, 0) => Ok(None),
        _ => Err("response contains an unexpected used Setup item".to_owned()),
    }
}

fn item_action_message(action: GuiAction) -> &'static str {
    match action {
        GuiAction::Equip { .. } => "Item equipped.",
        GuiAction::Unequip { .. } => "Item moved to inventory.",
        GuiAction::Drop { .. } => "Item dropped.",
        GuiAction::UseItem { .. } => "Item used.",
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
        | GuiAction::AllocateAbility { .. }
        | GuiAction::AllocateSkill { .. }
        | GuiAction::UseSkill { .. } => "GUI updated.",
    }
}

#[cfg(test)]
mod tests {
    use super::take_dropped_item;
    use super::take_used_setup_item;
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

    #[test]
    fn only_item_use_responses_can_activate_a_setup_item() {
        assert_eq!(
            take_used_setup_item(GuiAction::UseItem { inventory_index: 0 }, 3_010_072),
            Ok(Some(3_010_072))
        );
        assert_eq!(
            take_used_setup_item(GuiAction::UseItem { inventory_index: 0 }, 0),
            Ok(None)
        );
        assert_eq!(
            take_used_setup_item(GuiAction::Equip { inventory_index: 0 }, 3_010_072),
            Err("response contains an unexpected used Setup item".to_owned())
        );
    }
}
