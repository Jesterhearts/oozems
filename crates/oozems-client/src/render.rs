use oozems_proto::v1::Decoration;
use oozems_proto::v1::PlatformKind;
use oozems_proto::v1::PortalFrame;

use crate::assets::ready_image;
use crate::character_render;
use crate::character_render::CharacterPlacement;
use crate::game::Game;

pub fn draw(game: &Game) {
    let viewport_width = f64::from(game.canvas.width());
    let viewport_height = f64::from(game.canvas.height());
    let player_x = game
        .player
        .position
        .as_ref()
        .map_or(0.0, |position| f64::from(position.x));
    let player_y = game
        .player
        .position
        .as_ref()
        .map_or(0.0, |position| f64::from(position.y));
    let camera_x = camera_x(player_x, viewport_width, f64::from(game.map.width));
    let camera_y = camera_y(player_y, viewport_height, f64::from(game.map.height));

    draw_background(game, viewport_width, viewport_height, camera_x);
    draw_decorations(game, camera_x, camera_y, |layer| layer <= 0);
    draw_platforms(game, camera_x, camera_y);
    draw_portals(game, camera_x, camera_y);
    draw_player(game, camera_x, camera_y);
    draw_decorations(game, camera_x, camera_y, |layer| layer > 0);
    draw_hud(game);
}

fn camera_x(
    player_x: f64,
    viewport_width: f64,
    map_width: f64,
) -> f64 {
    (player_x - viewport_width * 0.45).clamp(0.0, (map_width - viewport_width).max(0.0))
}

fn camera_y(
    player_y: f64,
    viewport_height: f64,
    map_height: f64,
) -> f64 {
    (player_y - viewport_height * 0.55).clamp(0.0, (map_height - viewport_height).max(0.0))
}

fn draw_background(
    game: &Game,
    viewport_width: f64,
    viewport_height: f64,
    camera_x: f64,
) {
    let context = &game.context;
    context.set_fill_style_str("#87c9c0");
    context.fill_rect(0.0, 0.0, viewport_width, viewport_height);

    context.set_fill_style_str("#6da78b");
    context.begin_path();
    context.move_to(0.0, 390.0);
    context.line_to(180.0 - camera_x * 0.08, 250.0);
    context.line_to(420.0 - camera_x * 0.08, 410.0);
    context.line_to(680.0 - camera_x * 0.08, 230.0);
    context.line_to(viewport_width, 410.0);
    context.line_to(viewport_width, viewport_height);
    context.line_to(0.0, viewport_height);
    context.fill();

    context.set_fill_style_str("#4d8066");
    context.begin_path();
    context.move_to(0.0, 465.0);
    context.line_to(210.0 - camera_x * 0.16, 350.0);
    context.line_to(430.0 - camera_x * 0.16, 475.0);
    context.line_to(760.0 - camera_x * 0.16, 330.0);
    context.line_to(viewport_width, 460.0);
    context.line_to(viewport_width, viewport_height);
    context.line_to(0.0, viewport_height);
    context.fill();
}

fn draw_decorations<F>(
    game: &Game,
    camera_x: f64,
    camera_y: f64,
    include: F,
) where
    F: Fn(i32) -> bool,
{
    for decoration in &game.map.decorations {
        if include(decoration.layer) {
            draw_decoration(game, decoration, camera_x, camera_y);
        }
    }
}

fn draw_decoration(
    game: &Game,
    decoration: &Decoration,
    camera_x: f64,
    camera_y: f64,
) {
    draw_sprite(
        game,
        &decoration.asset_id,
        decoration.x,
        decoration.y,
        decoration.width,
        decoration.height,
        decoration.flip_x,
        camera_x,
        camera_y,
    );
}

#[allow(clippy::too_many_arguments)]
fn draw_sprite(
    game: &Game,
    asset_id: &str,
    map_x: f32,
    map_y: f32,
    map_width: f32,
    map_height: f32,
    flip_x: bool,
    camera_x: f64,
    camera_y: f64,
) {
    let x = f64::from(map_x) - camera_x;
    let y = f64::from(map_y) - camera_y;
    let width = f64::from(map_width);
    let height = f64::from(map_height);
    if x + width < 0.0
        || x > f64::from(game.canvas.width())
        || y + height < 0.0
        || y > f64::from(game.canvas.height())
    {
        return;
    }

    let Some(image) = ready_image(&game.images, asset_id) else {
        return;
    };

    if flip_x {
        game.context.save();
        let transformed = game
            .context
            .translate(x + width, y)
            .and_then(|()| game.context.scale(-1.0, 1.0));
        if transformed.is_ok() {
            let _ = game
                .context
                .draw_image_with_html_image_element_and_dw_and_dh(image, 0.0, 0.0, width, height);
        }
        game.context.restore();
    } else {
        let _ = game
            .context
            .draw_image_with_html_image_element_and_dw_and_dh(image, x, y, width, height);
    }
}

fn draw_portals(
    game: &Game,
    camera_x: f64,
    camera_y: f64,
) {
    for portal in &game.map.portals {
        let Some(index) = portal_frame_index(&portal.frames, game.frame_time_ms) else {
            continue;
        };
        let frame = &portal.frames[index];
        draw_sprite(
            game,
            &frame.asset_id,
            frame.x,
            frame.y,
            frame.width,
            frame.height,
            false,
            camera_x,
            camera_y,
        );
    }
}

fn portal_frame_index(
    frames: &[PortalFrame],
    timestamp_ms: f64,
) -> Option<usize> {
    let total_duration = frames
        .iter()
        .map(|frame| u64::from(frame.delay_ms.max(1)))
        .sum::<u64>();
    if total_duration == 0 {
        return None;
    }

    let mut animation_time = timestamp_ms.max(0.0) as u64 % total_duration;
    for (index, frame) in frames.iter().enumerate() {
        let delay = u64::from(frame.delay_ms.max(1));
        if animation_time < delay {
            return Some(index);
        }
        animation_time -= delay;
    }
    Some(frames.len() - 1)
}

fn draw_platforms(
    game: &Game,
    camera_x: f64,
    camera_y: f64,
) {
    for platform in &game.map.platforms {
        if platform.hidden {
            continue;
        }
        let x = f64::from(platform.x) - camera_x;
        let y = f64::from(platform.y) - camera_y;
        let width = f64::from(platform.width);
        let kind = PlatformKind::try_from(platform.kind).unwrap_or(PlatformKind::Unspecified);

        match kind {
            PlatformKind::Ground => {
                game.context.set_fill_style_str("#5b3d2b");
                game.context.fill_rect(x, y, width, 150.0);
                game.context.set_fill_style_str("#86ad48");
                game.context.fill_rect(x, y, width, 11.0);
                game.context.set_fill_style_str("#b4cb62");
                game.context.fill_rect(x, y, width, 4.0);
            }
            PlatformKind::Wood | PlatformKind::Unspecified => {
                game.context.set_fill_style_str("#4b3027");
                game.context.fill_rect(x, y - 4.0, width, 14.0);
                game.context.set_fill_style_str("#b8793e");
                game.context.fill_rect(x + 3.0, y - 7.0, width - 6.0, 8.0);
            }
        }
    }
}

fn draw_player(
    game: &Game,
    camera_x: f64,
    camera_y: f64,
) {
    let Some(position) = &game.player.position else {
        return;
    };
    let x = f64::from(position.x) - camera_x;
    let y = f64::from(position.y) - camera_y;

    game.context.set_fill_style_str("rgba(29, 45, 43, 0.25)");
    game.context.fill_rect(x - 23.0, y - 3.0, 46.0, 6.0);
    character_render::draw_character(
        &game.context,
        &game.images,
        &game.character_sprites,
        game.frame_time_ms,
        CharacterPlacement {
            anchor_x: x,
            anchor_y: y,
            scale: 1.0,
            facing_left: game.facing_left,
        },
    );
}

fn draw_hud(game: &Game) {
    game.context.set_fill_style_str("rgba(28, 45, 44, 0.82)");
    game.context.fill_rect(16.0, 16.0, 260.0, 66.0);
    game.context.set_fill_style_str("#fff8d8");
    game.context.set_font("bold 18px monospace");
    let _ = game.context.fill_text(
        &format!("{}  Lv.{}", game.player.name, game.player.level),
        28.0,
        43.0,
    );
    game.context.set_font("14px monospace");
    let _ = game.context.fill_text(&game.map.name, 28.0, 66.0);
}

#[cfg(test)]
mod tests {
    use oozems_proto::v1::PortalFrame;

    use super::portal_frame_index;

    #[test]
    fn portal_animation_uses_each_frame_delay() {
        let frames = vec![
            PortalFrame {
                delay_ms: 100,
                ..PortalFrame::default()
            },
            PortalFrame {
                delay_ms: 200,
                ..PortalFrame::default()
            },
        ];

        assert_eq!(portal_frame_index(&frames, 99.0), Some(0));
        assert_eq!(portal_frame_index(&frames, 100.0), Some(1));
        assert_eq!(portal_frame_index(&frames, 299.0), Some(1));
        assert_eq!(portal_frame_index(&frames, 300.0), Some(0));
    }
}
