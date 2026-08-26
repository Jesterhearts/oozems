use std::cell::RefCell;
use std::rc::Rc;

use oozems_proto::v1::MovementContact;
use oozems_proto::v1::MovementMode;
use oozems_proto::v1::MovementSnapshot;
use oozems_proto::v1::MovementUpdateResponse;
use oozems_proto::v1::Vec2;

use super::Game;
use crate::api;
use crate::character_render::CharacterAnimation;
use crate::movement;
use crate::movement::MapTransition;
use crate::render;
use crate::skill_effects;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct MovementObservation {
    map_id: u32,
    position: Vec2,
    mode: MovementMode,
}

#[derive(Default)]
pub(super) struct MovementSyncState {
    next_snapshot_ms: Option<f64>,
    next_sequence: u64,
    last_response_sequence: u64,
    last_observed_mode: Option<MovementMode>,
    pending_support: Option<MovementObservation>,
    pending_drop_through: Option<MovementObservation>,
}

pub(super) fn observation(
    map_id: u32,
    position: Option<Vec2>,
    motion: movement::MotionState,
) -> Option<MovementObservation> {
    position.map(|position| MovementObservation {
        map_id,
        position,
        mode: movement::motion_mode(motion),
    })
}

pub(super) fn update(
    state: &mut MovementSyncState,
    snapshot_interval_ms: u64,
    can_submit: bool,
    in_flight: bool,
    timestamp_ms: f64,
    observation: Option<MovementObservation>,
) -> bool {
    if let Some(observation) = observation {
        preserve_support_transition(state, observation);
    }
    let deadline_ms = state.next_snapshot_ms.get_or_insert(timestamp_ms);
    if timestamp_ms < *deadline_ms || !can_submit || in_flight {
        return false;
    }
    state.next_snapshot_ms = Some(timestamp_ms + snapshot_interval_ms.max(1) as f64);
    true
}

pub(super) fn begin(
    game: Rc<RefCell<Game>>,
    permit: super::requests::RequestPermit,
) {
    let request = {
        let mut game = game.borrow_mut();
        next_movement_snapshot(&mut game).map(|snapshot| (game.player.id.clone(), snapshot))
    };
    let Some((player_id, snapshot)) = request else {
        return;
    };
    super::requests::spawn_request(
        game,
        permit,
        move || async move {
            api::submit_movement(&player_id, snapshot)
                .await
                .map_err(|error| error.to_string())
        },
        |game, result, request_started_ms| match result {
            Ok(response) => match install_response(game, response, request_started_ms) {
                Ok(Some(reason)) => {
                    super::requests::RequestStatus::error(format!("Movement corrected: {reason}"))
                }
                Ok(None) => super::requests::RequestStatus::silent(),
                Err(error) => {
                    super::requests::RequestStatus::error(format!("Movement sync failed: {error}"))
                }
            },
            Err(error) => {
                super::requests::RequestStatus::error(format!("Movement sync failed: {error}"))
            }
        },
    );
}

pub(super) fn record_drop_through(
    state: &mut MovementSyncState,
    map_id: u32,
    position: Vec2,
    motion: movement::MotionState,
) {
    let origin = MovementObservation {
        map_id,
        position,
        mode: movement::motion_mode(motion),
    };
    if origin.mode != MovementMode::Grounded {
        return;
    }
    state.pending_drop_through = Some(origin);
    state.pending_support = None;
    state.next_snapshot_ms = Some(0.0);
}

pub(super) fn begin_portal(
    game: Rc<RefCell<Game>>,
    player_id: String,
    source: MovementSnapshot,
    transition: MapTransition,
    permit: super::requests::RequestPermit,
) {
    let request_game = game.clone();
    super::requests::spawn_request(
        game,
        permit,
        move || async move {
            request_map_transition(&request_game, &player_id, source, &transition).await
        },
        |_, result, _| match result {
            Ok(name) => super::requests::RequestStatus::success(format!("Entered {name}.")),
            Err(error) => {
                super::requests::RequestStatus::error(format!("Could not enter map: {error}"))
            }
        },
    );
}

async fn request_map_transition(
    game: &Rc<RefCell<Game>>,
    player_id: &str,
    source: MovementSnapshot,
    transition: &MapTransition,
) -> Result<String, String> {
    let request_started_ms = super::monotonic_time_ms();
    let mut response = api::enter_portal(
        player_id,
        source,
        transition.target_map_id,
        &transition.target_portal_name,
    )
    .await
    .map_err(|error| error.to_string())?;
    if !response.accepted {
        let reason = response.rejection_reason.clone();
        install_response(&mut game.borrow_mut(), response, request_started_ms)?;
        return Err(reason);
    }
    let authoritative =
        portal_authoritative(&mut game.borrow_mut(), &mut response, request_started_ms)?
            .ok_or("portal response was superseded by a newer movement response")?;
    let position = authoritative
        .position
        .ok_or("portal response did not contain a destination position")?;
    let map = api::get_map(player_id, authoritative.map_id)
        .await
        .map_err(|error| error.to_string())?;
    let name = map.name.clone();
    let mut game = game.borrow_mut();
    if authoritative.sequence < game.requests.movement.last_response_sequence {
        return Err("portal response was superseded while its map was loading".to_owned());
    }
    install_map(&mut game, map, position)?;
    let timestamp_ms = game.clock.now_ms;
    crate::mob_render::install_combat_events(
        &mut game.world.mob_render,
        std::mem::take(&mut response.combat_events),
        timestamp_ms,
    );
    Ok(name)
}

fn install_map(
    game: &mut Game,
    map: oozems_proto::v1::Map,
    position: Vec2,
) -> Result<(), String> {
    let position = movement::constrain_position(&map, position);
    crate::assets::insert_assets(&mut game.surface.images, map.assets.iter())?;
    let motion = movement::initial_motion_state(&map, &position);
    let world_layers = render::world_layers(&map);
    skill_effects::clear(&mut game.world.skill_effect_state);
    crate::render::npc::clear(&mut game.world.npc_animations);
    super::recovery_actions::reset(&mut game.requests.recovery);
    let now_ms = game.clock.now_ms;
    reset_schedule(&mut game.requests.movement, now_ms);

    game.player.map_id = map.id;
    game.player.position = Some(position);
    game.world.mob_render = crate::mob_render::new_map_state(map.simulation_sequence);
    game.world.map = map;
    game.ui.interaction.close();
    game.world.motion = motion;
    game.world.world_layers = world_layers;
    game.world.character_animation = super::runtime::new_character_animation_state(
        CharacterAnimation::Idle,
        true,
        game.clock.now_ms,
    );
    Ok(())
}

pub(super) fn install_relocation(
    game: &mut Game,
    map: oozems_proto::v1::Map,
    authoritative: MovementSnapshot,
) -> Result<bool, String> {
    if authoritative.map_id != map.id {
        return Err("taxi response map does not match its authoritative position".to_owned());
    }
    if authoritative.sequence < game.requests.movement.last_response_sequence {
        return Ok(false);
    }
    let position = authoritative
        .position
        .ok_or("taxi response did not contain a destination position")?;
    game.requests.movement.last_response_sequence = authoritative.sequence;
    install_map(game, map, position)?;
    Ok(true)
}

fn next_movement_snapshot(game: &mut Game) -> Option<MovementSnapshot> {
    let current = current_observation(game)?;
    capture_movement_snapshot(&mut game.requests.movement, Some(current))
}

pub(super) fn capture_movement_snapshot(
    state: &mut MovementSyncState,
    observation: Option<MovementObservation>,
) -> Option<MovementSnapshot> {
    let observation = observation?;
    let (support, drop_through) = take_pending_contact(state);
    Some(snapshot_from_observation(
        state,
        observation,
        support,
        drop_through,
    ))
}

fn take_pending_contact(state: &mut MovementSyncState) -> (Option<MovementObservation>, bool) {
    if let Some(origin) = state.pending_drop_through.take() {
        state.pending_support = None;
        return (Some(origin), true);
    }
    (state.pending_support.take(), false)
}

fn current_observation(game: &Game) -> Option<MovementObservation> {
    observation(game.world.map.id, game.player.position, game.world.motion)
}

fn snapshot_from_observation(
    state: &mut MovementSyncState,
    observation: MovementObservation,
    support: Option<MovementObservation>,
    drop_through: bool,
) -> MovementSnapshot {
    state.next_sequence = state.next_sequence.saturating_add(1);
    MovementSnapshot {
        sequence: state.next_sequence,
        map_id: observation.map_id,
        position: Some(observation.position),
        mode: observation.mode as i32,
        support_contact: support.map(|support| MovementContact {
            position: Some(support.position),
            mode: support.mode as i32,
        }),
        drop_through,
    }
}

fn preserve_support_transition(
    state: &mut MovementSyncState,
    observation: MovementObservation,
) {
    let changed = state
        .last_observed_mode
        .is_some_and(|mode| mode != observation.mode);
    state.last_observed_mode = Some(observation.mode);
    if !changed || observation.mode == MovementMode::Airborne {
        return;
    }
    state.pending_support = Some(observation);
}

fn reset_after_correction(
    state: &mut MovementSyncState,
    mode: MovementMode,
) {
    state.pending_support = None;
    state.pending_drop_through = None;
    state.last_observed_mode = Some(mode);
}

pub(super) fn portal_authoritative(
    game: &mut Game,
    response: &mut MovementUpdateResponse,
    request_started_ms: f64,
) -> Result<Option<MovementSnapshot>, String> {
    let authoritative = api::require_data(response.authoritative.take(), "authoritative snapshot")
        .map_err(|error| error.to_string())?;
    let (player, active_buffs) = super::responses::take_player_and_active_buffs(response)?;
    super::install_active_buffs(game, active_buffs, request_started_ms);
    super::install_full_player_update(game, player);
    if authoritative.sequence < game.requests.movement.last_response_sequence {
        return Ok(None);
    }
    game.requests.movement.last_response_sequence = authoritative.sequence;
    Ok(Some(authoritative))
}

pub(super) fn install_response(
    game: &mut Game,
    mut response: MovementUpdateResponse,
    request_started_ms: f64,
) -> Result<Option<String>, String> {
    let authoritative = api::require_data(response.authoritative.take(), "authoritative snapshot")
        .map_err(|error| error.to_string())?;
    let (player, active_buffs) = super::responses::take_player_and_active_buffs(&mut response)?;
    super::install_active_buffs(game, active_buffs, request_started_ms);
    if authoritative.map_id == game.world.map.id
        && crate::mob_render::accept_simulation_snapshot(
            &mut game.world.mob_render,
            response.simulation_sequence,
        )
    {
        crate::mob_render::install_snapshot(
            &mut game.world.mob_render,
            &mut game.world.map.mobs,
            std::mem::take(&mut response.mobs),
            game.clock.now_ms,
            game.world.movement_rules.snapshot_interval_ms,
        );
        crate::mob_render::install_projectile_snapshot(
            &mut game.world.mob_render,
            &mut game.world.map.mob_projectiles,
            std::mem::take(&mut response.mob_projectiles),
            game.clock.now_ms,
            game.world.movement_rules.snapshot_interval_ms,
        );
        game.world.map.dropped_items = std::mem::take(&mut response.dropped_items);
        crate::mob_render::install_combat_events(
            &mut game.world.mob_render,
            std::mem::take(&mut response.combat_events),
            game.clock.now_ms,
        );
    }
    super::install_full_player_update(game, player);
    if authoritative.sequence < game.requests.movement.last_response_sequence {
        return Ok(None);
    }
    game.requests.movement.last_response_sequence = authoritative.sequence;
    if response.accepted {
        return Ok(None);
    }
    if authoritative.map_id != game.world.map.id {
        if game
            .requests
            .admission
            .is_active(super::requests::RequestKind::Transition)
        {
            return Ok(None);
        }
        return Err(format!(
            "server position is on map {}, but the client has map {}",
            authoritative.map_id, game.world.map.id
        ));
    }
    let position = authoritative
        .position
        .ok_or("authoritative movement snapshot has no position")?;
    let position = movement::constrain_position(&game.world.map, position);
    let mode = MovementMode::try_from(authoritative.mode)
        .map_err(|_| "authoritative movement snapshot has an invalid mode")?;
    let motion = movement::authoritative_motion_state(
        &game.world.map,
        &game.world.movement_rules,
        &position,
        mode,
        game.world.motion.platform_layer,
    )?;
    reset_after_correction(&mut game.requests.movement, mode);
    game.player.position = Some(position);
    game.world.motion = motion;
    let animation = super::runtime::character_animation(
        &game.world.map,
        motion,
        movement::PlayerInput::default(),
    );
    let plays = !matches!(
        animation,
        CharacterAnimation::Ladder | CharacterAnimation::Rope
    );
    super::runtime::update_character_animation(
        &mut game.world.character_animation,
        animation,
        plays,
        game.clock.now_ms,
    );
    Ok(Some(response.rejection_reason))
}

pub(super) fn reset_schedule(
    state: &mut MovementSyncState,
    timestamp_ms: f64,
) {
    state.next_snapshot_ms = Some(timestamp_ms);
    state.pending_support = None;
    state.pending_drop_through = None;
    state.last_observed_mode = None;
}

#[cfg(test)]
mod tests {
    use oozems_proto::v1::MovementMode;
    use oozems_proto::v1::Vec2;

    use super::MovementObservation;
    use super::MovementSyncState;
    use super::capture_movement_snapshot;
    use super::record_drop_through;
    use super::snapshot_from_observation;
    use super::take_pending_contact;
    use super::update;
    use crate::movement::MotionState;

    #[test]
    fn snapshots_start_immediately_then_follow_the_server_interval() {
        let mut state = MovementSyncState::default();

        assert!(update(
            &mut state,
            200,
            true,
            false,
            1_000.0,
            Some(observation(MovementMode::Grounded)),
        ));
        assert!(!update(
            &mut state,
            200,
            true,
            false,
            1_199.0,
            Some(observation(MovementMode::Grounded)),
        ));
        assert!(update(
            &mut state,
            200,
            true,
            false,
            1_200.0,
            Some(observation(MovementMode::Grounded)),
        ));
    }

    #[test]
    fn a_busy_client_keeps_an_overdue_snapshot_pending() {
        let mut state = MovementSyncState::default();

        assert!(!update(
            &mut state,
            200,
            false,
            false,
            1_000.0,
            Some(observation(MovementMode::Grounded)),
        ));
        assert!(update(
            &mut state,
            200,
            true,
            false,
            1_001.0,
            Some(observation(MovementMode::Grounded)),
        ));
    }

    #[test]
    fn brief_ground_contact_is_preserved_until_the_next_snapshot() {
        let mut state = MovementSyncState::default();
        assert!(update(
            &mut state,
            200,
            true,
            false,
            1_000.0,
            Some(observation(MovementMode::Grounded)),
        ));
        assert!(!update(
            &mut state,
            200,
            true,
            true,
            1_050.0,
            Some(observation(MovementMode::Airborne)),
        ));
        assert!(!update(
            &mut state,
            200,
            true,
            true,
            1_100.0,
            Some(observation(MovementMode::Grounded)),
        ));
        assert!(!update(
            &mut state,
            200,
            true,
            true,
            1_150.0,
            Some(observation(MovementMode::Airborne)),
        ));

        assert!(update(
            &mut state,
            200,
            true,
            false,
            1_200.0,
            Some(observation(MovementMode::Airborne)),
        ));
        assert_eq!(
            state.pending_support.map(|pending| pending.mode),
            Some(MovementMode::Grounded),
        );
    }

    #[test]
    fn support_contact_does_not_replace_the_current_snapshot_position() {
        let mut state = MovementSyncState::default();
        let current = MovementObservation {
            map_id: 1,
            position: Vec2 { x: 140.0, y: 260.0 },
            mode: MovementMode::Airborne,
        };
        let support = MovementObservation {
            map_id: 1,
            position: Vec2 { x: 120.0, y: 300.0 },
            mode: MovementMode::Grounded,
        };

        let snapshot = snapshot_from_observation(&mut state, current, Some(support), false);

        assert_eq!(snapshot.position, Some(current.position));
        assert_eq!(
            snapshot
                .support_contact
                .and_then(|contact| contact.position),
            Some(support.position),
        );
    }

    #[test]
    fn drop_through_origin_is_sent_once_with_the_next_snapshot() {
        let mut state = MovementSyncState::default();
        record_drop_through(
            &mut state,
            1,
            Vec2 { x: 100.0, y: 300.0 },
            MotionState {
                on_ground: true,
                ..MotionState::default()
            },
        );

        let current = MovementObservation {
            map_id: 1,
            position: Vec2 { x: 100.0, y: 302.0 },
            mode: MovementMode::Airborne,
        };
        let snapshot =
            capture_movement_snapshot(&mut state, Some(current)).expect("portal snapshot");

        assert_eq!(state.next_snapshot_ms, Some(0.0));
        assert!(snapshot.drop_through);
        assert_eq!(
            snapshot
                .support_contact
                .and_then(|contact| contact.position),
            Some(Vec2 { x: 100.0, y: 300.0 }),
        );
        assert_eq!(take_pending_contact(&mut state), (None, false));
        assert_eq!(snapshot.sequence, 1);

        let later = capture_movement_snapshot(
            &mut state,
            Some(MovementObservation {
                position: Vec2 { x: 500.0, y: 302.0 },
                ..current
            }),
        )
        .expect("later snapshot");
        assert_eq!(later.sequence, 2);
        assert_eq!(snapshot.position, Some(Vec2 { x: 100.0, y: 302.0 }));
    }

    fn observation(mode: MovementMode) -> MovementObservation {
        MovementObservation {
            map_id: 1,
            position: Vec2 { x: 100.0, y: 300.0 },
            mode,
        }
    }
}
