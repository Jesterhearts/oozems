use std::collections::HashMap;

use oozems_proto::v1::MorphAnimation;
use oozems_proto::v1::MorphDefinition;
use oozems_proto::v1::MorphFrame;
use web_sys::CanvasRenderingContext2d;

use crate::assets::BrowserAsset;
use crate::assets::preferred_or_first_ready;
use crate::assets::ready_image;
use crate::character_render::CharacterAnimation;
use crate::character_render::CharacterPlacement;

pub fn draw_morph(
    context: &CanvasRenderingContext2d,
    assets: &HashMap<String, BrowserAsset>,
    definition: &MorphDefinition,
    animation: CharacterAnimation,
    timestamp_ms: f64,
    placement: CharacterPlacement,
) -> bool {
    let Some(frame) = ready_frame(definition, animation, timestamp_ms, |asset_id| {
        ready_image(assets, asset_id).is_some()
    }) else {
        return false;
    };
    let Some(image) = ready_image(assets, &frame.asset_id) else {
        return false;
    };
    context.save();
    let horizontal_scale = if placement.facing_left {
        placement.scale
    } else {
        -placement.scale
    };
    let transformed = context
        .translate(placement.anchor_x, placement.anchor_y)
        .and_then(|()| context.scale(horizontal_scale, placement.scale));
    let drawn = if transformed.is_ok() {
        context
            .draw_image_with_html_image_element_and_dw_and_dh(
                image,
                -f64::from(frame.origin_x),
                -f64::from(frame.origin_y),
                f64::from(frame.width),
                f64::from(frame.height),
            )
            .is_ok()
    } else {
        false
    };
    context.restore();
    drawn
}

fn ready_frame(
    definition: &MorphDefinition,
    animation: CharacterAnimation,
    timestamp_ms: f64,
    mut is_ready: impl FnMut(&str) -> bool,
) -> Option<&MorphFrame> {
    let preferred = animation_for(definition, animation)?;
    if let Some(frame) = ready_animation_frame(preferred, timestamp_ms, &mut is_ready) {
        return Some(frame);
    }
    definition
        .animations
        .iter()
        .find(|candidate| candidate.name == "stand" && candidate.name != preferred.name)
        .and_then(|stand| ready_animation_frame(stand, timestamp_ms, is_ready))
}

fn ready_animation_frame(
    animation: &MorphAnimation,
    timestamp_ms: f64,
    mut is_ready: impl FnMut(&str) -> bool,
) -> Option<&MorphFrame> {
    let preferred_index = crate::animation::frame_index(
        animation.frames.iter().map(|frame| frame.delay_ms),
        timestamp_ms,
        crate::animation::Playback::Loop,
    )?;
    let frame_index = preferred_or_first_ready(
        animation
            .frames
            .iter()
            .map(|frame| is_ready(&frame.asset_id)),
        preferred_index,
    )?;
    animation.frames.get(frame_index)
}

fn animation_for(
    definition: &MorphDefinition,
    animation: CharacterAnimation,
) -> Option<&MorphAnimation> {
    if animation == CharacterAnimation::Death {
        return None;
    }
    let preferred = match animation {
        CharacterAnimation::Idle | CharacterAnimation::Attack => "stand",
        CharacterAnimation::Walk => "walk",
        CharacterAnimation::Jump => "jump",
        CharacterAnimation::Ladder => "ladder",
        CharacterAnimation::Rope => "rope",
        CharacterAnimation::Death => unreachable!("death was handled above"),
    };
    definition
        .animations
        .iter()
        .find(|candidate| candidate.name == preferred)
        .or_else(|| {
            definition
                .animations
                .iter()
                .find(|candidate| candidate.name == "stand")
        })
}

#[cfg(test)]
mod tests {
    use oozems_proto::v1::MorphAnimation;
    use oozems_proto::v1::MorphDefinition;
    use oozems_proto::v1::MorphFrame;

    use super::animation_for;
    use super::ready_frame;
    use crate::character_render::CharacterAnimation;

    #[test]
    fn unavailable_motion_falls_back_to_stand() {
        let definition = MorphDefinition {
            animations: vec![MorphAnimation {
                name: "stand".to_owned(),
                frames: vec![MorphFrame::default()],
            }],
            ..MorphDefinition::default()
        };

        assert_eq!(
            animation_for(&definition, CharacterAnimation::Ladder).map(|value| value.name.as_str()),
            Some("stand")
        );
    }

    #[test]
    fn frame_timing_honors_each_archive_delay() {
        let frames = [
            MorphFrame {
                asset_id: "first".to_owned(),
                delay_ms: 50,
                ..MorphFrame::default()
            },
            MorphFrame {
                asset_id: "second".to_owned(),
                delay_ms: 100,
                ..MorphFrame::default()
            },
        ];

        let frame_index = |timestamp_ms| {
            crate::animation::frame_index(
                frames.iter().map(|frame| frame.delay_ms),
                timestamp_ms,
                crate::animation::Playback::Loop,
            )
        };
        assert_eq!(frame_index(49.0), Some(0));
        assert_eq!(frame_index(50.0), Some(1));
        assert_eq!(frame_index(150.0), Some(0));
    }

    #[test]
    fn unavailable_preferred_assets_fall_back_to_ready_stand_assets() {
        let definition = MorphDefinition {
            animations: vec![
                MorphAnimation {
                    name: "stand".to_owned(),
                    frames: vec![MorphFrame {
                        asset_id: "stand-ready".to_owned(),
                        ..MorphFrame::default()
                    }],
                },
                MorphAnimation {
                    name: "walk".to_owned(),
                    frames: vec![MorphFrame {
                        asset_id: "walk-unavailable".to_owned(),
                        ..MorphFrame::default()
                    }],
                },
            ],
            ..MorphDefinition::default()
        };

        let frame = ready_frame(&definition, CharacterAnimation::Walk, 0.0, |asset_id| {
            asset_id == "stand-ready"
        })
        .expect("stand fallback");

        assert_eq!(frame.asset_id, "stand-ready");
    }
}
