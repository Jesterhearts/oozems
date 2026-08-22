use oozems_proto::v1::CharacterStats;
use oozems_proto::v1::Decoration;
use oozems_proto::v1::DecorationFrame;
use oozems_proto::v1::GuiSprite;
use oozems_proto::v1::GuiWindow;
use oozems_proto::v1::ItemDefinition;
use oozems_proto::v1::Map;
use oozems_proto::v1::PlatformKind;
use oozems_proto::v1::PortalFrame;
use web_sys::HtmlImageElement;

use crate::assets::ready_image;
use crate::character_render;
use crate::character_render::CharacterPlacement;
use crate::game::Game;
use crate::game_gui;

mod skillbook;

const GAUGE_HEADER_HEIGHT: f64 = 15.0;
const GAUGE_FILL_TOP: f64 = 15.0;
const GAUGE_FILL_HEIGHT: f64 = 14.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LayerPass {
    Decorations,
    Platforms,
    Portals,
    DroppedItems,
    Player,
}

const ORDINARY_LAYER_PASSES: &[LayerPass] = &[
    LayerPass::Decorations,
    LayerPass::Platforms,
    LayerPass::Portals,
];
const PLAYER_LAYER_PASSES: &[LayerPass] = &[
    LayerPass::Decorations,
    LayerPass::Platforms,
    LayerPass::Portals,
    LayerPass::DroppedItems,
    LayerPass::Player,
];

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
    for layer in &game.world_layers {
        for pass in layer_passes(*layer == game.motion.platform_layer) {
            match pass {
                LayerPass::Decorations => draw_decorations(game, camera_x, camera_y, *layer),
                LayerPass::Platforms => draw_platforms(game, camera_x, camera_y, *layer),
                LayerPass::Portals => draw_portals(game, camera_x, camera_y, *layer),
                LayerPass::DroppedItems => draw_dropped_items(game, camera_x, camera_y),
                LayerPass::Player => draw_player(game, camera_x, camera_y),
            }
        }
    }
    draw_hud(game);
}

pub(crate) fn world_layers(map: &Map) -> Vec<i32> {
    let mut layers = Vec::with_capacity(
        map.decorations.len() + map.platforms.len() + map.ladders.len() + map.portals.len() + 1,
    );
    layers.push(0);
    layers.extend(map.decorations.iter().map(|decoration| decoration.layer));
    layers.extend(map.platforms.iter().map(|platform| platform.layer));
    layers.extend(map.ladders.iter().map(|ladder| ladder.layer));
    layers.extend(map.portals.iter().map(|portal| portal.layer));
    layers.sort_unstable();
    layers.dedup();
    layers
}

fn layer_passes(has_player: bool) -> &'static [LayerPass] {
    if has_player {
        PLAYER_LAYER_PASSES
    } else {
        ORDINARY_LAYER_PASSES
    }
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

fn draw_decorations(
    game: &Game,
    camera_x: f64,
    camera_y: f64,
    layer: i32,
) {
    for decoration in &game.map.decorations {
        if decoration.layer == layer {
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
    if let Some(index) = decoration_frame_index(&decoration.frames, game.frame_time_ms) {
        let frame = &decoration.frames[index];
        draw_sprite(
            game,
            &frame.asset_id,
            frame.x,
            frame.y,
            frame.width,
            frame.height,
            decoration.flip_x,
            camera_x,
            camera_y,
        );
        return;
    }
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
    layer: i32,
) {
    for portal in &game.map.portals {
        if portal.layer != layer {
            continue;
        }
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

fn draw_dropped_items(
    game: &Game,
    camera_x: f64,
    camera_y: f64,
) {
    let now_ms = js_sys::Date::now().max(0.0) as u64;
    for drop in &game.map.dropped_items {
        if drop.despawn_at_unix_ms <= now_ms {
            continue;
        }
        let Some(position) = &drop.position else {
            continue;
        };
        let Some(definition) = item_definition(game, drop.item_id) else {
            continue;
        };
        let bounce = (game.frame_time_ms / 220.0).sin() as f32 * 2.0;
        draw_sprite(
            game,
            &definition.icon_asset_id,
            position.x - definition.icon_width / 2.0,
            position.y - definition.icon_height - 3.0 + bounce,
            definition.icon_width,
            definition.icon_height,
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
    timed_frame_index(frames.iter().map(|frame| frame.delay_ms), timestamp_ms)
}

fn decoration_frame_index(
    frames: &[DecorationFrame],
    timestamp_ms: f64,
) -> Option<usize> {
    timed_frame_index(frames.iter().map(|frame| frame.delay_ms), timestamp_ms)
}

fn timed_frame_index(
    delays: impl Iterator<Item = u32> + Clone,
    timestamp_ms: f64,
) -> Option<usize> {
    let total_duration = delays
        .clone()
        .map(|delay| u64::from(delay.max(1)))
        .sum::<u64>();
    if total_duration == 0 {
        return None;
    }

    let mut animation_time = timestamp_ms.max(0.0) as u64 % total_duration;
    for (index, delay) in delays.enumerate() {
        let delay = u64::from(delay.max(1));
        if animation_time < delay {
            return Some(index);
        }
        animation_time -= delay;
    }
    None
}

fn draw_platforms(
    game: &Game,
    camera_x: f64,
    camera_y: f64,
    layer: i32,
) {
    for platform in &game.map.platforms {
        if platform.hidden || platform.layer != layer {
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
    if !draw_wz_hud(game) {
        draw_fallback_hud(game);
    }
    if game.gui_state.borrow().stats_open {
        draw_stat_window(game);
    }
    if game.gui_state.borrow().equipment_open {
        draw_equipment_window(game);
    }
    if game.gui_state.borrow().inventory_open {
        draw_inventory_window(game);
    }
    if game.gui_state.borrow().skills_open {
        skillbook::draw(game);
    }
    if game.gui_state.borrow().key_config_open {
        draw_key_config_window(game);
    }
}

fn draw_key_config_window(game: &Game) {
    let Some(window) = game.gui.key_config_window.as_ref() else {
        return;
    };
    if !draw_window(game, window) {
        return;
    }
    for placement in
        game_gui::bound_key_icons(&game.gui, &game.skill_book, &game.key_bindings.borrow())
    {
        draw_key_icon(game, &placement);
    }
    if let Some(drag) = game.key_drag.as_ref()
        && let Some(placement) = game_gui::dragged_key_icon(&game.gui, &game.skill_book, drag)
    {
        draw_key_icon(game, &placement);
    }
}

fn draw_key_icon(
    game: &Game,
    placement: &game_gui::KeyIconPlacement,
) {
    let Some(image) = ready_image(&game.images, &placement.asset_id) else {
        return;
    };
    let _ = game
        .context
        .draw_image_with_html_image_element_and_dw_and_dh(
            image,
            f64::from(placement.x),
            f64::from(placement.y),
            f64::from(placement.width),
            f64::from(placement.height),
        );
}

fn draw_equipment_window(game: &Game) {
    let Some(window) = game.gui.equipment_window.as_ref() else {
        return;
    };
    if !draw_window(game, window) {
        return;
    }
    let Some(inventory) = game.player.inventory.as_ref() else {
        return;
    };
    for equipped in &inventory.equipment {
        let Some((x, y)) = game_gui::equipment_slot_position(equipped.slot) else {
            continue;
        };
        if let Some(definition) = item_definition(game, equipped.item_id) {
            draw_item_icon(game, definition, window.x + x, window.y + y);
        }
    }
}

fn draw_inventory_window(game: &Game) {
    let Some(window) = game.gui.inventory_window.as_ref() else {
        return;
    };
    if !draw_window(game, window) {
        return;
    }
    let Some(inventory) = game.player.inventory.as_ref() else {
        return;
    };
    for (index, item_id) in inventory.item_ids.iter().enumerate() {
        let Some(definition) = item_definition(game, *item_id) else {
            continue;
        };
        let (x, y) = game_gui::inventory_slot_position(index);
        draw_item_icon(game, definition, window.x + x, window.y + y);
    }
}

fn draw_window(
    game: &Game,
    window: &GuiWindow,
) -> bool {
    let Some(layout) = window
        .layout
        .as_ref()
        .filter(|layout| game_gui::valid_layout(layout))
    else {
        return false;
    };
    let Some(background) = layout.background.as_ref() else {
        return false;
    };
    let Some(background_image) = ready_image(&game.images, &background.asset_id) else {
        return false;
    };
    let origin_x = f64::from(window.x);
    let origin_y = f64::from(window.y);
    let _ = game
        .context
        .draw_image_with_html_image_element_and_dw_and_dh(
            background_image,
            origin_x,
            origin_y,
            f64::from(background.width),
            f64::from(background.height),
        );
    for sprite in &layout.sprites {
        draw_window_sprite(game, sprite, origin_x, origin_y);
    }
    true
}

fn draw_item_icon(
    game: &Game,
    definition: &ItemDefinition,
    slot_x: f32,
    slot_y: f32,
) {
    let Some(image) = ready_image(&game.images, &definition.icon_asset_id) else {
        return;
    };
    let x = f64::from(slot_x + (32.0 - definition.icon_width) / 2.0);
    let y = f64::from(slot_y + (32.0 - definition.icon_height) / 2.0);
    let _ = game
        .context
        .draw_image_with_html_image_element_and_dw_and_dh(
            image,
            x,
            y,
            f64::from(definition.icon_width),
            f64::from(definition.icon_height),
        );
}

fn item_definition(
    game: &Game,
    item_id: u32,
) -> Option<&ItemDefinition> {
    game.gui
        .items
        .iter()
        .find(|definition| definition.item_id == item_id)
}

fn draw_wz_hud(game: &Game) -> bool {
    let Some(layout) = game
        .gui
        .status_bar
        .as_ref()
        .filter(|layout| game_gui::valid_layout(layout))
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
    let origin_y = f64::from(game_gui::status_bar_top(
        viewport_height as f32,
        layout.height,
    ));

    let _ = game
        .context
        .draw_image_with_html_image_element_and_dw_and_dh(
            background_image,
            0.0,
            origin_y + f64::from(background.y),
            viewport_width,
            f64::from(background.height),
        );
    let gui_state = *game.gui_state.borrow();
    for sprite in layout
        .sprites
        .iter()
        .filter(|sprite| game_gui::status_sprite_visible(gui_state, sprite))
    {
        draw_gui_sprite(
            game,
            sprite,
            viewport_width,
            f64::from(layout.width),
            origin_y,
        );
    }
    let gauge_origin = layout
        .sprites
        .iter()
        .find(|sprite| sprite.name == "gauge")
        .filter(|sprite| ready_image(&game.images, &sprite.asset_id).is_some())
        .map(|sprite| {
            (
                f64::from(game_gui::sprite_screen_x(
                    viewport_width as f32,
                    layout.width,
                    sprite,
                )),
                origin_y + f64::from(sprite.y),
            )
        });
    draw_status_bar_text(game, origin_y, f64::from(background.y), gauge_origin);
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
    let destination_x = f64::from(game_gui::sprite_screen_x(
        viewport_width as f32,
        layout_width as f32,
        sprite,
    ));
    let destination_y = origin_y + f64::from(sprite.y);
    if sprite.name == "gauge"
        && let Some(stats) = game.player.stats.as_ref()
        && draw_gauge_fill(game, image, sprite, stats, destination_x, destination_y)
    {
        return;
    }
    let _ = game
        .context
        .draw_image_with_html_image_element_and_dw_and_dh(
            image,
            destination_x,
            destination_y,
            f64::from(sprite.width),
            f64::from(sprite.height),
        );
}

fn draw_gauge_fill(
    game: &Game,
    image: &HtmlImageElement,
    sprite: &GuiSprite,
    stats: &CharacterStats,
    destination_x: f64,
    destination_y: f64,
) -> bool {
    if f64::from(sprite.width) < 340.0 || f64::from(sprite.height) < 31.0 {
        return false;
    }
    let _ = game
        .context
        .draw_image_with_html_image_element_and_sw_and_sh_and_dx_and_dy_and_dw_and_dh(
            image,
            0.0,
            0.0,
            f64::from(sprite.width),
            GAUGE_HEADER_HEIGHT,
            destination_x,
            destination_y,
            f64::from(sprite.width),
            GAUGE_HEADER_HEIGHT,
        );
    for fill in game_gui::gauge_fills(stats) {
        if fill.filled_width == 0.0 {
            continue;
        }
        let _ = game
            .context
            .draw_image_with_html_image_element_and_sw_and_sh_and_dx_and_dy_and_dw_and_dh(
                image,
                fill.source_x,
                GAUGE_FILL_TOP,
                fill.filled_width,
                GAUGE_FILL_HEIGHT,
                destination_x + fill.source_x,
                destination_y + GAUGE_FILL_TOP,
                fill.filled_width,
                GAUGE_FILL_HEIGHT,
            );
    }
    true
}

fn draw_status_bar_text(
    game: &Game,
    origin_y: f64,
    bar_y: f64,
    gauge_origin: Option<(f64, f64)>,
) {
    let bar_top = origin_y + bar_y;
    game.context.set_fill_style_str("#e9eef2");
    game.context.set_font("bold 12px monospace");
    let _ = game
        .context
        .fill_text(&game.player.level.to_string(), 44.0, bar_top + 61.0);

    game.context.set_font("bold 10px monospace");
    let job = game
        .player
        .stats
        .as_ref()
        .map_or("Beginner", |stats| job_name(stats.job_id));
    let _ = game
        .context
        .fill_text_with_max_width(job, 84.0, bar_top + 50.0, 118.0);
    game.context.set_font("11px monospace");
    let _ = game
        .context
        .fill_text_with_max_width(&game.player.name, 84.0, bar_top + 65.0, 118.0);

    game.context.set_fill_style_str("#263139");
    game.context.set_font("12px monospace");
    let _ = game
        .context
        .fill_text_with_max_width(&game.map.name, 10.0, bar_top + 22.0, 550.0);

    if let (Some(stats), Some((gauge_x, gauge_y))) = (game.player.stats.as_ref(), gauge_origin) {
        draw_gauge_text(game, stats, gauge_x, gauge_y);
    }
}

fn draw_gauge_text(
    game: &Game,
    stats: &CharacterStats,
    gauge_x: f64,
    gauge_y: f64,
) {
    let fills = game_gui::gauge_fills(stats);
    let labels = game_gui::gauge_labels(stats);
    game.context.set_font("bold 10px Arial");
    game.context.set_text_align("center");
    for (fill, label) in fills.into_iter().zip(labels) {
        let center_x = gauge_x + fill.source_x + fill.full_width / 2.0;
        game.context.set_fill_style_str("#202020");
        let _ = game.context.fill_text_with_max_width(
            &label,
            center_x + 1.0,
            gauge_y + 28.0,
            fill.full_width - 4.0,
        );
        game.context.set_fill_style_str("#ffffff");
        let _ = game.context.fill_text_with_max_width(
            &label,
            center_x,
            gauge_y + 27.0,
            fill.full_width - 4.0,
        );
    }
    game.context.set_text_align("left");
}

fn draw_stat_window(game: &Game) {
    let Some(window) = game.gui.stat_window.as_ref() else {
        return;
    };
    let Some(layout) = window
        .layout
        .as_ref()
        .filter(|layout| game_gui::valid_layout(layout))
    else {
        return;
    };
    let Some(background) = layout.background.as_ref() else {
        return;
    };
    let Some(background_image) = ready_image(&game.images, &background.asset_id) else {
        return;
    };
    let origin_x = f64::from(window.x);
    let origin_y = f64::from(window.y);
    let _ = game
        .context
        .draw_image_with_html_image_element_and_dw_and_dh(
            background_image,
            origin_x,
            origin_y,
            f64::from(background.width),
            f64::from(background.height),
        );
    for sprite in &layout.sprites {
        draw_window_sprite(game, sprite, origin_x, origin_y);
    }
    if let Some(stats) = game.player.stats.as_ref() {
        draw_stat_values(game, stats, origin_x, origin_y);
    }
}

fn draw_window_sprite(
    game: &Game,
    sprite: &GuiSprite,
    origin_x: f64,
    origin_y: f64,
) {
    let Some(image) = ready_image(&game.images, &sprite.asset_id) else {
        return;
    };
    let _ = game
        .context
        .draw_image_with_html_image_element_and_dw_and_dh(
            image,
            origin_x + f64::from(sprite.x),
            origin_y + f64::from(sprite.y),
            f64::from(sprite.width),
            f64::from(sprite.height),
        );
}

fn draw_stat_values(
    game: &Game,
    stats: &CharacterStats,
    origin_x: f64,
    origin_y: f64,
) {
    let values = [
        (game.player.name.clone(), 45.0),
        (game.player.level.to_string(), 89.0),
        ("-".to_owned(), 106.0),
        (format!("{} / {}", stats.hp, stats.max_hp), 124.0),
        (format!("{} / {}", stats.mp, stats.max_mp), 142.0),
        (stats.experience.to_string(), 160.0),
        (stats.fame.to_string(), 178.0),
    ];
    game.context.set_fill_style_str("#30383b");
    game.context.set_font("10px monospace");
    for (value, y) in values {
        let _ = game
            .context
            .fill_text_with_max_width(&value, origin_x + 60.0, origin_y + y, 106.0);
    }

    let _ = game.context.fill_text_with_max_width(
        &stats.ability_points.to_string(),
        origin_x + 64.0,
        origin_y + 226.0,
        22.0,
    );
    for (value, y) in [
        (stats.strength, 256.0),
        (stats.dexterity, 273.0),
        (stats.intelligence, 290.0),
        (stats.luck, 307.0),
    ] {
        let _ = game.context.fill_text_with_max_width(
            &value.to_string(),
            origin_x + 60.0,
            origin_y + y,
            106.0,
        );
    }
}

fn job_name(job_id: u32) -> &'static str {
    match job_id {
        0 => "Beginner",
        _ => "Unknown",
    }
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
    use oozems_proto::v1::Decoration;
    use oozems_proto::v1::DecorationFrame;
    use oozems_proto::v1::Ladder;
    use oozems_proto::v1::Map;
    use oozems_proto::v1::Platform;
    use oozems_proto::v1::Portal;
    use oozems_proto::v1::PortalFrame;

    use super::LayerPass;
    use super::decoration_frame_index;
    use super::layer_passes;
    use super::portal_frame_index;
    use super::world_layers;

    #[test]
    fn world_layers_include_each_layer_source_in_order() {
        let map = Map {
            decorations: vec![Decoration {
                layer: 3,
                ..Decoration::default()
            }],
            platforms: vec![Platform {
                layer: 1,
                ..Platform::default()
            }],
            ladders: vec![Ladder {
                layer: 4,
                ..Ladder::default()
            }],
            portals: vec![Portal {
                layer: 2,
                ..Portal::default()
            }],
            ..Map::default()
        };

        assert_eq!(world_layers(&map), vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn portals_render_after_decorations_on_their_layer() {
        assert_eq!(
            layer_passes(false),
            &[
                LayerPass::Decorations,
                LayerPass::Platforms,
                LayerPass::Portals,
            ]
        );
        assert_eq!(
            layer_passes(true),
            &[
                LayerPass::Decorations,
                LayerPass::Platforms,
                LayerPass::Portals,
                LayerPass::DroppedItems,
                LayerPass::Player,
            ]
        );
    }

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
    fn decoration_animation_uses_each_frame_delay() {
        let frames = vec![
            DecorationFrame {
                delay_ms: 130,
                ..DecorationFrame::default()
            },
            DecorationFrame {
                delay_ms: 260,
                ..DecorationFrame::default()
            },
        ];

        assert_eq!(decoration_frame_index(&frames, 129.0), Some(0));
        assert_eq!(decoration_frame_index(&frames, 130.0), Some(1));
        assert_eq!(decoration_frame_index(&frames, 389.0), Some(1));
        assert_eq!(decoration_frame_index(&frames, 390.0), Some(0));
    }
}
