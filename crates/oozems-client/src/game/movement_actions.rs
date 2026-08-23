use std::cell::Cell;
use std::cell::RefCell;
use std::rc::Rc;

use oozems_proto::v1::MovementContact;
use oozems_proto::v1::MovementMode;
use oozems_proto::v1::MovementSnapshot;
use oozems_proto::v1::MovementUpdateResponse;
use oozems_proto::v1::Vec2;
use wasm_bindgen_futures::spawn_local;

use super::Game;
use crate::api;
use crate::character_render::CharacterAnimation;
use crate::movement;
use crate::movement::MapTransition;
use crate::render;
use crate::show_status;
use crate::skill_effects;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct MovementObservation {
    map_id: u32,
    position: Vec2,
    mode: MovementMode,
}

#[derive(Default)]
pub(super) struct MovementSyncState {
    in_flight: Rc<Cell<bool>>,
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
    timestamp_ms: f64,
    observation: Option<MovementObservation>,
) -> bool {
    if let Some(observation) = observation {
        preserve_support_transition(state, observation);
    }
    let deadline_ms = state.next_snapshot_ms.get_or_insert(timestamp_ms);
    if timestamp_ms < *deadline_ms || !can_submit || state.in_flight.get() {
        return false;
    }
    state.next_snapshot_ms = Some(timestamp_ms + snapshot_interval_ms.max(1) as f64);
    true
}

pub(super) fn begin(game: Rc<RefCell<Game>>) {
    let in_flight = game.borrow().movement_sync.in_flight.clone();
    if in_flight.replace(true) {
        return;
    }
    let request = {
        let mut game = game.borrow_mut();
        next_movement_snapshot(&mut game).map(|snapshot| (game.player.id.clone(), snapshot))
    };
    let Some((player_id, snapshot)) = request else {
        in_flight.set(false);
        return;
    };
    spawn_local(async move {
        match api::submit_movement(&player_id, snapshot).await {
            Ok(response) => match install_response(&mut game.borrow_mut(), response) {
                Ok(Some(reason)) => show_status(&format!("Movement corrected: {reason}"), true),
                Ok(None) => {}
                Err(error) => show_status(&format!("Movement sync failed: {error}"), true),
            },
            Err(error) => show_status(&format!("Movement sync failed: {error}"), true),
        }
        in_flight.set(false);
    });
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
    transition: MapTransition,
) {
    let transition_in_flight = game.borrow().transition_in_flight.clone();
    spawn_local(async move {
        let request = {
            let mut game = game.borrow_mut();
            current_movement_snapshot(&mut game).map(|snapshot| (game.player.id.clone(), snapshot))
        };
        let result = match request {
            Some((player_id, source)) => {
                request_map_transition(&game, &player_id, source, &transition).await
            }
            None => Err("character has no movement position".to_owned()),
        };
        match result {
            Ok(name) => show_status(&format!("Entered {name}."), false),
            Err(error) => show_status(&format!("Could not enter map: {error}"), true),
        }
        transition_in_flight.set(false);
    });
}

async fn request_map_transition(
    game: &Rc<RefCell<Game>>,
    player_id: &str,
    source: MovementSnapshot,
    transition: &MapTransition,
) -> Result<String, String> {
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
        install_response(&mut game.borrow_mut(), response)?;
        return Err(reason);
    }
    let authoritative = portal_authoritative(&mut game.borrow_mut(), &response)?
        .ok_or("portal response was superseded by a newer movement response")?;
    let position = authoritative
        .position
        .ok_or("portal response did not contain a destination position")?;
    let map = api::get_map(authoritative.map_id)
        .await
        .map_err(|error| error.to_string())?;
    let name = map.name.clone();
    let mut game = game.borrow_mut();
    install_map(&mut game, map, position)?;
    if let Some(stats) = response.player_stats {
        game.player.stats = Some(stats);
    }
    let timestamp_ms = game.frame_time_ms;
    crate::mob_render::install_combat_events(
        &mut game.mob_render,
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
    crate::assets::insert_assets(&mut game.images, map.assets.iter())?;
    let motion = movement::initial_motion_state(&map, &position);
    let world_layers = render::world_layers(&map);
    skill_effects::clear(&mut game.skill_effect_state);
    super::recovery_actions::reset(&mut game.recovery_state);
    reset_schedule(&mut game.movement_sync, game.frame_time_ms);

    game.player.map_id = map.id;
    game.player.position = Some(position);
    game.mob_render = crate::mob_render::new_map_state(map.simulation_sequence);
    game.map = map;
    game.motion = motion;
    game.world_layers = world_layers;
    game.character_animation =
        super::new_character_animation_state(CharacterAnimation::Idle, true, game.frame_time_ms);
    Ok(())
}

fn next_movement_snapshot(game: &mut Game) -> Option<MovementSnapshot> {
    let current = current_observation(game)?;
    let (support, drop_through) = take_pending_contact(&mut game.movement_sync);
    Some(snapshot_from_observation(
        &mut game.movement_sync,
        current,
        support,
        drop_through,
    ))
}

fn current_movement_snapshot(game: &mut Game) -> Option<MovementSnapshot> {
    let observation = current_observation(game)?;
    game.movement_sync.pending_support = None;
    game.movement_sync.pending_drop_through = None;
    Some(snapshot_from_observation(
        &mut game.movement_sync,
        observation,
        None,
        false,
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
    observation(game.map.id, game.player.position, game.motion)
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
    response: &MovementUpdateResponse,
) -> Result<Option<MovementSnapshot>, String> {
    let authoritative = response
        .authoritative
        .ok_or("movement response did not contain an authoritative snapshot")?;
    if authoritative.sequence < game.movement_sync.last_response_sequence {
        return Ok(None);
    }
    game.movement_sync.last_response_sequence = authoritative.sequence;
    super::buffs::install(
        &mut game.active_buffs,
        response.active_buffs.clone().unwrap_or_default(),
        js_sys::Date::now().max(0.0) as u64,
    );
    Ok(Some(authoritative))
}

pub(super) fn install_response(
    game: &mut Game,
    mut response: MovementUpdateResponse,
) -> Result<Option<String>, String> {
    let authoritative = response
        .authoritative
        .ok_or("movement response did not contain an authoritative snapshot")?;
    if authoritative.sequence < game.movement_sync.last_response_sequence {
        return Ok(None);
    }
    game.movement_sync.last_response_sequence = authoritative.sequence;
    super::buffs::install(
        &mut game.active_buffs,
        response.active_buffs.take().unwrap_or_default(),
        js_sys::Date::now().max(0.0) as u64,
    );
    if authoritative.map_id == game.map.id
        && crate::mob_render::accept_simulation_snapshot(
            &mut game.mob_render,
            response.simulation_sequence,
        )
    {
        crate::mob_render::install_snapshot(
            &mut game.mob_render,
            &mut game.map.mobs,
            std::mem::take(&mut response.mobs),
            game.frame_time_ms,
            game.movement_rules.snapshot_interval_ms,
        );
        crate::mob_render::install_projectile_snapshot(
            &mut game.mob_render,
            &mut game.map.mob_projectiles,
            std::mem::take(&mut response.mob_projectiles),
            game.frame_time_ms,
            game.movement_rules.snapshot_interval_ms,
        );
        crate::mob_render::install_combat_events(
            &mut game.mob_render,
            std::mem::take(&mut response.combat_events),
            game.frame_time_ms,
        );
        if let Some(stats) = response.player_stats {
            game.player.stats = Some(stats);
        }
    }
    if response.accepted {
        return Ok(None);
    }
    if authoritative.map_id != game.map.id {
        if game.transition_in_flight.get() {
            return Ok(None);
        }
        return Err(format!(
            "server position is on map {}, but the client has map {}",
            authoritative.map_id, game.map.id
        ));
    }
    let position = authoritative
        .position
        .ok_or("authoritative movement snapshot has no position")?;
    let position = movement::constrain_position(&game.map, position);
    let mode = MovementMode::try_from(authoritative.mode)
        .map_err(|_| "authoritative movement snapshot has an invalid mode")?;
    let motion = movement::authoritative_motion_state(
        &game.map,
        &game.movement_rules,
        &position,
        mode,
        game.motion.platform_layer,
    )?;
    reset_after_correction(&mut game.movement_sync, mode);
    game.player.position = Some(position);
    game.motion = motion;
    let animation = super::character_animation(&game.map, motion, movement::PlayerInput::default());
    let plays = !matches!(
        animation,
        CharacterAnimation::Ladder | CharacterAnimation::Rope
    );
    super::update_character_animation(
        &mut game.character_animation,
        animation,
        plays,
        game.frame_time_ms,
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
            1_000.0,
            Some(observation(MovementMode::Grounded)),
        ));
        assert!(!update(
            &mut state,
            200,
            true,
            1_199.0,
            Some(observation(MovementMode::Grounded)),
        ));
        assert!(update(
            &mut state,
            200,
            true,
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
            1_000.0,
            Some(observation(MovementMode::Grounded)),
        ));
        assert!(update(
            &mut state,
            200,
            true,
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
            1_000.0,
            Some(observation(MovementMode::Grounded)),
        ));
        state.in_flight.set(true);
        assert!(!update(
            &mut state,
            200,
            true,
            1_050.0,
            Some(observation(MovementMode::Airborne)),
        ));
        assert!(!update(
            &mut state,
            200,
            true,
            1_100.0,
            Some(observation(MovementMode::Grounded)),
        ));
        assert!(!update(
            &mut state,
            200,
            true,
            1_150.0,
            Some(observation(MovementMode::Airborne)),
        ));

        state.in_flight.set(false);
        assert!(update(
            &mut state,
            200,
            true,
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
        let (support, drop_through) = take_pending_contact(&mut state);
        let snapshot = snapshot_from_observation(&mut state, current, support, drop_through);

        assert_eq!(state.next_snapshot_ms, Some(0.0));
        assert!(snapshot.drop_through);
        assert_eq!(
            snapshot
                .support_contact
                .and_then(|contact| contact.position),
            Some(Vec2 { x: 100.0, y: 300.0 }),
        );
        assert_eq!(take_pending_contact(&mut state), (None, false));
    }

    fn observation(mode: MovementMode) -> MovementObservation {
        MovementObservation {
            map_id: 1,
            position: Vec2 { x: 100.0, y: 300.0 },
            mode,
        }
    }
}
