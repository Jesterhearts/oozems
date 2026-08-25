use std::collections::HashMap;

use oozems_proto::v1::SkillAnimation;
use oozems_proto::v1::SkillAnimationFrame;
use oozems_proto::v1::SkillAnimationPlacement;
use oozems_proto::v1::SkillEffect;
use oozems_proto::v1::Vec2;
use wasm_bindgen::JsValue;
use wasm_bindgen_futures::JsFuture;
use wasm_bindgen_futures::spawn_local;
use web_sys::HtmlAudioElement;

use crate::assets;
use crate::assets::BrowserAsset;
use crate::game::Game;

const ASSET_READY_TIMEOUT_MS: f64 = 5_000.0;
const AUDIO_LIFETIME_MS: f64 = 15_000.0;
const PROJECTILE_DISTANCE: f32 = 160.0;
const PROJECTILE_HEIGHT: f32 = 35.0;

#[derive(Default)]
pub(crate) struct SkillEffectState {
    visuals: Vec<ActiveVisual>,
    sounds: Vec<ActiveSound>,
}

struct ActiveVisual {
    animations: Vec<SkillAnimation>,
    discarded: bool,
    queued_at_ms: f64,
    started_at_ms: Option<f64>,
    duration_ms: f64,
    origin_x: f32,
    origin_y: f32,
    facing_left: bool,
    target: Option<Vec2>,
    sound_url: Option<String>,
}

struct ActiveSound {
    element: HtmlAudioElement,
    expires_at_ms: f64,
}

pub(crate) fn install(
    game: &mut Game,
    effect: SkillEffect,
    target: Option<Vec2>,
) {
    if let Err(error) = assets::insert_assets(&mut game.surface.images, effect.assets.iter()) {
        warn(&format!(
            "Could not prepare skill animation assets: {error}"
        ));
    }
    let sound_url = effect.sound.map(|sound| sound.url);
    let Some(position) = game.player.position else {
        if let Some(url) = sound_url {
            play_sound(&mut game.world.skill_effect_state, &url, game.clock.now_ms);
        }
        return;
    };
    let duration_ms = effect_duration_ms(&effect.animations);
    if duration_ms == 0 {
        if let Some(url) = sound_url {
            play_sound(&mut game.world.skill_effect_state, &url, game.clock.now_ms);
        }
        return;
    }
    game.world.skill_effect_state.visuals.push(ActiveVisual {
        animations: effect.animations,
        discarded: false,
        queued_at_ms: game.clock.now_ms,
        started_at_ms: None,
        duration_ms: f64::from(duration_ms),
        origin_x: position.x,
        origin_y: position.y,
        facing_left: game.world.facing_left,
        target,
        sound_url,
    });
}

pub(crate) fn update(
    state: &mut SkillEffectState,
    images: &HashMap<String, BrowserAsset>,
    timestamp_ms: f64,
) {
    state
        .sounds
        .retain(|sound| timestamp_ms < sound.expires_at_ms && !sound.element.ended());
    let mut sounds_to_play = Vec::new();
    for visual in &mut state.visuals {
        if visual.started_at_ms.is_some() {
            continue;
        }
        let ready = assets::images_ready(
            images,
            visual
                .animations
                .iter()
                .flat_map(|animation| animation.frames.iter())
                .map(|frame| frame.asset_id.as_str()),
        );
        match asset_load_decision(ready, timestamp_ms - visual.queued_at_ms) {
            AssetLoadDecision::Wait => {}
            AssetLoadDecision::Start => {
                visual.started_at_ms = Some(timestamp_ms);
                sounds_to_play.extend(visual.sound_url.take());
            }
            AssetLoadDecision::Discard => {
                visual.discarded = true;
                sounds_to_play.extend(visual.sound_url.take());
                warn("Discarded a skill animation because its assets did not load");
            }
        }
    }
    for url in sounds_to_play {
        play_sound(state, &url, timestamp_ms);
    }
    state.visuals.retain(|visual| {
        !visual.discarded
            && visual
                .started_at_ms
                .is_none_or(|started_at_ms| timestamp_ms - started_at_ms < visual.duration_ms)
    });
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AssetLoadDecision {
    Wait,
    Start,
    Discard,
}

fn asset_load_decision(
    ready: bool,
    elapsed_ms: f64,
) -> AssetLoadDecision {
    if ready {
        AssetLoadDecision::Start
    } else if elapsed_ms >= ASSET_READY_TIMEOUT_MS {
        AssetLoadDecision::Discard
    } else {
        AssetLoadDecision::Wait
    }
}

pub(crate) fn clear(state: &mut SkillEffectState) {
    state.visuals.clear();
    for sound in state.sounds.drain(..) {
        let _ = sound.element.pause();
    }
}

pub(crate) fn draw(
    game: &Game,
    camera_x: f64,
    camera_y: f64,
) {
    for visual in &game.world.skill_effect_state.visuals {
        let Some(started_at_ms) = visual.started_at_ms else {
            continue;
        };
        let elapsed_ms = (game.clock.now_ms - started_at_ms).max(0.0);
        for animation in &visual.animations {
            draw_animation(game, visual, animation, elapsed_ms, camera_x, camera_y);
        }
    }
}

fn play_sound(
    state: &mut SkillEffectState,
    url: &str,
    timestamp_ms: f64,
) {
    let audio = match HtmlAudioElement::new_with_src(url) {
        Ok(audio) => audio,
        Err(error) => {
            warn(&format!(
                "Could not create a skill sound player: {}",
                crate::js_error(error)
            ));
            return;
        }
    };
    audio.set_preload("auto");
    match audio.play() {
        Ok(promise) => {
            spawn_local(async move {
                if let Err(error) = JsFuture::from(promise).await {
                    warn(&format!(
                        "The browser did not play a skill sound: {}",
                        crate::js_error(error)
                    ));
                }
            });
        }
        Err(error) => warn(&format!(
            "Could not start a skill sound: {}",
            crate::js_error(error)
        )),
    }
    state.sounds.push(ActiveSound {
        element: audio,
        expires_at_ms: timestamp_ms + AUDIO_LIFETIME_MS,
    });
}

fn draw_animation(
    game: &Game,
    visual: &ActiveVisual,
    animation: &SkillAnimation,
    effect_elapsed_ms: f64,
    camera_x: f64,
    camera_y: f64,
) {
    let elapsed_ms = effect_elapsed_ms - f64::from(animation.start_delay_ms);
    let Some((frame, progress)) = frame_at_time(animation, elapsed_ms) else {
        return;
    };
    let placement = SkillAnimationPlacement::try_from(animation.placement)
        .unwrap_or(SkillAnimationPlacement::Unspecified);
    let (anchor_x, anchor_y) = effect_anchor(visual, placement, progress);
    let flip_x = !visual.facing_left;
    let x = if flip_x {
        anchor_x - (frame.width - frame.origin_x)
    } else {
        anchor_x - frame.origin_x
    };
    let y = anchor_y - frame.origin_y;
    crate::render::draw_sprite(
        game,
        &frame.asset_id,
        x,
        y,
        frame.width,
        frame.height,
        flip_x,
        camera_x,
        camera_y,
    );
}

fn effect_anchor(
    visual: &ActiveVisual,
    placement: SkillAnimationPlacement,
    progress: f32,
) -> (f32, f32) {
    let direction = if visual.facing_left { -1.0 } else { 1.0 };
    let target = visual.target.unwrap_or(Vec2 {
        x: visual.origin_x + direction * PROJECTILE_DISTANCE,
        y: visual.origin_y,
    });
    match placement {
        SkillAnimationPlacement::Projectile => (
            visual.origin_x + (target.x - visual.origin_x) * progress,
            visual.origin_y - PROJECTILE_HEIGHT + (target.y - visual.origin_y) * progress,
        ),
        SkillAnimationPlacement::Target => (target.x, target.y),
        SkillAnimationPlacement::Caster | SkillAnimationPlacement::Unspecified => {
            (visual.origin_x, visual.origin_y)
        }
    }
}

fn frame_at_time(
    animation: &SkillAnimation,
    elapsed_ms: f64,
) -> Option<(&SkillAnimationFrame, f32)> {
    let duration_ms = animation_frame_duration_ms(animation);
    if elapsed_ms < 0.0 || elapsed_ms >= f64::from(duration_ms) || duration_ms == 0 {
        return None;
    }
    let progress = (elapsed_ms / f64::from(duration_ms)) as f32;
    let mut remaining_ms = elapsed_ms as u64;
    for frame in &animation.frames {
        let delay_ms = u64::from(frame.delay_ms.max(1));
        if remaining_ms < delay_ms {
            return Some((frame, progress));
        }
        remaining_ms -= delay_ms;
    }
    None
}

fn effect_duration_ms(animations: &[SkillAnimation]) -> u32 {
    animations.iter().fold(0, |duration, animation| {
        duration.max(
            animation
                .start_delay_ms
                .saturating_add(animation_frame_duration_ms(animation)),
        )
    })
}

fn animation_frame_duration_ms(animation: &SkillAnimation) -> u32 {
    animation.frames.iter().fold(0, |duration, frame| {
        duration.saturating_add(frame.delay_ms.max(1))
    })
}

fn warn(message: &str) {
    web_sys::console::warn_1(&JsValue::from_str(message));
}

#[cfg(test)]
mod tests {
    use oozems_proto::v1::SkillAnimation;
    use oozems_proto::v1::SkillAnimationFrame;
    use oozems_proto::v1::SkillAnimationPlacement;

    use super::ActiveVisual;
    use super::AssetLoadDecision;
    use super::asset_load_decision;
    use super::effect_anchor;
    use super::effect_duration_ms;
    use super::frame_at_time;

    fn animation() -> SkillAnimation {
        SkillAnimation {
            frames: vec![
                SkillAnimationFrame {
                    asset_id: "first".to_owned(),
                    delay_ms: 90,
                    ..SkillAnimationFrame::default()
                },
                SkillAnimationFrame {
                    asset_id: "second".to_owned(),
                    delay_ms: 110,
                    ..SkillAnimationFrame::default()
                },
            ],
            start_delay_ms: 40,
            ..SkillAnimation::default()
        }
    }

    #[test]
    fn one_shot_frames_respect_delay_and_stop_at_the_end() {
        let animation = animation();

        assert!(frame_at_time(&animation, -1.0).is_none());
        assert_eq!(frame_at_time(&animation, 89.0).unwrap().0.asset_id, "first");
        assert_eq!(
            frame_at_time(&animation, 90.0).unwrap().0.asset_id,
            "second"
        );
        assert!(frame_at_time(&animation, 200.0).is_none());
    }

    #[test]
    fn effect_duration_includes_animation_start_delays() {
        assert_eq!(effect_duration_ms(&[animation()]), 240);
    }

    #[test]
    fn incomplete_assets_never_start_an_effect() {
        assert_eq!(asset_load_decision(false, 4_999.0), AssetLoadDecision::Wait);
        assert_eq!(
            asset_load_decision(false, 5_000.0),
            AssetLoadDecision::Discard
        );
        assert_eq!(asset_load_decision(true, 5_000.0), AssetLoadDecision::Start);
    }

    #[test]
    fn projectiles_follow_the_captured_facing_direction() {
        let mut visual = ActiveVisual {
            animations: Vec::new(),
            discarded: false,
            queued_at_ms: 0.0,
            started_at_ms: Some(0.0),
            duration_ms: 100.0,
            origin_x: 300.0,
            origin_y: 200.0,
            facing_left: true,
            target: None,
            sound_url: None,
        };

        assert_eq!(
            effect_anchor(&visual, SkillAnimationPlacement::Projectile, 0.5),
            (220.0, 165.0)
        );
        visual.facing_left = false;
        assert_eq!(
            effect_anchor(&visual, SkillAnimationPlacement::Projectile, 0.5),
            (380.0, 165.0)
        );
    }
}
