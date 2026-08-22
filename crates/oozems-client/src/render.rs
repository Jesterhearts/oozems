use oozems_proto::v1::Decoration;
use oozems_proto::v1::GuiLayout;
use oozems_proto::v1::GuiSprite;
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
        game.character_animation,
        game.frame_time_ms - game.character_animation_started_ms,
        CharacterPlacement {
            anchor_x: x,
            anchor_y: y,
            scale: 1.0,
            facing_left: game.facing_left,
        },
    );
}

fn draw_hud(game: &Game) {
    if draw_wz_hud(game) {
        return;
    }
    draw_fallback_hud(game);
}

fn draw_wz_hud(game: &Game) -> bool {
    let Some(layout) = game
        .gui
        .status_bar
        .as_ref()
        .filter(|layout| valid_layout(layout))
    else {
        return false;
    };
    let Some(background) = layout.background.as_ref() else {
        return false;
    };
    let Some(background_image) = ready_image(&game.images, &background.asset_id) else {
        return false;
    };
    let viewport_width = f64::from(game.canvas.width());
    let viewport_height = f64::from(game.canvas.height());
    let origin_y = status_bar_top(viewport_height, f64::from(layout.height));

    let _ = game
        .context
        .draw_image_with_html_image_element_and_dw_and_dh(
            background_image,
            0.0,
            origin_y + f64::from(background.y),
            viewport_width,
            f64::from(background.height),
        );
    for sprite in &layout.sprites {
        draw_gui_sprite(
            game,
            sprite,
            viewport_width,
            f64::from(layout.width),
            origin_y,
        );
    }
    draw_status_bar_text(game, origin_y, f64::from(background.y));
    true
}

fn draw_gui_sprite(
    game: &Game,
    sprite: &GuiSprite,
    viewport_width: f64,
    layout_width: f64,
    origin_y: f64,
) {
    let Some(image) = ready_image(&game.images, &sprite.asset_id) else {
        return;
    };
    let _ = game
        .context
        .draw_image_with_html_image_element_and_dw_and_dh(
            image,
            gui_sprite_x(viewport_width, layout_width, sprite),
            origin_y + f64::from(sprite.y),
            f64::from(sprite.width),
            f64::from(sprite.height),
        );
}

fn draw_status_bar_text(
    game: &Game,
    origin_y: f64,
    bar_y: f64,
) {
    let bar_top = origin_y + bar_y;
    game.context.set_fill_style_str("#e9eef2");
    game.context.set_font("bold 12px monospace");
    let _ = game
        .context
        .fill_text(&game.player.level.to_string(), 44.0, bar_top + 61.0);

    // Character progression is not modeled yet, so every current character is a
    // Beginner.
    game.context.set_font("bold 10px monospace");
    let _ = game
        .context
        .fill_text_with_max_width("Beginner", 84.0, bar_top + 50.0, 118.0);
    game.context.set_font("11px monospace");
    let _ = game
        .context
        .fill_text_with_max_width(&game.player.name, 84.0, bar_top + 65.0, 118.0);

    game.context.set_fill_style_str("#263139");
    game.context.set_font("12px monospace");
    let _ = game
        .context
        .fill_text_with_max_width(&game.map.name, 10.0, bar_top + 22.0, 550.0);
}

fn valid_layout(layout: &GuiLayout) -> bool {
    layout.width.is_finite()
        && layout.height.is_finite()
        && layout.width > 0.0
        && layout.height > 0.0
        && layout
            .background
            .as_ref()
            .is_some_and(|sprite| valid_gui_sprite(sprite, layout.width, layout.height))
        && layout
            .sprites
            .iter()
            .all(|sprite| valid_gui_sprite(sprite, layout.width, layout.height))
}

fn valid_gui_sprite(
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

fn gui_sprite_x(
    viewport_width: f64,
    layout_width: f64,
    sprite: &GuiSprite,
) -> f64 {
    if sprite.anchor_right {
        viewport_width - (layout_width - f64::from(sprite.x))
    } else {
        f64::from(sprite.x)
    }
}

fn status_bar_top(
    viewport_height: f64,
    layout_height: f64,
) -> f64 {
    (viewport_height - layout_height).max(0.0)
}

fn draw_fallback_hud(game: &Game) {
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
    use oozems_proto::v1::GuiLayout;
    use oozems_proto::v1::GuiSprite;
    use oozems_proto::v1::PortalFrame;

    use super::gui_sprite_x;
    use super::portal_frame_index;
    use super::status_bar_top;
    use super::valid_layout;

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

    #[test]
    fn status_bar_uses_independent_left_and_right_anchors() {
        let left = GuiSprite {
            x: 10.0,
            ..GuiSprite::default()
        };
        let right = GuiSprite {
            x: 649.0,
            anchor_right: true,
            ..GuiSprite::default()
        };

        assert_eq!(gui_sprite_x(960.0, 800.0, &left), 10.0);
        assert_eq!(gui_sprite_x(960.0, 800.0, &right), 809.0);
        assert_eq!(status_bar_top(600.0, 151.0), 449.0);
        assert_eq!(status_bar_top(120.0, 151.0), 0.0);
    }

    #[test]
    fn invalid_gui_layouts_are_rejected_at_the_client_boundary() {
        let background = GuiSprite {
            asset_id: "background".to_owned(),
            width: 800.0,
            height: 71.0,
            y: 80.0,
            ..GuiSprite::default()
        };
        assert!(valid_layout(&GuiLayout {
            width: 800.0,
            height: 151.0,
            background: Some(background.clone()),
            ..GuiLayout::default()
        }));
        assert!(!valid_layout(&GuiLayout {
            width: f32::NAN,
            height: 151.0,
            background: Some(background.clone()),
            ..GuiLayout::default()
        }));
        assert!(!valid_layout(&GuiLayout {
            width: 799.0,
            height: 151.0,
            background: Some(background),
            ..GuiLayout::default()
        }));
    }
}
