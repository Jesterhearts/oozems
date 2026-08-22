use oozems_proto::v1::Ladder;
use oozems_proto::v1::Map;
use oozems_proto::v1::MovementMode;
use oozems_proto::v1::MovementRules;
use oozems_proto::v1::Platform;
use oozems_proto::v1::Portal;
use oozems_proto::v1::Vec2;

const PLAYER_HALF_WIDTH: f32 = 18.0;
const LADDER_TOP_EXIT_OFFSET: f32 = 5.0;
const LADDER_TOP_PLATFORM_REACH: f32 = 24.0;
const PLATFORM_CONTACT_TOLERANCE: f32 = 1.0;
const SCRIPT_PORTAL_TARGET: u32 = 999_999_999;

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
    pub speed_bonus: i32,
    pub jump_bonus: i32,
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

pub fn validate_rules(rules: &MovementRules) -> Result<(), String> {
    for (name, value) in [
        ("walk_speed", rules.walk_speed),
        ("climb_speed", rules.climb_speed),
        ("gravity", rules.gravity),
        ("jump_speed", rules.jump_speed),
        ("position_tolerance", rules.position_tolerance),
        ("ground_tolerance", rules.ground_tolerance),
        ("platform_edge_tolerance", rules.platform_edge_tolerance),
        ("ladder_reach", rules.ladder_reach),
        ("ladder_end_reach", rules.ladder_end_reach),
        ("portal_horizontal_reach", rules.portal_horizontal_reach),
        ("portal_vertical_reach", rules.portal_vertical_reach),
    ] {
        if !value.is_finite() || value <= 0.0 {
            return Err(format!("movement rule {name} must be finite and positive"));
        }
    }
    if rules.speed_cap == 0 || rules.jump_cap == 0 || rules.snapshot_interval_ms == 0 {
        return Err("movement stat caps and snapshot interval must be positive".to_owned());
    }
    if rules.maximum_snapshot_gap_ms < rules.snapshot_interval_ms {
        return Err("maximum movement snapshot gap is shorter than its interval".to_owned());
    }
    Ok(())
}

pub fn motion_mode(state: MotionState) -> MovementMode {
    if state.climbing.is_some() {
        MovementMode::Climbing
    } else if state.on_ground {
        MovementMode::Grounded
    } else {
        MovementMode::Airborne
    }
}

pub fn authoritative_motion_state(
    map: &Map,
    rules: &MovementRules,
    position: &Vec2,
    mode: MovementMode,
) -> Result<MotionState, String> {
    match mode {
        MovementMode::Grounded => Ok(grounded_motion_state(
            map,
            position,
            rules.platform_edge_tolerance,
        )),
        MovementMode::Airborne => Ok(MotionState::default()),
        MovementMode::Climbing => {
            let climbing = map
                .ladders
                .iter()
                .enumerate()
                .filter(|(_, ladder)| {
                    (position.x - ladder.x).abs() <= rules.ladder_reach
                        && position.y >= ladder.top - rules.ladder_end_reach
                        && position.y <= ladder.bottom + rules.ladder_end_reach
                })
                .min_by(|(_, left), (_, right)| {
                    (position.x - left.x)
                        .abs()
                        .total_cmp(&(position.x - right.x).abs())
                })
                .map(|(index, _)| index)
                .ok_or("authoritative climbing position has no nearby ladder")?;
            Ok(MotionState {
                climbing: Some(climbing),
                platform_layer: map.ladders[climbing].layer,
                ..MotionState::default()
            })
        }
        MovementMode::Unspecified => Err("authoritative movement mode is unspecified".to_owned()),
    }
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
    grounded_motion_state(map, position, 0.0)
}

fn grounded_motion_state(
    map: &Map,
    position: &Vec2,
    edge_tolerance: f32,
) -> MotionState {
    supporting_platform(map, position, edge_tolerance).map_or_else(
        MotionState::default,
        |contact| MotionState {
            on_ground: true,
            platform_layer: contact.layer,
            ..MotionState::default()
        },
    )
}

pub fn update_player(
    map: &Map,
    rules: &MovementRules,
    position: Vec2,
    state: MotionState,
    input: PlayerInput,
    elapsed_seconds: f32,
) -> MotionOutput {
    let position = constrain_position(map, position);
    if input.portal_pressed
        && let Some(portal) = find_usable_portal(&map.portals, &position, rules)
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
        && let Some(index) = find_climbable_ladder(
            &map.ladders,
            &position,
            input.vertical,
            rules.ladder_reach,
            rules.ladder_end_reach,
        )
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
            return move_with_gravity(map, rules, position, state, input, elapsed_seconds);
        };
        if input.jump_pressed {
            state.climbing = None;
            state.velocity_y = -modified_speed(rules.jump_speed, input.jump_bonus, rules.jump_cap);
            return move_with_gravity(map, rules, position, state, input, elapsed_seconds);
        }
        return move_on_ladder(
            map,
            rules,
            position,
            state,
            input.vertical,
            ladder,
            elapsed_seconds,
        );
    }

    if input.jump_pressed && state.on_ground {
        state.velocity_y = -modified_speed(rules.jump_speed, input.jump_bonus, rules.jump_cap);
        state.on_ground = false;
    }
    move_with_gravity(map, rules, position, state, input, elapsed_seconds)
}

fn move_with_gravity(
    map: &Map,
    rules: &MovementRules,
    mut position: Vec2,
    mut state: MotionState,
    input: PlayerInput,
    elapsed_seconds: f32,
) -> MotionOutput {
    let old_x = position.x;
    let old_y = position.y;
    let (left_edge, right_edge) = horizontal_movement_limits(map);
    let move_speed = modified_speed(rules.walk_speed, input.speed_bonus, rules.speed_cap);
    position.x =
        (position.x + input.horizontal * move_speed * elapsed_seconds).clamp(left_edge, right_edge);

    state.velocity_y += rules.gravity * elapsed_seconds;
    let proposed_y = position.y + state.velocity_y * elapsed_seconds;
    if let Some(contact) = find_landing_platform(
        map,
        old_x,
        position.x,
        old_y,
        proposed_y,
        rules.platform_edge_tolerance,
    ) {
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

fn horizontal_movement_limits(map: &Map) -> (f32, f32) {
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
        .map_or_else(
            || {
                let inset = PLAYER_HALF_WIDTH.min(map_right / 2.0);
                (inset, (map_right - inset).max(inset))
            },
            |bounds| (bounds.left, bounds.right),
        )
}

pub fn constrain_position(
    map: &Map,
    mut position: Vec2,
) -> Vec2 {
    let (left, right) = horizontal_movement_limits(map);
    position.x = position.x.clamp(left, right);
    position
}

fn modified_speed(
    base: f32,
    bonus: i32,
    cap: u32,
) -> f32 {
    let percentage = (100_i64 + i64::from(bonus)).clamp(0, i64::from(cap));
    base * percentage as f32 / 100.0
}

fn move_on_ladder(
    map: &Map,
    rules: &MovementRules,
    mut position: Vec2,
    mut state: MotionState,
    vertical: f32,
    ladder: &Ladder,
    elapsed_seconds: f32,
) -> MotionOutput {
    position.x = ladder.x;
    position.y += vertical * rules.climb_speed * elapsed_seconds;

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
    ladder_reach: f32,
    ladder_end_reach: f32,
) -> Option<usize> {
    ladders
        .iter()
        .enumerate()
        .filter(|(_, ladder)| {
            let within_vertical_reach = if vertical < 0.0 {
                position.y >= ladder.top && position.y <= ladder.bottom + ladder_end_reach
            } else {
                position.y >= ladder.top - ladder_end_reach && position.y <= ladder.bottom
            };
            (position.x - ladder.x).abs() <= ladder_reach && within_vertical_reach
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
    rules: &MovementRules,
) -> Option<&'a Portal> {
    portals
        .iter()
        .filter(|portal| portal.target_map_id != SCRIPT_PORTAL_TARGET)
        .filter(|portal| {
            (position.x - portal.x).abs() <= rules.portal_horizontal_reach
                && (position.y - portal.y).abs() <= rules.portal_vertical_reach
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

fn find_landing_platform(
    map: &Map,
    old_x: f32,
    new_x: f32,
    old_y: f32,
    new_y: f32,
    platform_edge_tolerance: f32,
) -> Option<GroundContact> {
    if new_y < old_y {
        return None;
    }

    map.platforms
        .iter()
        .filter_map(|platform| {
            let minimum_x = platform.x.min(platform.end_x);
            let maximum_x = platform.x.max(platform.end_x);
            if new_x < minimum_x - platform_edge_tolerance
                || new_x > maximum_x + platform_edge_tolerance
            {
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
    edge_tolerance: f32,
) -> Option<GroundContact> {
    map.platforms
        .iter()
        .filter_map(|platform| {
            let minimum_x = platform.x.min(platform.end_x);
            let maximum_x = platform.x.max(platform.end_x);
            if position.x < minimum_x - edge_tolerance || position.x > maximum_x + edge_tolerance {
                return None;
            }
            let y = platform_y(platform, position.x.clamp(minimum_x, maximum_x))?;
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
    use oozems_proto::v1::MapMovementBounds;
    use oozems_proto::v1::MovementMode;
    use oozems_proto::v1::MovementRules;
    use oozems_proto::v1::Platform;
    use oozems_proto::v1::Portal;
    use oozems_proto::v1::Vec2;

    use super::MapTransition;
    use super::MotionState;
    use super::PlayerInput;
    use super::authoritative_motion_state;
    use super::initial_motion_state;
    use super::modified_speed;
    use super::update_player;
    use super::validate_rules;

    #[test]
    fn skill_bonuses_scale_movement_without_allowing_negative_speed() {
        assert_eq!(modified_speed(220.0, 10, 200), 242.0);
        assert_eq!(modified_speed(480.0, 20, 200), 576.0);
        assert_eq!(modified_speed(220.0, -150, 200), 0.0);
        assert_eq!(modified_speed(220.0, 150, 200), 440.0);
    }

    #[test]
    fn rejects_invalid_rules_at_the_client_boundary() {
        let mut invalid = rules();
        invalid.speed_cap = 0;

        assert!(validate_rules(&rules()).is_ok());
        assert!(validate_rules(&invalid).is_err());
    }

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
            &rules(),
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
    fn walking_stops_at_the_wz_map_wall() {
        let map = map_with_inset_walls();

        let output = update_player(
            &map,
            &rules(),
            Vec2 { x: 670.0, y: 300.0 },
            MotionState {
                on_ground: true,
                ..MotionState::default()
            },
            PlayerInput {
                horizontal: 1.0,
                ..PlayerInput::default()
            },
            1.0,
        );

        assert_eq!(output.position, Vec2 { x: 682.0, y: 300.0 });
        assert!(output.state.on_ground);
    }

    #[test]
    fn jumping_cannot_cross_the_wz_map_wall() {
        let map = map_with_inset_walls();

        let output = update_player(
            &map,
            &rules(),
            Vec2 { x: 670.0, y: 300.0 },
            MotionState {
                on_ground: true,
                ..MotionState::default()
            },
            PlayerInput {
                horizontal: 1.0,
                jump_pressed: true,
                ..PlayerInput::default()
            },
            0.1,
        );

        assert_eq!(output.position.x, 682.0);
        assert!(!output.state.on_ground);
    }

    #[test]
    fn positions_are_constrained_before_non_walking_movement() {
        let map = map_with_inset_walls();

        let output = update_player(
            &map,
            &rules(),
            Vec2 { x: 740.0, y: 300.0 },
            MotionState {
                on_ground: true,
                ..MotionState::default()
            },
            PlayerInput::default(),
            0.0,
        );

        assert_eq!(output.position, Vec2 { x: 682.0, y: 300.0 });
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
            &rules(),
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
    fn authoritative_grounding_preserves_platform_edge_contact() {
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

        let state = authoritative_motion_state(
            &map,
            &rules(),
            &Vec2 { x: 218.0, y: 300.0 },
            MovementMode::Grounded,
        )
        .expect("authoritative state");

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
            &rules(),
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
            &rules(),
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
            &rules(),
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
            &rules(),
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
            width: 800,
            height: 600,
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
            &rules(),
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
            width: 800,
            height: 600,
            portals: vec![Portal {
                x: 200.0,
                y: 300.0,
                target_map_id: 999_999_999,
                ..Portal::default()
            }],
            ..Map::default()
        };
        let output = update_player(
            &map,
            &rules(),
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

    fn rules() -> MovementRules {
        MovementRules {
            walk_speed: 220.0,
            climb_speed: 135.0,
            gravity: 1_150.0,
            jump_speed: 480.0,
            speed_cap: 200,
            jump_cap: 200,
            snapshot_interval_ms: 200,
            maximum_snapshot_gap_ms: 1_000,
            position_tolerance: 24.0,
            ground_tolerance: 8.0,
            platform_edge_tolerance: 20.0,
            ladder_reach: 32.0,
            ladder_end_reach: 20.0,
            portal_horizontal_reach: 48.0,
            portal_vertical_reach: 64.0,
        }
    }

    fn map_with_inset_walls() -> Map {
        Map {
            width: 800,
            height: 600,
            platforms: vec![Platform {
                x: 100.0,
                y: 300.0,
                end_x: 700.0,
                end_y: 300.0,
                ..Platform::default()
            }],
            movement_bounds: Some(MapMovementBounds {
                left: 118.0,
                right: 682.0,
            }),
            ..Map::default()
        }
    }
}
