use oozems_proto::v1::GameGui;

use crate::game_gui;
use crate::game_gui::CanvasPoint;
use crate::game_gui::PointerButton;

pub(crate) const FALLBACK_WINDOW_X: f32 = 348.0;
pub(crate) const FALLBACK_WINDOW_Y: f32 = 234.0;
pub(crate) const FALLBACK_WINDOW_WIDTH: f32 = 264.0;
pub(crate) const FALLBACK_WINDOW_HEIGHT: f32 = 132.0;
pub(crate) const FALLBACK_BUTTON_X: f32 = 100.0;
pub(crate) const FALLBACK_BUTTON_Y: f32 = 92.0;
pub(crate) const FALLBACK_BUTTON_WIDTH: f32 = 65.0;
pub(crate) const FALLBACK_BUTTON_HEIGHT: f32 = 24.0;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct DeathUiState {
    pub(crate) started_ms: Option<f64>,
    pub(crate) respawn_requested: bool,
    pub(crate) respawn_in_flight: bool,
}

pub(crate) fn synchronize(
    state: &mut DeathUiState,
    dead: bool,
    timestamp_ms: f64,
) {
    if dead {
        state.started_ms.get_or_insert(timestamp_ms);
    } else {
        *state = DeathUiState::default();
    }
}

pub(crate) fn is_open(state: DeathUiState) -> bool {
    state.started_ms.is_some()
}

pub(crate) fn click_requests_respawn(
    gui: &GameGui,
    state: &mut DeathUiState,
    point: CanvasPoint,
    button: PointerButton,
) -> bool {
    if button != PointerButton::Left || !respawn_button_at(gui, *state, point) {
        return false;
    }
    state.respawn_requested = true;
    true
}

pub(crate) fn respawn_button_at(
    gui: &GameGui,
    state: DeathUiState,
    point: CanvasPoint,
) -> bool {
    if !is_open(state) || state.respawn_requested {
        return false;
    }
    let (x, y, width, height) = ok_region(gui);
    point.x >= x && point.x <= x + width && point.y >= y && point.y <= y + height
}

pub(crate) fn allow_retry(state: &mut DeathUiState) {
    state.respawn_requested = false;
    state.respawn_in_flight = false;
}

pub(crate) fn should_dispatch_respawn(state: DeathUiState) -> bool {
    state.respawn_requested && !state.respawn_in_flight
}

fn ok_region(gui: &GameGui) -> (f32, f32, f32, f32) {
    let Some(window) = gui.death_notice_window.as_ref() else {
        return fallback_ok_region();
    };
    let Some(layout) = window
        .layout
        .as_ref()
        .filter(|layout| game_gui::valid_layout(layout))
    else {
        return fallback_ok_region();
    };
    let Some(region) = game_gui::named_region(layout, "death-notice-ok") else {
        return fallback_ok_region();
    };
    (
        window.x + region.x,
        window.y + region.y,
        region.width,
        region.height,
    )
}

fn fallback_ok_region() -> (f32, f32, f32, f32) {
    (
        FALLBACK_WINDOW_X + FALLBACK_BUTTON_X,
        FALLBACK_WINDOW_Y + FALLBACK_BUTTON_Y,
        FALLBACK_BUTTON_WIDTH,
        FALLBACK_BUTTON_HEIGHT,
    )
}

#[cfg(test)]
mod tests {
    use oozems_proto::v1::GameGui;
    use oozems_proto::v1::GuiLayout;
    use oozems_proto::v1::GuiRegion;
    use oozems_proto::v1::GuiSprite;
    use oozems_proto::v1::GuiWindow;

    use super::DeathUiState;
    use super::click_requests_respawn;
    use super::should_dispatch_respawn;
    use super::synchronize;
    use crate::game_gui::CanvasPoint;
    use crate::game_gui::PointerButton;

    #[test]
    fn death_transition_opens_once_and_living_state_closes_the_notice() {
        let mut state = DeathUiState::default();

        synchronize(&mut state, true, 100.0);
        synchronize(&mut state, true, 300.0);
        assert_eq!(state.started_ms, Some(100.0));

        synchronize(&mut state, false, 400.0);
        assert_eq!(state, DeathUiState::default());
    }

    #[test]
    fn only_one_left_click_in_the_ok_region_requests_respawn() {
        let gui = gui();
        let mut state = DeathUiState {
            started_ms: Some(100.0),
            respawn_requested: false,
            respawn_in_flight: false,
        };

        assert!(!click_requests_respawn(
            &gui,
            &mut state,
            CanvasPoint { x: 41.0, y: 61.0 },
            PointerButton::Right,
        ));
        assert!(click_requests_respawn(
            &gui,
            &mut state,
            CanvasPoint { x: 41.0, y: 61.0 },
            PointerButton::Left,
        ));
        assert!(!click_requests_respawn(
            &gui,
            &mut state,
            CanvasPoint { x: 41.0, y: 61.0 },
            PointerButton::Left,
        ));
        assert!(should_dispatch_respawn(state));
        state.respawn_in_flight = true;
        assert!(!should_dispatch_respawn(state));
    }

    fn gui() -> GameGui {
        GameGui {
            death_notice_window: Some(GuiWindow {
                x: 10.0,
                y: 20.0,
                layout: Some(GuiLayout {
                    width: 100.0,
                    height: 80.0,
                    background: Some(GuiSprite {
                        name: "background".to_owned(),
                        asset_id: "background".to_owned(),
                        width: 100.0,
                        height: 80.0,
                        ..GuiSprite::default()
                    }),
                    regions: vec![GuiRegion {
                        name: "death-notice-ok".to_owned(),
                        x: 30.0,
                        y: 40.0,
                        width: 20.0,
                        height: 10.0,
                    }],
                    ..GuiLayout::default()
                }),
            }),
            ..GameGui::default()
        }
    }
}
