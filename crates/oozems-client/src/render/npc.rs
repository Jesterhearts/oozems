use oozems_proto::v1::NpcFrame;

use crate::assets;
use crate::game::Game;

pub(super) fn draw(
    game: &Game,
    camera_x: f64,
    camera_y: f64,
    layer: i32,
) {
    for npc in &game.map.npcs {
        if npc.layer != layer {
            continue;
        }
        let Some(position) = &npc.position else {
            continue;
        };
        let Some(preferred_index) = animation_frame_index(&npc.frames, game.frame_time_ms) else {
            continue;
        };
        let preferred = &npc.frames[preferred_index];
        if !super::sprite_is_visible(
            game,
            frame_x(position.x, preferred, npc.flip_x),
            position.y - preferred.origin_y,
            preferred.width,
            preferred.height,
            camera_x,
            camera_y,
        ) {
            continue;
        }
        let Some(index) = assets::ready_or_fallback_index(
            &game.images,
            npc.frames.iter().map(|frame| frame.asset_id.as_str()),
            preferred_index,
        ) else {
            continue;
        };
        let frame = &npc.frames[index];
        super::draw_sprite(
            game,
            &frame.asset_id,
            frame_x(position.x, frame, npc.flip_x),
            position.y - frame.origin_y,
            frame.width,
            frame.height,
            npc.flip_x,
            camera_x,
            camera_y,
        );
    }
}

fn animation_frame_index(
    frames: &[NpcFrame],
    timestamp_ms: f64,
) -> Option<usize> {
    super::timed_frame_index(frames.iter().map(|frame| frame.delay_ms), timestamp_ms)
}

fn frame_x(
    anchor_x: f32,
    frame: &NpcFrame,
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
    use oozems_proto::v1::NpcFrame;

    use super::animation_frame_index;
    use super::frame_x;

    #[test]
    fn npc_animation_uses_frame_delays() {
        let frames = vec![
            NpcFrame {
                asset_id: "first".to_owned(),
                delay_ms: 100,
                ..NpcFrame::default()
            },
            NpcFrame {
                asset_id: "second".to_owned(),
                delay_ms: 200,
                ..NpcFrame::default()
            },
        ];

        assert_eq!(
            frames[animation_frame_index(&frames, 99.0).expect("frame")].asset_id,
            "first"
        );
        assert_eq!(
            frames[animation_frame_index(&frames, 100.0).expect("frame")].asset_id,
            "second"
        );
        assert_eq!(
            frames[animation_frame_index(&frames, 300.0).expect("frame")].asset_id,
            "first"
        );
    }

    #[test]
    fn flipping_keeps_the_frame_origin_on_the_npc_anchor() {
        let frame = NpcFrame {
            width: 40.0,
            origin_x: 15.0,
            ..NpcFrame::default()
        };

        assert_eq!(frame_x(100.0, &frame, false), 85.0);
        assert_eq!(frame_x(100.0, &frame, true), 75.0);
    }
}
