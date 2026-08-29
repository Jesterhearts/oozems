use oozems_proto::v1::GameGui;
use oozems_proto::v1::GuiLayout;
use oozems_proto::v1::GuiWindow;

use super::CanvasPoint;
use super::CanvasRect;
use super::GuiState;
use super::rect_contains;
use super::valid_layout;

const WINDOW_TITLE_HEIGHT: f32 = 20.0;
const DRAG_THRESHOLD: f32 = 3.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowKind {
    Stats,
    Equipment,
    Inventory,
    Skills,
    KeyConfig,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WindowPlacements {
    stats: CanvasPoint,
    equipment: CanvasPoint,
    inventory: CanvasPoint,
    skills: CanvasPoint,
    key_config: CanvasPoint,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowPlacement<'a> {
    pub window: &'a GuiWindow,
    pub layout: &'a GuiLayout,
    pub origin: CanvasPoint,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowDrag {
    kind: WindowKind,
    pointer_start: CanvasPoint,
    origin_start: CanvasPoint,
    moved: bool,
}

pub fn resolve_window<'a>(
    gui: &'a GameGui,
    placements: WindowPlacements,
    kind: WindowKind,
    viewport_width: f32,
    viewport_height: f32,
) -> Option<WindowPlacement<'a>> {
    let window = window_for_kind(gui, kind)?;
    let layout = window
        .layout
        .as_ref()
        .filter(|layout| valid_layout(layout))?;
    let origin = resolve_window_origin(
        window,
        layout,
        window_offset(placements, kind),
        viewport_width,
        viewport_height,
    );
    Some(WindowPlacement {
        window,
        layout,
        origin,
    })
}

pub fn set_window_offset(
    placements: &mut WindowPlacements,
    kind: WindowKind,
    offset: CanvasPoint,
) {
    if !offset.x.is_finite() || !offset.y.is_finite() {
        return;
    }
    *offset_for_kind(placements, kind) = offset;
}

pub fn begin_window_drag(
    state: GuiState,
    gui: &GameGui,
    viewport_width: f32,
    viewport_height: f32,
    point: CanvasPoint,
) -> Option<WindowDrag> {
    let kind = frontmost_window_at_point(state, gui, viewport_width, viewport_height, point)?;
    let placement = resolve_window(
        gui,
        state.window_placements,
        kind,
        viewport_width,
        viewport_height,
    )?;
    let drag_handle = super::named_region(placement.layout, "window-drag-handle");
    let (x, y, width, height) = drag_handle.map_or(
        (
            0.0,
            0.0,
            placement.layout.width,
            placement.layout.height.min(WINDOW_TITLE_HEIGHT),
        ),
        |region| (region.x, region.y, region.width, region.height),
    );
    rect_contains(
        CanvasRect {
            x: placement.origin.x + x,
            y: placement.origin.y + y,
            width,
            height,
        },
        point,
    )
    .then_some(WindowDrag {
        kind,
        pointer_start: point,
        origin_start: placement.origin,
        moved: false,
    })
}

pub fn close_topmost_window(state: &mut GuiState) -> bool {
    let Some(kind) = visible_windows_front_to_back(*state).next() else {
        return false;
    };
    match kind {
        WindowKind::Stats => state.stats_open = false,
        WindowKind::Equipment => state.equipment_open = false,
        WindowKind::Inventory => state.inventory_open = false,
        WindowKind::Skills => state.skills_open = false,
        WindowKind::KeyConfig => state.key_config_open = false,
    }
    true
}

pub(super) fn frontmost_window_at_point(
    state: GuiState,
    gui: &GameGui,
    viewport_width: f32,
    viewport_height: f32,
    point: CanvasPoint,
) -> Option<WindowKind> {
    visible_windows_front_to_back(state).find(|kind| {
        resolve_window(
            gui,
            state.window_placements,
            *kind,
            viewport_width,
            viewport_height,
        )
        .is_some_and(|placement| {
            rect_contains(
                CanvasRect {
                    x: placement.origin.x,
                    y: placement.origin.y,
                    width: placement.layout.width,
                    height: placement.layout.height,
                },
                point,
            )
        })
    })
}

pub fn move_window_drag(
    placements: &mut WindowPlacements,
    gui: &GameGui,
    drag: &mut WindowDrag,
    viewport_width: f32,
    viewport_height: f32,
    point: CanvasPoint,
) {
    let delta = CanvasPoint {
        x: point.x - drag.pointer_start.x,
        y: point.y - drag.pointer_start.y,
    };
    if !delta.x.is_finite() || !delta.y.is_finite() {
        return;
    }
    if !drag.moved && delta.x * delta.x + delta.y * delta.y < DRAG_THRESHOLD * DRAG_THRESHOLD {
        return;
    }
    let Some(window) = window_for_kind(gui, drag.kind) else {
        return;
    };
    let Some(layout) = window.layout.as_ref().filter(|layout| valid_layout(layout)) else {
        return;
    };
    drag.moved = true;
    let desired_offset = CanvasPoint {
        x: drag.origin_start.x + delta.x - window.x,
        y: drag.origin_start.y + delta.y - window.y,
    };
    let origin = resolve_window_origin(
        window,
        layout,
        desired_offset,
        viewport_width,
        viewport_height,
    );
    set_window_offset(
        placements,
        drag.kind,
        CanvasPoint {
            x: origin.x - window.x,
            y: origin.y - window.y,
        },
    );
}

pub fn finish_window_drag(drag: WindowDrag) -> bool {
    drag.moved
}

fn resolve_window_origin(
    window: &GuiWindow,
    layout: &GuiLayout,
    offset: CanvasPoint,
    viewport_width: f32,
    viewport_height: f32,
) -> CanvasPoint {
    let viewport_width = finite_nonnegative(viewport_width);
    let viewport_height = finite_nonnegative(viewport_height);
    let maximum_x = if layout.width <= viewport_width {
        viewport_width - layout.width
    } else {
        0.0
    };
    let title_height = super::named_region(layout, "window-drag-handle")
        .map_or(layout.height.min(WINDOW_TITLE_HEIGHT), |region| {
            (region.y + region.height).min(layout.height)
        });
    let maximum_y = if layout.height <= viewport_height {
        viewport_height - layout.height
    } else {
        (viewport_height - title_height).max(0.0)
    };
    CanvasPoint {
        x: (window.x + offset.x).clamp(0.0, maximum_x),
        y: (window.y + offset.y).clamp(0.0, maximum_y),
    }
}

fn finite_nonnegative(value: f32) -> f32 {
    if value.is_finite() {
        value.max(0.0)
    } else {
        0.0
    }
}

fn window_offset(
    placements: WindowPlacements,
    kind: WindowKind,
) -> CanvasPoint {
    match kind {
        WindowKind::Stats => placements.stats,
        WindowKind::Equipment => placements.equipment,
        WindowKind::Inventory => placements.inventory,
        WindowKind::Skills => placements.skills,
        WindowKind::KeyConfig => placements.key_config,
    }
}

fn offset_for_kind(
    placements: &mut WindowPlacements,
    kind: WindowKind,
) -> &mut CanvasPoint {
    match kind {
        WindowKind::Stats => &mut placements.stats,
        WindowKind::Equipment => &mut placements.equipment,
        WindowKind::Inventory => &mut placements.inventory,
        WindowKind::Skills => &mut placements.skills,
        WindowKind::KeyConfig => &mut placements.key_config,
    }
}

fn window_for_kind(
    gui: &GameGui,
    kind: WindowKind,
) -> Option<&GuiWindow> {
    match kind {
        WindowKind::Stats => gui.stat_window.as_ref(),
        WindowKind::Equipment => gui.equipment_window.as_ref(),
        WindowKind::Inventory => gui.inventory_window.as_ref(),
        WindowKind::Skills => gui.skill_window.as_ref(),
        WindowKind::KeyConfig => gui.key_config_window.as_ref(),
    }
}

fn visible_windows_front_to_back(state: GuiState) -> impl Iterator<Item = WindowKind> {
    [
        (state.key_config_open, WindowKind::KeyConfig),
        (state.skills_open, WindowKind::Skills),
        (state.inventory_open, WindowKind::Inventory),
        (state.equipment_open, WindowKind::Equipment),
        (state.stats_open, WindowKind::Stats),
    ]
    .into_iter()
    .filter_map(|(open, kind)| open.then_some(kind))
}

#[cfg(test)]
mod tests {
    use oozems_proto::v1::GameGui;
    use oozems_proto::v1::GuiLayout;
    use oozems_proto::v1::GuiRegion;
    use oozems_proto::v1::GuiSprite;
    use oozems_proto::v1::GuiWindow;

    use super::WindowKind;
    use super::begin_window_drag;
    use super::close_topmost_window;
    use super::finish_window_drag;
    use super::frontmost_window_at_point;
    use super::move_window_drag;
    use super::resolve_window;
    use super::set_window_offset;
    use crate::game_gui::CanvasPoint;
    use crate::game_gui::GuiState;

    #[test]
    fn native_position_is_preserved_until_the_client_adds_an_offset() {
        let gui = gui_fixture();
        let mut state = GuiState::default();

        assert_eq!(
            origin(&gui, state, 960.0, 600.0),
            CanvasPoint { x: 20.0, y: 80.0 }
        );

        set_window_offset(
            &mut state.window_placements,
            WindowKind::Stats,
            CanvasPoint { x: 35.0, y: 45.0 },
        );
        assert_eq!(
            origin(&gui, state, 960.0, 600.0),
            CanvasPoint { x: 55.0, y: 125.0 }
        );
    }

    #[test]
    fn position_resolution_keeps_the_title_reachable_in_small_canvases() {
        let gui = gui_fixture();
        let mut state = GuiState::default();
        set_window_offset(
            &mut state.window_placements,
            WindowKind::Stats,
            CanvasPoint {
                x: 2_000.0,
                y: 2_000.0,
            },
        );

        assert_eq!(
            origin(&gui, state, 100.0, 50.0),
            CanvasPoint { x: 0.0, y: 30.0 }
        );
    }

    #[test]
    fn title_drag_updates_the_client_offset_and_clamps_the_result() {
        let gui = gui_fixture();
        let mut state = GuiState {
            stats_open: true,
            ..GuiState::default()
        };
        let mut drag =
            begin_window_drag(state, &gui, 300.0, 300.0, CanvasPoint { x: 30.0, y: 90.0 })
                .expect("stat title drag");

        move_window_drag(
            &mut state.window_placements,
            &gui,
            &mut drag,
            300.0,
            300.0,
            CanvasPoint { x: 500.0, y: 500.0 },
        );

        assert!(finish_window_drag(drag));
        assert_eq!(
            origin(&gui, state, 300.0, 300.0),
            CanvasPoint { x: 125.0, y: 100.0 }
        );
    }

    #[test]
    fn clicks_below_the_title_do_not_start_window_dragging() {
        let gui = gui_fixture();
        let state = GuiState {
            stats_open: true,
            ..GuiState::default()
        };

        assert!(
            begin_window_drag(state, &gui, 960.0, 600.0, CanvasPoint { x: 30.0, y: 110.0 },)
                .is_none()
        );
    }

    #[test]
    fn title_drag_follows_the_authored_drag_region() {
        let mut gui = gui_fixture();
        gui.stat_window
            .as_mut()
            .and_then(|window| window.layout.as_mut())
            .expect("stat layout")
            .regions
            .push(GuiRegion {
                name: "window-drag-handle".to_owned(),
                x: 40.0,
                y: 40.0,
                width: 80.0,
                height: 20.0,
            });
        let state = GuiState {
            stats_open: true,
            ..GuiState::default()
        };

        assert!(
            begin_window_drag(state, &gui, 960.0, 600.0, CanvasPoint { x: 30.0, y: 90.0 })
                .is_none()
        );
        assert!(
            begin_window_drag(state, &gui, 960.0, 600.0, CanvasPoint { x: 61.0, y: 121.0 })
                .is_some()
        );
    }

    #[test]
    fn a_front_window_body_blocks_the_covered_window_title() {
        let mut gui = gui_fixture();
        gui.inventory_window = Some(GuiWindow {
            x: 0.0,
            y: 0.0,
            layout: Some(GuiLayout {
                width: 100.0,
                height: 100.0,
                background: Some(GuiSprite {
                    name: "inventory-background".to_owned(),
                    asset_id: "inventory-background-asset".to_owned(),
                    width: 100.0,
                    height: 100.0,
                    ..GuiSprite::default()
                }),
                ..GuiLayout::default()
            }),
        });
        let state = GuiState {
            stats_open: true,
            inventory_open: true,
            ..GuiState::default()
        };
        let point = CanvasPoint { x: 30.0, y: 90.0 };

        assert_eq!(
            frontmost_window_at_point(state, &gui, 960.0, 600.0, point),
            Some(WindowKind::Inventory)
        );
        assert!(begin_window_drag(state, &gui, 960.0, 600.0, point).is_none());
    }

    #[test]
    fn windows_close_one_at_a_time_from_front_to_back() {
        let mut state = GuiState {
            stats_open: true,
            equipment_open: true,
            inventory_open: true,
            key_config_open: true,
            skills_open: true,
            ..GuiState::default()
        };

        for kind in [
            WindowKind::KeyConfig,
            WindowKind::Skills,
            WindowKind::Inventory,
            WindowKind::Equipment,
            WindowKind::Stats,
        ] {
            assert!(close_topmost_window(&mut state));
            assert!(!window_is_open(state, kind));
        }
        assert!(!close_topmost_window(&mut state));
    }

    fn window_is_open(
        state: GuiState,
        kind: WindowKind,
    ) -> bool {
        match kind {
            WindowKind::Stats => state.stats_open,
            WindowKind::Equipment => state.equipment_open,
            WindowKind::Inventory => state.inventory_open,
            WindowKind::Skills => state.skills_open,
            WindowKind::KeyConfig => state.key_config_open,
        }
    }

    fn origin(
        gui: &GameGui,
        state: GuiState,
        viewport_width: f32,
        viewport_height: f32,
    ) -> CanvasPoint {
        resolve_window(
            gui,
            state.window_placements,
            WindowKind::Stats,
            viewport_width,
            viewport_height,
        )
        .expect("stat placement")
        .origin
    }

    fn gui_fixture() -> GameGui {
        GameGui {
            stat_window: Some(GuiWindow {
                x: 20.0,
                y: 80.0,
                layout: Some(GuiLayout {
                    width: 175.0,
                    height: 200.0,
                    background: Some(GuiSprite {
                        name: "stat-background".to_owned(),
                        asset_id: "stat-background-asset".to_owned(),
                        width: 175.0,
                        height: 200.0,
                        ..GuiSprite::default()
                    }),
                    ..GuiLayout::default()
                }),
            }),
            ..GameGui::default()
        }
    }
}
