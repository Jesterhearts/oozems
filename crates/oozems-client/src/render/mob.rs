use oozems_proto::v1::MobAnimation;
use oozems_proto::v1::MobDefinition;
use oozems_proto::v1::MobFrame;
use oozems_proto::v1::MobMovementMode;

use crate::game::Game;

pub(super) fn draw(
    game: &Game,
    camera_x: f64,
    camera_y: f64,
    layer: i32,
) {
    for mob in &game.map.mobs {
        if mob.layer != layer {
            continue;
        }
        let Some(position) = crate::mob_render::position(&game.mob_render, mob, game.frame_time_ms)
        else {
            continue;
        };
        let Some(definition) = definition(game, mob.definition_id) else {
            continue;
        };
        let mode = MobMovementMode::try_from(mob.movement_mode).unwrap_or(MobMovementMode::Idle);
        let Some(animation) = movement_animation(definition, mode) else {
            continue;
        };
        let Some(frame) = animation_frame(animation, game.frame_time_ms) else {
            continue;
        };
        let x = frame_x(position.x, frame, mob.flip_x);
        super::draw_sprite(
            game,
            &frame.asset_id,
            x,
            position.y - frame.origin_y,
            frame.width,
            frame.height,
            mob.flip_x,
            camera_x,
            camera_y,
        );
    }
}

fn definition(
    game: &Game,
    definition_id: u32,
) -> Option<&MobDefinition> {
    game.map
        .mob_definitions
        .iter()
        .find(|definition| definition.id == definition_id)
}

fn movement_animation(
    definition: &MobDefinition,
    mode: MobMovementMode,
) -> Option<&MobAnimation> {
    let names = match mode {
        MobMovementMode::Walking => ["move", "fly", "stand"],
        MobMovementMode::Jumping => ["jump", "move", "fly"],
        MobMovementMode::Unspecified | MobMovementMode::Idle => ["stand", "move", "fly"],
    };
    names
        .into_iter()
        .find_map(|name| {
            definition
                .animations
                .iter()
                .find(|animation| animation.name == name && !animation.frames.is_empty())
        })
        .or_else(|| {
            definition
                .animations
                .iter()
                .find(|animation| !animation.frames.is_empty())
        })
}

fn animation_frame(
    animation: &MobAnimation,
    timestamp_ms: f64,
) -> Option<&MobFrame> {
    let index = super::timed_frame_index(
        animation.frames.iter().map(|frame| frame.delay_ms),
        timestamp_ms,
    )?;
    animation.frames.get(index)
}

fn frame_x(
    anchor_x: f32,
    frame: &MobFrame,
    flip_x: bool,
) -> f32 {
    if flip_x {
        anchor_x - (frame.width - frame.origin_x)
    } else {
        anchor_x - frame.origin_x
    }
}

#[cfg(test)]
mod tests {
    use oozems_proto::v1::MobAnimation;
    use oozems_proto::v1::MobDefinition;
    use oozems_proto::v1::MobFrame;
    use oozems_proto::v1::MobMovementMode;

    use super::animation_frame;
    use super::frame_x;
    use super::movement_animation;

    #[test]
    fn stand_animation_is_preferred_for_idle_mobs() {
        let definition = MobDefinition {
            animations: vec![animation("move", 100), animation("stand", 200)],
            ..MobDefinition::default()
        };

        assert_eq!(
            movement_animation(&definition, MobMovementMode::Idle)
                .expect("animation")
                .name,
            "stand"
        );
    }

    #[test]
    fn movement_mode_selects_walk_and_jump_animations() {
        let definition = MobDefinition {
            animations: vec![
                animation("stand", 100),
                animation("move", 100),
                animation("jump", 100),
            ],
            ..MobDefinition::default()
        };

        assert_eq!(
            movement_animation(&definition, MobMovementMode::Walking)
                .expect("walk animation")
                .name,
            "move"
        );
        assert_eq!(
            movement_animation(&definition, MobMovementMode::Jumping)
                .expect("jump animation")
                .name,
            "jump"
        );
    }

    #[test]
    fn mob_animation_uses_frame_delays() {
        let animation = MobAnimation {
            name: "stand".to_owned(),
            frames: vec![
                MobFrame {
                    asset_id: "first".to_owned(),
                    delay_ms: 100,
                    ..MobFrame::default()
                },
                MobFrame {
                    asset_id: "second".to_owned(),
                    delay_ms: 200,
                    ..MobFrame::default()
                },
            ],
        };

        assert_eq!(
            animation_frame(&animation, 99.0).expect("frame").asset_id,
            "first"
        );
        assert_eq!(
            animation_frame(&animation, 100.0).expect("frame").asset_id,
            "second"
        );
        assert_eq!(
            animation_frame(&animation, 300.0).expect("frame").asset_id,
            "first"
        );
    }

    #[test]
    fn flipping_keeps_the_frame_origin_on_the_mob_anchor() {
        let frame = MobFrame {
            width: 40.0,
            origin_x: 15.0,
            ..MobFrame::default()
        };

        assert_eq!(frame_x(100.0, &frame, false), 85.0);
        assert_eq!(frame_x(100.0, &frame, true), 75.0);
    }

    fn animation(
        name: &str,
        delay_ms: u32,
    ) -> MobAnimation {
        MobAnimation {
            name: name.to_owned(),
            frames: vec![MobFrame {
                delay_ms,
                ..MobFrame::default()
            }],
        }
    }
}
