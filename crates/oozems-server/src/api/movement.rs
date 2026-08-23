use axum::body::Bytes;
use axum::extract::State;
use axum::http::HeaderMap;
use oozems_proto::v1::EnterPortalRequest;
use oozems_proto::v1::GetMovementRulesRequest;
use oozems_proto::v1::GetMovementRulesResponse;
use oozems_proto::v1::MovementRules;
use oozems_proto::v1::MovementUpdateResponse;
use oozems_proto::v1::PlayerState;
use oozems_proto::v1::SubmitMovementRequest;

use super::ApiError;
use super::Protobuf;
use super::decode_request;
use super::load_map;
use super::lock_player;
use super::parse_player_id;
use super::record_recovery_activity;
use super::require_player;
use super::skill_rule_error;
use super::unix_time_ms;
use crate::app::AppState;
use crate::database::PlayerId;

pub async fn get_movement_rules(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Protobuf<GetMovementRulesResponse>, ApiError> {
    let _: GetMovementRulesRequest = decode_request(&headers, body)?;
    Ok(Protobuf(GetMovementRulesResponse {
        rules: Some(movement_rules_response(state.gameplay.movement)),
    }))
}

pub async fn submit_movement(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Protobuf<MovementUpdateResponse>, ApiError> {
    let request: SubmitMovementRequest = decode_request(&headers, body)?;
    let player_id = parse_player_id(&request.player_id)?;
    let _player_guard = lock_player(&state, &player_id).await?;
    let submitted = request
        .snapshot
        .ok_or_else(|| {
            ApiError::bad_request(
                "missing_movement_snapshot",
                "request does not contain a movement snapshot",
            )
        })
        .and_then(|snapshot| {
            crate::movement::parse_snapshot(snapshot).map_err(movement_request_error)
        })?;
    let current = require_player(&state, &player_id).await?;
    let now_ms = unix_time_ms()?;
    let decision = submit_with_lazy_map(&state, &current, submitted, now_ms).await?;
    apply_movement_side_effects(&state, &player_id, &current, &decision, now_ms).await?;
    Ok(Protobuf(
        movement_response(&state, &current, decision).await?,
    ))
}

async fn submit_with_lazy_map(
    state: &AppState,
    current: &PlayerState,
    submitted: crate::movement::SubmittedMovement,
    now_ms: u64,
) -> Result<crate::movement::MovementDecision, ApiError> {
    match crate::movement::submit_movement(
        &state.movement,
        current,
        submitted,
        state.gameplay.movement,
        now_ms,
    ) {
        Ok(decision) => Ok(decision),
        Err(crate::movement::MovementError::MissingMap { map_id }) => {
            let map = load_map(state, map_id).await?.ok_or_else(|| {
                ApiError::not_found("map_not_found", format!("map {map_id} does not exist"))
            })?;
            crate::movement::register_map(&state.movement, &map)?;
            Ok(crate::movement::submit_movement(
                &state.movement,
                current,
                submitted,
                state.gameplay.movement,
                now_ms,
            )?)
        }
        Err(error) => Err(error.into()),
    }
}

pub async fn enter_portal(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Protobuf<MovementUpdateResponse>, ApiError> {
    let request: EnterPortalRequest = decode_request(&headers, body)?;
    let player_id = parse_player_id(&request.player_id)?;
    let _player_guard = lock_player(&state, &player_id).await?;
    let source = request
        .source
        .ok_or_else(|| {
            ApiError::bad_request(
                "missing_movement_snapshot",
                "request does not contain a source movement snapshot",
            )
        })
        .and_then(|snapshot| {
            crate::movement::parse_snapshot(snapshot).map_err(movement_request_error)
        })?;
    let current = require_player(&state, &player_id).await?;
    let source_map = load_map(&state, current.map_id).await?.ok_or_else(|| {
        ApiError::not_found(
            "map_not_found",
            format!("map {} does not exist", current.map_id),
        )
    })?;
    let target_map = load_map(&state, request.target_map_id)
        .await?
        .ok_or_else(|| {
            ApiError::bad_request(
                "invalid_portal_target",
                format!("map {} does not exist", request.target_map_id),
            )
        })?;
    let now_ms = unix_time_ms()?;
    let decision = crate::movement::enter_portal(
        &state.movement,
        &current,
        crate::movement::PortalMovement {
            source_map: &source_map,
            target_map: &target_map,
            source,
            target_portal_name: &request.target_portal_name,
        },
        state.gameplay.movement,
        now_ms,
    )?;
    apply_movement_side_effects(&state, &player_id, &current, &decision, now_ms).await?;
    Ok(Protobuf(
        movement_response(&state, &current, decision).await?,
    ))
}

fn movement_request_error(error: crate::movement::MovementError) -> ApiError {
    match error {
        crate::movement::MovementError::Tracker => error.into(),
        _ => ApiError::bad_request("invalid_movement_snapshot", error.to_string()),
    }
}

async fn apply_movement_side_effects(
    state: &AppState,
    player_id: &PlayerId,
    current: &PlayerState,
    decision: &crate::movement::MovementDecision,
    now_ms: u64,
) -> Result<(), ApiError> {
    if decision.activity {
        record_recovery_activity(state, player_id.as_str(), now_ms);
    }
    if !decision.persist {
        return Ok(());
    }
    let authoritative = crate::movement::synchronize_player(&state.movement, current.clone())?;
    match crate::database::save_player_position(&state.database, &authoritative).await {
        Ok(()) => crate::movement::mark_persisted(&state.movement, player_id.as_str(), now_ms)?,
        Err(error) => {
            tracing::error!(%error, "failed to persist authoritative movement; a later snapshot will retry");
        }
    }
    Ok(())
}

async fn movement_response(
    state: &AppState,
    current: &PlayerState,
    decision: crate::movement::MovementDecision,
) -> Result<MovementUpdateResponse, ApiError> {
    let map_id = decision.authoritative.map_id;
    let map = load_map(state, map_id).await?.ok_or_else(|| {
        ApiError::not_found("map_not_found", format!("map {map_id} does not exist"))
    })?;
    let authoritative_player =
        crate::movement::synchronize_player(&state.movement, current.clone())?;
    let simulation = crate::mobs::observe_player(&state.mobs, &map, &authoritative_player)?;
    let player_damage = simulation.player_damage();
    let mut updated_player = authoritative_player;
    if player_damage > 0 {
        let stats = updated_player.stats.get_or_insert_default();
        stats.hp = stats
            .hp
            .saturating_sub(u32::try_from(player_damage).unwrap_or(u32::MAX));
        updated_player = match crate::database::save_player(&state.database, &updated_player).await
        {
            Ok(player) => player,
            Err(error) => {
                crate::mobs::restore_player_events(
                    &state.mobs,
                    map_id,
                    &updated_player.id,
                    simulation.combat_events.clone(),
                )?;
                return Err(error.into());
            }
        };
        record_recovery_activity(state, &updated_player.id, unix_time_ms()?);
    }
    let active_buffs =
        crate::skills::active_skill_buffs(&state.skill_buffs, &updated_player.id, unix_time_ms()?)
            .map_err(skill_rule_error)?;
    Ok(MovementUpdateResponse {
        authoritative: Some(decision.authoritative),
        accepted: decision.accepted,
        rejection_reason: decision.rejection_reason,
        mobs: simulation.mobs,
        player_stats: updated_player.stats,
        mob_projectiles: simulation.mob_projectiles,
        combat_events: simulation.combat_events,
        simulation_sequence: simulation.sequence,
        active_buffs: Some(active_buffs),
    })
}

fn movement_rules_response(config: crate::gameplay::MovementConfig) -> MovementRules {
    MovementRules {
        walk_speed: config.walk_speed,
        climb_speed: config.climb_speed,
        gravity: config.gravity,
        jump_speed: config.jump_speed,
        speed_cap: config.speed_cap,
        jump_cap: config.jump_cap,
        snapshot_interval_ms: duration_millis(config.snapshot_interval),
        maximum_snapshot_gap_ms: duration_millis(config.maximum_snapshot_gap),
        position_tolerance: config.position_tolerance,
        ground_tolerance: config.ground_tolerance,
        ladder_reach: config.ladder_reach,
        portal_horizontal_reach: config.portal_horizontal_reach,
        portal_vertical_reach: config.portal_vertical_reach,
        platform_edge_tolerance: config.platform_edge_tolerance,
        ladder_end_reach: config.ladder_end_reach,
    }
}

fn duration_millis(duration: std::time::Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}
