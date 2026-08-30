use axum::body::Bytes;
use axum::extract::State;
use axum::http::HeaderMap;
use oozems_proto::v1::EnterPortalRequest;
use oozems_proto::v1::GetMovementRulesRequest;
use oozems_proto::v1::GetMovementRulesResponse;
use oozems_proto::v1::MovementRules;
use oozems_proto::v1::MovementSnapshot;
use oozems_proto::v1::MovementUpdateResponse;
use oozems_proto::v1::PlayerState;
use oozems_proto::v1::SubmitMovementRequest;

use super::ApiError;
use super::Protobuf;
use super::begin_player_mutation;
use super::decode_request;
use super::load_map;
use super::lock_player;
use super::merge_dropped_items;
use super::parse_player_id;
use super::prepare_simulation_player_effects;
use super::unix_time_ms;
use crate::app::AppState;

pub(super) fn submit_action_movement(
    state: &AppState,
    player: &PlayerState,
    snapshot: Option<MovementSnapshot>,
    modifiers: crate::movement::MovementModifiers,
    map: &oozems_proto::v1::Map,
    now_ms: u64,
) -> Result<crate::movement::SynchronizedPlayer, ApiError> {
    let submitted = snapshot
        .ok_or_else(|| {
            ApiError::bad_request(
                "missing_movement_snapshot",
                "combat request does not contain a movement snapshot",
            )
        })
        .and_then(|snapshot| {
            crate::movement::parse_snapshot(snapshot).map_err(movement_request_error)
        })?;
    let submitted_sequence = submitted.sequence;
    crate::movement::register_map(&state.movement, map)?;
    let decision = crate::movement::submit_combat_movement_with_modifiers(
        &state.movement,
        player,
        submitted,
        modifiers,
        state.gameplay.movement,
        now_ms,
    )?;
    if !decision.accepted && decision.authoritative.sequence <= submitted_sequence {
        return Err(ApiError::bad_request(
            "invalid_combat_movement",
            decision.rejection_reason,
        ));
    }
    Ok(crate::movement::synchronize_player_observation(
        &state.movement,
        player.clone(),
    )?)
}

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
    let player_guard = lock_player(&state, &player_id, &headers).await?;
    let now_ms = unix_time_ms()?;
    let mutation = begin_player_mutation(&state, &player_guard, &player_id, now_ms).await?;
    let player_is_dead = mutation
        .player
        .stats
        .as_ref()
        .ok_or_else(|| ApiError::PlayerData("character stats are missing".to_owned()))?
        .hp
        == 0;
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
    let projected = mutation.effects.projected().modifiers;
    let modifiers = crate::movement::MovementModifiers {
        speed: projected.speed,
        jump: projected.jump,
    };
    let decision = if player_is_dead {
        submit_stationary_with_lazy_map(&state, &mutation.player, submitted, modifiers, now_ms)
            .await?
    } else {
        submit_with_lazy_map(&state, &mutation.player, submitted, modifiers, now_ms).await?
    };
    Ok(Protobuf(
        movement_response(
            &state,
            &player_guard,
            mutation,
            decision,
            false,
            now_ms,
            None,
        )
        .await?,
    ))
}

async fn submit_with_lazy_map(
    state: &AppState,
    current: &PlayerState,
    submitted: crate::movement::SubmittedMovement,
    modifiers: crate::movement::MovementModifiers,
    now_ms: u64,
) -> Result<crate::movement::MovementDecision, ApiError> {
    match crate::movement::submit_movement_with_modifiers(
        &state.movement,
        current,
        submitted,
        modifiers,
        state.gameplay.movement,
        now_ms,
    ) {
        Ok(decision) => Ok(decision),
        Err(crate::movement::MovementError::MissingMap { map_id }) => {
            let map = load_map(state, map_id).await?.ok_or_else(|| {
                ApiError::not_found("map_not_found", format!("map {map_id} does not exist"))
            })?;
            crate::movement::register_map(&state.movement, &map)?;
            Ok(crate::movement::submit_movement_with_modifiers(
                &state.movement,
                current,
                submitted,
                modifiers,
                state.gameplay.movement,
                now_ms,
            )?)
        }
        Err(error) => Err(error.into()),
    }
}

async fn submit_stationary_with_lazy_map(
    state: &AppState,
    current: &PlayerState,
    submitted: crate::movement::SubmittedMovement,
    modifiers: crate::movement::MovementModifiers,
    now_ms: u64,
) -> Result<crate::movement::MovementDecision, ApiError> {
    match crate::movement::submit_stationary_observation(
        &state.movement,
        current,
        submitted,
        modifiers,
        state.gameplay.movement,
        now_ms,
    ) {
        Ok(decision) => Ok(decision),
        Err(crate::movement::MovementError::MissingMap { map_id }) => {
            let map = load_map(state, map_id).await?.ok_or_else(|| {
                ApiError::not_found("map_not_found", format!("map {map_id} does not exist"))
            })?;
            crate::movement::register_map(&state.movement, &map)?;
            Ok(crate::movement::submit_stationary_observation(
                &state.movement,
                current,
                submitted,
                modifiers,
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
    let player_guard = lock_player(&state, &player_id, &headers).await?;
    let now_ms = unix_time_ms()?;
    let mutation = begin_player_mutation(&state, &player_guard, &player_id, now_ms).await?;
    super::require_living_player(&mutation.player, "enter a portal")?;
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
    let source_map = load_map(&state, mutation.player.map_id)
        .await?
        .ok_or_else(|| {
            ApiError::not_found(
                "map_not_found",
                format!("map {} does not exist", mutation.player.map_id),
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
    let projected = mutation.effects.projected().modifiers;
    let (decision, relocation) = crate::movement::enter_portal_with_modifiers(
        &state.movement,
        &mutation.player,
        crate::movement::PortalMovement {
            source_map: &source_map,
            target_map: &target_map,
            source,
            target_portal_name: &request.target_portal_name,
        },
        crate::movement::MovementModifiers {
            speed: projected.speed,
            jump: projected.jump,
        },
        state.gameplay.movement,
        now_ms,
    )?;
    if let Some(relocation) = relocation {
        let response = movement_response(
            &state,
            &player_guard,
            mutation,
            decision,
            true,
            now_ms,
            Some(relocation),
        )
        .await?;
        return Ok(Protobuf(response));
    }
    Ok(Protobuf(
        movement_response(
            &state,
            &player_guard,
            mutation,
            decision,
            false,
            now_ms,
            None,
        )
        .await?,
    ))
}

fn movement_request_error(error: crate::movement::MovementError) -> ApiError {
    match error {
        crate::movement::MovementError::Tracker => error.into(),
        _ => ApiError::bad_request("invalid_movement_snapshot", error.to_string()),
    }
}

async fn movement_response(
    state: &AppState,
    guard: &crate::player_lock::PlayerGuard,
    mutation: super::PlayerMutation,
    decision: crate::movement::MovementDecision,
    persistence_required: bool,
    now_unix_ms: u64,
    relocation: Option<crate::movement::RelocationPlan>,
) -> Result<MovementUpdateResponse, ApiError> {
    let current = mutation.player;
    let planned_authoritative = relocation
        .as_ref()
        .map(|plan| crate::movement::project_relocation_observation(plan, current.clone()))
        .transpose()?;
    let mut transaction = crate::player_transaction::new_player_transaction(
        mutation.original,
        current.clone(),
        crate::player_transaction::PlayerPersistence::None,
    );
    let relocated = relocation.is_some();
    if let Some(plan) = relocation {
        crate::player_transaction::stage_relocation(&mut transaction, state.movement.clone(), plan);
    }
    let preparation = async {
        let map_id = decision.authoritative.map_id;
        let map = load_map(state, map_id).await?.ok_or_else(|| {
            ApiError::not_found("map_not_found", format!("map {map_id} does not exist"))
        })?;
        let authoritative = match planned_authoritative {
            Some(authoritative) => authoritative,
            None => {
                crate::movement::synchronize_player_observation(&state.movement, current.clone())?
            }
        };
        let authoritative_player = authoritative.player;
        let effects = mutation.effects;
        let equipment =
            crate::items::equipment_stats(&authoritative_player, state.catalog.as_ref())
                .map_err(super::item_rule_error)?;
        let dropped_items = crate::items::map_drops(&state.drops, map_id)?;
        let simulation = crate::mobs::observe_player_with_effects(
            &state.mobs,
            &map,
            &authoritative_player,
            authoritative.platform_layer,
            super::project_combat_effects(effects.projected(), equipment),
        )
        .await?;
        let has_player_effects =
            simulation.player_damage() > 0 || !simulation.mob_deaths.is_empty();
        let prepared = prepare_simulation_player_effects(
            state,
            authoritative_player,
            &simulation,
            effects.clone(),
            now_unix_ms,
            persistence_required,
        );
        Ok::<_, ApiError>((prepared, simulation, dropped_items, has_player_effects, map))
    }
    .await;
    let (mut transaction, (prepared, simulation, mut dropped_items, has_player_effects, map)) =
        resolve_movement_preparation(&state.database, guard, transaction, preparation).await?;
    crate::player_transaction::replace_staged_player(
        &mut transaction,
        prepared.player,
        if prepared.should_save {
            crate::player_transaction::PlayerPersistence::Full
        } else {
            crate::player_transaction::PlayerPersistence::None
        },
    );
    crate::player_transaction::stage_effects(
        &mut transaction,
        state.active_effects.clone(),
        mutation.original_effects,
        prepared.effects,
    );
    crate::player_transaction::stage_mob_update(
        &mut transaction,
        state.mobs.clone(),
        state.drops.clone(),
        simulation,
    )?;
    if decision.activity || relocated || has_player_effects {
        crate::player_transaction::stage_activity(
            &mut transaction,
            state.recovery_timers.clone(),
            current.id.clone(),
            now_unix_ms,
        );
    }
    let committed =
        crate::player_transaction::commit_player_transaction(&state.database, guard, transaction)
            .await?;
    let updated_player = committed.player;
    let effects = committed
        .effects
        .expect("movement transaction stages effects");
    merge_dropped_items(&mut dropped_items, committed.committed_drops);
    let mut simulation = committed
        .mob_update
        .expect("movement transaction stages a mob update");
    let mut relocation_map =
        take_relocation_map(relocated, map, &mut dropped_items, &mut simulation);
    if let Some(map) = relocation_map.as_mut() {
        let quest_definitions = state.catalog.quest_definitions().collect::<Vec<_>>();
        crate::quests::project_npc_quest_indicators(
            map,
            &updated_player,
            &effects,
            &quest_definitions,
            state.catalog.item_definition_slice(),
            &state.quest_scripts,
            crate::quests::QuestEnvironment {
                now_unix_ms,
                world_id: state.gameplay.world_id,
            },
        );
    }
    Ok(MovementUpdateResponse {
        authoritative: Some(decision.authoritative),
        accepted: decision.accepted,
        rejection_reason: decision.rejection_reason,
        mobs: std::mem::take(&mut simulation.mobs),
        player: Some(updated_player),
        mob_projectiles: std::mem::take(&mut simulation.mob_projectiles),
        combat_events: simulation.combat_events,
        simulation_sequence: simulation.sequence,
        active_buffs: Some(crate::effects::state(&effects, now_unix_ms)),
        dropped_items,
        reactors: simulation.reactors,
        map: relocation_map,
    })
}

fn take_relocation_map(
    relocated: bool,
    mut map: oozems_proto::v1::Map,
    dropped_items: &mut Vec<oozems_proto::v1::DroppedItem>,
    simulation: &mut crate::mobs::MobUpdate,
) -> Option<oozems_proto::v1::Map> {
    if !relocated {
        return None;
    }
    map.dropped_items = std::mem::take(dropped_items);
    map.mobs = std::mem::take(&mut simulation.mobs);
    map.mob_projectiles = std::mem::take(&mut simulation.mob_projectiles);
    map.reactors = std::mem::take(&mut simulation.reactors);
    map.simulation_sequence = simulation.sequence;
    Some(map)
}

async fn resolve_movement_preparation<T>(
    database: &crate::database::Database,
    guard: &crate::player_lock::PlayerGuard,
    transaction: crate::player_transaction::PlayerTransaction,
    preparation: Result<T, ApiError>,
) -> Result<(crate::player_transaction::PlayerTransaction, T), ApiError> {
    match preparation {
        Ok(prepared) => Ok((transaction, prepared)),
        Err(error) => {
            crate::player_transaction::abort_player_transaction(
                database,
                guard,
                transaction,
                error.to_string(),
            )
            .await?;
            Err(error)
        }
    }
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use oozems_proto::v1::DroppedItem;
    use oozems_proto::v1::Map;
    use oozems_proto::v1::Platform;
    use oozems_proto::v1::PlayerState;
    use oozems_proto::v1::Portal;
    use oozems_proto::v1::Vec2;

    use super::resolve_movement_preparation;
    use super::take_relocation_map;
    use crate::database::PlayerId;
    use crate::movement::MovementTracker;
    use crate::player_lock::PlayerLocks;
    use crate::player_lock::acquire_player;

    #[test]
    fn relocation_map_contains_the_complete_live_map_state() {
        let mut dropped_items = vec![DroppedItem::default()];
        let mut simulation = crate::mobs::MobUpdate::default();
        simulation.mobs.push(oozems_proto::v1::Mob::default());
        simulation
            .mob_projectiles
            .push(oozems_proto::v1::MobProjectile::default());
        simulation
            .reactors
            .push(oozems_proto::v1::Reactor::default());
        simulation.sequence = 42;

        let map = take_relocation_map(true, Map::default(), &mut dropped_items, &mut simulation)
            .expect("relocation map");

        assert_eq!(map.dropped_items.len(), 1);
        assert_eq!(map.mobs.len(), 1);
        assert_eq!(map.mob_projectiles.len(), 1);
        assert_eq!(map.reactors.len(), 1);
        assert_eq!(map.simulation_sequence, 42);
        assert!(dropped_items.is_empty());
        assert!(simulation.mobs.is_empty());
        assert!(simulation.mob_projectiles.is_empty());
        assert!(simulation.reactors.is_empty());
    }

    #[tokio::test]
    async fn preparation_failure_leaves_a_planned_relocation_uncommitted() {
        let directory = tempfile::tempdir().expect("temporary database directory");
        let database = crate::database::open_sqlite(&directory.path().join("players.sqlite"))
            .expect("open database");
        let player_id = PlayerId::parse("relocation-preparation").expect("player ID");
        let locks = PlayerLocks::default();
        let guard = acquire_player(&locks, player_id.as_str())
            .await
            .expect("player guard");
        let original = PlayerState {
            id: player_id.as_str().to_owned(),
            map_id: 100,
            position: Some(Vec2 { x: 10.0, y: 20.0 }),
            ..PlayerState::default()
        };
        let mut source_map = Map {
            id: 100,
            width: 800,
            height: 600,
            platforms: vec![Platform {
                x: 0.0,
                y: 20.0,
                end_x: 800.0,
                end_y: 20.0,
                ..Platform::default()
            }],
            ..Map::default()
        };
        source_map.portals.push(Portal {
            name: "source".to_owned(),
            x: 10.0,
            y: 20.0,
            target_map_id: 200,
            target_name: "taxi".to_owned(),
            ..Portal::default()
        });
        let target_map = Map {
            id: 200,
            width: 800,
            height: 600,
            platforms: source_map.platforms.clone(),
            portals: vec![Portal {
                name: "taxi".to_owned(),
                x: 700.0,
                y: 20.0,
                ..Portal::default()
            }],
            ..Map::default()
        };
        let movement = Arc::new(MovementTracker::default());
        let movement_config = movement_config();
        crate::movement::initialize_player(
            &movement,
            &original,
            &source_map,
            movement_config,
            1_000,
        )
        .expect("initialize movement");
        let (_, plan) = crate::movement::relocate_player(
            &movement,
            &original,
            &source_map,
            &target_map,
            "taxi",
            movement_config,
            1_200,
        )
        .expect("stage relocation");
        let relocated = crate::movement::project_relocation_player(&plan, original.clone())
            .expect("project relocation");
        let mut transaction = crate::player_transaction::new_player_transaction(
            original.clone(),
            relocated,
            crate::player_transaction::PlayerPersistence::Full,
        );
        crate::player_transaction::stage_relocation(&mut transaction, movement.clone(), plan);

        let planned = crate::movement::synchronize_player(&movement, original.clone())
            .expect("planned movement remains at source");
        assert_eq!(planned.map_id, original.map_id);
        assert_eq!(planned.position, original.position);

        let error = resolve_movement_preparation::<()>(
            &database,
            &guard,
            transaction,
            Err(super::ApiError::not_found(
                "map_not_found",
                "target map preparation failed",
            )),
        )
        .await
        .err()
        .expect("preparation must fail");

        assert!(matches!(
            error,
            super::ApiError::Client {
                status: axum::http::StatusCode::NOT_FOUND,
                ..
            }
        ));
        let unchanged = crate::movement::synchronize_player(&movement, original.clone())
            .expect("synchronize unchanged movement");
        assert_eq!(unchanged.map_id, original.map_id);
        assert_eq!(unchanged.position, original.position);
        assert!(
            crate::database::load_player(&database, &player_id)
                .await
                .expect("load durable player")
                .is_none()
        );
    }

    #[tokio::test]
    async fn cancelled_portal_preparation_leaves_the_source_session_unchanged() {
        let movement = Arc::new(MovementTracker::default());
        let original = PlayerState {
            id: "cancelled-relocation".to_owned(),
            map_id: 100,
            position: Some(Vec2 { x: 10.0, y: 20.0 }),
            ..PlayerState::default()
        };
        let mut source_map = Map {
            id: 100,
            width: 800,
            height: 600,
            platforms: vec![Platform {
                x: 0.0,
                y: 20.0,
                end_x: 800.0,
                end_y: 20.0,
                ..Platform::default()
            }],
            ..Map::default()
        };
        source_map.portals.push(Portal {
            name: "source".to_owned(),
            x: 10.0,
            y: 20.0,
            target_map_id: 200,
            target_name: "taxi".to_owned(),
            ..Portal::default()
        });
        let target_map = Map {
            id: 200,
            width: 800,
            height: 600,
            platforms: source_map.platforms.clone(),
            portals: vec![Portal {
                name: "taxi".to_owned(),
                x: 700.0,
                y: 20.0,
                ..Portal::default()
            }],
            ..Map::default()
        };
        crate::movement::initialize_player(
            &movement,
            &original,
            &source_map,
            movement_config(),
            1_000,
        )
        .expect("initialize movement");
        let (_, plan) = crate::movement::enter_portal_with_modifiers(
            &movement,
            &original,
            crate::movement::PortalMovement {
                source_map: &source_map,
                target_map: &target_map,
                source: crate::movement::SubmittedMovement {
                    sequence: 1,
                    map_id: source_map.id,
                    position: crate::movement::Position { x: 10.0, y: 20.0 },
                    mode: crate::movement::MovementMode::Grounded,
                    support_contact: None,
                    drop_through: false,
                },
                target_portal_name: "taxi",
            },
            crate::movement::MovementModifiers::default(),
            movement_config(),
            1_200,
        )
        .expect("plan portal relocation");
        let plan = plan.expect("accepted portal relocation plan");
        let relocated = crate::movement::project_relocation_player(&plan, original.clone())
            .expect("project relocation");
        let mut transaction = crate::player_transaction::new_player_transaction(
            original.clone(),
            relocated,
            crate::player_transaction::PlayerPersistence::Full,
        );
        crate::player_transaction::stage_relocation(&mut transaction, movement.clone(), plan);

        let preparation = tokio::spawn(async move {
            let _transaction = transaction;
            std::future::pending::<()>().await;
        });
        tokio::task::yield_now().await;
        preparation.abort();
        let _ = preparation.await;

        let unchanged = crate::movement::synchronize_player(&movement, original.clone())
            .expect("synchronize source movement");
        assert_eq!(unchanged.map_id, original.map_id);
        assert_eq!(unchanged.position, original.position);
    }

    fn movement_config() -> crate::gameplay::MovementConfig {
        crate::gameplay::MovementConfig {
            walk_speed: 125.0,
            climb_speed: 100.0,
            gravity: 2_000.0,
            jump_speed: 500.0,
            speed_cap: 140,
            jump_cap: 123,
            snapshot_interval: Duration::from_millis(100),
            maximum_snapshot_gap: Duration::from_secs(2),
            position_tolerance: 20.0,
            ground_tolerance: 10.0,
            platform_edge_tolerance: 10.0,
            ladder_reach: 20.0,
            ladder_end_reach: 20.0,
            portal_horizontal_reach: 80.0,
            portal_vertical_reach: 80.0,
        }
    }
}
