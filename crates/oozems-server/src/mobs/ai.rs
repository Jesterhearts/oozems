use oozems_proto::v1::MobMovementMode;
use oozems_proto::v1::Platform;
use shipyard::IntoIter;
use shipyard::UniqueView;
use shipyard::ViewMut;

use super::components::MobCombat;
use super::components::MobMotion;
use super::components::Position;
use super::components::TargetCache;
use super::components::Terrain;
use super::components::Tick;

const MOB_GRAVITY: f32 = 900.0;
const MOB_JUMP_SPEED: f32 = 330.0;
const MAX_JUMP_RISE: f32 = MOB_JUMP_SPEED * MOB_JUMP_SPEED / (2.0 * MOB_GRAVITY);
const JUMP_LEDGE_LOOKAHEAD: f32 = 48.0;
const WALKABLE_HEIGHT_CHANGE: f32 = 12.0;
const PHYSICS_STEP_SECONDS: f32 = 1.0 / 60.0;
const MIN_DECISION_SECONDS: f32 = 0.75;
const DECISION_RANGE_SECONDS: f32 = 1.75;
const AGGRO_STOP_DISTANCE: f32 = 12.0;

pub(super) fn advance_mobs(
    mut positions: ViewMut<Position>,
    mut motions: ViewMut<MobMotion>,
    combats: ViewMut<MobCombat>,
    terrain: UniqueView<Terrain>,
    targets: UniqueView<TargetCache>,
    tick: UniqueView<Tick>,
) {
    let mut remaining = tick.elapsed_seconds.max(0.0);
    while remaining > 0.0 {
        let step = remaining.min(PHYSICS_STEP_SECONDS);
        for (mut position, motion, combat) in (&mut positions, &mut motions, &combats).iter() {
            advance_mob(
                &terrain,
                &targets,
                &mut position,
                motion,
                combat,
                tick.now_ms,
                step,
            );
        }
        remaining -= step;
    }
}

fn advance_mob(
    terrain: &Terrain,
    targets: &TargetCache,
    position: &mut Position,
    motion: &mut MobMotion,
    combat: &MobCombat,
    now_ms: u64,
    elapsed_seconds: f32,
) {
    if combat.current_hp == 0 {
        motion.mode = MobMovementMode::Idle;
        return;
    }
    if now_ms < combat.movement_resume_ms {
        motion.mode = MobMovementMode::Idle;
        return;
    }
    if combat.attack_until_ms > now_ms {
        motion.mode = MobMovementMode::Attacking;
        return;
    }
    if motion.mode == MobMovementMode::Jumping {
        advance_jump(terrain, position, motion, elapsed_seconds);
        return;
    }
    if let Some(target) = combat
        .aggro_target
        .as_deref()
        .and_then(|target_id| targets.0.iter().find(|target| target.id == target_id))
    {
        choose_aggro_behavior(position, motion, target.position);
    } else {
        motion.decision_seconds -= elapsed_seconds;
        if motion.decision_seconds <= 0.0 {
            choose_random_behavior(motion);
        }
    }
    if motion.flies {
        advance_flying(position, motion, elapsed_seconds);
    } else {
        advance_grounded(terrain, position, motion, elapsed_seconds);
    }
}

fn choose_aggro_behavior(
    position: &Position,
    motion: &mut MobMotion,
    target: Position,
) {
    let delta_x = target.x - position.x;
    let direction = if !motion.can_move || delta_x.abs() <= AGGRO_STOP_DISTANCE {
        0
    } else if delta_x < 0.0 {
        -1
    } else {
        1
    };
    set_direction(motion, direction);
    motion.mode = if direction == 0 {
        MobMovementMode::Idle
    } else {
        MobMovementMode::Walking
    };
}

fn choose_random_behavior(motion: &mut MobMotion) {
    let choice = next_random(&mut motion.random_state) % 5;
    let direction = if !motion.can_move || motion.move_speed <= 0.0 {
        0
    } else {
        match choice {
            0 => 0,
            1 | 2 => -1,
            _ => 1,
        }
    };
    set_direction(motion, direction);
    motion.mode = if direction == 0 {
        MobMovementMode::Idle
    } else {
        MobMovementMode::Walking
    };
    let duration_fraction = next_random(&mut motion.random_state) as f64 / u64::MAX as f64;
    motion.decision_seconds =
        MIN_DECISION_SECONDS + duration_fraction as f32 * DECISION_RANGE_SECONDS;
}

fn advance_flying(
    position: &mut Position,
    motion: &mut MobMotion,
    elapsed_seconds: f32,
) {
    let proposed_x = position.x + f32::from(motion.direction) * motion.move_speed * elapsed_seconds;
    position.x = proposed_x.clamp(motion.roam_left, motion.roam_right);
    if proposed_x != position.x {
        reverse_direction(motion);
    }
}

fn advance_grounded(
    terrain: &Terrain,
    position: &mut Position,
    motion: &mut MobMotion,
    elapsed_seconds: f32,
) {
    let Some(support) = valid_support(terrain, motion.support, *position).or_else(|| {
        nearest_platform(
            &terrain.platforms,
            position.x,
            position.y,
            Some(position.layer),
        )
    }) else {
        set_direction(motion, 0);
        motion.mode = MobMovementMode::Idle;
        return;
    };
    motion.support = Some(support);
    if motion.direction == 0 {
        snap_to_platform(terrain, position, motion, support, position.x);
        return;
    }

    let proposed_x = (position.x
        + f32::from(motion.direction) * motion.move_speed * elapsed_seconds)
        .clamp(motion.roam_left, motion.roam_right);
    if let Some(next_support) = walkable_support(terrain, position, motion, support, proposed_x) {
        snap_to_platform(terrain, position, motion, next_support, proposed_x);
        if proposed_x == motion.roam_left || proposed_x == motion.roam_right {
            reverse_direction(motion);
        }
        return;
    }
    if motion.can_jump && has_reachable_ledge(terrain, position, motion) {
        motion.velocity_y = -MOB_JUMP_SPEED;
        motion.mode = MobMovementMode::Jumping;
        advance_jump(terrain, position, motion, elapsed_seconds);
        return;
    }
    stop_at_platform_edge(terrain, position, motion, support);
}

fn valid_support(
    terrain: &Terrain,
    support: Option<usize>,
    position: Position,
) -> Option<usize> {
    let support = support?;
    platform_surface_at_x(terrain.platforms.get(support)?, position.x).map(|_| support)
}

fn walkable_support(
    terrain: &Terrain,
    position: &Position,
    motion: &MobMotion,
    current_support: usize,
    x: f32,
) -> Option<usize> {
    let current = terrain.platforms.get(current_support)?;
    if platform_surface_at_x(current, x).is_some() {
        return Some(current_support);
    }
    nearest_platform(&terrain.platforms, x, position.y, Some(current.layer)).filter(|index| {
        platform_surface_at_x(&terrain.platforms[*index], x)
            .is_some_and(|surface| (surface - position.y).abs() <= WALKABLE_HEIGHT_CHANGE)
            && x >= motion.roam_left
            && x <= motion.roam_right
    })
}

fn has_reachable_ledge(
    terrain: &Terrain,
    position: &Position,
    motion: &MobMotion,
) -> bool {
    let direction = f32::from(motion.direction);
    terrain.platforms.iter().any(|platform| {
        let edge_x = if direction > 0.0 {
            platform.x.min(platform.end_x)
        } else {
            platform.x.max(platform.end_x)
        };
        let distance = (edge_x - position.x) * direction;
        if !(0.0..=JUMP_LEDGE_LOOKAHEAD).contains(&distance)
            || edge_x < motion.roam_left
            || edge_x > motion.roam_right
        {
            return false;
        }
        let sample_x = (edge_x + direction).clamp(motion.roam_left, motion.roam_right);
        platform_surface_at_x(platform, sample_x).is_some_and(|surface| {
            let rise = position.y - surface;
            rise > WALKABLE_HEIGHT_CHANGE && rise <= MAX_JUMP_RISE
        })
    })
}

fn stop_at_platform_edge(
    terrain: &Terrain,
    position: &mut Position,
    motion: &mut MobMotion,
    support: usize,
) {
    let Some(platform) = terrain.platforms.get(support) else {
        return;
    };
    let edge_x = if motion.direction > 0 {
        platform.x.max(platform.end_x)
    } else {
        platform.x.min(platform.end_x)
    }
    .clamp(motion.roam_left, motion.roam_right);
    snap_to_platform(terrain, position, motion, support, edge_x);
    reverse_direction(motion);
}

fn snap_to_platform(
    terrain: &Terrain,
    position: &mut Position,
    motion: &mut MobMotion,
    support: usize,
    x: f32,
) {
    let Some(platform) = terrain.platforms.get(support) else {
        return;
    };
    let Some(y) = platform_surface_at_x(platform, x) else {
        return;
    };
    position.x = x;
    position.y = y;
    position.layer = platform.layer;
    motion.support = Some(support);
    motion.velocity_y = 0.0;
    motion.mode = if motion.direction == 0 {
        MobMovementMode::Idle
    } else {
        MobMovementMode::Walking
    };
}

fn advance_jump(
    terrain: &Terrain,
    position: &mut Position,
    motion: &mut MobMotion,
    elapsed_seconds: f32,
) {
    let next_velocity_y = motion.velocity_y + MOB_GRAVITY * elapsed_seconds;
    let next_x = (position.x + f32::from(motion.direction) * motion.move_speed * elapsed_seconds)
        .clamp(motion.roam_left, motion.roam_right);
    let next_y = position.y + (motion.velocity_y + next_velocity_y) * 0.5 * elapsed_seconds;
    if next_velocity_y >= 0.0
        && let Some(support) = landing_support(terrain, position.y, next_x, next_y)
    {
        snap_to_platform(terrain, position, motion, support, next_x);
        return;
    }
    position.x = next_x;
    position.y = next_y;
    motion.velocity_y = next_velocity_y;
    if (next_x == motion.roam_left && motion.direction < 0)
        || (next_x == motion.roam_right && motion.direction > 0)
    {
        reverse_direction(motion);
    }
    if next_y > terrain.height + 100.0 {
        reset_mob(position, motion);
    }
}

fn landing_support(
    terrain: &Terrain,
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

pub(super) fn reset_mob(
    position: &mut Position,
    motion: &mut MobMotion,
) {
    *position = motion.spawn_position;
    motion.support = motion.spawn_support;
    motion.velocity_y = 0.0;
    motion.decision_seconds = 0.0;
    set_direction(motion, 0);
    motion.mode = MobMovementMode::Idle;
}

pub(super) fn nearest_platform(
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

pub(super) fn set_direction(
    motion: &mut MobMotion,
    direction: i8,
) {
    motion.direction = direction.clamp(-1, 1);
    if direction != 0 {
        // Classic WZ mob frames face left before the map flip flag is applied.
        motion.flip_x = direction > 0;
    }
}

fn reverse_direction(motion: &mut MobMotion) {
    let direction = -motion.direction;
    set_direction(motion, direction);
    motion.decision_seconds = motion.decision_seconds.max(MIN_DECISION_SECONDS / 2.0);
}

pub(super) fn next_random(state: &mut u64) -> u64 {
    let mut value = *state;
    value ^= value << 13;
    value ^= value >> 7;
    value ^= value << 17;
    *state = value;
    value
}
