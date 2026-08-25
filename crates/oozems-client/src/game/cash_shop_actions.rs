use std::cell::RefCell;
use std::rc::Rc;

use oozems_proto::v1::PurchaseCashShopItemResponse;
use wasm_bindgen_futures::spawn_local;

use super::Game;
use crate::api;
use crate::show_status;

pub(super) fn begin_open(game: Rc<RefCell<Game>>) {
    if game.borrow().gui.cash_shop_window.is_none() {
        show_status("Cash Shop requires UI.wz.", true);
        return;
    }
    let request = game.borrow_mut().cash_shop.begin_open();
    let Some(in_flight) = request else {
        show_status("A Cash Shop request is already in progress.", true);
        return;
    };
    *game.borrow().gui_state.borrow_mut() = crate::game_gui::GuiState::default();
    spawn_local(async move {
        match api::get_cash_shop().await {
            Ok(response) => {
                game.borrow_mut()
                    .cash_shop
                    .install_catalog(response.offers, response.currency_name);
            }
            Err(error) => {
                let message = format!("Cash Shop could not load: {error}");
                game.borrow_mut()
                    .cash_shop
                    .install_load_error(message.clone());
                show_status(&message, true);
            }
        }
        in_flight.set(false);
    });
}

pub(super) fn begin_purchase(
    game: Rc<RefCell<Game>>,
    offer_id: u32,
) {
    if mutation_in_flight(&game.borrow()) {
        show_status("Another player update is already in progress.", true);
        return;
    }
    let expected_item_id = game
        .borrow()
        .cash_shop
        .offers
        .as_ref()
        .and_then(|offers| offers.iter().find(|offer| offer.offer_id == offer_id))
        .map(|offer| offer.item_id);
    let Some(expected_item_id) = expected_item_id else {
        show_status("The selected Cash Shop offer is no longer available.", true);
        return;
    };
    let request = game.borrow().cash_shop.begin_purchase();
    let Some(in_flight) = request else {
        show_status("A Cash Shop request is already in progress.", true);
        return;
    };
    let player_id = game.borrow().player.id.clone();
    spawn_local(async move {
        let request_started_ms = super::monotonic_time_ms();
        let result = match api::purchase_cash_shop_item(&player_id, offer_id).await {
            Ok(response) => install_purchase(
                &mut game.borrow_mut(),
                response,
                offer_id,
                expected_item_id,
                request_started_ms,
            ),
            Err(error) => Err(format!("Cash Shop purchase failed: {error}")),
        };
        match result {
            Ok(true) => show_status("Cash Shop purchase complete.", false),
            Ok(false) => {
                game.borrow_mut().cash_shop.close();
                show_status(
                    "Cash Shop purchase completed after the offer changed. Reopen the shop before \
                     buying again.",
                    true,
                );
            }
            Err(error) => show_status(&error, true),
        }
        in_flight.set(false);
    });
}

fn mutation_in_flight(game: &Game) -> bool {
    game.save_in_flight.get()
        || game.item_action_in_flight.get()
        || game.skill_action_in_flight.get()
        || game.transition_in_flight.get()
        || super::recovery_actions::is_in_flight(&game.recovery_state)
}

fn install_purchase(
    game: &mut Game,
    mut response: PurchaseCashShopItemResponse,
    expected_offer_id: u32,
    expected_item_id: u32,
    request_started_ms: f64,
) -> Result<bool, String> {
    let offer_matches =
        response.offer_id == expected_offer_id && response.item_id == expected_item_id;
    let player =
        api::require_data(response.player.take(), "player").map_err(|error| error.to_string())?;
    let active_buffs = api::require_data(response.active_buffs.take(), "active buffs")
        .map_err(|error| error.to_string())?;
    let active_buffs = super::validate_active_buffs(active_buffs)?;
    super::install_full_player_update(game, player);
    super::install_active_buffs(game, active_buffs, request_started_ms);
    Ok(offer_matches)
}
