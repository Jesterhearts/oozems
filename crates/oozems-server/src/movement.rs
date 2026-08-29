use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use oozems_proto::v1::Map;
use oozems_proto::v1::MovementMode as ProtoMovementMode;
use oozems_proto::v1::MovementSnapshot;
use oozems_proto::v1::PlayerState;
use oozems_proto::v1::Vec2;
use thiserror::Error;

use crate::gameplay::MovementConfig;

mod terrain;

use terrain::clamp_to_movement_bounds;
use terrain::default_spawn_position as resolve_default_spawn_position;
use terrain::movement_crosses_vertical_foothold;
use terrain::named_portal_position as resolve_named_portal_position;
use terrain::platform_y;
use terrain::supporting_foothold;
use terrain::supporting_platform;
use terrain::within_map;

const SCRIPT_PORTAL_TARGET: u32 = 999_999_999;
const DROP_THROUGH_MINIMUM: f32 = 1.0;
const DROP_THROUGH_DESTINATION_CLEARANCE: f32 = 2.0;

#[derive(Default)]
pub struct MovementTracker {
    players: Mutex<HashMap<String, PlayerMovement>>,
    maps: Mutex<HashMap<u32, Arc<Map>>>,
}

#[derive(Default)]
struct PlayerMovement {
    session: Option<MovementSession>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct MovementSession {
    sequence: u64,
    map_id: u32,
    position: Position,
    mode: MovementMode,
    platform_layer: i32,
    received_at_ms: u64,
    airborne: Option<AirborneState>,
    horizontal_credit: f32,
    vertical_credit: f32,
    climb_credit: f32,
    modifiers: MovementModifiers,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct AirborneState {
    origin_y: f32,
    started_at_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MovementMode {
    Grounded,
    Airborne,
    Climbing,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Position {
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct SubmittedMovement {
    pub sequence: u64,
    pub map_id: u32,
    pub position: Position,
    pub mode: MovementMode,
    pub support_contact: Option<SupportContact>,
    pub drop_through: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct SupportContact {
    pub position: Position,
    pub mode: MovementMode,
}

pub struct PortalMovement<'a> {
    pub source_map: &'a Map,
    pub target_map: &'a Map,
    pub source: SubmittedMovement,
    pub target_portal_name: &'a str,
}

#[derive(Clone, Debug)]
pub struct MovementDecision {
    pub authoritative: MovementSnapshot,
    pub accepted: bool,
    pub rejection_reason: String,
    pub activity: bool,
}

pub struct SynchronizedPlayer {
    pub player: PlayerState,
    pub platform_layer: i32,
}

#[derive(Clone, Debug)]
pub struct RelocationPlan {
    player_id: String,
    expected_source: Option<MovementSession>,
    target: MovementSession,
}

#[derive(Debug)]
pub struct CommittedRelocation {
    plan: RelocationPlan,
}

#[derive(Debug, Error)]
pub enum MovementError {
    #[error("the movement tracker is unavailable")]
    Tracker,
    #[error("map {map_id} is not available to the movement tracker")]
    MissingMap { map_id: u32 },
    #[error("the movement snapshot does not contain a position")]
    MissingPosition,
    #[error("the movement snapshot position must be finite")]
    NonFinitePosition,
    #[error("the movement snapshot mode is invalid")]
    InvalidMode,
    #[error("the movement support contact does not contain a position")]
    MissingSupportPosition,
    #[error("the movement support contact position must be finite")]
    NonFiniteSupportPosition,
    #[error("the movement support contact mode must be grounded or climbing")]
    InvalidSupportMode,
    #[error("the player does not have an authoritative position")]
    MissingPlayerPosition,
    #[error("the player does not have an authoritative movement session")]
    MissingSession,
    #[error("player map {player_map_id} does not match supplied map {map_id}")]
    PlayerMapMismatch { player_map_id: u32, map_id: u32 },
    #[error("the relocation plan belongs to player {expected:?}, not {actual:?}")]
    RelocationPlayerMismatch { expected: String, actual: String },
    #[error("the destination map has no usable default spawn portal")]
    MissingDefaultSpawn,
    #[error("the destination map has no usable portal named {portal_name:?}")]
    MissingDestinationPortal { portal_name: String },
    #[error("the player movement session changed before relocation commit")]
    RelocationSourceChanged,
    #[error("the player movement session changed before relocation rollback")]
    RelocationTargetChanged,
}

pub fn parse_snapshot(snapshot: MovementSnapshot) -> Result<SubmittedMovement, MovementError> {
    let position = snapshot.position.ok_or(MovementError::MissingPosition)?;
    if !position.x.is_finite() || !position.y.is_finite() {
        return Err(MovementError::NonFinitePosition);
    }
    let mode = parse_mode(snapshot.mode)?;
    let support_contact = snapshot
        .support_contact
        .map(|contact| {
            let position = contact
                .position
                .ok_or(MovementError::MissingSupportPosition)?;
            if !position.x.is_finite() || !position.y.is_finite() {
                return Err(MovementError::NonFiniteSupportPosition);
            }
            let mode = parse_mode(contact.mode).map_err(|_| MovementError::InvalidSupportMode)?;
            if mode == MovementMode::Airborne {
                return Err(MovementError::InvalidSupportMode);
            }
            Ok(SupportContact {
                position: Position {
                    x: position.x,
                    y: position.y,
                },
                mode,
            })
        })
        .transpose()?;
    Ok(SubmittedMovement {
        sequence: snapshot.sequence,
        map_id: snapshot.map_id,
        position: Position {
            x: position.x,
            y: position.y,
        },
        mode,
        support_contact,
        drop_through: snapshot.drop_through,
    })
}

fn parse_mode(value: i32) -> Result<MovementMode, MovementError> {
    match ProtoMovementMode::try_from(value) {
        Ok(ProtoMovementMode::Grounded) => Ok(MovementMode::Grounded),
        Ok(ProtoMovementMode::Airborne) => Ok(MovementMode::Airborne),
        Ok(ProtoMovementMode::Climbing) => Ok(MovementMode::Climbing),
        Ok(ProtoMovementMode::Unspecified) | Err(_) => Err(MovementError::InvalidMode),
    }
}

pub fn initialize_player(
    tracker: &MovementTracker,
    player: &PlayerState,
    map: &Map,
    config: MovementConfig,
    now_ms: u64,
) -> Result<(), MovementError> {
    require_player_map(player, map)?;
    register_map(tracker, map)?;
    let position = player
        .position
        .as_ref()
        .ok_or(MovementError::MissingPlayerPosition)?;
    let session = initial_session(
        player.map_id,
        Position {
            x: position.x,
            y: position.y,
        },
        map,
        config,
        now_ms,
    );
    tracker
        .players
        .lock()
        .map_err(|_| MovementError::Tracker)?
        .insert(
            player.id.clone(),
            PlayerMovement {
                session: Some(session),
            },
        );
    Ok(())
}

pub fn synchronize_player(
    tracker: &MovementTracker,
    mut player: PlayerState,
) -> Result<PlayerState, MovementError> {
    let players = tracker.players.lock().map_err(|_| MovementError::Tracker)?;
    if let Some(session) = players
        .get(&player.id)
        .and_then(|movement| movement.session)
    {
        player.map_id = session.map_id;
        player.position = Some(Vec2 {
            x: session.position.x,
            y: session.position.y,
        });
    }
    Ok(player)
}

pub fn synchronize_player_observation(
    tracker: &MovementTracker,
    mut player: PlayerState,
) -> Result<SynchronizedPlayer, MovementError> {
    let players = tracker.players.lock().map_err(|_| MovementError::Tracker)?;
    let session = players
        .get(&player.id)
        .and_then(|movement| movement.session)
        .ok_or(MovementError::MissingSession)?;
    player.map_id = session.map_id;
    player.position = Some(Vec2 {
        x: session.position.x,
        y: session.position.y,
    });
    Ok(SynchronizedPlayer {
        player,
        platform_layer: session.platform_layer,
    })
}

#[cfg(test)]
pub fn submit_movement(
    tracker: &MovementTracker,
    player: &PlayerState,
    submitted: SubmittedMovement,
    config: MovementConfig,
    now_ms: u64,
) -> Result<MovementDecision, MovementError> {
    submit_movement_with_modifiers(
        tracker,
        player,
        submitted,
        MovementModifiers::default(),
        config,
        now_ms,
    )
}

pub fn submit_movement_with_modifiers(
    tracker: &MovementTracker,
    player: &PlayerState,
    submitted: SubmittedMovement,
    modifiers: MovementModifiers,
    config: MovementConfig,
    now_ms: u64,
) -> Result<MovementDecision, MovementError> {
    submit_movement_with_policy(tracker, player, submitted, modifiers, config, now_ms, true)
}

pub fn submit_combat_movement_with_modifiers(
    tracker: &MovementTracker,
    player: &PlayerState,
    submitted: SubmittedMovement,
    modifiers: MovementModifiers,
    config: MovementConfig,
    now_ms: u64,
) -> Result<MovementDecision, MovementError> {
    submit_movement_with_policy(tracker, player, submitted, modifiers, config, now_ms, false)
}

fn submit_movement_with_policy(
    tracker: &MovementTracker,
    player: &PlayerState,
    submitted: SubmittedMovement,
    modifiers: MovementModifiers,
    config: MovementConfig,
    now_ms: u64,
    accept_superseded: bool,
) -> Result<MovementDecision, MovementError> {
    let map = movement_map(tracker, player.map_id)?;
    let mut players = tracker.players.lock().map_err(|_| MovementError::Tracker)?;
    let movement = players.entry(player.id.clone()).or_default();
    ensure_session(movement, player, &map, config, now_ms)?;
    let session = movement
        .session
        .as_mut()
        .expect("movement session was initialized above");
    Ok(apply_snapshot(
        session,
        submitted,
        &map,
        config,
        modifiers,
        now_ms,
        accept_superseded,
    ))
}

pub fn submit_stationary_observation(
    tracker: &MovementTracker,
    player: &PlayerState,
    submitted: SubmittedMovement,
    modifiers: MovementModifiers,
    config: MovementConfig,
    now_ms: u64,
) -> Result<MovementDecision, MovementError> {
    let map = movement_map(tracker, player.map_id)?;
    let mut players = tracker.players.lock().map_err(|_| MovementError::Tracker)?;
    let movement = players.entry(player.id.clone()).or_default();
    ensure_session(movement, player, &map, config, now_ms)?;
    let session = movement
        .session
        .as_mut()
        .expect("movement session was initialized above");
    Ok(apply_stationary_observation(
        session, submitted, modifiers, now_ms,
    ))
}

pub fn enter_portal_with_modifiers(
    tracker: &MovementTracker,
    player: &PlayerState,
    portal: PortalMovement<'_>,
    modifiers: MovementModifiers,
    config: MovementConfig,
    now_ms: u64,
) -> Result<(MovementDecision, Option<RelocationPlan>), MovementError> {
    register_map(tracker, portal.source_map)?;
    register_map(tracker, portal.target_map)?;
    let mut players = tracker.players.lock().map_err(|_| MovementError::Tracker)?;
    let expected_source = players
        .get(&player.id)
        .and_then(|movement| movement.session);
    let mut session = source_session(expected_source, player, portal.source_map, config, now_ms)?;
    let source_decision = apply_snapshot(
        &mut session,
        portal.source,
        portal.source_map,
        config,
        modifiers,
        now_ms,
        true,
    );
    if !source_decision.accepted {
        commit_source_session(&mut players, &player.id, session);
        return Ok((source_decision, None));
    }
    let activity = source_decision.activity;
    let Some(source_portal) = portal.source_map.portals.iter().find(|source_portal| {
        source_portal.target_map_id == portal.target_map.id
            && source_portal.target_map_id != SCRIPT_PORTAL_TARGET
            && source_portal.target_name == portal.target_portal_name
            && (session.position.x - source_portal.x).abs() <= config.portal_horizontal_reach
            && (session.position.y - source_portal.y).abs() <= config.portal_vertical_reach
    }) else {
        let decision = reject(
            &session,
            "the authoritative position is not at that portal",
            activity,
        );
        commit_source_session(&mut players, &player.id, session);
        return Ok((decision, None));
    };
    let Some(position) =
        resolve_named_portal_position(portal.target_map, &source_portal.target_name)
    else {
        let decision = reject(
            &session,
            "the destination map has no usable portal",
            activity,
        );
        commit_source_session(&mut players, &player.id, session);
        return Ok((decision, None));
    };
    let mut target = session;
    let (mode, platform_layer) = initial_motion(portal.target_map, &position, config);
    target.map_id = portal.target_map.id;
    target.position = position;
    target.mode = mode;
    target.platform_layer = platform_layer;
    target.airborne = airborne_state(target.mode, position.y, now_ms);
    let plan = RelocationPlan {
        player_id: player.id.clone(),
        expected_source,
        target,
    };
    Ok((accept(&target, true), Some(plan)))
}

pub fn default_spawn_position(map: &Map) -> Result<Vec2, MovementError> {
    let position = resolve_default_spawn_position(map).ok_or(MovementError::MissingDefaultSpawn)?;
    Ok(Vec2 {
        x: position.x,
        y: position.y,
    })
}

pub fn named_portal_position(
    map: &Map,
    portal_name: &str,
) -> Result<Vec2, MovementError> {
    let position = resolve_named_portal_position(map, portal_name).ok_or_else(|| {
        MovementError::MissingDestinationPortal {
            portal_name: portal_name.to_owned(),
        }
    })?;
    Ok(Vec2 {
        x: position.x,
        y: position.y,
    })
}

pub fn relocate_player(
    tracker: &MovementTracker,
    player: &PlayerState,
    source_map: &Map,
    target_map: &Map,
    target_portal_name: &str,
    config: MovementConfig,
    now_ms: u64,
) -> Result<(MovementDecision, RelocationPlan), MovementError> {
    let destination = named_portal_position(target_map, target_portal_name)?;
    relocate_player_to_position(
        tracker,
        player,
        source_map,
        target_map,
        destination,
        config,
        now_ms,
    )
}

pub fn relocate_player_to_position(
    tracker: &MovementTracker,
    player: &PlayerState,
    source_map: &Map,
    target_map: &Map,
    destination: Vec2,
    config: MovementConfig,
    now_ms: u64,
) -> Result<(MovementDecision, RelocationPlan), MovementError> {
    register_map(tracker, source_map)?;
    register_map(tracker, target_map)?;
    let position = clamp_to_movement_bounds(
        target_map,
        Position {
            x: destination.x,
            y: destination.y,
        },
    );
    let players = tracker.players.lock().map_err(|_| MovementError::Tracker)?;
    let expected_source = players
        .get(&player.id)
        .and_then(|movement| movement.session);
    let mut target = source_session(expected_source, player, source_map, config, now_ms)?;
    let (mode, platform_layer) = initial_motion(target_map, &position, config);
    target.map_id = target_map.id;
    target.position = position;
    target.mode = mode;
    target.platform_layer = platform_layer;
    target.received_at_ms = now_ms;
    target.airborne = airborne_state(mode, position.y, now_ms);
    target.horizontal_credit = 0.0;
    target.vertical_credit = 0.0;
    target.climb_credit = 0.0;
    let plan = RelocationPlan {
        player_id: player.id.clone(),
        expected_source,
        target,
    };
    Ok((accept(&target, true), plan))
}

pub fn project_relocation_player(
    plan: &RelocationPlan,
    mut player: PlayerState,
) -> Result<PlayerState, MovementError> {
    if player.id != plan.player_id {
        return Err(MovementError::RelocationPlayerMismatch {
            expected: plan.player_id.clone(),
            actual: player.id,
        });
    }
    player.map_id = plan.target.map_id;
    player.position = Some(Vec2 {
        x: plan.target.position.x,
        y: plan.target.position.y,
    });
    Ok(player)
}

pub fn project_relocation_observation(
    plan: &RelocationPlan,
    player: PlayerState,
) -> Result<SynchronizedPlayer, MovementError> {
    Ok(SynchronizedPlayer {
        player: project_relocation_player(plan, player)?,
        platform_layer: plan.target.platform_layer,
    })
}

pub fn relocation_player_id(plan: &RelocationPlan) -> &str {
    &plan.player_id
}

pub fn relocation_target_map_id(plan: &RelocationPlan) -> u32 {
    plan.target.map_id
}

pub fn relocation_target_position(plan: &RelocationPlan) -> Vec2 {
    Vec2 {
        x: plan.target.position.x,
        y: plan.target.position.y,
    }
}

pub fn commit_relocation(
    tracker: &MovementTracker,
    plan: &RelocationPlan,
) -> Result<CommittedRelocation, MovementError> {
    let mut players = tracker.players.lock().map_err(|_| MovementError::Tracker)?;
    let current = players
        .get(&plan.player_id)
        .and_then(|movement| movement.session);
    if current != plan.expected_source {
        return Err(MovementError::RelocationSourceChanged);
    }
    players.entry(plan.player_id.clone()).or_default().session = Some(plan.target);
    Ok(CommittedRelocation { plan: plan.clone() })
}

pub fn restore_relocation(
    tracker: &MovementTracker,
    committed: CommittedRelocation,
) -> Result<(), MovementError> {
    let mut players = tracker.players.lock().map_err(|_| MovementError::Tracker)?;
    let current = players
        .get(&committed.plan.player_id)
        .and_then(|movement| movement.session);
    if current != Some(committed.plan.target) {
        return Err(MovementError::RelocationTargetChanged);
    }
    match committed.plan.expected_source {
        Some(source) => {
            players.entry(committed.plan.player_id).or_default().session = Some(source);
        }
        None => {
            players.remove(&committed.plan.player_id);
        }
    }
    Ok(())
}

pub fn register_map(
    tracker: &MovementTracker,
    map: &Map,
) -> Result<(), MovementError> {
    tracker
        .maps
        .lock()
        .map_err(|_| MovementError::Tracker)?
        .entry(map.id)
        .or_insert_with(|| Arc::new(map.clone()));
    Ok(())
}

fn movement_map(
    tracker: &MovementTracker,
    map_id: u32,
) -> Result<Arc<Map>, MovementError> {
    tracker
        .maps
        .lock()
        .map_err(|_| MovementError::Tracker)?
        .get(&map_id)
        .cloned()
        .ok_or(MovementError::MissingMap { map_id })
}

fn source_session(
    current: Option<MovementSession>,
    player: &PlayerState,
    map: &Map,
    config: MovementConfig,
    now_ms: u64,
) -> Result<MovementSession, MovementError> {
    if let Some(session) = current {
        return Ok(session);
    }
    let position = player
        .position
        .as_ref()
        .ok_or(MovementError::MissingPlayerPosition)?;
    Ok(initial_session(
        player.map_id,
        Position {
            x: position.x,
            y: position.y,
        },
        map,
        config,
        now_ms,
    ))
}

fn commit_source_session(
    players: &mut HashMap<String, PlayerMovement>,
    player_id: &str,
    session: MovementSession,
) {
    players.entry(player_id.to_owned()).or_default().session = Some(session);
}

fn ensure_session(
    movement: &mut PlayerMovement,
    player: &PlayerState,
    map: &Map,
    config: MovementConfig,
    now_ms: u64,
) -> Result<(), MovementError> {
    require_player_map(player, map)?;
    if movement.session.is_none() {
        let position = player
            .position
            .as_ref()
            .ok_or(MovementError::MissingPlayerPosition)?;
        movement.session = Some(initial_session(
            player.map_id,
            Position {
                x: position.x,
                y: position.y,
            },
            map,
            config,
            now_ms,
        ));
    }
    Ok(())
}

fn require_player_map(
    player: &PlayerState,
    map: &Map,
) -> Result<(), MovementError> {
    if player.map_id == map.id {
        Ok(())
    } else {
        Err(MovementError::PlayerMapMismatch {
            player_map_id: player.map_id,
            map_id: map.id,
        })
    }
}

fn initial_session(
    map_id: u32,
    position: Position,
    map: &Map,
    config: MovementConfig,
    now_ms: u64,
) -> MovementSession {
    let position = clamp_to_movement_bounds(map, position);
    let (mode, platform_layer) = initial_motion(map, &position, config);
    MovementSession {
        sequence: 0,
        map_id,
        position,
        mode,
        platform_layer,
        received_at_ms: now_ms,
        airborne: airborne_state(mode, position.y, now_ms),
        horizontal_credit: config.position_tolerance,
        vertical_credit: config.position_tolerance,
        climb_credit: config.position_tolerance,
        modifiers: MovementModifiers::default(),
    }
}

fn initial_motion(
    map: &Map,
    position: &Position,
    config: MovementConfig,
) -> (MovementMode, i32) {
    supporting_foothold(
        map,
        position,
        config.ground_tolerance,
        config.platform_edge_tolerance,
    )
    .map_or((MovementMode::Airborne, 0), |platform| {
        (MovementMode::Grounded, platform.layer)
    })
}

fn apply_snapshot(
    session: &mut MovementSession,
    submitted: SubmittedMovement,
    map: &Map,
    config: MovementConfig,
    modifiers: MovementModifiers,
    now_ms: u64,
    accept_superseded: bool,
) -> MovementDecision {
    if submitted.sequence <= session.sequence {
        return if accept_superseded {
            accept(session, false)
        } else {
            reject(session, "the movement sequence is not newer", false)
        };
    }
    session.sequence = submitted.sequence;
    let interval_modifiers = endpoint_modifiers(session.modifiers, modifiers);
    session.modifiers = modifiers;
    let elapsed_ms = now_ms
        .saturating_sub(session.received_at_ms)
        .min(duration_millis(config.maximum_snapshot_gap));
    session.received_at_ms = now_ms;
    if submitted.map_id != session.map_id || map.id != session.map_id {
        return reject(session, "map changes require an accepted portal", false);
    }
    if !within_map(map, &submitted.position) {
        return reject(session, "the position is outside the map", false);
    }
    if submitted
        .support_contact
        .is_some_and(|contact| !support_contact_is_valid(map, contact, config))
    {
        return reject(session, "the reported support contact is invalid", false);
    }
    if submitted.drop_through && !drop_through_is_valid(map, session, submitted, config) {
        return reject(session, "the drop-through transition is invalid", false);
    }
    let contact_layer = submitted
        .support_contact
        .and_then(|contact| movement_mode_layer(map, contact.position, contact.mode, config))
        .unwrap_or(session.platform_layer);
    if movement_crosses_vertical_foothold(
        map,
        session.position,
        session.platform_layer,
        submitted,
        contact_layer,
    ) {
        return reject(session, "the movement crosses a vertical foothold", false);
    }

    let elapsed_seconds = elapsed_ms as f32 / 1_000.0;
    let walk_speed = modified_speed(
        config.walk_speed,
        interval_modifiers.speed,
        config.speed_cap,
    );
    let jump_speed = modified_speed(config.jump_speed, interval_modifiers.jump, config.jump_cap);
    replenish_movement_credit(
        session,
        config,
        walk_speed,
        jump_speed,
        elapsed_seconds,
        now_ms,
    );
    if !movement_credit_is_sufficient(session, submitted) {
        return reject(
            session,
            "the movement exceeds its accumulated allowance",
            false,
        );
    }
    let distance = movement_distance(session, submitted);
    if distance.horizontal > walk_speed * elapsed_seconds + config.position_tolerance {
        return reject(session, "the horizontal movement is too fast", false);
    }
    if !vertical_displacement_is_valid(
        session,
        submitted,
        map,
        config,
        jump_speed,
        elapsed_seconds,
        now_ms,
    ) {
        return reject(session, "the vertical movement is not reachable", false);
    }
    if submitted.mode == MovementMode::Grounded
        && !supporting_platform(
            map,
            &submitted.position,
            config.ground_tolerance,
            config.platform_edge_tolerance,
        )
    {
        return reject(
            session,
            "the grounded position has no supporting foothold",
            false,
        );
    }
    if submitted.mode == MovementMode::Climbing
        && !climbing_position_is_valid(map, session, submitted, config, elapsed_seconds)
    {
        return reject(session, "the climbing position is not reachable", false);
    }

    let platform_layer = submitted_platform_layer(map, session, submitted, config);
    let activity = submitted.support_contact.is_some()
        || session.position != submitted.position
        || session.mode != submitted.mode;
    consume_movement_credit(session, submitted);
    let airborne_origin = submitted
        .support_contact
        .map_or(session.position.y, |contact| contact.position.y);
    if submitted.support_contact.is_some() {
        session.airborne = None;
    }
    update_airborne_state(session, submitted.mode, airborne_origin, now_ms);
    session.position = submitted.position;
    session.mode = submitted.mode;
    session.platform_layer = platform_layer;
    accept(session, activity)
}

fn apply_stationary_observation(
    session: &mut MovementSession,
    submitted: SubmittedMovement,
    modifiers: MovementModifiers,
    now_ms: u64,
) -> MovementDecision {
    if submitted.sequence <= session.sequence {
        return accept(session, false);
    }
    session.sequence = submitted.sequence;
    session.received_at_ms = now_ms;
    session.modifiers = modifiers;
    if submitted.map_id != session.map_id {
        return reject(session, "map changes require an accepted portal", false);
    }
    if submitted.position != session.position || submitted.mode != session.mode {
        return reject(
            session,
            "a dead player cannot change position or movement mode",
            false,
        );
    }
    session.airborne = airborne_state(session.mode, session.position.y, now_ms);
    accept(session, false)
}

fn replenish_movement_credit(
    session: &mut MovementSession,
    config: MovementConfig,
    walk_speed: f32,
    jump_speed: f32,
    elapsed_seconds: f32,
    now_ms: u64,
) {
    let maximum_gap_seconds = config.maximum_snapshot_gap.as_secs_f32();
    let airborne_seconds = session.airborne.map_or(0.0, |airborne| {
        now_ms.saturating_sub(airborne.started_at_ms) as f32 / 1_000.0
    });
    let vertical_speed = jump_speed + config.gravity * airborne_seconds;
    session.horizontal_credit = replenish_credit(
        session.horizontal_credit,
        walk_speed,
        elapsed_seconds,
        maximum_gap_seconds,
        config.position_tolerance,
    );
    session.vertical_credit = replenish_credit(
        session.vertical_credit,
        vertical_speed,
        elapsed_seconds,
        maximum_gap_seconds,
        config.position_tolerance,
    );
    session.climb_credit = replenish_credit(
        session.climb_credit,
        config.climb_speed,
        elapsed_seconds,
        maximum_gap_seconds,
        config.position_tolerance,
    );
}

fn replenish_credit(
    credit: f32,
    rate: f32,
    elapsed_seconds: f32,
    maximum_gap_seconds: f32,
    tolerance: f32,
) -> f32 {
    (credit + rate * elapsed_seconds).min(rate * maximum_gap_seconds + tolerance)
}

fn movement_credit_is_sufficient(
    session: &MovementSession,
    submitted: SubmittedMovement,
) -> bool {
    let distance = movement_distance(session, submitted);
    let vertical_credit = if uses_climb_credit(session, submitted) {
        session.climb_credit
    } else {
        session.vertical_credit
    };
    distance.horizontal <= session.horizontal_credit && distance.vertical <= vertical_credit
}

fn consume_movement_credit(
    session: &mut MovementSession,
    submitted: SubmittedMovement,
) {
    let distance = movement_distance(session, submitted);
    session.horizontal_credit = (session.horizontal_credit - distance.horizontal).max(0.0);
    if uses_climb_credit(session, submitted) {
        session.climb_credit = (session.climb_credit - distance.vertical).max(0.0);
    } else {
        session.vertical_credit = (session.vertical_credit - distance.vertical).max(0.0);
    }
}

#[derive(Clone, Copy, Debug)]
struct MovementDistance {
    horizontal: f32,
    vertical: f32,
}

fn movement_distance(
    session: &MovementSession,
    submitted: SubmittedMovement,
) -> MovementDistance {
    let Some(contact) = submitted.support_contact else {
        return MovementDistance {
            horizontal: (submitted.position.x - session.position.x).abs(),
            vertical: (submitted.position.y - session.position.y).abs(),
        };
    };
    MovementDistance {
        horizontal: (contact.position.x - session.position.x).abs()
            + (submitted.position.x - contact.position.x).abs(),
        vertical: (contact.position.y - session.position.y).abs()
            + (submitted.position.y - contact.position.y).abs(),
    }
}

fn uses_climb_credit(
    session: &MovementSession,
    submitted: SubmittedMovement,
) -> bool {
    session.mode == MovementMode::Climbing
        && submitted.mode == MovementMode::Climbing
        && submitted.support_contact.is_none()
}

fn vertical_displacement_is_valid(
    session: &MovementSession,
    submitted: SubmittedMovement,
    map: &Map,
    config: MovementConfig,
    jump_speed: f32,
    elapsed_seconds: f32,
    now_ms: u64,
) -> bool {
    let airborne_seconds = session.airborne.map_or(0.0, |airborne| {
        now_ms.saturating_sub(airborne.started_at_ms) as f32 / 1_000.0
    });
    let maximum_vertical_speed = jump_speed + config.gravity * airborne_seconds;
    let maximum_y = maximum_vertical_speed * elapsed_seconds + config.position_tolerance;
    if movement_distance(session, submitted).vertical > maximum_y {
        return false;
    }
    if submitted.mode != MovementMode::Airborne {
        return true;
    }
    let airborne = submitted.support_contact.map_or_else(
        || {
            session.airborne.unwrap_or(AirborneState {
                origin_y: session.position.y,
                started_at_ms: session.received_at_ms,
            })
        },
        |contact| AirborneState {
            origin_y: contact.position.y,
            started_at_ms: now_ms,
        },
    );
    let maximum_ascent = jump_speed.powi(2) / (2.0 * config.gravity);
    if submitted.position.y < airborne.origin_y - maximum_ascent - config.position_tolerance {
        return false;
    }
    let fall_distance = (map.height as f32 - airborne.origin_y).max(0.0);
    let maximum_airborne_seconds = (jump_speed
        + (jump_speed.powi(2) + 2.0 * config.gravity * fall_distance).sqrt())
        / config.gravity
        + config.maximum_snapshot_gap.as_secs_f32();
    now_ms.saturating_sub(airborne.started_at_ms) as f32 <= maximum_airborne_seconds * 1_000.0
}

fn climbing_position_is_valid(
    map: &Map,
    session: &MovementSession,
    submitted: SubmittedMovement,
    config: MovementConfig,
    elapsed_seconds: f32,
) -> bool {
    if !climbing_contact_is_valid(map, submitted.position, config) {
        return false;
    }
    if session.mode != MovementMode::Climbing || submitted.support_contact.is_some() {
        return true;
    }
    let maximum_y = config.climb_speed * elapsed_seconds + config.position_tolerance;
    (submitted.position.y - session.position.y).abs() <= maximum_y
}

fn support_contact_is_valid(
    map: &Map,
    contact: SupportContact,
    config: MovementConfig,
) -> bool {
    within_map(map, &contact.position)
        && movement_mode_layer(map, contact.position, contact.mode, config).is_some()
}

fn movement_mode_layer(
    map: &Map,
    position: Position,
    mode: MovementMode,
    config: MovementConfig,
) -> Option<i32> {
    match mode {
        MovementMode::Grounded => supporting_foothold(
            map,
            &position,
            config.ground_tolerance,
            config.platform_edge_tolerance,
        )
        .map(|platform| platform.layer),
        MovementMode::Climbing => climbing_layer(map, position, config),
        MovementMode::Airborne => None,
    }
}

fn submitted_platform_layer(
    map: &Map,
    session: &MovementSession,
    submitted: SubmittedMovement,
    config: MovementConfig,
) -> i32 {
    movement_mode_layer(map, submitted.position, submitted.mode, config)
        .or_else(|| {
            submitted.support_contact.and_then(|contact| {
                movement_mode_layer(map, contact.position, contact.mode, config)
            })
        })
        .unwrap_or(session.platform_layer)
}

fn drop_through_is_valid(
    map: &Map,
    session: &MovementSession,
    submitted: SubmittedMovement,
    config: MovementConfig,
) -> bool {
    let Some(origin) = submitted.support_contact else {
        return false;
    };
    if session.mode != MovementMode::Grounded
        || origin.mode != MovementMode::Grounded
        || submitted.mode == MovementMode::Climbing
        || submitted.position.y <= origin.position.y + DROP_THROUGH_MINIMUM
    {
        return false;
    }
    supporting_foothold(
        map,
        &origin.position,
        config.ground_tolerance,
        config.platform_edge_tolerance,
    )
    .is_some()
        && has_foothold_below(
            map,
            origin.position,
            config
                .ground_tolerance
                .max(DROP_THROUGH_DESTINATION_CLEARANCE),
        )
}

fn has_foothold_below(
    map: &Map,
    position: Position,
    clearance: f32,
) -> bool {
    map.platforms.iter().any(|platform| {
        platform_y(platform, position.x).is_some_and(|surface| surface > position.y + clearance)
    })
}

fn climbing_contact_is_valid(
    map: &Map,
    position: Position,
    config: MovementConfig,
) -> bool {
    climbing_layer(map, position, config).is_some()
}

fn climbing_layer(
    map: &Map,
    position: Position,
    config: MovementConfig,
) -> Option<i32> {
    map.ladders
        .iter()
        .filter(|ladder| {
            (position.x - ladder.x).abs() <= config.ladder_reach
                && position.y >= ladder.top - config.ladder_end_reach
                && position.y <= ladder.bottom + config.ladder_end_reach
        })
        .min_by(|left, right| {
            (position.x - left.x)
                .abs()
                .total_cmp(&(position.x - right.x).abs())
        })
        .map(|ladder| ladder.layer)
}

fn update_airborne_state(
    session: &mut MovementSession,
    mode: MovementMode,
    origin_y: f32,
    now_ms: u64,
) {
    match (session.airborne, mode) {
        (None, MovementMode::Airborne) => {
            session.airborne = Some(AirborneState {
                origin_y,
                started_at_ms: now_ms,
            });
        }
        (Some(_), MovementMode::Grounded | MovementMode::Climbing) => session.airborne = None,
        _ => {}
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MovementModifiers {
    pub speed: i32,
    pub jump: i32,
}

fn endpoint_modifiers(
    previous: MovementModifiers,
    current: MovementModifiers,
) -> MovementModifiers {
    MovementModifiers {
        speed: previous.speed.max(current.speed),
        jump: previous.jump.max(current.jump),
    }
}

fn modified_speed(
    base: f32,
    bonus: i32,
    cap: u32,
) -> f32 {
    let percentage = (100_i64 + i64::from(bonus)).clamp(0, i64::from(cap));
    base * percentage as f32 / 100.0
}

fn airborne_state(
    mode: MovementMode,
    origin_y: f32,
    now_ms: u64,
) -> Option<AirborneState> {
    (mode == MovementMode::Airborne).then_some(AirborneState {
        origin_y,
        started_at_ms: now_ms,
    })
}

fn accept(
    session: &MovementSession,
    activity: bool,
) -> MovementDecision {
    MovementDecision {
        authoritative: snapshot(session),
        accepted: true,
        rejection_reason: String::new(),
        activity,
    }
}

fn reject(
    session: &MovementSession,
    reason: &str,
    activity: bool,
) -> MovementDecision {
    MovementDecision {
        authoritative: snapshot(session),
        accepted: false,
        rejection_reason: reason.to_owned(),
        activity,
    }
}

fn snapshot(session: &MovementSession) -> MovementSnapshot {
    MovementSnapshot {
        sequence: session.sequence,
        map_id: session.map_id,
        position: Some(Vec2 {
            x: session.position.x,
            y: session.position.y,
        }),
        mode: proto_mode(session.mode) as i32,
        support_contact: None,
        drop_through: false,
    }
}

fn proto_mode(mode: MovementMode) -> ProtoMovementMode {
    match mode {
        MovementMode::Grounded => ProtoMovementMode::Grounded,
        MovementMode::Airborne => ProtoMovementMode::Airborne,
        MovementMode::Climbing => ProtoMovementMode::Climbing,
    }
}

fn duration_millis(duration: std::time::Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use oozems_proto::v1::Ladder;
    use oozems_proto::v1::Map;
    use oozems_proto::v1::MapMovementBounds;
    use oozems_proto::v1::Platform;
    use oozems_proto::v1::PlayerState;
    use oozems_proto::v1::Portal;
    use oozems_proto::v1::Vec2;

    use super::MovementMode;
    use super::MovementModifiers;
    use super::MovementTracker;
    use super::PortalMovement;
    use super::Position;
    use super::SubmittedMovement;
    use super::SupportContact;
    use super::commit_relocation;
    use super::default_spawn_position;
    use super::enter_portal_with_modifiers;
    use super::initialize_player;
    use super::named_portal_position;
    use super::project_relocation_player;
    use super::register_map;
    use super::relocate_player;
    use super::restore_relocation;
    use super::submit_combat_movement_with_modifiers;
    use super::submit_movement;
    use super::submit_movement_with_modifiers;
    use super::submit_stationary_observation;
    use super::synchronize_player;
    use super::synchronize_player_observation;
    use crate::gameplay::MovementConfig;

    #[test]
    fn rejects_teleports_without_advancing_the_authoritative_position() {
        let tracker = initialized_tracker();
        let decision = submit_movement(
            &tracker,
            &player(),
            submitted(1, 400.0, 300.0, MovementMode::Grounded),
            config(),
            1_200,
        )
        .expect("movement decision");

        assert!(!decision.accepted);
        assert_eq!(decision.authoritative.position.expect("position").x, 100.0);
    }

    #[test]
    fn accepted_displacement_reports_recovery_activity() {
        let tracker = initialized_tracker();
        let decision = submit_movement(
            &tracker,
            &player(),
            submitted(1, 140.0, 300.0, MovementMode::Grounded),
            config(),
            1_200,
        )
        .expect("movement decision");

        assert!(decision.accepted);
        assert!(decision.activity);
    }

    #[test]
    fn superseded_snapshots_are_successful_no_ops() {
        let tracker = initialized_tracker();
        let newer = submit_movement(
            &tracker,
            &player(),
            submitted(2, 140.0, 300.0, MovementMode::Grounded),
            config(),
            1_200,
        )
        .expect("newer movement");
        let superseded = submit_movement(
            &tracker,
            &player(),
            submitted(1, 120.0, 300.0, MovementMode::Grounded),
            config(),
            1_300,
        )
        .expect("superseded movement");

        assert!(newer.accepted);
        assert!(superseded.accepted);
        assert!(!superseded.activity);
        assert_eq!(superseded.authoritative.sequence, 2);
        assert_eq!(
            superseded.authoritative.position.expect("position").x,
            140.0,
        );
    }

    #[test]
    fn combat_movement_rejects_an_exact_sequence_replay() {
        let tracker = initialized_tracker();
        let movement = submitted(1, 120.0, 300.0, MovementMode::Grounded);
        let first = submit_combat_movement_with_modifiers(
            &tracker,
            &player(),
            movement,
            MovementModifiers::default(),
            config(),
            1_200,
        )
        .expect("first combat movement");
        let replay = submit_combat_movement_with_modifiers(
            &tracker,
            &player(),
            movement,
            MovementModifiers::default(),
            config(),
            1_300,
        )
        .expect("replayed combat movement");

        assert!(first.accepted);
        assert!(!replay.accepted);
        assert_eq!(replay.authoritative.sequence, 1);
    }

    #[test]
    fn airborne_observations_preserve_the_launch_platform_layer() {
        let mut map = map();
        map.platforms[0].layer = 3;
        map.platforms.push(Platform {
            x: 0.0,
            y: 200.0,
            end_x: 800.0,
            end_y: 200.0,
            layer: 7,
            ..Platform::default()
        });
        let tracker = MovementTracker::default();
        initialize_player(&tracker, &player(), &map, config(), 1_000).expect("initialize");

        let decision = submit_movement(
            &tracker,
            &player(),
            submitted(1, 100.0, 230.0, MovementMode::Airborne),
            config(),
            1_200,
        )
        .expect("airborne movement");
        let observation =
            synchronize_player_observation(&tracker, player()).expect("authoritative observation");

        assert!(decision.accepted);
        assert_eq!(observation.player.position.expect("position").y, 230.0);
        assert_eq!(observation.platform_layer, 3);
    }

    #[test]
    fn stationary_observation_advances_sequence_without_reporting_activity() {
        let tracker = initialized_tracker();
        let observation = submit_stationary_observation(
            &tracker,
            &player(),
            submitted(1, 100.0, 300.0, MovementMode::Grounded),
            MovementModifiers::default(),
            config(),
            1_200,
        )
        .expect("stationary observation");

        assert!(observation.accepted);
        assert!(!observation.activity);
        assert_eq!(observation.authoritative.sequence, 1);
    }

    #[test]
    fn stationary_observation_rejects_position_and_mode_changes() {
        let tracker = initialized_tracker();
        let moved = submit_stationary_observation(
            &tracker,
            &player(),
            submitted(1, 120.0, 300.0, MovementMode::Grounded),
            MovementModifiers::default(),
            config(),
            1_200,
        )
        .expect("displaced observation");
        let changed_mode = submit_stationary_observation(
            &tracker,
            &player(),
            submitted(2, 100.0, 300.0, MovementMode::Airborne),
            MovementModifiers::default(),
            config(),
            1_300,
        )
        .expect("mode-changing observation");

        assert!(!moved.accepted);
        assert!(!changed_mode.accepted);
        assert_eq!(moved.authoritative.position.expect("position").x, 100.0);
        assert_eq!(
            synchronize_player(&tracker, player())
                .expect("authoritative player")
                .position
                .expect("position")
                .x,
            100.0
        );
    }

    #[test]
    fn stationary_observation_refreshes_modifiers_before_revival() {
        let tracker = initialized_tracker();
        let haste = MovementModifiers {
            speed: 100,
            jump: 0,
        };
        submit_movement_with_modifiers(
            &tracker,
            &player(),
            submitted(1, 100.0, 300.0, MovementMode::Grounded),
            haste,
            config(),
            1_100,
        )
        .expect("active Haste endpoint");
        submit_stationary_observation(
            &tracker,
            &player(),
            submitted(2, 100.0, 300.0, MovementMode::Grounded),
            MovementModifiers::default(),
            config(),
            1_200,
        )
        .expect("observation after Haste expires");

        let movement = submit_movement_with_modifiers(
            &tracker,
            &player(),
            submitted(3, 180.0, 300.0, MovementMode::Grounded),
            MovementModifiers::default(),
            config(),
            1_400,
        )
        .expect("movement after revival");

        assert!(!movement.accepted);
    }

    #[test]
    fn haste_bonus_is_capped_by_the_configured_speed_stat() {
        let tracker = initialized_tracker();
        let modifiers = MovementModifiers {
            speed: 150,
            jump: 0,
        };

        let accepted = submit_movement_with_modifiers(
            &tracker,
            &player(),
            submitted(1, 190.0, 300.0, MovementMode::Grounded),
            modifiers,
            config(),
            1_200,
        )
        .expect("capped movement");
        let rejected = submit_movement_with_modifiers(
            &tracker,
            &player(),
            submitted(2, 305.0, 300.0, MovementMode::Grounded),
            modifiers,
            config(),
            1_400,
        )
        .expect("excess movement");

        assert!(accepted.accepted);
        assert!(!rejected.accepted);
    }

    #[test]
    fn jump_bonus_is_capped_by_the_configured_jump_stat() {
        let accepted_tracker = initialized_tracker();
        let modifiers = MovementModifiers {
            speed: 0,
            jump: 150,
        };
        let accepted = submit_movement_with_modifiers(
            &accepted_tracker,
            &player(),
            submitted(1, 100.0, 110.0, MovementMode::Airborne),
            modifiers,
            config(),
            1_200,
        )
        .expect("capped jump");

        let rejected_tracker = initialized_tracker();
        let rejected = submit_movement_with_modifiers(
            &rejected_tracker,
            &player(),
            submitted(1, 100.0, 80.0, MovementMode::Airborne),
            modifiers,
            config(),
            1_200,
        )
        .expect("excess jump");

        assert!(accepted.accepted);
        assert!(!rejected.accepted);
    }

    #[test]
    fn an_endpoint_haste_modifier_covers_the_interval_where_it_expires() {
        let tracker = initialized_tracker();
        let haste = MovementModifiers {
            speed: 100,
            jump: 0,
        };
        let active_endpoint = submit_movement_with_modifiers(
            &tracker,
            &player(),
            submitted(1, 100.0, 300.0, MovementMode::Grounded),
            haste,
            config(),
            1_000,
        )
        .expect("active Haste endpoint");
        let expiration_endpoint = submit_movement_with_modifiers(
            &tracker,
            &player(),
            submitted(2, 190.0, 300.0, MovementMode::Grounded),
            MovementModifiers::default(),
            config(),
            1_200,
        )
        .expect("expired Haste endpoint");
        let unbuffed_interval = submit_movement_with_modifiers(
            &tracker,
            &player(),
            submitted(3, 280.0, 300.0, MovementMode::Grounded),
            MovementModifiers::default(),
            config(),
            1_400,
        )
        .expect("unbuffed interval");

        assert!(active_endpoint.accepted);
        assert!(expiration_endpoint.accepted);
        assert!(!unbuffed_interval.accepted);
    }

    #[test]
    fn silence_does_not_accumulate_an_unbounded_movement_budget() {
        let tracker = initialized_tracker();

        let decision = submit_movement(
            &tracker,
            &player(),
            submitted(1, 400.0, 300.0, MovementMode::Grounded),
            config(),
            20_000,
        )
        .expect("movement after silence");

        assert!(!decision.accepted);
    }

    #[test]
    fn rapid_snapshots_cannot_reuse_position_tolerance() {
        let tracker = initialized_tracker();
        let first = submit_movement(
            &tracker,
            &player(),
            submitted(1, 115.0, 300.0, MovementMode::Grounded),
            config(),
            1_001,
        )
        .expect("first movement");
        let second = submit_movement(
            &tracker,
            &player(),
            submitted(2, 130.0, 300.0, MovementMode::Grounded),
            config(),
            1_002,
        )
        .expect("second movement");

        assert!(first.accepted);
        assert!(!second.accepted);
        assert_eq!(second.authoritative.position.expect("position").x, 115.0);
    }

    #[test]
    fn registering_a_missing_map_accepts_later_projected_modifiers() {
        let tracker = MovementTracker::default();
        let missing = submit_movement(
            &tracker,
            &player(),
            submitted(1, 100.0, 300.0, MovementMode::Grounded),
            config(),
            1_200,
        );
        assert!(matches!(
            missing,
            Err(super::MovementError::MissingMap { .. })
        ));

        register_map(&tracker, &map()).expect("register map");
        let modifiers = MovementModifiers {
            speed: 150,
            jump: 0,
        };
        let heartbeat = submit_movement_with_modifiers(
            &tracker,
            &player(),
            submitted(1, 100.0, 300.0, MovementMode::Grounded),
            modifiers,
            config(),
            1_200,
        )
        .expect("initial heartbeat");
        let haste_movement = submit_movement_with_modifiers(
            &tracker,
            &player(),
            submitted(2, 190.0, 300.0, MovementMode::Grounded),
            modifiers,
            config(),
            1_400,
        )
        .expect("movement with pending effect");

        assert!(heartbeat.accepted);
        assert!(haste_movement.accepted);
    }

    #[test]
    fn grounded_snapshots_require_a_real_foothold() {
        let tracker = initialized_tracker();

        let decision = submit_movement(
            &tracker,
            &player(),
            submitted(1, 100.0, 200.0, MovementMode::Grounded),
            config(),
            1_200,
        )
        .expect("unsupported grounding");

        assert!(!decision.accepted);
    }

    #[test]
    fn grounded_snapshots_keep_contact_just_beyond_a_platform_edge() {
        let tracker = MovementTracker::default();
        let mut player = player();
        player.position = Some(Vec2 { x: 195.0, y: 300.0 });
        let mut edge_map = map();
        edge_map.platforms[0].end_x = 200.0;
        initialize_player(&tracker, &player, &edge_map, config(), 1_000).expect("initialize");

        let decision = submit_movement(
            &tracker,
            &player,
            submitted(1, 218.0, 300.0, MovementMode::Grounded),
            config(),
            1_200,
        )
        .expect("edge movement");

        assert!(decision.accepted);
    }

    #[test]
    fn grounded_players_can_drop_through_their_supporting_foothold() {
        let tracker = MovementTracker::default();
        let mut stacked_map = map();
        stacked_map.platforms.push(Platform {
            y: 400.0,
            end_y: 400.0,
            ..platform()
        });
        initialize_player(&tracker, &player(), &stacked_map, config(), 1_000).expect("initialize");
        let mut movement = submitted(1, 100.0, 302.0, MovementMode::Airborne);
        movement.support_contact = Some(SupportContact {
            position: Position { x: 100.0, y: 300.0 },
            mode: MovementMode::Grounded,
        });
        movement.drop_through = true;

        let decision = submit_movement(&tracker, &player(), movement, config(), 1_200)
            .expect("drop-through movement");

        assert!(decision.accepted);
        assert_eq!(
            decision.authoritative.position,
            Some(Vec2 { x: 100.0, y: 302.0 })
        );
    }

    #[test]
    fn moving_players_can_drop_through_across_adjacent_foothold_segments() {
        let tracker = MovementTracker::default();
        let mut player = player();
        player.position = Some(Vec2 { x: 130.0, y: 300.0 });
        let mut stacked_map = map();
        stacked_map.platforms[0].end_x = 150.0;
        stacked_map.platforms.push(Platform {
            x: 150.0,
            ..platform()
        });
        stacked_map.platforms.push(Platform {
            y: 400.0,
            end_y: 400.0,
            ..platform()
        });
        initialize_player(&tracker, &player, &stacked_map, config(), 1_000).expect("initialize");
        let mut movement = submitted(1, 182.0, 302.0, MovementMode::Airborne);
        movement.support_contact = Some(SupportContact {
            position: Position { x: 180.0, y: 300.0 },
            mode: MovementMode::Grounded,
        });
        movement.drop_through = true;

        let decision = submit_movement(&tracker, &player, movement, config(), 1_200)
            .expect("moving drop-through movement");

        assert!(decision.accepted);
    }

    #[test]
    fn bottom_footholds_cannot_be_dropped_through() {
        let tracker = MovementTracker::default();
        let mut offset_map = map();
        offset_map.platforms.push(Platform {
            x: 300.0,
            y: 400.0,
            end_x: 800.0,
            end_y: 400.0,
            ..Platform::default()
        });
        initialize_player(&tracker, &player(), &offset_map, config(), 1_000).expect("initialize");
        let mut movement = submitted(1, 100.0, 302.0, MovementMode::Airborne);
        movement.support_contact = Some(SupportContact {
            position: Position { x: 100.0, y: 300.0 },
            mode: MovementMode::Grounded,
        });
        movement.drop_through = true;

        let decision = submit_movement(&tracker, &player(), movement, config(), 1_200)
            .expect("bottom drop-through movement");

        assert!(!decision.accepted);
        assert_eq!(
            decision.rejection_reason,
            "the drop-through transition is invalid"
        );
    }

    #[test]
    fn drop_through_requires_a_grounded_origin_contact() {
        let tracker = initialized_tracker();
        let mut movement = submitted(1, 100.0, 302.0, MovementMode::Airborne);
        movement.drop_through = true;

        let decision = submit_movement(&tracker, &player(), movement, config(), 1_200)
            .expect("invalid drop-through movement");

        assert!(!decision.accepted);
        assert_eq!(
            decision.rejection_reason,
            "the drop-through transition is invalid"
        );
    }

    #[test]
    fn snapshots_cannot_cross_wz_map_walls() {
        let tracker = MovementTracker::default();
        let mut bounded_map = map();
        bounded_map.movement_bounds = Some(MapMovementBounds {
            left: 68.0,
            right: 732.0,
        });
        initialize_player(&tracker, &player(), &bounded_map, config(), 1_000).expect("initialize");

        let decision = submit_movement(
            &tracker,
            &player(),
            submitted(1, 60.0, 300.0, MovementMode::Grounded),
            config(),
            1_200,
        )
        .expect("bounded movement");

        assert!(!decision.accepted);
        assert_eq!(decision.rejection_reason, "the position is outside the map");
    }

    #[test]
    fn snapshots_cannot_walk_through_vertical_footholds() {
        let tracker = MovementTracker::default();
        let step_map = map_with_step_wall();
        let mut lower_player = player();
        lower_player.position = Some(Vec2 { x: 250.0, y: 400.0 });
        initialize_player(&tracker, &lower_player, &step_map, config(), 1_000).expect("initialize");

        let decision = submit_movement(
            &tracker,
            &lower_player,
            submitted(1, 200.0, 400.0, MovementMode::Grounded),
            config(),
            1_200,
        )
        .expect("step movement");

        assert!(!decision.accepted);
        assert_eq!(
            decision.rejection_reason,
            "the movement crosses a vertical foothold"
        );
    }

    #[test]
    fn snapshots_ignore_vertical_footholds_on_other_layers() {
        let tracker = MovementTracker::default();
        let layered_map = map_with_background_wall();
        let mut foreground_player = player();
        foreground_player.position = Some(Vec2 { x: 250.0, y: 400.0 });
        initialize_player(&tracker, &foreground_player, &layered_map, config(), 1_000)
            .expect("initialize");

        let decision = submit_movement(
            &tracker,
            &foreground_player,
            submitted(1, 206.0, 400.0, MovementMode::Grounded),
            config(),
            1_200,
        )
        .expect("foreground movement");

        assert!(decision.accepted);
    }

    #[test]
    fn snapshots_can_descend_over_vertical_foothold_tops() {
        let tracker = MovementTracker::default();
        let step_map = map_with_step_wall();
        let mut upper_player = player();
        upper_player.position = Some(Vec2 { x: 150.0, y: 300.0 });
        initialize_player(&tracker, &upper_player, &step_map, config(), 1_000).expect("initialize");

        let decision = submit_movement(
            &tracker,
            &upper_player,
            submitted(1, 220.0, 400.0, MovementMode::Grounded),
            config(),
            1_400,
        )
        .expect("step descent");

        assert!(decision.accepted);
    }

    #[test]
    fn stored_positions_are_clamped_inside_current_map_walls() {
        let tracker = MovementTracker::default();
        let mut bounded_map = map();
        bounded_map.movement_bounds = Some(MapMovementBounds {
            left: 68.0,
            right: 732.0,
        });
        let mut outside_player = player();
        outside_player.position = Some(Vec2 { x: 780.0, y: 300.0 });
        initialize_player(&tracker, &outside_player, &bounded_map, config(), 1_000)
            .expect("initialize");

        let synchronized = synchronize_player(&tracker, outside_player).expect("synchronize");

        assert_eq!(synchronized.position, Some(Vec2 { x: 732.0, y: 300.0 }));
    }

    #[test]
    fn airborne_snapshots_cannot_hover_indefinitely() {
        let tracker = initialized_tracker();
        let first = submit_movement(
            &tracker,
            &player(),
            submitted(1, 100.0, 260.0, MovementMode::Airborne),
            config(),
            1_200,
        )
        .expect("jump");
        let late = submit_movement(
            &tracker,
            &player(),
            submitted(2, 100.0, 260.0, MovementMode::Airborne),
            config(),
            20_000,
        )
        .expect("late airborne movement");

        assert!(first.accepted);
        assert!(!late.accepted);
    }

    #[test]
    fn validated_ground_contacts_reset_airborne_time_between_repeated_jumps() {
        let tracker = initialized_tracker();
        let first = submit_movement(
            &tracker,
            &player(),
            submitted(1, 100.0, 260.0, MovementMode::Airborne),
            config(),
            1_200,
        )
        .expect("first jump snapshot");
        assert!(first.accepted);

        for sequence in 2..=6 {
            let mut movement = submitted(sequence, 100.0, 260.0, MovementMode::Airborne);
            movement.support_contact = Some(SupportContact {
                position: Position { x: 100.0, y: 300.0 },
                mode: MovementMode::Grounded,
            });
            let airborne = submit_movement(
                &tracker,
                &player(),
                movement,
                config(),
                200 + sequence * 1_000,
            )
            .expect("jump with ground contact");
            assert!(airborne.accepted);
        }
    }

    #[test]
    fn unsupported_ground_contacts_cannot_reset_airborne_time() {
        let tracker = initialized_tracker();
        let first = submit_movement(
            &tracker,
            &player(),
            submitted(1, 100.0, 260.0, MovementMode::Airborne),
            config(),
            1_200,
        )
        .expect("first jump snapshot");
        assert!(first.accepted);

        let mut movement = submitted(2, 100.0, 260.0, MovementMode::Airborne);
        movement.support_contact = Some(SupportContact {
            position: Position { x: 100.0, y: 200.0 },
            mode: MovementMode::Grounded,
        });
        let decision = submit_movement(&tracker, &player(), movement, config(), 2_200)
            .expect("invalid ground contact");

        assert!(!decision.accepted);
        assert_eq!(
            decision.rejection_reason,
            "the reported support contact is invalid",
        );
    }

    #[test]
    fn climbing_requires_a_nearby_ladder() {
        let tracker = initialized_tracker();
        let approach = submit_movement(
            &tracker,
            &player(),
            submitted(1, 180.0, 300.0, MovementMode::Grounded),
            config(),
            1_400,
        )
        .expect("ladder approach");
        let accepted = submit_movement(
            &tracker,
            &player(),
            submitted(2, 200.0, 280.0, MovementMode::Climbing),
            config(),
            1_600,
        )
        .expect("ladder movement");
        let rejected = submit_movement(
            &tracker,
            &player(),
            submitted(3, 260.0, 260.0, MovementMode::Climbing),
            config(),
            1_800,
        )
        .expect("off-ladder movement");

        assert!(approach.accepted);
        assert!(accepted.accepted);
        assert!(!rejected.accepted);
    }

    #[test]
    fn climbing_can_attach_just_below_a_ladder_endpoint() {
        let tracker = MovementTracker::default();
        let mut player = player();
        player.position = Some(Vec2 { x: 175.0, y: 314.0 });
        initialize_player(&tracker, &player, &map(), config(), 1_000).expect("initialize");

        let decision = submit_movement(
            &tracker,
            &player,
            submitted(1, 200.0, 312.0, MovementMode::Climbing),
            config(),
            1_200,
        )
        .expect("ladder attachment");

        assert!(decision.accepted);
    }

    #[test]
    fn airborne_side_attachment_uses_the_airborne_approach_allowance() {
        let tracker = MovementTracker::default();
        let mut player = player();
        player.position = Some(Vec2 { x: 150.0, y: 150.0 });
        initialize_player(&tracker, &player, &map(), config(), 1_000).expect("initialize");

        let decision = submit_movement(
            &tracker,
            &player,
            submitted(1, 200.0, 250.0, MovementMode::Climbing),
            config(),
            1_200,
        )
        .expect("side attachment");

        assert!(decision.accepted);
    }

    #[test]
    fn jumping_off_a_ladder_uses_the_airborne_departure_allowance() {
        let tracker = MovementTracker::default();
        let mut player = player();
        player.position = Some(Vec2 { x: 200.0, y: 250.0 });
        initialize_player(&tracker, &player, &map(), config(), 1_000).expect("initialize");
        let attached = submit_movement(
            &tracker,
            &player,
            submitted(1, 200.0, 250.0, MovementMode::Climbing),
            config(),
            1_200,
        )
        .expect("ladder attachment");

        let jumped = submit_movement(
            &tracker,
            &player,
            submitted(2, 200.0, 170.0, MovementMode::Airborne),
            config(),
            1_400,
        )
        .expect("ladder departure");

        assert!(attached.accepted);
        assert!(jumped.accepted);
    }

    #[test]
    fn portal_entry_requires_proximity_and_uses_the_destination_portal() {
        let tracker = initialized_tracker();
        let mut source_map = map();
        source_map.portals.push(Portal {
            name: "east".to_owned(),
            x: 120.0,
            y: 300.0,
            target_map_id: 2,
            target_name: "west".to_owned(),
            ..Portal::default()
        });
        let target_map = Map {
            id: 2,
            width: 800,
            height: 600,
            platforms: vec![platform()],
            portals: vec![Portal {
                name: "west".to_owned(),
                x: 700.0,
                y: 300.0,
                ..Portal::default()
            }],
            movement_bounds: Some(MapMovementBounds {
                left: 18.0,
                right: 682.0,
            }),
            ..Map::default()
        };
        let (decision, plan) = enter_portal_with_modifiers(
            &tracker,
            &player(),
            PortalMovement {
                source_map: &source_map,
                target_map: &target_map,
                source: submitted(1, 120.0, 300.0, MovementMode::Grounded),
                target_portal_name: "west",
            },
            MovementModifiers::default(),
            config(),
            1_200,
        )
        .expect("portal movement");

        assert!(decision.accepted);
        assert_eq!(decision.authoritative.map_id, 2);
        assert_eq!(
            decision.authoritative.position.expect("position"),
            Vec2 { x: 682.0, y: 300.0 }
        );
        let unchanged = synchronize_player(&tracker, player()).expect("unchanged portal player");
        assert_eq!(unchanged.map_id, 1);
        assert_eq!(unchanged.position, Some(Vec2 { x: 100.0, y: 300.0 }));

        let committed = commit_relocation(&tracker, &plan.expect("portal plan"))
            .expect("commit portal relocation");
        let relocated = synchronize_player(&tracker, player()).expect("relocated portal player");
        assert_eq!(relocated.map_id, 2);
        assert_eq!(relocated.position, Some(Vec2 { x: 682.0, y: 300.0 }));

        restore_relocation(&tracker, committed).expect("restore portal source");
        let restored = synchronize_player(&tracker, player()).expect("restored portal player");
        assert_eq!(restored.map_id, 1);
        assert_eq!(restored.position, Some(Vec2 { x: 100.0, y: 300.0 }));
    }

    #[test]
    fn authorized_relocations_can_be_rolled_back_to_the_source_position() {
        let tracker = initialized_tracker();
        let source_map = map();
        let target_map = Map {
            id: 2,
            width: 800,
            height: 600,
            platforms: vec![platform()],
            portals: vec![Portal {
                name: "taxi".to_owned(),
                x: 700.0,
                y: 300.0,
                ..Portal::default()
            }],
            ..Map::default()
        };

        let (decision, plan) = relocate_player(
            &tracker,
            &player(),
            &source_map,
            &target_map,
            "taxi",
            config(),
            1_200,
        )
        .expect("authorized relocation");
        assert_eq!(decision.authoritative.map_id, 2);
        assert_eq!(
            synchronize_player(&tracker, player())
                .expect("unchanged planned player")
                .map_id,
            1
        );
        assert_eq!(
            project_relocation_player(&plan, player())
                .expect("project relocated player")
                .map_id,
            2
        );

        let committed = commit_relocation(&tracker, &plan).expect("commit relocation");
        restore_relocation(&tracker, committed).expect("restore source location");
        let restored = synchronize_player(&tracker, player()).expect("restored player");
        assert_eq!(restored.map_id, 1);
        assert_eq!(restored.position, Some(Vec2 { x: 100.0, y: 300.0 }));
    }

    #[test]
    fn relocation_commit_rejects_an_exact_source_session_conflict() {
        let tracker = initialized_tracker();
        let source_map = map();
        let target_map = Map {
            id: 2,
            width: 800,
            height: 600,
            platforms: vec![platform()],
            portals: vec![Portal {
                name: "taxi".to_owned(),
                x: 700.0,
                y: 300.0,
                ..Portal::default()
            }],
            ..Map::default()
        };
        let (_, plan) = relocate_player(
            &tracker,
            &player(),
            &source_map,
            &target_map,
            "taxi",
            config(),
            1_200,
        )
        .expect("plan relocation");
        submit_movement(
            &tracker,
            &player(),
            submitted(1, 120.0, 300.0, MovementMode::Grounded),
            config(),
            1_200,
        )
        .expect("concurrent source movement");

        assert!(matches!(
            commit_relocation(&tracker, &plan),
            Err(super::MovementError::RelocationSourceChanged)
        ));
        assert_eq!(
            synchronize_player(&tracker, player())
                .expect("conflicting source player")
                .position,
            Some(Vec2 { x: 120.0, y: 300.0 })
        );
    }

    #[test]
    fn named_portals_do_not_fall_back_to_the_default_spawn() {
        let mut target = map();
        target.portals.push(Portal {
            name: "spawn".to_owned(),
            kind: 0,
            x: 400.0,
            y: 300.0,
            ..Portal::default()
        });

        assert!(matches!(
            named_portal_position(&target, "missing"),
            Err(super::MovementError::MissingDestinationPortal { .. })
        ));
        assert_eq!(
            default_spawn_position(&target).expect("default spawn"),
            Vec2 { x: 400.0, y: 300.0 }
        );
    }

    fn initialized_tracker() -> MovementTracker {
        let tracker = MovementTracker::default();
        initialize_player(&tracker, &player(), &map(), config(), 1_000).expect("initialize");
        tracker
    }

    fn player() -> PlayerState {
        PlayerState {
            id: "player".to_owned(),
            map_id: 1,
            position: Some(Vec2 { x: 100.0, y: 300.0 }),
            ..PlayerState::default()
        }
    }

    fn map() -> Map {
        Map {
            id: 1,
            width: 800,
            height: 600,
            platforms: vec![platform()],
            ladders: vec![Ladder {
                x: 200.0,
                top: 200.0,
                bottom: 300.0,
                ..Ladder::default()
            }],
            ..Map::default()
        }
    }

    fn platform() -> Platform {
        Platform {
            x: 0.0,
            y: 300.0,
            end_x: 800.0,
            end_y: 300.0,
            ..Platform::default()
        }
    }

    fn map_with_step_wall() -> Map {
        Map {
            id: 1,
            width: 800,
            height: 600,
            platforms: vec![
                Platform {
                    x: 0.0,
                    y: 300.0,
                    end_x: 200.0,
                    end_y: 300.0,
                    ..Platform::default()
                },
                Platform {
                    x: 200.0,
                    y: 300.0,
                    end_x: 200.0,
                    end_y: 400.0,
                    ..Platform::default()
                },
                Platform {
                    x: 200.0,
                    y: 400.0,
                    end_x: 800.0,
                    end_y: 400.0,
                    ..Platform::default()
                },
            ],
            ..Map::default()
        }
    }

    fn map_with_background_wall() -> Map {
        Map {
            id: 1,
            width: 800,
            height: 600,
            platforms: vec![
                Platform {
                    x: 0.0,
                    y: 400.0,
                    end_x: 800.0,
                    end_y: 400.0,
                    layer: 1,
                    ..Platform::default()
                },
                Platform {
                    x: 200.0,
                    y: 300.0,
                    end_x: 200.0,
                    end_y: 400.0,
                    layer: 0,
                    ..Platform::default()
                },
            ],
            ..Map::default()
        }
    }

    fn submitted(
        sequence: u64,
        x: f32,
        y: f32,
        mode: MovementMode,
    ) -> SubmittedMovement {
        SubmittedMovement {
            sequence,
            map_id: 1,
            position: Position { x, y },
            mode,
            support_contact: None,
            drop_through: false,
        }
    }

    fn config() -> MovementConfig {
        MovementConfig {
            walk_speed: 220.0,
            climb_speed: 135.0,
            gravity: 1_150.0,
            jump_speed: 480.0,
            speed_cap: 200,
            jump_cap: 200,
            snapshot_interval: Duration::from_millis(200),
            maximum_snapshot_gap: Duration::from_secs(1),
            position_tolerance: 24.0,
            ground_tolerance: 8.0,
            platform_edge_tolerance: 20.0,
            ladder_reach: 32.0,
            ladder_end_reach: 20.0,
            portal_horizontal_reach: 48.0,
            portal_vertical_reach: 64.0,
        }
    }
}
