use axum::body::Bytes;
use axum::extract::State;
use axum::http::HeaderMap;
use oozems_proto::v1::Map;
use oozems_proto::v1::PlayerState;
use oozems_proto::v1::RespawnPlayerRequest;
use oozems_proto::v1::RespawnPlayerResponse;
use oozems_proto::v1::Vec2;
use thiserror::Error;

use super::ApiError;
use super::Protobuf;
use super::begin_player_mutation;
use super::decode_request;
use super::load_map;
use super::lock_player;
use super::parse_player_id;
use super::prepare_player_mutation;
use super::unix_time_ms;
use crate::app::AppState;

const NO_RETURN_MAP: u32 = 999_999_999;

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
enum RespawnRuleError {
    #[error("the player does not have character stats")]
    MissingStats,
    #[error("a living player cannot respawn")]
    PlayerLiving,
    #[error("the player's maximum health must be positive")]
    InvalidMaximumHealth,
}

pub async fn respawn_player(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Protobuf<RespawnPlayerResponse>, ApiError> {
    let request: RespawnPlayerRequest = decode_request(&headers, body)?;
    let player_id = parse_player_id(&request.player_id)?;
    let player_guard = lock_player(&state, &player_id).await?;
    let now_unix_ms = unix_time_ms()?;
    let mutation = begin_player_mutation(&state, &player_guard, &player_id, now_unix_ms).await?;
    let source_map = load_map(&state, mutation.player.map_id)
        .await?
        .ok_or_else(|| ApiError::not_found("map_not_found", "the player's map does not exist"))?;
    let target_map_id = respawn_map_id(&source_map, state.gameplay.initial_map_id);
    let (mut target_map, position) = load_respawn_target(&state, target_map_id).await?;
    let current = mutation.player.clone();
    let player =
        respawned_player(current.clone(), target_map.id, position).map_err(respawn_rule_error)?;

    target_map.dropped_items = crate::items::map_drops(&state.drops, target_map.id)?;
    let simulation = crate::mobs::map_snapshot(&state.mobs, &target_map).await?;
    target_map.mobs = simulation.mobs;
    target_map.mob_projectiles = simulation.mob_projectiles;
    target_map.simulation_sequence = simulation.sequence;

    let (decision, relocation) = crate::movement::relocate_player_to_position(
        &state.movement,
        &current,
        &source_map,
        &target_map,
        position,
        state.gameplay.movement,
        now_unix_ms,
    )?;
    let (mut transaction, _) = prepare_player_mutation(&state, mutation, player, true, true);
    crate::player_transaction::stage_relocation(
        &mut transaction,
        state.movement.clone(),
        relocation,
    );
    let committed = crate::player_transaction::commit_player_transaction(
        &state.database,
        &player_guard,
        transaction,
    )
    .await?;
    let player = committed.player;
    let effects = committed
        .effects
        .expect("respawn transaction stages active effects");
    let active_buffs = crate::effects::state(&effects, now_unix_ms);
    let quest_definitions = state.catalog.quest_definitions().collect::<Vec<_>>();
    crate::quests::project_npc_quest_indicators(
        &mut target_map,
        &player,
        &effects,
        &quest_definitions,
        state.catalog.item_definition_slice(),
        &state.quest_scripts,
        crate::quests::QuestEnvironment {
            now_unix_ms,
            world_id: state.gameplay.world_id,
        },
    );

    Ok(Protobuf(RespawnPlayerResponse {
        player: Some(player),
        map: Some(target_map),
        authoritative: Some(decision.authoritative),
        active_buffs: Some(active_buffs),
    }))
}

pub(super) fn respawn_map_id(
    map: &Map,
    fallback_map_id: u32,
) -> u32 {
    map.return_map_id
        .filter(|map_id| *map_id != NO_RETURN_MAP)
        .or_else(|| map.town.then_some(map.id))
        .unwrap_or(fallback_map_id)
}

pub(super) async fn load_respawn_target(
    state: &AppState,
    preferred_map_id: u32,
) -> Result<(Map, Vec2), ApiError> {
    for (index, map_id) in [preferred_map_id, state.gameplay.initial_map_id]
        .into_iter()
        .enumerate()
    {
        if index > 0 {
            if map_id == preferred_map_id {
                continue;
            }
            tracing::warn!(
                preferred_map_id,
                fallback_map_id = map_id,
                "return town is unavailable; using the initial map for respawn"
            );
        }
        let Some(map) = load_map(state, map_id).await? else {
            continue;
        };
        let Ok(position) = crate::movement::default_spawn_position(&map) else {
            continue;
        };
        return Ok((map, position));
    }
    Err(ApiError::not_found(
        "respawn_map_not_found",
        "neither the return town nor the initial map has a usable spawn portal",
    ))
}

fn respawned_player(
    mut player: PlayerState,
    map_id: u32,
    position: Vec2,
) -> Result<PlayerState, RespawnRuleError> {
    let stats = player
        .stats
        .as_mut()
        .ok_or(RespawnRuleError::MissingStats)?;
    if stats.hp > 0 {
        return Err(RespawnRuleError::PlayerLiving);
    }
    if stats.max_hp == 0 {
        return Err(RespawnRuleError::InvalidMaximumHealth);
    }
    stats.hp = stats.max_hp;
    player.map_id = map_id;
    player.position = Some(position);
    Ok(player)
}

fn respawn_rule_error(error: RespawnRuleError) -> ApiError {
    match error {
        RespawnRuleError::MissingStats | RespawnRuleError::InvalidMaximumHealth => {
            ApiError::PlayerData(error.to_string())
        }
        RespawnRuleError::PlayerLiving => ApiError::bad_request("player_alive", error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use oozems_proto::v1::CharacterStats;
    use oozems_proto::v1::Map;
    use oozems_proto::v1::PlayerState;
    use oozems_proto::v1::Vec2;

    use super::RespawnRuleError;
    use super::respawn_map_id;
    use super::respawned_player;

    #[test]
    fn field_respawn_uses_its_configured_return_town() {
        let map = Map {
            id: 100_000_100,
            return_map_id: Some(100_000_000),
            ..Map::default()
        };

        assert_eq!(respawn_map_id(&map, 10_000), 100_000_000);
    }

    #[test]
    fn town_without_an_explicit_return_map_respawns_into_itself() {
        let map = Map {
            id: 100_000_000,
            town: true,
            ..Map::default()
        };

        assert_eq!(respawn_map_id(&map, 10_000), 100_000_000);
    }

    #[test]
    fn missing_and_sentinel_return_maps_use_the_initial_town() {
        let missing = Map {
            id: 104_010_000,
            ..Map::default()
        };
        let sentinel = Map {
            id: 104_010_000,
            return_map_id: Some(super::NO_RETURN_MAP),
            ..Map::default()
        };

        assert_eq!(respawn_map_id(&missing, 10_000), 10_000);
        assert_eq!(respawn_map_id(&sentinel, 10_000), 10_000);
    }

    #[test]
    fn respawn_restores_full_health_and_relocates_together() {
        let player = PlayerState {
            map_id: 100_000_100,
            position: Some(Vec2 { x: 40.0, y: 80.0 }),
            stats: Some(CharacterStats {
                hp: 0,
                max_hp: 125,
                ..CharacterStats::default()
            }),
            ..PlayerState::default()
        };
        let destination = Vec2 { x: 160.0, y: 420.0 };

        let respawned = respawned_player(player, 100_000_000, destination).expect("respawn");

        assert_eq!(respawned.map_id, 100_000_000);
        assert_eq!(respawned.position, Some(destination));
        assert_eq!(respawned.stats.expect("stats").hp, 125);
    }

    #[test]
    fn living_players_cannot_request_respawn() {
        let player = PlayerState {
            stats: Some(CharacterStats {
                hp: 1,
                max_hp: 100,
                ..CharacterStats::default()
            }),
            ..PlayerState::default()
        };

        assert_eq!(
            respawned_player(player, 1, Vec2::default()),
            Err(RespawnRuleError::PlayerLiving)
        );
    }
}
