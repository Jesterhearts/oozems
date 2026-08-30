use axum::body::Bytes;
use axum::extract::State;
use axum::http::HeaderMap;
use oozems_proto::v1::DropItemRequest;
use oozems_proto::v1::EquipItemRequest;
use oozems_proto::v1::ItemActionResponse;
use oozems_proto::v1::ItemCategory;
use oozems_proto::v1::PickUpItemRequest;
use oozems_proto::v1::UnequipItemRequest;
use oozems_proto::v1::UseItemRequest;

use super::ApiError;
use super::Protobuf;
use super::begin_player_mutation;
use super::current_map_quest_indicators;
use super::decode_request;
use super::item_rule_error;
use super::lock_player;
use super::parse_player_id;
use super::persist_player_mutation;
use super::prepare_player_mutation;
use super::require_living_player;
use super::unix_time_ms;
use crate::app::AppState;

pub async fn equip_item(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Protobuf<ItemActionResponse>, ApiError> {
    let request: EquipItemRequest = decode_request(&headers, body)?;
    let player_id = parse_player_id(&request.player_id)?;
    let player_guard = lock_player(&state, &player_id, &headers).await?;
    let activity_time_ms = unix_time_ms()?;
    let mutation =
        begin_player_mutation(&state, &player_guard, &player_id, activity_time_ms).await?;
    crate::items::validate_inventory_selection(
        &mutation.player,
        request.inventory_index,
        request.expected_item_id,
        request.expected_expires_at_unix_ms,
    )
    .map_err(item_rule_error)?;
    let updated = crate::items::equip_inventory_item(
        mutation.player.clone(),
        request.inventory_index,
        state.catalog.as_ref(),
    )
    .map_err(item_rule_error)?;
    let committed =
        persist_player_mutation(&state, &player_guard, mutation, updated, true, true).await?;
    let player = committed.player;
    let effects = committed
        .effects
        .expect("equip item transaction stages active effects");
    let quest_indicators =
        current_map_quest_indicators(&state, &player, &effects, activity_time_ms).await;

    Ok(Protobuf(ItemActionResponse {
        player: Some(player),
        dropped_item: None,
        picked_up_drop_id: String::new(),
        active_buffs: Some(crate::effects::state(&effects, activity_time_ms)),
        used_setup_item_id: 0,
        quest_indicators,
    }))
}

pub async fn unequip_item(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Protobuf<ItemActionResponse>, ApiError> {
    let request: UnequipItemRequest = decode_request(&headers, body)?;
    let player_id = parse_player_id(&request.player_id)?;
    let player_guard = lock_player(&state, &player_id, &headers).await?;
    let activity_time_ms = unix_time_ms()?;
    let mutation =
        begin_player_mutation(&state, &player_guard, &player_id, activity_time_ms).await?;
    let updated = crate::items::unequip_item(
        mutation.player.clone(),
        request.slot,
        state.catalog.as_ref(),
    )
    .map_err(item_rule_error)?;
    let committed =
        persist_player_mutation(&state, &player_guard, mutation, updated, true, true).await?;
    let player = committed.player;
    let effects = committed
        .effects
        .expect("unequip item transaction stages active effects");
    let quest_indicators =
        current_map_quest_indicators(&state, &player, &effects, activity_time_ms).await;

    Ok(Protobuf(ItemActionResponse {
        player: Some(player),
        dropped_item: None,
        picked_up_drop_id: String::new(),
        active_buffs: Some(crate::effects::state(&effects, activity_time_ms)),
        used_setup_item_id: 0,
        quest_indicators,
    }))
}

pub async fn drop_item(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Protobuf<ItemActionResponse>, ApiError> {
    let request: DropItemRequest = decode_request(&headers, body)?;
    let player_id = parse_player_id(&request.player_id)?;
    let player_guard = lock_player(&state, &player_id, &headers).await?;
    let activity_time_ms = unix_time_ms()?;
    let mutation =
        begin_player_mutation(&state, &player_guard, &player_id, activity_time_ms).await?;
    crate::items::validate_inventory_selection(
        &mutation.player,
        request.inventory_index,
        request.expected_item_id,
        request.expected_expires_at_unix_ms,
    )
    .map_err(item_rule_error)?;
    let removed = crate::items::remove_inventory_item(
        mutation.player.clone(),
        request.inventory_index,
        state.catalog.as_ref(),
    )
    .map_err(item_rule_error)?;
    let staged_drop = crate::items::stage_inventory_drop(&state.drops, &removed)?;
    let dropped_item = staged_drop.item().clone();
    let (mut transaction, _) =
        prepare_player_mutation(&state, mutation, removed.player, true, true);
    crate::player_transaction::stage_drops(&mut transaction, state.drops.clone(), [staged_drop])?;
    let committed = crate::player_transaction::commit_player_transaction(
        &state.database,
        &player_guard,
        transaction,
    )
    .await?;
    let player = committed.player;
    let effects = committed
        .effects
        .expect("drop item transaction stages active effects");
    let quest_indicators =
        current_map_quest_indicators(&state, &player, &effects, activity_time_ms).await;

    Ok(Protobuf(ItemActionResponse {
        player: Some(player),
        dropped_item: Some(dropped_item),
        picked_up_drop_id: String::new(),
        active_buffs: Some(crate::effects::state(&effects, activity_time_ms)),
        used_setup_item_id: 0,
        quest_indicators,
    }))
}

pub async fn use_item(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Protobuf<ItemActionResponse>, ApiError> {
    let request: UseItemRequest = decode_request(&headers, body)?;
    let player_id = parse_player_id(&request.player_id)?;
    let player_guard = lock_player(&state, &player_id, &headers).await?;
    let activity_time_ms = unix_time_ms()?;
    let mut mutation =
        begin_player_mutation(&state, &player_guard, &player_id, activity_time_ms).await?;
    require_living_player(&mutation.player, "use items")?;
    crate::items::validate_inventory_selection(
        &mutation.player,
        request.inventory_index,
        request.expected_item_id,
        request.expected_expires_at_unix_ms,
    )
    .map_err(item_rule_error)?;
    let used = crate::items::use_inventory_item(
        mutation.player.clone(),
        request.inventory_index,
        state.catalog.as_ref(),
    )
    .map_err(item_rule_error)?;
    let consumes_inventory = used.category == ItemCategory::Consume;
    let used_setup_item_id = if used.category == ItemCategory::Install {
        used.item_id
    } else {
        0
    };
    let updated = if used.category == ItemCategory::Consume {
        let definition = state
            .catalog
            .consume_effect_definition(used.item_id)
            .ok_or(crate::items::ItemRuleError::UnusableItem {
                item_id: used.item_id,
            })
            .map_err(item_rule_error)?;
        crate::effects::apply_consume_effect(
            used.player,
            &mut mutation.effects,
            definition,
            activity_time_ms,
        )
    } else {
        used.player
    };
    let (transaction, _) =
        prepare_player_mutation(&state, mutation, updated, consumes_inventory, true);
    let committed = crate::player_transaction::commit_player_transaction(
        &state.database,
        &player_guard,
        transaction,
    )
    .await?;
    let player = committed.player;
    let effects = committed
        .effects
        .expect("use item transaction stages active effects");
    let quest_indicators =
        current_map_quest_indicators(&state, &player, &effects, activity_time_ms).await;

    Ok(Protobuf(ItemActionResponse {
        player: Some(player),
        dropped_item: None,
        picked_up_drop_id: String::new(),
        active_buffs: Some(crate::effects::state(&effects, activity_time_ms)),
        used_setup_item_id,
        quest_indicators,
    }))
}

pub async fn pick_up_item(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Protobuf<ItemActionResponse>, ApiError> {
    let request: PickUpItemRequest = decode_request(&headers, body)?;
    let player_id = parse_player_id(&request.player_id)?;
    let player_guard = lock_player(&state, &player_id, &headers).await?;
    let activity_time_ms = unix_time_ms()?;
    let mutation =
        begin_player_mutation(&state, &player_guard, &player_id, activity_time_ms).await?;
    require_living_player(&mutation.player, "pick up items")?;
    let position = mutation
        .player
        .position
        .ok_or(crate::movement::MovementError::MissingPlayerPosition)?;
    let picked = crate::items::pick_up_nearest(
        &state.drops,
        mutation.player.clone(),
        position,
        state.catalog.as_ref(),
    )
    .map_err(pick_up_error)?;
    let map_id = picked.player.map_id;
    let picked_up_drop_id = picked.drop.id.clone();
    let (mut transaction, _) =
        prepare_player_mutation(&state, mutation, picked.player.clone(), true, true);
    crate::player_transaction::stage_pickup(&mut transaction, state.drops.clone(), map_id, &picked);
    let committed = crate::player_transaction::commit_player_transaction(
        &state.database,
        &player_guard,
        transaction,
    )
    .await?;
    let player = committed.player;
    let effects = committed
        .effects
        .expect("pick up item transaction stages active effects");
    let quest_indicators =
        current_map_quest_indicators(&state, &player, &effects, activity_time_ms).await;

    Ok(Protobuf(ItemActionResponse {
        player: Some(player),
        dropped_item: None,
        picked_up_drop_id,
        active_buffs: Some(crate::effects::state(&effects, activity_time_ms)),
        used_setup_item_id: 0,
        quest_indicators,
    }))
}

fn pick_up_error(error: crate::items::PickUpError) -> ApiError {
    match error {
        crate::items::PickUpError::Rule(error) => item_rule_error(error),
        crate::items::PickUpError::Store(error) => error.into(),
    }
}
