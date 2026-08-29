use oozems_proto::v1::AnimationFrame;

use crate::assets::ready_image;
use crate::game::Game;
use crate::game_gui::CanvasPoint;

pub(super) fn draw(game: &Game) {
    let Some((pointer, frame)) = crate::game::visible_mouse_cursor(game) else {
        return;
    };
    let Some(image) = ready_image(&game.surface.images, &frame.asset_id) else {
        return;
    };
    let (x, y) = cursor_position(pointer, frame);
    let _ = game
        .surface
        .context
        .draw_image_with_html_image_element_and_dw_and_dh(
            image,
            x,
            y,
            f64::from(frame.width),
            f64::from(frame.height),
        );
}

fn cursor_position(
    pointer: CanvasPoint,
    frame: &AnimationFrame,
) -> (f64, f64) {
    (
        f64::from(pointer.x - frame.origin_x),
        f64::from(pointer.y - frame.origin_y),
    )
}

#[cfg(test)]
mod tests {
    use oozems_proto::v1::AnimationFrame;

    use super::cursor_position;
    use crate::game_gui::CanvasPoint;

    #[test]
    fn wz_origin_places_the_cursor_hotspot_at_the_pointer() {
        let frame = AnimationFrame {
            origin_x: 3.0,
            origin_y: 6.0,
            ..AnimationFrame::default()
        };

        assert_eq!(
            cursor_position(CanvasPoint { x: 100.0, y: 80.0 }, &frame),
            (97.0, 74.0)
        );
    }
}
