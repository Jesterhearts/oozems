use std::collections::HashMap;

use oozems_proto::v1::AnimationFrame;
use wasm_bindgen::JsValue;

use crate::animation;
use crate::animation::Playback;
use crate::assets;
use crate::assets::BrowserAsset;
use crate::assets::ready_image;
use crate::game::Game;

const ASSET_READY_TIMEOUT_MS: f64 = 5_000.0;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct LevelUpEffectState {
    queued_at_ms: Option<f64>,
    started_ms: Option<f64>,
}

pub(crate) fn start(
    state: &mut LevelUpEffectState,
    timestamp_ms: f64,
) {
    state.queued_at_ms = Some(timestamp_ms);
    state.started_ms = None;
}

pub(crate) fn update(
    state: &mut LevelUpEffectState,
    frames: &[AnimationFrame],
    images: &HashMap<String, BrowserAsset>,
    timestamp_ms: f64,
) {
    let Some(queued_at_ms) = state.queued_at_ms else {
        return;
    };
    if frames.is_empty() {
        *state = LevelUpEffectState::default();
        return;
    }
    let ready = assets::images_ready(images, frames.iter().map(|frame| frame.asset_id.as_str()));
    match asset_load_decision(ready, timestamp_ms - queued_at_ms) {
        AssetLoadDecision::Wait => {}
        AssetLoadDecision::Start => {
            state.queued_at_ms = None;
            state.started_ms = Some(timestamp_ms);
        }
        AssetLoadDecision::Discard => {
            *state = LevelUpEffectState::default();
            warn("Discarded the level-up animation because its assets did not load");
        }
    }
}

pub(crate) fn draw(game: &Game) {
    let Some(started_ms) = game.ui.level_up.started_ms else {
        return;
    };
    let frames = &game.ui.gui.level_up_frames;
    let elapsed_ms = (game.clock.now_ms - started_ms).max(0.0);
    let Some(preferred_index) = animation::frame_index(
        frames.iter().map(|frame| frame.delay_ms),
        elapsed_ms,
        Playback::Once,
    ) else {
        return;
    };
    let Some(index) = assets::ready_or_fallback_index(
        &game.surface.images,
        frames.iter().map(|frame| frame.asset_id.as_str()),
        preferred_index,
    ) else {
        return;
    };
    let frame = &frames[index];
    let Some(image) = ready_image(&game.surface.images, &frame.asset_id) else {
        return;
    };
    let (anchor_x, anchor_y) = crate::render::player_canvas_position(game).unwrap_or_else(|| {
        (
            f64::from(game.surface.canvas.width()) / 2.0,
            f64::from(game.surface.canvas.height()) * 2.0 / 3.0,
        )
    });
    let _ = game
        .surface
        .context
        .draw_image_with_html_image_element_and_dw_and_dh(
            image,
            anchor_x - f64::from(frame.origin_x),
            anchor_y - f64::from(frame.origin_y),
            f64::from(frame.width),
            f64::from(frame.height),
        );
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

fn warn(message: &str) {
    web_sys::console::warn_1(&JsValue::from_str(message));
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::AssetLoadDecision;
    use super::LevelUpEffectState;
    use super::asset_load_decision;
    use super::start;
    use super::update;

    #[test]
    fn a_new_level_restarts_an_active_effect() {
        let mut state = LevelUpEffectState::default();

        start(&mut state, 100.0);
        state.started_ms = Some(150.0);
        start(&mut state, 250.0);

        assert_eq!(state.queued_at_ms, Some(250.0));
        assert_eq!(state.started_ms, None);
    }

    #[test]
    fn playback_clock_waits_for_assets_and_failed_loads_time_out() {
        assert_eq!(asset_load_decision(false, 4_999.0), AssetLoadDecision::Wait);
        assert_eq!(asset_load_decision(true, 5_000.0), AssetLoadDecision::Start);
        assert_eq!(
            asset_load_decision(false, 5_000.0),
            AssetLoadDecision::Discard
        );
    }

    #[test]
    fn an_empty_projection_cancels_a_pending_effect() {
        let mut state = LevelUpEffectState::default();
        start(&mut state, 100.0);

        update(&mut state, &[], &HashMap::new(), 200.0);

        assert_eq!(state, LevelUpEffectState::default());
    }
}
