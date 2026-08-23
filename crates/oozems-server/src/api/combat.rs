use axum::body::Bytes;
use axum::extract::State;
use axum::http::HeaderMap;
use oozems_proto::v1::BasicAttackRequest;
use oozems_proto::v1::BasicAttackResponse;

use super::ApiError;
use super::Protobuf;
use super::decode_request;
use super::load_map;
use super::lock_player;
use super::parse_player_id;
use super::record_recovery_activity;
use super::require_player;
use super::save_simulation_player_damage;
use super::unix_time_ms;
use crate::app::AppState;

pub async fn use_basic_attack(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Protobuf<BasicAttackResponse>, ApiError> {
    let request: BasicAttackRequest = decode_request(&headers, body)?;
    let player_id = parse_player_id(&request.player_id)?;
    let _player_guard = lock_player(&state, &player_id).await?;
    let player = require_player(&state, &player_id).await?;
    let damage = crate::attacks::calculate_basic_attack(&player, &state.formulas)
        .map_err(attack_rule_error)?;
    let map = load_map(&state, player.map_id).await?.ok_or_else(|| {
        ApiError::not_found(
            "map_not_found",
            format!("map {} does not exist", player.map_id),
        )
    })?;
    let now_ms = unix_time_ms()?;
    let attack_deadline_ms = crate::attacks::reserve_basic_attack(
        &state.basic_attack_cooldowns,
        player_id.as_str(),
        now_ms,
        state.gameplay.combat.player_attack_interval,
    )
    .map_err(attack_rule_error)?;
    let simulation = match crate::mobs::use_player_attack(
        &state.mobs,
        &map,
        &player,
        crate::mobs::PlayerAttack {
            target_mob_id: "",
            facing_left: request.facing_left,
            minimum_damage: damage.minimum,
            maximum_damage: damage.maximum,
            fixed_damage: false,
        },
    ) {
        Ok(simulation) => simulation,
        Err(error) => {
            if let Err(release_error) = crate::attacks::release_basic_attack(
                &state.basic_attack_cooldowns,
                player_id.as_str(),
                attack_deadline_ms,
            ) {
                tracing::error!(%release_error, "failed to release a basic attack cooldown after a simulation error");
            }
            return Err(error.into());
        }
    };
    let player = save_simulation_player_damage(&state, player, &simulation).await?;
    let dropped_items = crate::items::map_drops(&state.drops, map.id)?;
    record_recovery_activity(&state, player_id.as_str(), now_ms);

    Ok(Protobuf(BasicAttackResponse {
        player: Some(player),
        mobs: simulation.mobs,
        mob_projectiles: simulation.mob_projectiles,
        combat_events: simulation.combat_events,
        simulation_sequence: simulation.sequence,
        dropped_items,
    }))
}

fn attack_rule_error(error: crate::attacks::AttackRuleError) -> ApiError {
    match error {
        crate::attacks::AttackRuleError::Cooldown { .. } => {
            ApiError::bad_request("invalid_attack_action", error.to_string())
        }
        _ => ApiError::AttackRules(error),
    }
}
