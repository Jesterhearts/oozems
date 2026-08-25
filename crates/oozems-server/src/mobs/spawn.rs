use std::collections::HashMap;
use std::time::Duration;

use oozems_proto::v1::Map;
use oozems_proto::v1::MobDefinition;
use oozems_proto::v1::MobMovementMode;
use oozems_proto::v1::MobSpawnPoint;

use super::ai;
use super::components::MobCombat;
use super::components::MobIdentity;
use super::components::MobMotion;
use super::components::Position;
use super::finite_position;

const BASE_MOVE_SPEED: f32 = 80.0;

pub(super) fn spawn_mob_components(
    map: &Map,
    default_respawn: Duration,
) -> Vec<(MobIdentity, Position, MobMotion, MobCombat)> {
    let definitions = map
        .mob_definitions
        .iter()
        .map(|definition| (definition.id, definition))
        .collect::<HashMap<_, _>>();
    map.mob_spawn_points
        .iter()
        .filter_map(|spawn| {
            spawn_mob(
                map,
                spawn,
                definitions.get(&spawn.mob_id).copied(),
                default_respawn,
            )
        })
        .collect()
}

fn spawn_mob(
    map: &Map,
    spawn: &MobSpawnPoint,
    definition: Option<&MobDefinition>,
    default_respawn: Duration,
) -> Option<(MobIdentity, Position, MobMotion, MobCombat)> {
    let definition = definition.filter(|definition| {
        definition
            .animations
            .iter()
            .any(|animation| !animation.frames.is_empty())
    })?;
    let source_position = spawn.position.filter(finite_position)?;
    let position = Position {
        x: source_position.x,
        y: source_position.y,
        layer: spawn.layer,
    };
    let (roam_left, roam_right) = roam_bounds(map, spawn, position.x);
    let spawn_support = map
        .platforms
        .iter()
        .position(|platform| spawn.foothold_id != 0 && platform.id == spawn.foothold_id)
        .or_else(|| {
            ai::nearest_platform(&map.platforms, position.x, position.y, Some(position.layer))
        });
    let flies = has_animation(definition, "fly");
    let can_move = has_animation(definition, "move") || flies;
    let respawn_delay_ms = if spawn.respawn_seconds == 0 {
        u64::try_from(default_respawn.as_millis()).unwrap_or(u64::MAX)
    } else {
        u64::from(spawn.respawn_seconds).saturating_mul(1_000)
    };
    Some((
        MobIdentity {
            public_id: format!("{}:{}:0", map.id, spawn.spawn_id),
            definition_id: definition.id,
            spawn_id: spawn.spawn_id,
        },
        position,
        MobMotion {
            spawn_position: position,
            spawn_support,
            support: spawn_support,
            roam_left,
            roam_right,
            move_speed: movement_speed(definition.speed),
            can_move,
            can_jump: definition.can_jump,
            flies,
            flip_x: spawn.flip_x,
            direction: 0,
            velocity_y: 0.0,
            decision_seconds: 0.0,
            random_state: random_seed(map.id, spawn.spawn_id),
            mode: MobMovementMode::Idle,
        },
        MobCombat {
            level: definition.level,
            maximum_hp: definition.max_hp.max(1),
            current_hp: definition.max_hp.max(1),
            physical_attack: definition.physical_attack,
            physical_defense: definition.physical_defense,
            magic_attack: definition.magic_attack,
            magic_defense: definition.magic_defense,
            accuracy: definition.accuracy,
            avoidability: definition.avoidability,
            body_attack: definition.body_attack,
            aggro_target: None,
            next_attack_ms: 0,
            attack_until_ms: 0,
            movement_resume_ms: 0,
            dead_until_ms: None,
            respawn_delay_ms,
            player_attack_transaction: None,
        },
    ))
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

fn random_seed(
    map_id: u32,
    spawn_id: u32,
) -> u64 {
    let seed = (u64::from(map_id) << 32) | u64::from(spawn_id);
    seed ^ 0x9e37_79b9_7f4a_7c15
}
