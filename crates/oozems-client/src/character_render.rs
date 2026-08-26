use std::collections::HashMap;

use oozems_proto::v1::CharacterFrame;
use oozems_proto::v1::CharacterSpriteSet;
use web_sys::CanvasRenderingContext2d;

use crate::assets;
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
    Attack,
    Death,
}

pub fn draw_character(
    context: &CanvasRenderingContext2d,
    assets: &HashMap<String, BrowserAsset>,
    sprites: &CharacterSpriteSet,
    animation: CharacterAnimation,
    timestamp_ms: f64,
    placement: CharacterPlacement,
) {
    let frames = animation_frames(sprites, animation);
    let playback = if animation == CharacterAnimation::Death {
        crate::animation::Playback::Once
    } else {
        crate::animation::Playback::Loop
    };
    let preferred_index = crate::animation::frame_index(
        frames.iter().map(|frame| frame.delay_ms),
        timestamp_ms,
        playback,
    )
    .or_else(|| {
        (animation == CharacterAnimation::Death)
            .then(|| frames.len().checked_sub(1))
            .flatten()
    });
    let Some(preferred_index) = preferred_index else {
        return;
    };
    let Some(frame) = drawable_frame(assets, sprites, frames, preferred_index) else {
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

fn drawable_frame<'a>(
    browser_assets: &HashMap<String, BrowserAsset>,
    sprites: &'a CharacterSpriteSet,
    selected_frames: &[CharacterFrame],
    selected_index: usize,
) -> Option<&'a CharacterFrame> {
    let preferred = selected_frames.get(selected_index)?;
    let preferred_index = all_frames(sprites).position(|frame| std::ptr::eq(frame, preferred))?;
    let index = assets::preferred_or_first_ready(
        all_frames(sprites).map(|frame| {
            !frame.layers.is_empty()
                && assets::images_ready(
                    browser_assets,
                    frame.layers.iter().map(|layer| layer.asset_id.as_str()),
                )
        }),
        preferred_index,
    )?;
    all_frames(sprites).nth(index)
}

fn all_frames(sprites: &CharacterSpriteSet) -> impl Iterator<Item = &CharacterFrame> {
    sprites
        .idle_frames
        .iter()
        .chain(&sprites.walk_frames)
        .chain(&sprites.jump_frames)
        .chain(&sprites.ladder_frames)
        .chain(&sprites.rope_frames)
        .chain(&sprites.attack_frames)
        .chain(&sprites.death_frames)
}

pub fn animation_duration_ms(
    sprites: &CharacterSpriteSet,
    animation: CharacterAnimation,
) -> u64 {
    animation_frames(sprites, animation)
        .iter()
        .map(|frame| u64::from(frame.delay_ms.max(1)))
        .sum()
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
        CharacterAnimation::Attack => &sprites.attack_frames,
        CharacterAnimation::Death => &sprites.death_frames,
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

#[cfg(test)]
mod tests {
    use oozems_proto::v1::CharacterFrame;
    use oozems_proto::v1::CharacterSpriteSet;

    use super::CharacterAnimation;
    use super::animation_duration_ms;
    use super::animation_frames;
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
        assert_eq!(
            animation_frames(&sprites, CharacterAnimation::Attack),
            sprites.idle_frames
        );
        assert_eq!(
            animation_frames(&sprites, CharacterAnimation::Death),
            sprites.idle_frames
        );
    }

    #[test]
    fn animation_uses_each_frame_delay() {
        let frames = [
            CharacterFrame {
                delay_ms: 100,
                ..CharacterFrame::default()
            },
            CharacterFrame {
                delay_ms: 200,
                ..CharacterFrame::default()
            },
        ];

        let frame_index = |timestamp_ms| {
            crate::animation::frame_index(
                frames.iter().map(|frame| frame.delay_ms),
                timestamp_ms,
                crate::animation::Playback::Loop,
            )
        };
        assert_eq!(frame_index(99.0), Some(0));
        assert_eq!(frame_index(100.0), Some(1));
        assert_eq!(frame_index(299.0), Some(1));
        assert_eq!(frame_index(300.0), Some(0));
    }

    #[test]
    fn attack_duration_comes_from_its_wz_frames() {
        let sprites = CharacterSpriteSet {
            attack_frames: vec![
                CharacterFrame {
                    delay_ms: 100,
                    ..CharacterFrame::default()
                },
                CharacterFrame {
                    delay_ms: 200,
                    ..CharacterFrame::default()
                },
            ],
            ..CharacterSpriteSet::default()
        };

        assert_eq!(
            animation_duration_ms(&sprites, CharacterAnimation::Attack),
            300
        );
    }
}
