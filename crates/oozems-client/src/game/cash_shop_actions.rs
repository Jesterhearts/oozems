use std::cell::RefCell;
use std::rc::Rc;

use oozems_proto::v1::PurchaseCashShopItemResponse;

use super::Game;
use crate::api;
use crate::show_status;

pub(super) fn begin_open(
    game: Rc<RefCell<Game>>,
    permit: super::requests::RequestPermit,
) {
    if game.borrow().ui.gui.cash_shop_window.is_none() {
        show_status("Cash Shop requires UI.wz.", true);
        return;
    }
    game.borrow_mut().ui.cash_shop.begin_open();
    let window_placements = game.borrow().ui.gui_state.borrow().window_placements;
    *game.borrow().ui.gui_state.borrow_mut() = crate::game_gui::GuiState {
        window_placements,
        ..crate::game_gui::GuiState::default()
    };
    super::requests::spawn_request(
        game,
        permit,
        || async {
            api::get_cash_shop()
                .await
                .map_err(|error| error.to_string())
        },
        |game, result, _| match result {
            Ok(response) => {
                game.ui
                    .cash_shop
                    .install_catalog(response.offers, response.currency_name);
                super::requests::RequestStatus::silent()
            }
            Err(error) => {
                let message = format!("Cash Shop could not load: {error}");
                game.ui.cash_shop.install_load_error(message.clone());
                super::requests::RequestStatus::error(message)
            }
        },
    );
}

pub(super) fn begin_purchase(
    game: Rc<RefCell<Game>>,
    offer_id: u32,
    permit: super::requests::RequestPermit,
) {
    let expected_item_id = game
        .borrow()
        .ui
        .cash_shop
        .offers
        .as_ref()
        .and_then(|offers| offers.iter().find(|offer| offer.offer_id == offer_id))
        .map(|offer| offer.item_id);
    let Some(expected_item_id) = expected_item_id else {
        show_status("The selected Cash Shop offer is no longer available.", true);
        return;
    };
    let player_id = game.borrow().player.id.clone();
    super::requests::spawn_request(
        game,
        permit,
        move || async move {
            api::purchase_cash_shop_item(&player_id, offer_id)
                .await
                .map_err(|error| error.to_string())
        },
        move |game, result, request_started_ms| match result {
            Ok(response) => match install_purchase(
                game,
                response,
                offer_id,
                expected_item_id,
                request_started_ms,
            ) {
                Ok(true) => super::requests::RequestStatus::success("Cash Shop purchase complete."),
                Ok(false) => {
                    game.ui.cash_shop.close();
                    super::requests::RequestStatus::error(
                        "Cash Shop purchase completed after the offer changed. Reopen the shop \
                         before buying again.",
                    )
                }
                Err(error) => super::requests::RequestStatus::error(error),
            },
            Err(error) => {
                super::requests::RequestStatus::error(format!("Cash Shop purchase failed: {error}"))
            }
        },
    );
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
    let (player, active_buffs) = super::responses::take_player_and_active_buffs(&mut response)?;
    let player_map_id = player.map_id;
    super::install_full_player_update(game, player);
    super::install_active_buffs(game, active_buffs, request_started_ms);
    if player_map_id == game.world.map.id {
        super::install_quest_indicators(game, &response.quest_indicators);
    }
    Ok(offer_matches)
}
