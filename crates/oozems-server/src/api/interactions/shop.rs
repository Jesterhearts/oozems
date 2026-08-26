use oozems_proto::v1::Npc;
use oozems_proto::v1::NpcInteractionResponse;
use oozems_proto::v1::NpcShopCurrency;
use oozems_proto::v1::NpcShopOffer;
use oozems_proto::v1::NpcShopView;
use oozems_proto::v1::PlayerState;
use oozems_proto::v1::npc_interaction;

use super::interaction;
use super::invalid;
use crate::api::ApiError;
use crate::api::PlayerMutation;
use crate::api::item_rule_error;
use crate::api::prepare_player_mutation;
use crate::app::AppState;
use crate::interactions::ShopCurrency;
use crate::player_lock::PlayerGuard;

pub(super) async fn buy_item(
    state: &AppState,
    guard: &PlayerGuard,
    mutation: PlayerMutation,
    npc: &Npc,
    item_id: u32,
) -> Result<NpcInteractionResponse, ApiError> {
    let map_id = mutation.player.map_id;
    let shop = state
        .interactions
        .shop(map_id, npc.spawn_id)
        .ok_or_else(|| invalid("this NPC does not operate a shop"))?;
    let offer = shop
        .offers
        .iter()
        .find(|offer| offer.item_id == item_id)
        .ok_or_else(|| invalid("the selected item is not sold by this shop"))?;
    let player = crate::items::buy_shop_item(
        mutation.player.clone(),
        item_id,
        offer.buy_price,
        shop.currency,
        state.catalog.as_ref(),
    )
    .map_err(|error| shop_item_rule_error(error, shop, state.cash_shop.currency_name()))?;
    let (transaction, _) = prepare_player_mutation(state, mutation, player, true, true);
    let player =
        crate::player_transaction::commit_player_transaction(&state.database, guard, transaction)
            .await?
            .player;
    Ok(shop_response(state, player, npc, shop))
}

pub(super) async fn sell_item(
    state: &AppState,
    guard: &PlayerGuard,
    mutation: PlayerMutation,
    npc: &Npc,
    inventory_index: u32,
    expected_item_id: u32,
    expected_expires_at_unix_ms: u64,
) -> Result<NpcInteractionResponse, ApiError> {
    let map_id = mutation.player.map_id;
    let shop = state
        .interactions
        .shop(map_id, npc.spawn_id)
        .ok_or_else(|| invalid("this NPC does not operate a shop"))?;
    validate_shop_sale(shop)?;
    crate::items::validate_inventory_selection(
        &mutation.player,
        inventory_index,
        expected_item_id,
        expected_expires_at_unix_ms,
    )
    .map_err(item_rule_error)?;
    let player = crate::items::sell_inventory_item(
        mutation.player.clone(),
        inventory_index,
        state.catalog.as_ref(),
    )
    .map_err(item_rule_error)?;
    let (transaction, _) = prepare_player_mutation(state, mutation, player, true, true);
    let player =
        crate::player_transaction::commit_player_transaction(&state.database, guard, transaction)
            .await?
            .player;
    Ok(shop_response(state, player, npc, shop))
}

pub(super) fn validate_shop_sale(
    shop: &crate::interactions::ShopDefinition
) -> Result<(), ApiError> {
    if shop.currency == ShopCurrency::CashPoints {
        Err(invalid("cash-point shops do not buy items"))
    } else {
        Ok(())
    }
}

fn shop_item_rule_error(
    error: crate::items::ItemRuleError,
    shop: &crate::interactions::ShopDefinition,
    premium_currency_name: &str,
) -> ApiError {
    match error {
        crate::items::ItemRuleError::InsufficientCashPoints
            if shop.currency == ShopCurrency::CashPoints =>
        {
            invalid(format!(
                "the player does not have enough {premium_currency_name}"
            ))
        }
        error => item_rule_error(error),
    }
}

fn shop_response(
    state: &AppState,
    player: PlayerState,
    npc: &Npc,
    shop: &crate::interactions::ShopDefinition,
) -> NpcInteractionResponse {
    NpcInteractionResponse {
        interaction: Some(interaction(
            player.map_id,
            npc,
            npc_interaction::View::Shop(shop_view(shop, state.cash_shop.currency_name())),
        )),
        player: Some(player),
        authoritative: None,
        map: None,
        npc_animation: None,
        active_buffs: None,
        quest_indicators: Vec::new(),
    }
}

pub(super) fn shop_view(
    shop: &crate::interactions::ShopDefinition,
    premium_currency_name: &str,
) -> NpcShopView {
    NpcShopView {
        offers: shop
            .offers
            .iter()
            .map(|offer| NpcShopOffer {
                item_id: offer.item_id,
                buy_price: offer.buy_price,
            })
            .collect(),
        currency: match shop.currency {
            ShopCurrency::Mesos => NpcShopCurrency::Mesos as i32,
            ShopCurrency::CashPoints => NpcShopCurrency::CashPoints as i32,
        },
        currency_name: match shop.currency {
            ShopCurrency::Mesos => "mesos",
            ShopCurrency::CashPoints => premium_currency_name,
        }
        .to_owned(),
    }
}
