use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

use oozems_proto::v1::Map;
use oozems_proto::v1::Mob;
use oozems_proto::v1::MobDefinition;
use oozems_proto::v1::MobMovementMode;
use oozems_proto::v1::MobSpawnPoint;
use oozems_proto::v1::Platform;
use oozems_proto::v1::Vec2;
use thiserror::Error;

const BASE_MOVE_SPEED: f32 = 80.0;
const MOB_GRAVITY: f32 = 900.0;
const MOB_JUMP_SPEED: f32 = 330.0;
const MAX_JUMP_RISE: f32 = MOB_JUMP_SPEED * MOB_JUMP_SPEED / (2.0 * MOB_GRAVITY);
const JUMP_LEDGE_LOOKAHEAD: f32 = 48.0;
const WALKABLE_HEIGHT_CHANGE: f32 = 12.0;
const PHYSICS_STEP_SECONDS: f32 = 1.0 / 60.0;
const MAX_CATCH_UP: Duration = Duration::from_secs(1);
const MIN_DECISION_SECONDS: f32 = 0.75;
const DECISION_RANGE_SECONDS: f32 = 1.75;

#[derive(Default)]
pub struct MobStore {
    maps: Mutex<HashMap<u32, MobMapState>>,
}

struct MobMapState {
    terrain: MobTerrain,
    agents: Vec<MobAgent>,
    updated_at: Instant,
}

struct MobTerrain {
    platforms: Vec<Platform>,
    height: f32,
}

#[derive(Clone, Debug)]
struct MobAgent {
    mob: Mob,
    spawn_position: Vec2,
    spawn_support: Option<usize>,
    support: Option<usize>,
    roam_left: f32,
    roam_right: f32,
    move_speed: f32,
    can_move: bool,
    can_jump: bool,
    flies: bool,
    direction: i8,
    velocity_y: f32,
    decision_seconds: f32,
    random_state: u64,
}

#[derive(Debug, Error)]
pub enum MobStoreError {
    #[error("the mob store lock was poisoned")]
    Lock,
}

pub fn map_mobs(
    store: &MobStore,
    map: &Map,
) -> Result<Vec<Mob>, MobStoreError> {
    map_mobs_at(store, map, Instant::now())
}

pub fn current_mobs(
    store: &MobStore,
    map_id: u32,
) -> Result<Option<Vec<Mob>>, MobStoreError> {
    current_mobs_at(store, map_id, Instant::now())
}

#[cfg(test)]
pub fn spawn_mobs(map: &Map) -> Vec<Mob> {
    spawn_agents(map)
        .into_iter()
        .map(|agent| agent.mob)
        .collect()
}

fn map_mobs_at(
    store: &MobStore,
    map: &Map,
    now: Instant,
) -> Result<Vec<Mob>, MobStoreError> {
    let mut maps = store.maps.lock().map_err(|_| MobStoreError::Lock)?;
    let state = maps
        .entry(map.id)
        .or_insert_with(|| build_map_state(map, now));
    advance_map_to(state, now);
    Ok(mob_snapshot(state))
}

fn current_mobs_at(
    store: &MobStore,
    map_id: u32,
    now: Instant,
) -> Result<Option<Vec<Mob>>, MobStoreError> {
    let mut maps = store.maps.lock().map_err(|_| MobStoreError::Lock)?;
    let Some(state) = maps.get_mut(&map_id) else {
        return Ok(None);
    };
    advance_map_to(state, now);
    Ok(Some(mob_snapshot(state)))
}

fn build_map_state(
    map: &Map,
    now: Instant,
) -> MobMapState {
    MobMapState {
        terrain: MobTerrain {
            platforms: map.platforms.clone(),
            height: map.height as f32,
        },
        agents: spawn_agents(map),
        updated_at: now,
    }
}

fn advance_map_to(
    state: &mut MobMapState,
    now: Instant,
) {
    // Inactive maps resume near their last state instead of simulating an
    // unbounded backlog and sending large position jumps to the first client.
    let elapsed = now
        .checked_duration_since(state.updated_at)
        .unwrap_or_default()
        .min(MAX_CATCH_UP)
        .as_secs_f32();
    state.updated_at = now;
    advance_agents(&state.terrain, &mut state.agents, elapsed);
}

fn mob_snapshot(state: &MobMapState) -> Vec<Mob> {
    state.agents.iter().map(|agent| agent.mob.clone()).collect()
}

fn spawn_agents(map: &Map) -> Vec<MobAgent> {
    let definitions = map
        .mob_definitions
        .iter()
        .map(|definition| (definition.id, definition))
        .collect::<HashMap<_, _>>();
    map.mob_spawn_points
        .iter()
        .filter_map(|spawn| spawn_agent(map, spawn, definitions.get(&spawn.mob_id).copied()))
        .collect()
}

fn spawn_agent(
    map: &Map,
    spawn: &MobSpawnPoint,
    definition: Option<&MobDefinition>,
) -> Option<MobAgent> {
    let definition = definition.filter(|definition| {
        definition
            .animations
            .iter()
            .any(|animation| !animation.frames.is_empty())
    })?;
    let position = spawn
        .position
        .filter(|position| position.x.is_finite() && position.y.is_finite())?;
    let (roam_left, roam_right) = roam_bounds(map, spawn, position.x);
    let spawn_support = spawn_support(map, spawn, position);
    let flies = has_animation(definition, "fly");
    let can_move = has_animation(definition, "move") || flies;
    Some(MobAgent {
        mob: Mob {
            id: format!("{}:{}:0", map.id, spawn.spawn_id),
            definition_id: definition.id,
            position: Some(position),
            flip_x: spawn.flip_x,
            layer: spawn.layer,
            current_hp: definition.max_hp.max(1),
            spawn_id: spawn.spawn_id,
            movement_mode: MobMovementMode::Idle as i32,
        },
        spawn_position: position,
        spawn_support,
        support: spawn_support,
        roam_left,
        roam_right,
        move_speed: movement_speed(definition.speed),
        can_move,
        can_jump: definition.can_jump,
        flies,
        direction: 0,
        velocity_y: 0.0,
        decision_seconds: 0.0,
        random_state: random_seed(map.id, spawn.spawn_id),
    })
}

fn roam_bounds(
    map: &Map,
    spawn: &MobSpawnPoint,
    spawn_x: f32,
) -> (f32, f32) {
    let map_right = map.width as f32;
    let left = spawn.roam_left.min(spawn.roam_right);
    let right = spawn.roam_left.max(spawn.roam_right);
    if !left.is_finite() || !right.is_finite() || left >= right {
        let x = spawn_x.clamp(0.0, map_right);
        return (x, x);
    }
    (left.clamp(0.0, map_right), right.clamp(0.0, map_right))
}

fn spawn_support(
    map: &Map,
    spawn: &MobSpawnPoint,
    position: Vec2,
) -> Option<usize> {
    map.platforms
        .iter()
        .position(|platform| spawn.foothold_id != 0 && platform.id == spawn.foothold_id)
        .or_else(|| nearest_platform(&map.platforms, position.x, position.y, None))
}

fn movement_speed(wz_speed: i32) -> f32 {
    let percentage = (100_i64 + i64::from(wz_speed)).clamp(0, 200) as f32;
    BASE_MOVE_SPEED * percentage / 100.0
}

fn has_animation(
    definition: &MobDefinition,
    name: &str,
) -> bool {
    definition
        .animations
        .iter()
        .any(|animation| animation.name == name && !animation.frames.is_empty())
}

fn advance_agents(
    terrain: &MobTerrain,
    agents: &mut [MobAgent],
    elapsed_seconds: f32,
) {
    let mut remaining = elapsed_seconds.max(0.0);
    while remaining > 0.0 {
        let step = remaining.min(PHYSICS_STEP_SECONDS);
        for agent in &mut *agents {
            advance_agent(terrain, agent, step);
        }
        remaining -= step;
    }
}

fn advance_agent(
    terrain: &MobTerrain,
    agent: &mut MobAgent,
    elapsed_seconds: f32,
) {
    if agent.mob.current_hp == 0 {
        set_mode(agent, MobMovementMode::Idle);
        return;
    }
    if movement_mode(agent) == MobMovementMode::Jumping {
        advance_jump(terrain, agent, elapsed_seconds);
        return;
    }
    agent.decision_seconds -= elapsed_seconds;
    if agent.decision_seconds <= 0.0 {
        choose_behavior(agent);
    }
    if agent.flies {
        advance_flying(agent, elapsed_seconds);
    } else {
        advance_grounded(terrain, agent, elapsed_seconds);
    }
}

fn choose_behavior(agent: &mut MobAgent) {
    let choice = next_random(&mut agent.random_state) % 5;
    let direction = if !agent.can_move || agent.move_speed <= 0.0 {
        0
    } else {
        match choice {
            0 => 0,
            1 | 2 => -1,
            _ => 1,
        }
    };
    set_direction(agent, direction);
    set_mode(
        agent,
        if direction == 0 {
            MobMovementMode::Idle
        } else {
            MobMovementMode::Walking
        },
    );
    let duration_fraction = next_random(&mut agent.random_state) as f64 / u64::MAX as f64;
    agent.decision_seconds =
        MIN_DECISION_SECONDS + duration_fraction as f32 * DECISION_RANGE_SECONDS;
}

fn advance_flying(
    agent: &mut MobAgent,
    elapsed_seconds: f32,
) {
    let Some(mut position) = agent.mob.position else {
        return;
    };
    let proposed_x = position.x + f32::from(agent.direction) * agent.move_speed * elapsed_seconds;
    position.x = proposed_x.clamp(agent.roam_left, agent.roam_right);
    if proposed_x != position.x {
        reverse_direction(agent);
    }
    agent.mob.position = Some(position);
}

fn advance_grounded(
    terrain: &MobTerrain,
    agent: &mut MobAgent,
    elapsed_seconds: f32,
) {
    let Some(position) = agent.mob.position else {
        return;
    };
    let Some(support) = valid_support(terrain, agent.support, position).or_else(|| {
        nearest_platform(
            &terrain.platforms,
            position.x,
            position.y,
            Some(agent.mob.layer),
        )
    }) else {
        set_direction(agent, 0);
        set_mode(agent, MobMovementMode::Idle);
        return;
    };
    agent.support = Some(support);
    if agent.direction == 0 {
        snap_to_platform(terrain, agent, support, position.x);
        return;
    }

    let proposed_x = (position.x + f32::from(agent.direction) * agent.move_speed * elapsed_seconds)
        .clamp(agent.roam_left, agent.roam_right);
    if let Some(next_support) = walkable_support(terrain, agent, support, proposed_x) {
        snap_to_platform(terrain, agent, next_support, proposed_x);
        if proposed_x == agent.roam_left || proposed_x == agent.roam_right {
            reverse_direction(agent);
        }
        return;
    }
    if agent.can_jump && has_reachable_ledge(terrain, agent, position) {
        agent.velocity_y = -MOB_JUMP_SPEED;
        set_mode(agent, MobMovementMode::Jumping);
        advance_jump(terrain, agent, elapsed_seconds);
        return;
    }
    stop_at_platform_edge(terrain, agent, support);
}

fn valid_support(
    terrain: &MobTerrain,
    support: Option<usize>,
    position: Vec2,
) -> Option<usize> {
    let support = support?;
    platform_surface_at_x(terrain.platforms.get(support)?, position.x).map(|_| support)
}

fn walkable_support(
    terrain: &MobTerrain,
    agent: &MobAgent,
    current_support: usize,
    x: f32,
) -> Option<usize> {
    let current = terrain.platforms.get(current_support)?;
    let current_y = agent.mob.position?.y;
    if platform_surface_at_x(current, x).is_some() {
        return Some(current_support);
    }
    nearest_platform(&terrain.platforms, x, current_y, Some(current.layer)).filter(|index| {
        platform_surface_at_x(&terrain.platforms[*index], x)
            .is_some_and(|surface| (surface - current_y).abs() <= WALKABLE_HEIGHT_CHANGE)
    })
}

fn has_reachable_ledge(
    terrain: &MobTerrain,
    agent: &MobAgent,
    position: Vec2,
) -> bool {
    let direction = f32::from(agent.direction);
    terrain.platforms.iter().any(|platform| {
        let edge_x = if direction > 0.0 {
            platform.x.min(platform.end_x)
        } else {
            platform.x.max(platform.end_x)
        };
        let distance = (edge_x - position.x) * direction;
        if !(0.0..=JUMP_LEDGE_LOOKAHEAD).contains(&distance)
            || edge_x < agent.roam_left
            || edge_x > agent.roam_right
        {
            return false;
        }
        let sample_x = (edge_x + direction).clamp(agent.roam_left, agent.roam_right);
        platform_surface_at_x(platform, sample_x).is_some_and(|surface| {
            let rise = position.y - surface;
            rise > WALKABLE_HEIGHT_CHANGE && rise <= MAX_JUMP_RISE
        })
    })
}

fn stop_at_platform_edge(
    terrain: &MobTerrain,
    agent: &mut MobAgent,
    support: usize,
) {
    let Some(platform) = terrain.platforms.get(support) else {
        return;
    };
    let edge_x = if agent.direction > 0 {
        platform.x.max(platform.end_x)
    } else {
        platform.x.min(platform.end_x)
    }
    .clamp(agent.roam_left, agent.roam_right);
    snap_to_platform(terrain, agent, support, edge_x);
    reverse_direction(agent);
}

fn snap_to_platform(
    terrain: &MobTerrain,
    agent: &mut MobAgent,
    support: usize,
    x: f32,
) {
    let Some(platform) = terrain.platforms.get(support) else {
        return;
    };
    let Some(y) = platform_surface_at_x(platform, x) else {
        return;
    };
    agent.mob.position = Some(Vec2 { x, y });
    agent.mob.layer = platform.layer;
    agent.support = Some(support);
    agent.velocity_y = 0.0;
    set_mode(
        agent,
        if agent.direction == 0 {
            MobMovementMode::Idle
        } else {
            MobMovementMode::Walking
        },
    );
}

fn advance_jump(
    terrain: &MobTerrain,
    agent: &mut MobAgent,
    elapsed_seconds: f32,
) {
    let Some(position) = agent.mob.position else {
        return;
    };
    let next_velocity_y = agent.velocity_y + MOB_GRAVITY * elapsed_seconds;
    let next_x = (position.x + f32::from(agent.direction) * agent.move_speed * elapsed_seconds)
        .clamp(agent.roam_left, agent.roam_right);
    let next_y = position.y + (agent.velocity_y + next_velocity_y) * 0.5 * elapsed_seconds;
    if next_velocity_y >= 0.0
        && let Some(support) = landing_support(terrain, position.y, next_x, next_y)
    {
        snap_to_platform(terrain, agent, support, next_x);
        return;
    }
    agent.mob.position = Some(Vec2 {
        x: next_x,
        y: next_y,
    });
    agent.velocity_y = next_velocity_y;
    if (next_x == agent.roam_left && agent.direction < 0)
        || (next_x == agent.roam_right && agent.direction > 0)
    {
        reverse_direction(agent);
    }
    if next_y > terrain.height + 100.0 {
        reset_agent(agent);
    }
}

fn landing_support(
    terrain: &MobTerrain,
    previous_y: f32,
    x: f32,
    next_y: f32,
) -> Option<usize> {
    terrain
        .platforms
        .iter()
        .enumerate()
        .filter_map(|(index, platform)| {
            let surface = platform_surface_at_x(platform, x)?;
            (surface >= previous_y && surface <= next_y).then_some((index, surface))
        })
        .min_by(|left, right| left.1.total_cmp(&right.1))
        .map(|(index, _)| index)
}

fn reset_agent(agent: &mut MobAgent) {
    agent.mob.position = Some(agent.spawn_position);
    agent.support = agent.spawn_support;
    agent.velocity_y = 0.0;
    agent.decision_seconds = 0.0;
    set_direction(agent, 0);
    set_mode(agent, MobMovementMode::Idle);
}

fn nearest_platform(
    platforms: &[Platform],
    x: f32,
    reference_y: f32,
    layer: Option<i32>,
) -> Option<usize> {
    platforms
        .iter()
        .enumerate()
        .filter(|(_, platform)| layer.is_none_or(|layer| platform.layer == layer))
        .filter_map(|(index, platform)| {
            let surface = platform_surface_at_x(platform, x)?;
            Some((index, (surface - reference_y).abs()))
        })
        .min_by(|left, right| left.1.total_cmp(&right.1))
        .map(|(index, _)| index)
}

fn platform_surface_at_x(
    platform: &Platform,
    x: f32,
) -> Option<f32> {
    let minimum_x = platform.x.min(platform.end_x);
    let maximum_x = platform.x.max(platform.end_x);
    if !(minimum_x..=maximum_x).contains(&x) {
        return None;
    }
    let delta_x = platform.end_x - platform.x;
    if delta_x.abs() < f32::EPSILON {
        return None;
    }
    let progress = (x - platform.x) / delta_x;
    Some(platform.y + progress * (platform.end_y - platform.y))
}

fn movement_mode(agent: &MobAgent) -> MobMovementMode {
    MobMovementMode::try_from(agent.mob.movement_mode).unwrap_or(MobMovementMode::Idle)
}

fn set_mode(
    agent: &mut MobAgent,
    mode: MobMovementMode,
) {
    agent.mob.movement_mode = mode as i32;
}

fn set_direction(
    agent: &mut MobAgent,
    direction: i8,
) {
    agent.direction = direction.clamp(-1, 1);
    if direction != 0 {
        // Classic WZ mob frames face left before the map flip flag is applied.
        agent.mob.flip_x = direction > 0;
    }
}

fn reverse_direction(agent: &mut MobAgent) {
    set_direction(agent, -agent.direction);
    agent.decision_seconds = agent.decision_seconds.max(MIN_DECISION_SECONDS / 2.0);
}

fn random_seed(
    map_id: u32,
    spawn_id: u32,
) -> u64 {
    let seed = (u64::from(map_id) << 32) | u64::from(spawn_id);
    seed ^ 0x9e37_79b9_7f4a_7c15
}

fn next_random(state: &mut u64) -> u64 {
    let mut value = *state;
    value ^= value << 13;
    value ^= value >> 7;
    value ^= value << 17;
    *state = value;
    value
}

#[cfg(test)]
mod tests {
    use std::time::Duration;
    use std::time::Instant;

    use oozems_proto::v1::Map;
    use oozems_proto::v1::MobAnimation;
    use oozems_proto::v1::MobDefinition;
    use oozems_proto::v1::MobFrame;
    use oozems_proto::v1::MobMovementMode;
    use oozems_proto::v1::MobSpawnPoint;
    use oozems_proto::v1::Platform;
    use oozems_proto::v1::Vec2;

    use super::MobStore;
    use super::advance_agents;
    use super::build_map_state;
    use super::current_mobs_at;
    use super::map_mobs_at;
    use super::spawn_agents;
    use super::spawn_mobs;

    #[test]
    fn spawn_points_become_idle_mobs_with_full_health() {
        let map = map(false);

        let mobs = spawn_mobs(&map);

        assert_eq!(mobs.len(), 1);
        assert_eq!(mobs[0].id, "100010000:3:0");
        assert_eq!(mobs[0].definition_id, 100_101);
        assert_eq!(mobs[0].current_hp, 15);
        assert_eq!(mobs[0].position, Some(Vec2 { x: 50.0, y: 300.0 }));
        assert_eq!(
            MobMovementMode::try_from(mobs[0].movement_mode),
            Ok(MobMovementMode::Idle)
        );
    }

    #[test]
    fn store_advances_a_registered_map_once_for_elapsed_time() {
        let store = MobStore::default();
        let map = map(false);
        let started_at = Instant::now();

        let first = map_mobs_at(&store, &map, started_at).expect("first lookup");
        let second = current_mobs_at(&store, map.id, started_at + Duration::from_secs(1))
            .expect("second lookup")
            .expect("registered map");
        let third = current_mobs_at(&store, map.id, started_at + Duration::from_secs(1))
            .expect("third lookup")
            .expect("registered map");

        assert_ne!(first[0].position, second[0].position);
        assert_eq!(second, third);
    }

    #[test]
    fn current_lookup_does_not_create_an_unknown_map() {
        let store = MobStore::default();

        assert_eq!(
            current_mobs_at(&store, 123, Instant::now()).expect("lookup"),
            None
        );
    }

    #[test]
    fn non_jumping_mob_turns_at_a_raised_ledge() {
        let map = ledge_map(false);
        let mut agents = spawn_agents(&map);
        agents[0].direction = 1;
        agents[0].decision_seconds = 10.0;

        advance_agents(
            &build_map_state(&map, Instant::now()).terrain,
            &mut agents,
            0.25,
        );

        let position = agents[0].mob.position.expect("position");
        assert!(position.x < 100.0);
        assert_eq!(position.y, 300.0);
        assert_eq!(agents[0].direction, -1);
        assert_ne!(
            MobMovementMode::try_from(agents[0].mob.movement_mode),
            Ok(MobMovementMode::Jumping)
        );
    }

    #[test]
    fn jumping_mob_climbs_a_reachable_ledge() {
        let map = ledge_map(true);
        let state = build_map_state(&map, Instant::now());
        let mut agents = spawn_agents(&map);
        agents[0].direction = 1;
        agents[0].decision_seconds = 10.0;

        advance_agents(&state.terrain, &mut agents, 0.25);

        assert_eq!(
            MobMovementMode::try_from(agents[0].mob.movement_mode),
            Ok(MobMovementMode::Jumping)
        );
        assert!(agents[0].mob.position.expect("airborne").y < 300.0);

        advance_agents(&state.terrain, &mut agents, 0.8);

        let position = agents[0].mob.position.expect("landed");
        assert_eq!(position.y, 250.0);
        assert!(position.x >= 100.0);
        assert_eq!(agents[0].mob.layer, 2);
        assert_eq!(
            MobMovementMode::try_from(agents[0].mob.movement_mode),
            Ok(MobMovementMode::Walking)
        );
    }

    #[test]
    fn jumping_mob_turns_when_a_ledge_is_too_high() {
        let mut map = ledge_map(true);
        map.platforms[1].y = 200.0;
        map.platforms[1].end_y = 200.0;
        let state = build_map_state(&map, Instant::now());
        let mut agents = spawn_agents(&map);
        agents[0].direction = 1;
        agents[0].decision_seconds = 10.0;

        advance_agents(&state.terrain, &mut agents, 0.25);

        assert_eq!(agents[0].direction, -1);
        assert_ne!(
            MobMovementMode::try_from(agents[0].mob.movement_mode),
            Ok(MobMovementMode::Jumping)
        );
    }

    #[test]
    fn definitions_without_visuals_do_not_spawn() {
        let mut map = map(false);
        map.mob_definitions[0].animations.clear();

        assert!(spawn_mobs(&map).is_empty());
    }

    fn map(can_jump: bool) -> Map {
        Map {
            id: 100_010_000,
            width: 800,
            height: 600,
            platforms: vec![platform(1, 0.0, 300.0, 800.0, 300.0, 2)],
            mob_spawn_points: vec![MobSpawnPoint {
                spawn_id: 3,
                mob_id: 100_101,
                position: Some(Vec2 { x: 50.0, y: 300.0 }),
                roam_left: 0.0,
                roam_right: 800.0,
                layer: 2,
                foothold_id: 1,
                ..MobSpawnPoint::default()
            }],
            mob_definitions: vec![definition(can_jump)],
            ..Map::default()
        }
    }

    fn ledge_map(can_jump: bool) -> Map {
        let mut map = map(can_jump);
        map.platforms = vec![
            platform(1, 0.0, 300.0, 100.0, 300.0, 1),
            platform(2, 100.0, 250.0, 300.0, 250.0, 2),
        ];
        map.mob_spawn_points[0].position = Some(Vec2 { x: 90.0, y: 300.0 });
        map.mob_spawn_points[0].roam_right = 300.0;
        map.mob_spawn_points[0].layer = 1;
        map
    }

    fn definition(can_jump: bool) -> MobDefinition {
        let mut animations = vec![animation("move")];
        if can_jump {
            animations.push(animation("jump"));
        }
        MobDefinition {
            id: 100_101,
            max_hp: 15,
            speed: 0,
            animations,
            can_jump,
            ..MobDefinition::default()
        }
    }

    fn animation(name: &str) -> MobAnimation {
        MobAnimation {
            name: name.to_owned(),
            frames: vec![MobFrame {
                asset_id: name.to_owned(),
                ..MobFrame::default()
            }],
        }
    }

    fn platform(
        id: u32,
        x: f32,
        y: f32,
        end_x: f32,
        end_y: f32,
        layer: i32,
    ) -> Platform {
        Platform {
            id,
            x,
            y,
            end_x,
            end_y,
            layer,
            ..Platform::default()
        }
    }
}
