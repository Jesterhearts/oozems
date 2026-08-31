use std::ops::RangeInclusive;

use oozems_proto::v1::MapBackground;
use oozems_proto::v1::MapBackgroundFrame;
use oozems_proto::v1::MapBackgroundMode;
use web_sys::HtmlImageElement;

use crate::assets;
use crate::game::Game;

pub(super) fn draw(
    game: &Game,
    front: bool,
    viewport_width: f64,
    viewport_height: f64,
    camera_x: f64,
    camera_y: f64,
) {
    for background in &game.world.map.backgrounds {
        if background.front == front {
            draw_background(
                game,
                background,
                viewport_width,
                viewport_height,
                camera_x,
                camera_y,
            );
        }
    }
}

fn draw_background(
    game: &Game,
    background: &MapBackground,
    viewport_width: f64,
    viewport_height: f64,
    camera_x: f64,
    camera_y: f64,
) {
    let Some(preferred_index) = crate::animation::frame_index(
        background.frames.iter().map(|frame| frame.delay_ms),
        game.clock.now_ms,
        crate::animation::Playback::Loop,
    ) else {
        return;
    };
    let Some(index) = assets::ready_or_fallback_index(
        &game.surface.images,
        background
            .frames
            .iter()
            .map(|frame| frame.asset_id.as_str()),
        preferred_index,
    ) else {
        return;
    };
    let frame = &background.frames[index];
    let Some(image) = assets::ready_image(&game.surface.images, &frame.asset_id) else {
        return;
    };
    let Ok(mode) = MapBackgroundMode::try_from(background.mode) else {
        return;
    };
    let Some((repeat_horizontally, repeat_vertically)) = repeat_axes(mode) else {
        return;
    };

    let repeat_x = effective_interval(background.repeat_x, frame.width);
    let repeat_y = effective_interval(background.repeat_y, frame.height);
    let (shift_x, shift_y) = movement_shift(
        mode,
        background.horizontal_rate,
        background.vertical_rate,
        game.clock.now_ms,
        repeat_x,
        repeat_y,
    );
    let x = parallax_position(
        frame.x,
        background.horizontal_rate,
        camera_x,
        viewport_width,
    ) + shift_x;
    let y =
        parallax_position(frame.y, background.vertical_rate, camera_y, viewport_height) + shift_y;
    let x_indices = copy_indices(
        x,
        f64::from(frame.width),
        repeat_x,
        viewport_width,
        repeat_horizontally,
    );
    let y_indices = copy_indices(
        y,
        f64::from(frame.height),
        repeat_y,
        viewport_height,
        repeat_vertically,
    );
    let (Some(x_indices), Some(y_indices)) = (x_indices, y_indices) else {
        return;
    };

    let context = &game.surface.context;
    context.save();
    context.set_global_alpha(f64::from(background.alpha.min(255)) / 255.0);
    for x_index in x_indices {
        let copy_x = x + f64::from(x_index) * repeat_x;
        for y_index in y_indices.clone() {
            let copy_y = y + f64::from(y_index) * repeat_y;
            draw_frame(context, image, frame, background.flip_x, copy_x, copy_y);
        }
    }
    context.restore();
}

fn draw_frame(
    context: &web_sys::CanvasRenderingContext2d,
    image: &HtmlImageElement,
    frame: &MapBackgroundFrame,
    flip_x: bool,
    x: f64,
    y: f64,
) {
    let width = f64::from(frame.width);
    let height = f64::from(frame.height);
    if flip_x {
        context.save();
        let transformed = context
            .translate(x + width, y)
            .and_then(|()| context.scale(-1.0, 1.0));
        if transformed.is_ok() {
            let _ = context
                .draw_image_with_html_image_element_and_dw_and_dh(image, 0.0, 0.0, width, height);
        }
        context.restore();
    } else {
        let _ =
            context.draw_image_with_html_image_element_and_dw_and_dh(image, x, y, width, height);
    }
}

fn parallax_position(
    base: f32,
    rate: i32,
    camera: f64,
    viewport: f64,
) -> f64 {
    let center = viewport / 2.0;
    f64::from(base) + center + f64::from(rate) * (camera + center) / 100.0
}

fn effective_interval(
    configured: u32,
    frame_size: f32,
) -> f64 {
    if configured == 0 {
        f64::from(frame_size)
    } else {
        f64::from(configured)
    }
}

fn repeat_axes(mode: MapBackgroundMode) -> Option<(bool, bool)> {
    let axes = match mode {
        MapBackgroundMode::Regular => (false, false),
        MapBackgroundMode::HorizontalTiling | MapBackgroundMode::HorizontalMoving => (true, false),
        MapBackgroundMode::VerticalTiling | MapBackgroundMode::VerticalMoving => (false, true),
        MapBackgroundMode::HorizontalAndVerticalTiling
        | MapBackgroundMode::HorizontalMovingWithTiling
        | MapBackgroundMode::VerticalMovingWithTiling => (true, true),
        MapBackgroundMode::Unspecified => return None,
    };
    Some(axes)
}

fn movement_shift(
    mode: MapBackgroundMode,
    horizontal_rate: i32,
    vertical_rate: i32,
    elapsed_ms: f64,
    repeat_x: f64,
    repeat_y: f64,
) -> (f64, f64) {
    match mode {
        MapBackgroundMode::HorizontalMoving | MapBackgroundMode::HorizontalMovingWithTiling
            if repeat_x > 0.0 =>
        {
            (
                f64::from(horizontal_rate) * elapsed_ms / 200.0 % repeat_x,
                0.0,
            )
        }
        MapBackgroundMode::VerticalMoving | MapBackgroundMode::VerticalMovingWithTiling
            if repeat_y > 0.0 =>
        {
            (
                0.0,
                f64::from(vertical_rate) * elapsed_ms / 200.0 % repeat_y,
            )
        }
        _ => (0.0, 0.0),
    }
}

fn copy_indices(
    start: f64,
    size: f64,
    interval: f64,
    viewport: f64,
    repeats: bool,
) -> Option<RangeInclusive<i32>> {
    if !start.is_finite()
        || !size.is_finite()
        || !interval.is_finite()
        || !viewport.is_finite()
        || size <= 0.0
        || viewport <= 0.0
    {
        return None;
    }
    if !repeats || interval <= 0.0 {
        return (start + size >= 0.0 && start <= viewport).then_some(0..=0);
    }

    let first = ((-size - start) / interval).ceil() as i32;
    let last = ((viewport - start) / interval).floor() as i32;
    (first <= last).then_some(first..=last)
}

#[cfg(test)]
mod tests {
    use oozems_proto::v1::MapBackgroundMode;

    use super::copy_indices;
    use super::effective_interval;
    use super::movement_shift;
    use super::parallax_position;
    use super::repeat_axes;

    #[test]
    fn parallax_rates_cover_fixed_partial_and_world_positions() {
        assert_eq!(parallax_position(100.0, 0, 200.0, 800.0), 500.0);
        assert_eq!(parallax_position(500.0, -40, 200.0, 800.0), 660.0);
        assert_eq!(parallax_position(1_100.0, -100, 200.0, 800.0), 900.0);
    }

    #[test]
    fn copy_range_jumps_directly_to_visible_tiles() {
        assert_eq!(copy_indices(100.0, 256.0, 256.0, 800.0, true), Some(-1..=2));
        assert_eq!(
            copy_indices(-1_000_000.0, 256.0, 256.0, 800.0, true),
            Some(3_906..=3_909)
        );
        assert_eq!(copy_indices(900.0, 100.0, 100.0, 800.0, false), None);
        assert_eq!(copy_indices(700.0, 100.0, 100.0, 800.0, false), Some(0..=0));
    }

    #[test]
    fn zero_copy_interval_uses_the_current_frame_size() {
        assert_eq!(effective_interval(0, 256.0), 256.0);
        assert_eq!(effective_interval(300, 256.0), 300.0);
    }

    #[test]
    fn background_modes_select_the_wz_copy_axes() {
        assert_eq!(
            repeat_axes(MapBackgroundMode::Regular),
            Some((false, false))
        );
        assert_eq!(
            repeat_axes(MapBackgroundMode::HorizontalMoving),
            Some((true, false))
        );
        assert_eq!(
            repeat_axes(MapBackgroundMode::VerticalMoving),
            Some((false, true))
        );
        assert_eq!(
            repeat_axes(MapBackgroundMode::HorizontalMovingWithTiling),
            Some((true, true))
        );
        assert_eq!(repeat_axes(MapBackgroundMode::Unspecified), None);
    }

    #[test]
    fn moving_backgrounds_wrap_on_their_copy_interval() {
        assert_eq!(
            movement_shift(
                MapBackgroundMode::HorizontalMoving,
                -5,
                0,
                10_000.0,
                256.0,
                100.0,
            ),
            (-250.0, 0.0)
        );
        assert_eq!(
            movement_shift(
                MapBackgroundMode::VerticalMovingWithTiling,
                0,
                8,
                10_000.0,
                256.0,
                128.0,
            ),
            (0.0, 16.0)
        );
    }
}
