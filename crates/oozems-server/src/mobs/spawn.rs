use std::collections::HashMap;
use std::time::Duration;

use oozems_proto::v1::Map;
use oozems_proto::v1::MobDefinition;
use oozems_proto::v1::MobMovementMode;
use oozems_proto::v1::MobSpawnPoint;

use super::ai;
use super::components::MobCombat;
use super::components::MobHitbox;
use super::components::MobIdentity;
use super::components::MobMotion;
use super::components::Position;
use super::finite_position;
use crate::attacks::DEFAULT_TARGET_VERTICAL_BOUNDS;
use crate::attacks::VerticalBounds;

const BASE_MOVE_SPEED: f32 = 80.0;

pub(super) fn spawn_mob_components(
    map: &Map,
    default_respawn: Duration,
) -> Vec<(MobIdentity, Position, MobHitbox, MobMotion, MobCombat)> {
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
) -> Option<(MobIdentity, Position, MobHitbox, MobMotion, MobCombat)> {
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
        MobHitbox(vertical_bounds(definition)),
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
            random_state: crate::random::map_spawn_seed(map.id, spawn.spawn_id),
            mode: MobMovementMode::Idle,
        },
        MobCombat {
            level: definition.level,
            maximum_hp: definition.max_hp.max(1),
            current_hp: definition.max_hp.max(1),
            stagger_threshold: definition.stagger_threshold,
            stagger_duration_ms: animation_duration_ms(definition, "hit1"),
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
            movement_resume_mode: None,
            dead_until_ms: None,
            respawn_delay_ms,
            player_attack_transaction: None,
        },
    ))
}

fn animation_duration_ms(
    definition: &MobDefinition,
    name: &str,
) -> u64 {
    definition
        .animations
        .iter()
        .find(|animation| animation.name == name)
        .map_or(0, |animation| {
            animation.frames.iter().fold(0_u64, |duration, frame| {
                duration.saturating_add(u64::from(frame.delay_ms.max(1)))
            })
        })
}

fn vertical_bounds(definition: &MobDefinition) -> VerticalBounds {
    frame_vertical_bounds(
        definition
            .animations
            .iter()
            .filter(|animation| matches!(animation.name.as_str(), "stand" | "move" | "fly"))
            .flat_map(|animation| &animation.frames),
    )
    .or_else(|| {
        frame_vertical_bounds(
            definition
                .animations
                .iter()
                .flat_map(|animation| &animation.frames),
        )
    })
    .unwrap_or(DEFAULT_TARGET_VERTICAL_BOUNDS)
}

fn frame_vertical_bounds<'a>(
    frames: impl IntoIterator<Item = &'a oozems_proto::v1::MobFrame>
) -> Option<VerticalBounds> {
    frames
        .into_iter()
        .filter_map(|frame| {
            let top = -frame.origin_y;
            let bottom = frame.height - frame.origin_y;
            (frame.height > 0.0 && top.is_finite() && bottom.is_finite() && top <= bottom)
                .then_some(VerticalBounds { top, bottom })
        })
        .reduce(|bounds, frame| VerticalBounds {
            top: bounds.top.min(frame.top),
            bottom: bounds.bottom.max(frame.bottom),
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

#[cfg(test)]
mod tests {
    use oozems_proto::v1::MobAnimation;
    use oozems_proto::v1::MobDefinition;
    use oozems_proto::v1::MobFrame;

    use super::animation_duration_ms;
    use super::vertical_bounds;
    use crate::attacks::VerticalBounds;

    #[test]
    fn movement_frames_define_the_fake_mob_vertical_bounds() {
        let definition = MobDefinition {
            animations: vec![
                MobAnimation {
                    name: "stand".to_owned(),
                    frames: vec![MobFrame {
                        height: 48.0,
                        origin_y: 42.0,
                        ..MobFrame::default()
                    }],
                },
                MobAnimation {
                    name: "attack1".to_owned(),
                    frames: vec![MobFrame {
                        height: 200.0,
                        origin_y: 100.0,
                        ..MobFrame::default()
                    }],
                },
            ],
            ..MobDefinition::default()
        };

        assert_eq!(
            vertical_bounds(&definition),
            VerticalBounds {
                top: -42.0,
                bottom: 6.0,
            }
        );
    }

    #[test]
    fn stagger_duration_uses_the_complete_hit_animation() {
        let definition = MobDefinition {
            animations: vec![MobAnimation {
                name: "hit1".to_owned(),
                frames: vec![
                    MobFrame {
                        delay_ms: 200,
                        ..MobFrame::default()
                    },
                    MobFrame {
                        delay_ms: 0,
                        ..MobFrame::default()
                    },
                ],
            }],
            ..MobDefinition::default()
        };

        assert_eq!(animation_duration_ms(&definition, "hit1"), 201);
        assert_eq!(animation_duration_ms(&definition, "die1"), 0);
    }
}
