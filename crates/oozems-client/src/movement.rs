use oozems_proto::v1::Ladder;
use oozems_proto::v1::Map;
use oozems_proto::v1::Platform;
use oozems_proto::v1::Portal;
use oozems_proto::v1::Vec2;

const MOVE_SPEED: f32 = 220.0;
const CLIMB_SPEED: f32 = 135.0;
const GRAVITY: f32 = 1_150.0;
const JUMP_SPEED: f32 = 480.0;
const PLAYER_HALF_WIDTH: f32 = 18.0;
const LADDER_REACH: f32 = 24.0;
const LADDER_END_REACH: f32 = 14.0;
const LADDER_TOP_EXIT_OFFSET: f32 = 5.0;
const LADDER_TOP_PLATFORM_REACH: f32 = 24.0;
const PORTAL_HORIZONTAL_REACH: f32 = 48.0;
const PORTAL_VERTICAL_REACH: f32 = 64.0;
const PORTAL_FLOOR_PENETRATION_LIMIT: f32 = 16.0;
const PLATFORM_CONTACT_TOLERANCE: f32 = 1.0;
const SCRIPT_PORTAL_TARGET: u32 = 999_999_999;
const SPAWN_PORTAL_KIND: u32 = 0;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MotionState {
    pub velocity_y: f32,
    pub on_ground: bool,
    pub climbing: Option<usize>,
    pub platform_layer: i32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PlayerInput {
    pub horizontal: f32,
    pub vertical: f32,
    pub jump_pressed: bool,
    pub portal_pressed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MapTransition {
    pub target_map_id: u32,
    pub target_portal_name: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MotionOutput {
    pub position: Vec2,
    pub state: MotionState,
    pub transition: Option<MapTransition>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct GroundContact {
    y: f32,
    layer: i32,
}

pub fn initial_motion_state(
    map: &Map,
    position: &Vec2,
) -> MotionState {
    supporting_platform(map, position).map_or_else(MotionState::default, |contact| MotionState {
        on_ground: true,
        platform_layer: contact.layer,
        ..MotionState::default()
    })
}

pub fn update_player(
    map: &Map,
    position: Vec2,
    state: MotionState,
    input: PlayerInput,
    elapsed_seconds: f32,
) -> MotionOutput {
    if input.portal_pressed
        && let Some(portal) = find_usable_portal(&map.portals, &position)
    {
        return MotionOutput {
            position,
            state,
            transition: Some(MapTransition {
                target_map_id: portal.target_map_id,
                target_portal_name: portal.target_name.clone(),
            }),
        };
    }

    let mut position = position;
    let mut state = state;
    if state.climbing.is_none()
        && input.vertical != 0.0
        && let Some(index) = find_climbable_ladder(&map.ladders, &position, input.vertical)
    {
        state.climbing = Some(index);
        state.velocity_y = 0.0;
        state.on_ground = false;
        state.platform_layer = map.ladders[index].layer;
        position.x = map.ladders[index].x;
    }

    if let Some(index) = state.climbing {
        let Some(ladder) = map.ladders.get(index) else {
            state.climbing = None;
            return move_with_gravity(map, position, state, input, elapsed_seconds);
        };
        if input.jump_pressed {
            state.climbing = None;
            state.velocity_y = -JUMP_SPEED;
            return move_with_gravity(map, position, state, input, elapsed_seconds);
        }
        return move_on_ladder(
            map,
            position,
            state,
            input.vertical,
            ladder,
            elapsed_seconds,
        );
    }

    if input.jump_pressed && state.on_ground {
        state.velocity_y = -JUMP_SPEED;
        state.on_ground = false;
    }
    move_with_gravity(map, position, state, input, elapsed_seconds)
}

fn move_with_gravity(
    map: &Map,
    mut position: Vec2,
    mut state: MotionState,
    input: PlayerInput,
    elapsed_seconds: f32,
) -> MotionOutput {
    let old_x = position.x;
    let old_y = position.y;
    let edge = PLAYER_HALF_WIDTH.min(map.width as f32 / 2.0);
    position.x = (position.x + input.horizontal * MOVE_SPEED * elapsed_seconds)
        .clamp(edge, (map.width as f32 - edge).max(edge));

    state.velocity_y += GRAVITY * elapsed_seconds;
    let proposed_y = position.y + state.velocity_y * elapsed_seconds;
    if let Some(contact) = find_landing_platform(map, old_x, position.x, old_y, proposed_y) {
        position.y = contact.y;
        state.velocity_y = 0.0;
        state.on_ground = true;
        state.platform_layer = contact.layer;
    } else {
        position.y = proposed_y.min(map.height as f32);
        state.on_ground = false;
    }

    MotionOutput {
        position,
        state,
        transition: None,
    }
}

fn move_on_ladder(
    map: &Map,
    mut position: Vec2,
    mut state: MotionState,
    vertical: f32,
    ladder: &Ladder,
    elapsed_seconds: f32,
) -> MotionOutput {
    position.x = ladder.x;
    position.y += vertical * CLIMB_SPEED * elapsed_seconds;

    if vertical < 0.0 && position.y <= ladder.top {
        if ladder.upper_floor {
            let exit_y = ladder.top - LADDER_TOP_EXIT_OFFSET;
            if let Some(contact) = upper_floor_contact(map, ladder, exit_y) {
                position.y = contact.y;
                state.on_ground = true;
                state.platform_layer = contact.layer;
            } else {
                position.y = exit_y;
                state.on_ground = false;
            }
            state.climbing = None;
        } else {
            position.y = ladder.top;
        }
    } else if vertical > 0.0 && position.y >= ladder.bottom {
        position.y = ladder.bottom;
        state.climbing = None;
        state.on_ground = false;
    } else {
        position.y = position.y.clamp(ladder.top, ladder.bottom);
    }
    state.velocity_y = 0.0;

    MotionOutput {
        position,
        state,
        transition: None,
    }
}

fn upper_floor_contact(
    map: &Map,
    ladder: &Ladder,
    exit_y: f32,
) -> Option<GroundContact> {
    map.platforms
        .iter()
        .filter_map(|platform| {
            let y = platform_surface_near_ladder(platform, ladder.x)?;
            Some(GroundContact {
                y,
                layer: platform.layer,
            })
        })
        .filter(|contact| {
            contact.y <= ladder.top && (contact.y - exit_y).abs() <= LADDER_TOP_PLATFORM_REACH
        })
        .min_by(|left, right| (left.y - exit_y).abs().total_cmp(&(right.y - exit_y).abs()))
}

fn platform_surface_near_ladder(
    platform: &Platform,
    ladder_x: f32,
) -> Option<f32> {
    let minimum_x = platform.x.min(platform.end_x);
    let maximum_x = platform.x.max(platform.end_x);
    if ladder_x < minimum_x - PLAYER_HALF_WIDTH || ladder_x > maximum_x + PLAYER_HALF_WIDTH {
        return None;
    }
    platform_y(platform, ladder_x.clamp(minimum_x, maximum_x))
}

fn find_climbable_ladder(
    ladders: &[Ladder],
    position: &Vec2,
    vertical: f32,
) -> Option<usize> {
    ladders
        .iter()
        .enumerate()
        .filter(|(_, ladder)| {
            let within_vertical_reach = if vertical < 0.0 {
                position.y >= ladder.top && position.y <= ladder.bottom + LADDER_END_REACH
            } else {
                position.y >= ladder.top - LADDER_END_REACH && position.y <= ladder.bottom
            };
            (position.x - ladder.x).abs() <= LADDER_REACH && within_vertical_reach
        })
        .min_by(|(_, left), (_, right)| {
            (position.x - left.x)
                .abs()
                .total_cmp(&(position.x - right.x).abs())
        })
        .map(|(index, _)| index)
}

fn find_usable_portal<'a>(
    portals: &'a [Portal],
    position: &Vec2,
) -> Option<&'a Portal> {
    portals
        .iter()
        .filter(|portal| portal.target_map_id != SCRIPT_PORTAL_TARGET)
        .filter(|portal| {
            (position.x - portal.x).abs() <= PORTAL_HORIZONTAL_REACH
                && (position.y - portal.y).abs() <= PORTAL_VERTICAL_REACH
        })
        .min_by(|left, right| {
            squared_distance(position, left).total_cmp(&squared_distance(position, right))
        })
}

fn squared_distance(
    position: &Vec2,
    portal: &Portal,
) -> f32 {
    (position.x - portal.x).powi(2) + (position.y - portal.y).powi(2)
}

pub fn destination_position(
    map: &Map,
    target_portal_name: &str,
) -> Option<Vec2> {
    let target = (!target_portal_name.is_empty())
        .then(|| {
            map.portals
                .iter()
                .find(|portal| portal.name == target_portal_name)
        })
        .flatten()
        .or_else(|| {
            map.portals
                .iter()
                .find(|portal| portal.kind == SPAWN_PORTAL_KIND)
        })?;
    Some(place_portal_on_floor(map, target))
}

fn place_portal_on_floor(
    map: &Map,
    portal: &Portal,
) -> Vec2 {
    let floor = map
        .platforms
        .iter()
        .filter_map(|platform| platform_surface_at_x(platform, portal.x))
        .filter(|surface| {
            let penetration = portal.y - surface;
            (0.0..=PORTAL_FLOOR_PENETRATION_LIMIT).contains(&penetration)
        })
        .max_by(f32::total_cmp);

    Vec2 {
        x: portal.x,
        y: floor.unwrap_or(portal.y),
    }
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
    platform_y(platform, x)
}

fn find_landing_platform(
    map: &Map,
    old_x: f32,
    new_x: f32,
    old_y: f32,
    new_y: f32,
) -> Option<GroundContact> {
    if new_y < old_y {
        return None;
    }

    map.platforms
        .iter()
        .filter_map(|platform| {
            let minimum_x = platform.x.min(platform.end_x);
            let maximum_x = platform.x.max(platform.end_x);
            if new_x < minimum_x - 16.0 || new_x > maximum_x + 16.0 {
                return None;
            }
            let old_surface = platform_y(platform, old_x.clamp(minimum_x, maximum_x))?;
            let new_surface = platform_y(platform, new_x.clamp(minimum_x, maximum_x))?;
            if old_y <= old_surface + 1.0 && new_y >= new_surface {
                Some(GroundContact {
                    y: new_surface,
                    layer: platform.layer,
                })
            } else {
                None
            }
        })
        .min_by(|left, right| left.y.total_cmp(&right.y))
}

fn supporting_platform(
    map: &Map,
    position: &Vec2,
) -> Option<GroundContact> {
    map.platforms
        .iter()
        .filter_map(|platform| {
            let y = platform_surface_at_x(platform, position.x)?;
            ((y - position.y).abs() <= PLATFORM_CONTACT_TOLERANCE).then_some(GroundContact {
                y,
                layer: platform.layer,
            })
        })
        .min_by(|left, right| {
            (left.y - position.y)
                .abs()
                .total_cmp(&(right.y - position.y).abs())
        })
}

fn platform_y(
    platform: &Platform,
    x: f32,
) -> Option<f32> {
    let delta_x = platform.end_x - platform.x;
    if delta_x.abs() < f32::EPSILON {
        return None;
    }
    let progress = (x - platform.x) / delta_x;
    Some(platform.y + progress * (platform.end_y - platform.y))
}

#[cfg(test)]
mod tests {
    use oozems_proto::v1::Ladder;
    use oozems_proto::v1::Map;
    use oozems_proto::v1::Platform;
    use oozems_proto::v1::Portal;
    use oozems_proto::v1::Vec2;

    use super::MapTransition;
    use super::MotionState;
    use super::PlayerInput;
    use super::destination_position;
    use super::initial_motion_state;
    use super::update_player;

    #[test]
    fn player_lands_on_a_sloped_foothold() {
        let map = Map {
            platforms: vec![Platform {
                x: 100.0,
                y: 300.0,
                width: 100.0,
                end_x: 200.0,
                end_y: 250.0,
                layer: 2,
                ..Platform::default()
            }],
            width: 800,
            height: 600,
            ..Map::default()
        };
        let output = update_player(
            &map,
            Vec2 { x: 140.0, y: 280.0 },
            MotionState {
                velocity_y: 200.0,
                ..MotionState::default()
            },
            PlayerInput {
                horizontal: 1.0,
                ..PlayerInput::default()
            },
            10.0 / 220.0,
        );

        assert_eq!(output.position.y, 275.0);
        assert!(output.state.on_ground);
        assert_eq!(output.state.platform_layer, 2);
    }

    #[test]
    fn player_keeps_the_platform_layer_while_jumping() {
        let map = Map {
            width: 800,
            height: 600,
            ..Map::default()
        };
        let output = update_player(
            &map,
            Vec2 { x: 140.0, y: 280.0 },
            MotionState {
                on_ground: true,
                platform_layer: 3,
                ..MotionState::default()
            },
            PlayerInput {
                jump_pressed: true,
                ..PlayerInput::default()
            },
            0.016,
        );

        assert!(!output.state.on_ground);
        assert_eq!(output.state.platform_layer, 3);
    }

    #[test]
    fn initial_state_uses_the_supporting_platform_layer() {
        let map = Map {
            platforms: vec![Platform {
                x: 100.0,
                y: 300.0,
                end_x: 200.0,
                end_y: 300.0,
                layer: 2,
                ..Platform::default()
            }],
            ..Map::default()
        };

        let state = initial_motion_state(&map, &Vec2 { x: 150.0, y: 300.0 });

        assert!(state.on_ground);
        assert_eq!(state.platform_layer, 2);
    }

    #[test]
    fn vertical_input_attaches_to_and_climbs_a_ladder() {
        let map = Map {
            width: 800,
            height: 600,
            ladders: vec![Ladder {
                x: 200.0,
                top: 100.0,
                bottom: 300.0,
                upper_floor: true,
                layer: 2,
                ..Ladder::default()
            }],
            ..Map::default()
        };
        let output = update_player(
            &map,
            Vec2 { x: 215.0, y: 280.0 },
            MotionState::default(),
            PlayerInput {
                vertical: -1.0,
                ..PlayerInput::default()
            },
            0.1,
        );

        assert_eq!(output.position, Vec2 { x: 200.0, y: 266.5 });
        assert_eq!(output.state.climbing, Some(0));
        assert_eq!(output.state.velocity_y, 0.0);
        assert_eq!(output.state.platform_layer, 2);
    }

    #[test]
    fn climbing_reaches_the_upper_floor_without_gravity() {
        let map = Map {
            width: 800,
            height: 600,
            platforms: vec![Platform {
                x: 150.0,
                y: 95.0,
                end_x: 250.0,
                end_y: 95.0,
                layer: 3,
                ..Platform::default()
            }],
            ladders: vec![Ladder {
                x: 200.0,
                top: 100.0,
                bottom: 300.0,
                upper_floor: true,
                ..Ladder::default()
            }],
            ..Map::default()
        };
        let output = update_player(
            &map,
            Vec2 { x: 200.0, y: 105.0 },
            MotionState {
                climbing: Some(0),
                ..MotionState::default()
            },
            PlayerInput {
                vertical: -1.0,
                ..PlayerInput::default()
            },
            0.1,
        );

        assert_eq!(output.position, Vec2 { x: 200.0, y: 95.0 });
        assert_eq!(output.state.climbing, None);
        assert!(output.state.on_ground);
        assert_eq!(output.state.platform_layer, 3);
        assert_eq!(output.state.velocity_y, 0.0);

        let settled = update_player(
            &map,
            output.position,
            output.state,
            PlayerInput {
                vertical: -1.0,
                ..PlayerInput::default()
            },
            0.016,
        );
        assert_eq!(settled.position, Vec2 { x: 200.0, y: 95.0 });
        assert_eq!(settled.state.climbing, None);
        assert!(settled.state.on_ground);
    }

    #[test]
    fn upper_exit_moves_above_the_ladder_when_no_platform_is_nearby() {
        let map = Map {
            width: 800,
            height: 600,
            ladders: vec![Ladder {
                x: 200.0,
                top: 100.0,
                bottom: 300.0,
                upper_floor: true,
                ..Ladder::default()
            }],
            ..Map::default()
        };
        let output = update_player(
            &map,
            Vec2 { x: 200.0, y: 101.0 },
            MotionState {
                climbing: Some(0),
                ..MotionState::default()
            },
            PlayerInput {
                vertical: -1.0,
                ..PlayerInput::default()
            },
            0.1,
        );

        assert_eq!(output.position, Vec2 { x: 200.0, y: 95.0 });
        assert_eq!(output.state.climbing, None);
        assert!(!output.state.on_ground);
    }

    #[test]
    fn up_at_a_direct_portal_requests_a_transition() {
        let map = Map {
            portals: vec![Portal {
                name: "out".to_owned(),
                x: 200.0,
                y: 300.0,
                target_map_id: 100_000_001,
                target_name: "in".to_owned(),
                ..Portal::default()
            }],
            ..Map::default()
        };
        let output = update_player(
            &map,
            Vec2 { x: 210.0, y: 300.0 },
            MotionState::default(),
            PlayerInput {
                portal_pressed: true,
                ..PlayerInput::default()
            },
            0.0,
        );

        assert_eq!(
            output.transition,
            Some(MapTransition {
                target_map_id: 100_000_001,
                target_portal_name: "in".to_owned(),
            })
        );
    }

    #[test]
    fn script_portals_do_not_transition_without_server_scripts() {
        let map = Map {
            portals: vec![Portal {
                x: 200.0,
                y: 300.0,
                target_map_id: 999_999_999,
                ..Portal::default()
            }],
            width: 800,
            height: 600,
            ..Map::default()
        };
        let output = update_player(
            &map,
            Vec2 { x: 200.0, y: 300.0 },
            MotionState::default(),
            PlayerInput {
                portal_pressed: true,
                ..PlayerInput::default()
            },
            0.0,
        );

        assert_eq!(output.transition, None);
    }

    #[test]
    fn destination_uses_the_named_portal_then_falls_back_to_spawn() {
        let map = Map {
            portals: vec![
                Portal {
                    name: "sp".to_owned(),
                    x: 10.0,
                    y: 20.0,
                    kind: 0,
                    ..Portal::default()
                },
                Portal {
                    name: "west".to_owned(),
                    x: 30.0,
                    y: 40.0,
                    kind: 2,
                    ..Portal::default()
                },
            ],
            ..Map::default()
        };

        assert_eq!(
            destination_position(&map, "west"),
            Some(Vec2 { x: 30.0, y: 40.0 })
        );
        assert_eq!(
            destination_position(&map, "missing"),
            Some(Vec2 { x: 10.0, y: 20.0 })
        );
    }

    #[test]
    fn destination_snaps_shallow_penetration_without_moving_air_portals() {
        let map = Map {
            platforms: vec![Platform {
                x: 100.0,
                y: 300.0,
                end_x: 200.0,
                end_y: 350.0,
                ..Platform::default()
            }],
            portals: vec![
                Portal {
                    name: "inside".to_owned(),
                    x: 150.0,
                    y: 327.0,
                    ..Portal::default()
                },
                Portal {
                    name: "air".to_owned(),
                    x: 150.0,
                    y: 323.0,
                    ..Portal::default()
                },
                Portal {
                    name: "deep".to_owned(),
                    x: 150.0,
                    y: 350.0,
                    ..Portal::default()
                },
            ],
            ..Map::default()
        };

        assert_eq!(
            destination_position(&map, "inside"),
            Some(Vec2 { x: 150.0, y: 325.0 })
        );
        assert_eq!(
            destination_position(&map, "air"),
            Some(Vec2 { x: 150.0, y: 323.0 })
        );
        assert_eq!(
            destination_position(&map, "deep"),
            Some(Vec2 { x: 150.0, y: 350.0 })
        );
    }
}
