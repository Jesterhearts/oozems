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
use terrain::destination_position;
use terrain::movement_crosses_vertical_foothold;
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
    effects: HashMap<u32, MovementEffect>,
}

#[derive(Clone, Copy, Debug)]
struct MovementSession {
    sequence: u64,
    map_id: u32,
    position: Position,
    mode: MovementMode,
    platform_layer: i32,
    received_at_ms: u64,
    persisted_at_ms: u64,
    dirty: bool,
    airborne: Option<AirborneState>,
    horizontal_credit: f32,
    vertical_credit: f32,
    climb_credit: f32,
}

#[derive(Clone, Copy, Debug)]
struct AirborneState {
    origin_y: f32,
    started_at_ms: u64,
}

#[derive(Clone, Copy, Debug)]
struct MovementEffect {
    speed_bonus: i32,
    jump_bonus: i32,
    expires_at_ms: u64,
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
    pub persist: bool,
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
                effects: HashMap::new(),
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

pub fn submit_movement(
    tracker: &MovementTracker,
    player: &PlayerState,
    submitted: SubmittedMovement,
    config: MovementConfig,
    now_ms: u64,
) -> Result<MovementDecision, MovementError> {
    let map = movement_map(tracker, player.map_id)?;
    let mut players = tracker.players.lock().map_err(|_| MovementError::Tracker)?;
    let movement = players.entry(player.id.clone()).or_default();
    ensure_session(movement, player, &map, config, now_ms)?;
    let modifiers = active_modifiers(&mut movement.effects, now_ms);
    let session = movement
        .session
        .as_mut()
        .expect("movement session was initialized above");
    Ok(apply_snapshot(
        session, submitted, &map, config, modifiers, now_ms,
    ))
}

pub fn enter_portal(
    tracker: &MovementTracker,
    player: &PlayerState,
    portal: PortalMovement<'_>,
    config: MovementConfig,
    now_ms: u64,
) -> Result<MovementDecision, MovementError> {
    register_map(tracker, portal.source_map)?;
    register_map(tracker, portal.target_map)?;
    let mut players = tracker.players.lock().map_err(|_| MovementError::Tracker)?;
    let movement = players.entry(player.id.clone()).or_default();
    ensure_session(movement, player, portal.source_map, config, now_ms)?;
    let modifiers = active_modifiers(&mut movement.effects, now_ms);
    let session = movement
        .session
        .as_mut()
        .expect("movement session was initialized above");
    let source_decision = apply_snapshot(
        session,
        portal.source,
        portal.source_map,
        config,
        modifiers,
        now_ms,
    );
    if !source_decision.accepted {
        return Ok(source_decision);
    }
    let activity = source_decision.activity;
    let persist = source_decision.persist;
    let Some(source_portal) = portal.source_map.portals.iter().find(|source_portal| {
        source_portal.target_map_id == portal.target_map.id
            && source_portal.target_map_id != SCRIPT_PORTAL_TARGET
            && source_portal.target_name == portal.target_portal_name
            && (session.position.x - source_portal.x).abs() <= config.portal_horizontal_reach
            && (session.position.y - source_portal.y).abs() <= config.portal_vertical_reach
    }) else {
        return Ok(MovementDecision {
            persist,
            ..reject(
                session,
                "the authoritative position is not at that portal",
                activity,
            )
        });
    };
    let Some(position) = destination_position(portal.target_map, &source_portal.target_name) else {
        return Ok(MovementDecision {
            persist,
            ..reject(
                session,
                "the destination map has no usable portal",
                activity,
            )
        });
    };
    let position = clamp_to_movement_bounds(portal.target_map, position);
    let (mode, platform_layer) = initial_motion(portal.target_map, &position, config);
    session.map_id = portal.target_map.id;
    session.position = position;
    session.mode = mode;
    session.platform_layer = platform_layer;
    session.airborne = airborne_state(session.mode, position.y, now_ms);
    session.dirty = true;
    Ok(MovementDecision {
        persist: true,
        ..accept(session, true, now_ms, config)
    })
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

pub fn mark_persisted(
    tracker: &MovementTracker,
    player_id: &str,
    now_ms: u64,
) -> Result<(), MovementError> {
    if let Some(session) = tracker
        .players
        .lock()
        .map_err(|_| MovementError::Tracker)?
        .get_mut(player_id)
        .and_then(|movement| movement.session.as_mut())
    {
        session.persisted_at_ms = now_ms;
        session.dirty = false;
    }
    Ok(())
}

pub fn record_skill_effect(
    tracker: &MovementTracker,
    player_id: &str,
    skill_id: u32,
    speed_bonus: i32,
    jump_bonus: i32,
    duration_ms: u64,
    now_ms: u64,
) -> Result<(), MovementError> {
    let mut players = tracker.players.lock().map_err(|_| MovementError::Tracker)?;
    let movement = players.entry(player_id.to_owned()).or_default();
    movement.effects.remove(&skill_id);
    if duration_ms > 0 && (speed_bonus != 0 || jump_bonus != 0) {
        movement.effects.insert(
            skill_id,
            MovementEffect {
                speed_bonus,
                jump_bonus,
                expires_at_ms: now_ms.saturating_add(duration_ms),
            },
        );
    }
    Ok(())
}

fn ensure_session(
    movement: &mut PlayerMovement,
    player: &PlayerState,
    map: &Map,
    config: MovementConfig,
    now_ms: u64,
) -> Result<(), MovementError> {
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
        persisted_at_ms: now_ms,
        dirty: false,
        airborne: airborne_state(mode, position.y, now_ms),
        horizontal_credit: config.position_tolerance,
        vertical_credit: config.position_tolerance,
        climb_credit: config.position_tolerance,
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
) -> MovementDecision {
    if submitted.sequence <= session.sequence {
        return reject(session, "the movement sequence is not newer", false);
    }
    session.sequence = submitted.sequence;
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
    let walk_speed = modified_speed(config.walk_speed, modifiers.speed, config.speed_cap);
    let jump_speed = modified_speed(config.jump_speed, modifiers.jump, config.jump_cap);
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
    session.dirty |= activity;
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
    accept(session, activity, now_ms, config)
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

#[derive(Clone, Copy, Debug, Default)]
struct MovementModifiers {
    speed: i32,
    jump: i32,
}

fn active_modifiers(
    effects: &mut HashMap<u32, MovementEffect>,
    now_ms: u64,
) -> MovementModifiers {
    effects.retain(|_, effect| effect.expires_at_ms > now_ms);
    effects
        .values()
        .fold(MovementModifiers::default(), |mut modifiers, effect| {
            modifiers.speed = modifiers.speed.saturating_add(effect.speed_bonus);
            modifiers.jump = modifiers.jump.saturating_add(effect.jump_bonus);
            modifiers
        })
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
    now_ms: u64,
    config: MovementConfig,
) -> MovementDecision {
    MovementDecision {
        authoritative: snapshot(session),
        accepted: true,
        rejection_reason: String::new(),
        activity,
        persist: session.dirty
            && now_ms.saturating_sub(session.persisted_at_ms)
                >= duration_millis(config.persistence_interval),
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
        persist: false,
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
    use super::MovementTracker;
    use super::PortalMovement;
    use super::Position;
    use super::SubmittedMovement;
    use super::SupportContact;
    use super::enter_portal;
    use super::initialize_player;
    use super::record_skill_effect;
    use super::register_map;
    use super::submit_movement;
    use super::synchronize_player;
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
    fn a_stationary_heartbeat_persists_prior_movement_when_due() {
        let tracker = initialized_tracker();
        let movement = submit_movement(
            &tracker,
            &player(),
            submitted(1, 140.0, 300.0, MovementMode::Grounded),
            config(),
            1_200,
        )
        .expect("movement");
        let heartbeat = submit_movement(
            &tracker,
            &player(),
            submitted(2, 140.0, 300.0, MovementMode::Grounded),
            config(),
            3_000,
        )
        .expect("stationary heartbeat");

        assert!(!movement.persist);
        assert!(!heartbeat.activity);
        assert!(heartbeat.persist);
    }

    #[test]
    fn haste_bonus_is_capped_by_the_configured_speed_stat() {
        let tracker = initialized_tracker();
        record_skill_effect(&tracker, "player", 4_001_005, 150, 0, 10_000, 1_000)
            .expect("movement effect");

        let accepted = submit_movement(
            &tracker,
            &player(),
            submitted(1, 190.0, 300.0, MovementMode::Grounded),
            config(),
            1_200,
        )
        .expect("capped movement");
        let rejected = submit_movement(
            &tracker,
            &player(),
            submitted(2, 305.0, 300.0, MovementMode::Grounded),
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
        record_skill_effect(
            &accepted_tracker,
            "player",
            4_001_005,
            0,
            150,
            10_000,
            1_000,
        )
        .expect("jump effect");
        let accepted = submit_movement(
            &accepted_tracker,
            &player(),
            submitted(1, 100.0, 110.0, MovementMode::Airborne),
            config(),
            1_200,
        )
        .expect("capped jump");

        let rejected_tracker = initialized_tracker();
        record_skill_effect(
            &rejected_tracker,
            "player",
            4_001_005,
            0,
            150,
            10_000,
            1_000,
        )
        .expect("jump effect");
        let rejected = submit_movement(
            &rejected_tracker,
            &player(),
            submitted(1, 100.0, 80.0, MovementMode::Airborne),
            config(),
            1_200,
        )
        .expect("excess jump");

        assert!(accepted.accepted);
        assert!(!rejected.accepted);
    }

    #[test]
    fn expired_movement_effects_no_longer_expand_the_envelope() {
        let tracker = initialized_tracker();
        record_skill_effect(&tracker, "player", 4_001_005, 40, 20, 100, 1_000)
            .expect("movement effect");

        let decision = submit_movement(
            &tracker,
            &player(),
            submitted(1, 190.0, 300.0, MovementMode::Grounded),
            config(),
            1_200,
        )
        .expect("expired effect movement");

        assert!(!decision.accepted);
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
    fn registering_a_missing_map_preserves_pending_skill_effects() {
        let tracker = MovementTracker::default();
        record_skill_effect(&tracker, "player", 4_001_005, 150, 0, 10_000, 1_000)
            .expect("movement effect");
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
        let heartbeat = submit_movement(
            &tracker,
            &player(),
            submitted(1, 100.0, 300.0, MovementMode::Grounded),
            config(),
            1_200,
        )
        .expect("initial heartbeat");
        let haste_movement = submit_movement(
            &tracker,
            &player(),
            submitted(2, 190.0, 300.0, MovementMode::Grounded),
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
    fn legacy_positions_are_clamped_inside_new_map_walls() {
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
        let decision = enter_portal(
            &tracker,
            &player(),
            PortalMovement {
                source_map: &source_map,
                target_map: &target_map,
                source: submitted(1, 120.0, 300.0, MovementMode::Grounded),
                target_portal_name: "west",
            },
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
            persistence_interval: Duration::from_secs(2),
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
