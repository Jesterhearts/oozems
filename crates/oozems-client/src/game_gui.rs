use oozems_proto::v1::GameGui;
use oozems_proto::v1::GuiLayout;
use oozems_proto::v1::GuiSprite;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GuiState {
    pub stats_open: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CanvasPoint {
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct CanvasRect {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GuiAction {
    ToggleStats,
    CloseStats,
}

pub fn handle_click(
    state: &mut GuiState,
    gui: &GameGui,
    viewport_width: f32,
    viewport_height: f32,
    point: CanvasPoint,
) -> bool {
    let Some(action) = click_action(*state, gui, viewport_width, viewport_height, point) else {
        return false;
    };

    match action {
        GuiAction::ToggleStats => state.stats_open = !state.stats_open,
        GuiAction::CloseStats => state.stats_open = false,
    }
    true
}

pub fn canvas_point(
    offset_x: i32,
    offset_y: i32,
    canvas_width: u32,
    canvas_height: u32,
    client_width: i32,
    client_height: i32,
) -> Option<CanvasPoint> {
    if offset_x < 0
        || offset_y < 0
        || canvas_width == 0
        || canvas_height == 0
        || client_width <= 0
        || client_height <= 0
    {
        return None;
    }
    Some(CanvasPoint {
        x: offset_x as f32 * canvas_width as f32 / client_width as f32,
        y: offset_y as f32 * canvas_height as f32 / client_height as f32,
    })
}

pub fn sprite_screen_x(
    viewport_width: f32,
    layout_width: f32,
    sprite: &GuiSprite,
) -> f32 {
    if sprite.anchor_right {
        viewport_width - (layout_width - sprite.x)
    } else {
        sprite.x
    }
}

pub fn status_bar_top(
    viewport_height: f32,
    layout_height: f32,
) -> f32 {
    (viewport_height - layout_height).max(0.0)
}

pub fn status_sprite_visible(
    state: GuiState,
    sprite: &GuiSprite,
) -> bool {
    sprite.name != "stats-pressed" || state.stats_open
}

pub fn valid_layout(layout: &GuiLayout) -> bool {
    layout.width.is_finite()
        && layout.height.is_finite()
        && layout.width > 0.0
        && layout.height > 0.0
        && layout
            .background
            .as_ref()
            .is_some_and(|sprite| valid_sprite(sprite, layout.width, layout.height))
        && layout
            .sprites
            .iter()
            .all(|sprite| valid_sprite(sprite, layout.width, layout.height))
}

fn click_action(
    state: GuiState,
    gui: &GameGui,
    viewport_width: f32,
    viewport_height: f32,
    point: CanvasPoint,
) -> Option<GuiAction> {
    if state.stats_open && stat_close_rect(gui).is_some_and(|rect| rect_contains(rect, point)) {
        return Some(GuiAction::CloseStats);
    }
    stat_button_rect(gui, viewport_width, viewport_height)
        .filter(|rect| rect_contains(*rect, point))
        .map(|_| GuiAction::ToggleStats)
}

fn stat_button_rect(
    gui: &GameGui,
    viewport_width: f32,
    viewport_height: f32,
) -> Option<CanvasRect> {
    let layout = gui
        .status_bar
        .as_ref()
        .filter(|layout| valid_layout(layout))?;
    let sprite = named_sprite(layout, "stats")?;
    Some(CanvasRect {
        x: sprite_screen_x(viewport_width, layout.width, sprite),
        y: status_bar_top(viewport_height, layout.height) + sprite.y,
        width: sprite.width,
        height: sprite.height,
    })
}

fn stat_close_rect(gui: &GameGui) -> Option<CanvasRect> {
    let window = gui.stat_window.as_ref()?;
    let layout = window
        .layout
        .as_ref()
        .filter(|layout| valid_layout(layout))?;
    let sprite = named_sprite(layout, "stat-close")?;
    Some(CanvasRect {
        x: window.x + sprite.x,
        y: window.y + sprite.y,
        width: sprite.width,
        height: sprite.height,
    })
}

fn named_sprite<'a>(
    layout: &'a GuiLayout,
    name: &str,
) -> Option<&'a GuiSprite> {
    layout.sprites.iter().find(|sprite| sprite.name == name)
}

fn valid_sprite(
    sprite: &GuiSprite,
    layout_width: f32,
    layout_height: f32,
) -> bool {
    let values = [sprite.x, sprite.y, sprite.width, sprite.height];
    !sprite.asset_id.is_empty()
        && values.iter().all(|value| value.is_finite())
        && sprite.x >= 0.0
        && sprite.y >= 0.0
        && sprite.width > 0.0
        && sprite.height > 0.0
        && sprite.x + sprite.width <= layout_width
        && sprite.y + sprite.height <= layout_height
}

fn rect_contains(
    rect: CanvasRect,
    point: CanvasPoint,
) -> bool {
    point.x >= rect.x
        && point.x < rect.x + rect.width
        && point.y >= rect.y
        && point.y < rect.y + rect.height
}

#[cfg(test)]
mod tests {
    use oozems_proto::v1::GameGui;
    use oozems_proto::v1::GuiLayout;
    use oozems_proto::v1::GuiSprite;
    use oozems_proto::v1::GuiWindow;

    use super::CanvasPoint;
    use super::GuiState;
    use super::canvas_point;
    use super::handle_click;
    use super::sprite_screen_x;
    use super::status_sprite_visible;
    use super::valid_layout;

    #[test]
    fn stat_button_toggles_the_window_and_close_hides_it() {
        let gui = gui_fixture();
        let mut state = GuiState::default();

        assert!(handle_click(
            &mut state,
            &gui,
            960.0,
            600.0,
            CanvasPoint { x: 635.0, y: 540.0 },
        ));
        assert!(state.stats_open);
        assert!(handle_click(
            &mut state,
            &gui,
            960.0,
            600.0,
            CanvasPoint { x: 181.0, y: 86.0 },
        ));
        assert!(!state.stats_open);
    }

    #[test]
    fn css_pointer_coordinates_scale_to_the_canvas() {
        assert_eq!(
            canvas_point(480, 300, 960, 600, 480, 300),
            Some(CanvasPoint { x: 960.0, y: 600.0 })
        );
        assert_eq!(canvas_point(10, 10, 960, 600, 0, 300), None);
    }

    #[test]
    fn right_anchored_sprites_follow_the_viewport_edge() {
        let sprite = GuiSprite {
            x: 649.0,
            anchor_right: true,
            ..GuiSprite::default()
        };

        assert_eq!(sprite_screen_x(960.0, 800.0, &sprite), 809.0);
    }

    #[test]
    fn pressed_stat_sprite_is_visible_only_while_the_window_is_open() {
        let normal = sprite("stats", 634.0, 88.0, 28.0, 20.0);
        let pressed = sprite("stats-pressed", 634.0, 88.0, 28.0, 20.0);

        assert!(status_sprite_visible(GuiState::default(), &normal));
        assert!(!status_sprite_visible(GuiState::default(), &pressed));
        assert!(status_sprite_visible(
            GuiState { stats_open: true },
            &pressed
        ));
    }

    #[test]
    fn invalid_layouts_are_rejected_at_the_client_boundary() {
        let gui = gui_fixture();
        let valid = gui.status_bar.expect("status bar");
        let mut invalid = valid.clone();
        invalid.width = f32::NAN;

        assert!(valid_layout(&valid));
        assert!(!valid_layout(&invalid));
    }

    fn gui_fixture() -> GameGui {
        GameGui {
            status_bar: Some(GuiLayout {
                width: 800.0,
                height: 151.0,
                background: Some(sprite("background", 0.0, 80.0, 800.0, 71.0)),
                sprites: vec![sprite("stats", 634.0, 88.0, 28.0, 20.0)],
            }),
            stat_window: Some(GuiWindow {
                x: 20.0,
                y: 80.0,
                layout: Some(GuiLayout {
                    width: 175.0,
                    height: 347.0,
                    background: Some(sprite("stat-background", 0.0, 0.0, 175.0, 347.0)),
                    sprites: vec![sprite("stat-close", 160.0, 5.0, 10.0, 10.0)],
                }),
            }),
            ..GameGui::default()
        }
    }

    fn sprite(
        name: &str,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    ) -> GuiSprite {
        GuiSprite {
            name: name.to_owned(),
            asset_id: format!("asset-{name}"),
            x,
            y,
            width,
            height,
            anchor_right: false,
        }
    }
}
