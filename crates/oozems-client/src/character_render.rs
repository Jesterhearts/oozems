use std::collections::HashMap;

use oozems_proto::v1::CharacterFrame;
use oozems_proto::v1::CharacterSpriteSet;
use web_sys::CanvasRenderingContext2d;

use crate::assets::BrowserAsset;
use crate::assets::ready_image;

pub struct CharacterPlacement {
    pub anchor_x: f64,
    pub anchor_y: f64,
    pub scale: f64,
    pub facing_left: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CharacterAnimation {
    #[default]
    Idle,
    Walk,
    Jump,
    Ladder,
    Rope,
}

pub fn draw_character(
    context: &CanvasRenderingContext2d,
    assets: &HashMap<String, BrowserAsset>,
    sprites: &CharacterSpriteSet,
    animation: CharacterAnimation,
    timestamp_ms: f64,
    placement: CharacterPlacement,
) {
    let Some(frame) = frame_at_time(animation_frames(sprites, animation), timestamp_ms) else {
        return;
    };
    context.save();
    let horizontal_scale = horizontal_scale(placement.scale, placement.facing_left);
    let transformed = context
        .translate(placement.anchor_x, placement.anchor_y)
        .and_then(|()| context.scale(horizontal_scale, placement.scale));
    if transformed.is_ok() {
        for layer in &frame.layers {
            let Some(image) = ready_image(assets, &layer.asset_id) else {
                continue;
            };
            let _ = context.draw_image_with_html_image_element_and_dw_and_dh(
                image,
                f64::from(layer.x),
                f64::from(layer.y),
                f64::from(layer.width),
                f64::from(layer.height),
            );
        }
    }
    context.restore();
}

fn animation_frames(
    sprites: &CharacterSpriteSet,
    animation: CharacterAnimation,
) -> &[CharacterFrame] {
    let selected = match animation {
        CharacterAnimation::Idle => &sprites.idle_frames,
        CharacterAnimation::Walk => &sprites.walk_frames,
        CharacterAnimation::Jump => &sprites.jump_frames,
        CharacterAnimation::Ladder => &sprites.ladder_frames,
        CharacterAnimation::Rope => &sprites.rope_frames,
    };
    if selected.is_empty() {
        &sprites.idle_frames
    } else {
        selected
    }
}

fn horizontal_scale(
    scale: f64,
    facing_left: bool,
) -> f64 {
    // Classic WZ character frames face left in their source orientation.
    if facing_left { scale } else { -scale }
}

fn frame_at_time(
    frames: &[CharacterFrame],
    timestamp_ms: f64,
) -> Option<&CharacterFrame> {
    let total_duration = frames
        .iter()
        .map(|frame| u64::from(frame.delay_ms.max(1)))
        .sum::<u64>();
    if total_duration == 0 {
        return None;
    }

    let mut animation_time = timestamp_ms.max(0.0) as u64 % total_duration;
    for frame in frames {
        let delay = u64::from(frame.delay_ms.max(1));
        if animation_time < delay {
            return Some(frame);
        }
        animation_time -= delay;
    }
    frames.last()
}

#[cfg(test)]
mod tests {
    use oozems_proto::v1::CharacterFrame;
    use oozems_proto::v1::CharacterSpriteSet;

    use super::CharacterAnimation;
    use super::animation_frames;
    use super::frame_at_time;
    use super::horizontal_scale;

    #[test]
    fn source_frame_is_mirrored_only_when_facing_right() {
        assert_eq!(horizontal_scale(2.5, true), 2.5);
        assert_eq!(horizontal_scale(2.5, false), -2.5);
    }

    #[test]
    fn missing_action_frames_fall_back_to_idle() {
        let sprites = CharacterSpriteSet {
            idle_frames: vec![CharacterFrame::default()],
            ..CharacterSpriteSet::default()
        };

        assert_eq!(
            animation_frames(&sprites, CharacterAnimation::Walk),
            sprites.idle_frames
        );
        assert_eq!(
            animation_frames(&sprites, CharacterAnimation::Jump),
            sprites.idle_frames
        );
        assert_eq!(
            animation_frames(&sprites, CharacterAnimation::Ladder),
            sprites.idle_frames
        );
        assert_eq!(
            animation_frames(&sprites, CharacterAnimation::Rope),
            sprites.idle_frames
        );
    }

    #[test]
    fn animation_uses_each_frame_delay() {
        let frames = vec![
            CharacterFrame {
                delay_ms: 100,
                ..CharacterFrame::default()
            },
            CharacterFrame {
                delay_ms: 200,
                ..CharacterFrame::default()
            },
        ];

        assert_eq!(frame_at_time(&frames, 99.0), Some(&frames[0]));
        assert_eq!(frame_at_time(&frames, 100.0), Some(&frames[1]));
        assert_eq!(frame_at_time(&frames, 299.0), Some(&frames[1]));
        assert_eq!(frame_at_time(&frames, 300.0), Some(&frames[0]));
    }
}
