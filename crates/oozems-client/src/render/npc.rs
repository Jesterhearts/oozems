use oozems_proto::v1::NpcFrame;

use crate::assets;
use crate::game::Game;
use crate::game_gui::CanvasPoint;

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

pub(super) fn at_point(
    game: &Game,
    point: CanvasPoint,
    camera_x: f64,
    camera_y: f64,
) -> Option<u32> {
    game.world_layers.iter().rev().find_map(|layer| {
        game.map.npcs.iter().rev().find_map(|npc| {
            if npc.layer != *layer {
                return None;
            }
            let position = npc.position.as_ref()?;
            let preferred_index = animation_frame_index(&npc.frames, game.frame_time_ms)?;
            let index = assets::ready_or_fallback_index(
                &game.images,
                npc.frames.iter().map(|frame| frame.asset_id.as_str()),
                preferred_index,
            )?;
            let frame = &npc.frames[index];
            let left = f64::from(frame_x(position.x, frame, npc.flip_x)) - camera_x;
            let top = f64::from(position.y - frame.origin_y) - camera_y;
            point_in_frame(
                point,
                left,
                top,
                f64::from(frame.width),
                f64::from(frame.height),
            )
            .then_some(npc.spawn_id)
        })
    })
}

fn point_in_frame(
    point: CanvasPoint,
    left: f64,
    top: f64,
    width: f64,
    height: f64,
) -> bool {
    f64::from(point.x) >= left
        && f64::from(point.x) <= left + width
        && f64::from(point.y) >= top
        && f64::from(point.y) <= top + height
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
    use super::point_in_frame;
    use crate::game_gui::CanvasPoint;

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

    #[test]
    fn npc_frame_bounds_include_their_edges() {
        assert!(point_in_frame(
            CanvasPoint { x: 10.0, y: 20.0 },
            10.0,
            20.0,
            30.0,
            40.0,
        ));
        assert!(!point_in_frame(
            CanvasPoint { x: 41.0, y: 20.0 },
            10.0,
            20.0,
            30.0,
            40.0,
        ));
    }
}
