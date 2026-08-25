use axum::body::Bytes;
use axum::extract::State;
use axum::http::HeaderMap;
use oozems_proto::v1::CashShopOffer as CashShopOfferView;
use oozems_proto::v1::GetCashShopRequest;
use oozems_proto::v1::GetCashShopResponse;
use oozems_proto::v1::PurchaseCashShopItemRequest;
use oozems_proto::v1::PurchaseCashShopItemResponse;

use super::ApiError;
use super::Protobuf;
use super::advance_automatic_player;
use super::decode_request;
use super::lock_player;
use super::parse_player_id;
use super::record_recovery_activity;
use super::require_player_at;
use super::unix_time_ms;
use crate::app::AppState;
use crate::cash_shop::CashShopPurchaseError;
use crate::cash_shop::OfferLifetime;

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Protobuf<GetCashShopResponse>, ApiError> {
    let _: GetCashShopRequest = decode_request(&headers, body)?;
    Ok(Protobuf(GetCashShopResponse {
        offers: state.cash_shop.offers().iter().map(offer_view).collect(),
        currency_name: state.cash_shop.currency_name().to_owned(),
    }))
}

pub async fn purchase(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Protobuf<PurchaseCashShopItemResponse>, ApiError> {
    let request: PurchaseCashShopItemRequest = decode_request(&headers, body)?;
    let player_id = parse_player_id(&request.player_id)?;
    let _player_guard = lock_player(&state, &player_id).await?;
    let now_unix_ms = unix_time_ms()?;
    let current = require_player_at(&state, &player_id, now_unix_ms).await?;
    let offer = state
        .cash_shop
        .offer(request.offer_id)
        .ok_or_else(|| invalid("the selected cash-shop offer does not exist"))?;
    let result = crate::cash_shop::purchase(current, offer, state.catalog.as_ref(), now_unix_ms)
        .map_err(|error| purchase_error(error, state.cash_shop.currency_name()))?;
    let effects = crate::effects::snapshot(&state.active_effects, &result.player.id, now_unix_ms)?;
    let advanced = advance_automatic_player(&state, result.player, effects, now_unix_ms);
    let player = crate::database::save_player(&state.database, &advanced.player).await?;
    let active_buffs = crate::effects::state(&advanced.effects, now_unix_ms);
    crate::effects::commit(&state.active_effects, &player.id, advanced.effects)?;
    record_recovery_activity(&state, player_id.as_str(), now_unix_ms);
    Ok(Protobuf(PurchaseCashShopItemResponse {
        active_buffs: Some(active_buffs),
        player: Some(player),
        offer_id: result.offer_id,
        item_id: result.item_id,
        expires_at_unix_ms: result.expires_at_unix_ms,
    }))
}

fn offer_view(offer: &crate::cash_shop::CashShopOffer) -> CashShopOfferView {
    CashShopOfferView {
        offer_id: offer.offer_id,
        item_id: offer.item_id,
        price: offer.price,
        duration_ms: match offer.lifetime {
            OfferLifetime::Permanent => 0,
            OfferLifetime::Timed { duration_ms } => duration_ms,
        },
    }
}

fn invalid(message: impl Into<String>) -> ApiError {
    ApiError::bad_request("invalid_cash_shop_purchase", message)
}

fn purchase_error(
    error: CashShopPurchaseError,
    currency_name: &str,
) -> ApiError {
    match error {
        CashShopPurchaseError::InsufficientCashPoints => {
            invalid(format!("the player does not have enough {currency_name}"))
        }
        error => invalid(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::offer_view;
    use super::purchase_error;
    use crate::cash_shop::CashShopOffer;
    use crate::cash_shop::CashShopPurchaseError;
    use crate::cash_shop::OfferLifetime;

    #[test]
    fn offer_view_preserves_price_and_projects_permanent_as_zero_duration() {
        let permanent = offer_view(&CashShopOffer {
            offer_id: 1,
            item_id: 5_010_000,
            price: 1_200,
            lifetime: OfferLifetime::Permanent,
        });
        let timed = offer_view(&CashShopOffer {
            offer_id: 2,
            item_id: 5_010_001,
            price: 1_500,
            lifetime: OfferLifetime::Timed { duration_ms: 42 },
        });

        assert_eq!(
            (
                permanent.offer_id,
                permanent.item_id,
                permanent.price,
                permanent.duration_ms
            ),
            (1, 5_010_000, 1_200, 0)
        );
        assert_eq!(timed.duration_ms, 42);
    }

    #[test]
    fn insufficient_balance_error_uses_the_configured_currency_name() {
        assert_eq!(
            purchase_error(CashShopPurchaseError::InsufficientCashPoints, "Ooze").to_string(),
            "the player does not have enough Ooze"
        );
    }
}
