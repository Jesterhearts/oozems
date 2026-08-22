use oozems_proto::v1::MobAnimation;
use oozems_proto::v1::MobDefinition;
use oozems_proto::v1::MobFrame;

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
        let Some(position) = mob.position.as_ref() else {
            continue;
        };
        let Some(definition) = definition(game, mob.definition_id) else {
            continue;
        };
        let Some(animation) = idle_animation(definition) else {
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

fn idle_animation(definition: &MobDefinition) -> Option<&MobAnimation> {
    ["stand", "move", "fly"]
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

    use super::animation_frame;
    use super::frame_x;
    use super::idle_animation;

    #[test]
    fn stand_animation_is_preferred_for_idle_mobs() {
        let definition = MobDefinition {
            animations: vec![animation("move", 100), animation("stand", 200)],
            ..MobDefinition::default()
        };

        assert_eq!(
            idle_animation(&definition).expect("animation").name,
            "stand"
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
