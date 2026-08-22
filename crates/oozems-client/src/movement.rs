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
const SCRIPT_PORTAL_TARGET: u32 = 999_999_999;
const SPAWN_PORTAL_KIND: u32 = 0;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MotionState {
    pub velocity_y: f32,
    pub on_ground: bool,
    pub climbing: Option<usize>,
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
    if let Some(landing_y) = find_landing_platform(map, old_x, position.x, old_y, proposed_y) {
        position.y = landing_y;
        state.velocity_y = 0.0;
        state.on_ground = true;
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
            let (exit_y, on_ground) = upper_floor_exit(map, ladder);
            position.y = exit_y;
            state.climbing = None;
            state.on_ground = on_ground;
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

fn upper_floor_exit(
    map: &Map,
    ladder: &Ladder,
) -> (f32, bool) {
    let exit_y = ladder.top - LADDER_TOP_EXIT_OFFSET;
    let platform_y = map
        .platforms
        .iter()
        .filter_map(|platform| platform_surface_near_ladder(platform, ladder.x))
        .filter(|surface| {
            *surface <= ladder.top && (*surface - exit_y).abs() <= LADDER_TOP_PLATFORM_REACH
        })
        .min_by(|left, right| (left - exit_y).abs().total_cmp(&(right - exit_y).abs()));
    platform_y.map_or((exit_y, false), |surface| (surface, true))
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
    Some(Vec2 {
        x: target.x,
        y: target.y,
    })
}

fn find_landing_platform(
    map: &Map,
    old_x: f32,
    new_x: f32,
    old_y: f32,
    new_y: f32,
) -> Option<f32> {
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
                Some(new_surface)
            } else {
                None
            }
        })
        .min_by(f32::total_cmp)
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
}
