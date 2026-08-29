use oozems_proto::v1::Map;
use oozems_proto::v1::Platform;

use super::Position;
use super::SubmittedMovement;

const PLAYER_HALF_WIDTH: f32 = 18.0;
const PORTAL_FLOOR_PENETRATION_LIMIT: f32 = 16.0;
const SPAWN_PORTAL_KIND: u32 = 0;

#[derive(Clone, Copy, Debug)]
struct VerticalWall {
    x: f32,
    top: f32,
    bottom: f32,
}

pub(super) fn supporting_platform(
    map: &Map,
    position: &Position,
    vertical_tolerance: f32,
    edge_tolerance: f32,
) -> bool {
    supporting_foothold(map, position, vertical_tolerance, edge_tolerance).is_some()
}

pub(super) fn supporting_foothold<'a>(
    map: &'a Map,
    position: &Position,
    vertical_tolerance: f32,
    edge_tolerance: f32,
) -> Option<&'a Platform> {
    map.platforms
        .iter()
        .filter_map(|platform| {
            let surface = platform_y_near_edge(platform, position.x, edge_tolerance)?;
            let distance = (surface - position.y).abs();
            (distance <= vertical_tolerance).then_some((platform, distance))
        })
        .min_by(|left, right| left.1.total_cmp(&right.1))
        .map(|(platform, _)| platform)
}

pub(super) fn platform_y_near_edge(
    platform: &Platform,
    x: f32,
    edge_tolerance: f32,
) -> Option<f32> {
    let minimum_x = platform.x.min(platform.end_x);
    let maximum_x = platform.x.max(platform.end_x);
    if x < minimum_x - edge_tolerance || x > maximum_x + edge_tolerance {
        return None;
    }
    platform_y(platform, x.clamp(minimum_x, maximum_x))
}

pub(super) fn platform_y(
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

pub(super) fn movement_crosses_vertical_foothold(
    map: &Map,
    origin: Position,
    origin_layer: i32,
    submitted: SubmittedMovement,
    contact_layer: i32,
) -> bool {
    let first_destination = submitted
        .support_contact
        .map_or(submitted.position, |contact| contact.position);
    if segment_crosses_vertical_foothold(map, origin, first_destination, origin_layer) {
        return true;
    }
    submitted.support_contact.is_some()
        && segment_crosses_vertical_foothold(
            map,
            first_destination,
            submitted.position,
            contact_layer,
        )
}

fn segment_crosses_vertical_foothold(
    map: &Map,
    origin: Position,
    destination: Position,
    platform_layer: i32,
) -> bool {
    map.platforms
        .iter()
        .filter(|platform| platform.layer == platform_layer)
        .filter_map(vertical_wall)
        .filter(|wall| {
            vertical_wall_blocks(*wall, origin.y) && vertical_wall_blocks(*wall, destination.y)
        })
        .any(|wall| horizontal_segment_crosses_wall(wall, origin.x, destination.x))
}

fn horizontal_segment_crosses_wall(
    wall: VerticalWall,
    origin_x: f32,
    destination_x: f32,
) -> bool {
    if destination_x > origin_x {
        return origin_x < wall.x && destination_x > wall.x - PLAYER_HALF_WIDTH;
    }
    if destination_x < origin_x {
        return origin_x > wall.x && destination_x < wall.x + PLAYER_HALF_WIDTH;
    }
    false
}

fn vertical_wall(platform: &Platform) -> Option<VerticalWall> {
    if (platform.end_x - platform.x).abs() >= f32::EPSILON {
        return None;
    }
    let top = platform.y.min(platform.end_y);
    let bottom = platform.y.max(platform.end_y);
    (top < bottom).then_some(VerticalWall {
        x: platform.x,
        top,
        bottom,
    })
}

fn vertical_wall_blocks(
    wall: VerticalWall,
    y: f32,
) -> bool {
    y > wall.top && y <= wall.bottom
}

pub(super) fn within_map(
    map: &Map,
    position: &Position,
) -> bool {
    let (left, right) = horizontal_movement_bounds(map);
    position.x >= left
        && position.x <= right
        && position.y >= 0.0
        && position.y <= map.height as f32
}

pub(super) fn clamp_to_movement_bounds(
    map: &Map,
    mut position: Position,
) -> Position {
    let (left, right) = horizontal_movement_bounds(map);
    position.x = position.x.clamp(left, right);
    position.y = position.y.clamp(0.0, map.height as f32);
    position
}

fn horizontal_movement_bounds(map: &Map) -> (f32, f32) {
    let map_right = map.width as f32;
    map.movement_bounds
        .as_ref()
        .filter(|bounds| {
            bounds.left.is_finite()
                && bounds.right.is_finite()
                && bounds.left >= 0.0
                && bounds.right <= map_right
                && bounds.left <= bounds.right
        })
        .map_or((0.0, map_right), |bounds| (bounds.left, bounds.right))
}

pub(super) fn default_spawn_position(map: &Map) -> Option<Position> {
    let target = map
        .portals
        .iter()
        .find(|portal| portal.kind == SPAWN_PORTAL_KIND)?;
    Some(portal_position(map, target.x, target.y))
}

pub(super) fn named_portal_position(
    map: &Map,
    name: &str,
) -> Option<Position> {
    let target = map.portals.iter().find(|portal| portal.name == name)?;
    Some(portal_position(map, target.x, target.y))
}

fn portal_position(
    map: &Map,
    x: f32,
    y: f32,
) -> Position {
    let floor = map
        .platforms
        .iter()
        .filter_map(|platform| platform_y(platform, x))
        .filter(|surface| {
            let penetration = y - surface;
            (0.0..=PORTAL_FLOOR_PENETRATION_LIMIT).contains(&penetration)
        })
        .max_by(f32::total_cmp);
    clamp_to_movement_bounds(
        map,
        Position {
            x,
            y: floor.unwrap_or(y),
        },
    )
}
