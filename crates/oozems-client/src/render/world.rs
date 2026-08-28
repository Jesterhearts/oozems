use oozems_proto::v1::Decoration;
use oozems_proto::v1::DecorationFrame;
use oozems_proto::v1::Map;
use oozems_proto::v1::PortalFrame;

use crate::assets;
use crate::assets::ready_image;
use crate::character_render;
use crate::character_render::CharacterPlacement;
use crate::game::Game;
use crate::game::character_animation_elapsed_ms;
use crate::game_gui;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LayerPass {
    Decorations,
    Portals,
    Npcs,
    Reactors,
    Mobs,
    DroppedItems,
    Player,
    SkillEffects,
}

const ORDINARY_LAYER_PASSES: &[LayerPass] = &[
    LayerPass::Decorations,
    LayerPass::Portals,
    LayerPass::Npcs,
    LayerPass::Reactors,
    LayerPass::Mobs,
];
const PLAYER_LAYER_PASSES: &[LayerPass] = &[
    LayerPass::Decorations,
    LayerPass::Portals,
    LayerPass::Npcs,
    LayerPass::Reactors,
    LayerPass::Mobs,
    LayerPass::DroppedItems,
    LayerPass::Player,
    LayerPass::SkillEffects,
];

pub(super) fn draw(game: &Game) {
    let viewport_width = f64::from(game.surface.canvas.width());
    let viewport_height = f64::from(game.surface.canvas.height());
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
    let camera_x = camera_x(player_x, viewport_width, f64::from(game.world.map.width));
    let camera_y = camera_y(player_y, viewport_height, f64::from(game.world.map.height));

    draw_background(game, viewport_width, viewport_height, camera_x);
    for layer in &game.world.world_layers {
        for pass in layer_passes(*layer == game.world.motion.platform_layer) {
            match pass {
                LayerPass::Decorations => draw_decorations(game, camera_x, camera_y, *layer),
                LayerPass::Portals => draw_portals(game, camera_x, camera_y, *layer),
                LayerPass::Npcs => super::npc::draw(game, camera_x, camera_y, *layer),
                LayerPass::Reactors => super::reactor::draw(game, camera_x, camera_y, *layer),
                LayerPass::Mobs => super::mob::draw(game, camera_x, camera_y, *layer),
                LayerPass::DroppedItems => draw_dropped_items(game, camera_x, camera_y),
                LayerPass::Player => draw_player(game, camera_x, camera_y),
                LayerPass::SkillEffects => {
                    crate::skill_effects::draw(game, camera_x, camera_y);
                }
            }
        }
    }
    super::mob::draw_combat_texts(game, camera_x, camera_y);
}

pub(crate) fn world_layers(map: &Map) -> Vec<i32> {
    let mut layers = Vec::with_capacity(
        map.decorations.len()
            + map.platforms.len()
            + map.ladders.len()
            + map.portals.len()
            + map.npcs.len()
            + map.mobs.len()
            + map.mob_projectiles.len()
            + map.reactor_spawn_points.len()
            + 1,
    );
    layers.push(0);
    layers.extend(map.decorations.iter().map(|decoration| decoration.layer));
    layers.extend(map.platforms.iter().map(|platform| platform.layer));
    layers.extend(map.ladders.iter().map(|ladder| ladder.layer));
    layers.extend(map.portals.iter().map(|portal| portal.layer));
    layers.extend(map.npcs.iter().map(|npc| npc.layer));
    layers.extend(map.reactor_spawn_points.iter().map(|reactor| reactor.layer));
    layers.extend(map.mobs.iter().map(|mob| mob.layer));
    layers.extend(
        map.mob_projectiles
            .iter()
            .map(|projectile| projectile.layer),
    );
    layers.sort_unstable();
    layers.dedup();
    layers
}

pub(crate) fn npc_at_point(
    game: &Game,
    point: game_gui::CanvasPoint,
) -> Option<u32> {
    let viewport_width = f64::from(game.surface.canvas.width());
    let viewport_height = f64::from(game.surface.canvas.height());
    let position = game.player.position.as_ref()?;
    let camera_x = camera_x(
        f64::from(position.x),
        viewport_width,
        f64::from(game.world.map.width),
    );
    let camera_y = camera_y(
        f64::from(position.y),
        viewport_height,
        f64::from(game.world.map.height),
    );
    super::npc::at_point(game, point, camera_x, camera_y)
}

pub(super) fn layer_passes(has_player: bool) -> &'static [LayerPass] {
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
    let context = &game.surface.context;
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
    for decoration in &game.world.map.decorations {
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
    if let Some(preferred_index) = decoration_frame_index(&decoration.frames, game.clock.now_ms) {
        let preferred = &decoration.frames[preferred_index];
        if !sprite_is_visible(
            game,
            preferred.x,
            preferred.y,
            preferred.width,
            preferred.height,
            camera_x,
            camera_y,
        ) {
            return;
        }
        let Some(index) = assets::ready_or_fallback_index(
            &game.surface.images,
            decoration
                .frames
                .iter()
                .map(|frame| frame.asset_id.as_str()),
            preferred_index,
        ) else {
            return;
        };
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

pub(crate) fn draw_sprite(
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
    if !sprite_is_visible(
        game, map_x, map_y, map_width, map_height, camera_x, camera_y,
    ) {
        return;
    }
    let x = f64::from(map_x) - camera_x;
    let y = f64::from(map_y) - camera_y;
    let width = f64::from(map_width);
    let height = f64::from(map_height);

    let Some(image) = ready_image(&game.surface.images, asset_id) else {
        return;
    };

    if flip_x {
        game.surface.context.save();
        let transformed = game
            .surface
            .context
            .translate(x + width, y)
            .and_then(|()| game.surface.context.scale(-1.0, 1.0));
        if transformed.is_ok() {
            let _ = game
                .surface
                .context
                .draw_image_with_html_image_element_and_dw_and_dh(image, 0.0, 0.0, width, height);
        }
        game.surface.context.restore();
    } else {
        let _ = game
            .surface
            .context
            .draw_image_with_html_image_element_and_dw_and_dh(image, x, y, width, height);
    }
}

pub(crate) fn sprite_is_visible(
    game: &Game,
    map_x: f32,
    map_y: f32,
    map_width: f32,
    map_height: f32,
    camera_x: f64,
    camera_y: f64,
) -> bool {
    let x = f64::from(map_x) - camera_x;
    let y = f64::from(map_y) - camera_y;
    let width = f64::from(map_width);
    let height = f64::from(map_height);
    x + width >= 0.0
        && x <= f64::from(game.surface.canvas.width())
        && y + height >= 0.0
        && y <= f64::from(game.surface.canvas.height())
}

fn draw_portals(
    game: &Game,
    camera_x: f64,
    camera_y: f64,
    layer: i32,
) {
    for portal in &game.world.map.portals {
        if portal.layer != layer {
            continue;
        }
        let Some(preferred_index) = portal_frame_index(&portal.frames, game.clock.now_ms) else {
            continue;
        };
        let preferred = &portal.frames[preferred_index];
        if !sprite_is_visible(
            game,
            preferred.x,
            preferred.y,
            preferred.width,
            preferred.height,
            camera_x,
            camera_y,
        ) {
            continue;
        }
        let Some(index) = assets::ready_or_fallback_index(
            &game.surface.images,
            portal.frames.iter().map(|frame| frame.asset_id.as_str()),
            preferred_index,
        ) else {
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
    for drop in &game.world.map.dropped_items {
        if drop.despawn_at_unix_ms <= now_ms
            || (drop.expires_at_unix_ms != 0 && drop.expires_at_unix_ms <= now_ms)
        {
            continue;
        }
        let Some(position) = &drop.position else {
            continue;
        };
        let Some(definition) = super::item_definition(game, drop.item_id) else {
            continue;
        };
        let bounce = (game.clock.now_ms / 220.0).sin() as f32 * 2.0;
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

pub(super) fn portal_frame_index(
    frames: &[PortalFrame],
    timestamp_ms: f64,
) -> Option<usize> {
    crate::animation::frame_index(
        frames.iter().map(|frame| frame.delay_ms),
        timestamp_ms,
        crate::animation::Playback::Loop,
    )
}

pub(super) fn decoration_frame_index(
    frames: &[DecorationFrame],
    timestamp_ms: f64,
) -> Option<usize> {
    crate::animation::frame_index(
        frames.iter().map(|frame| frame.delay_ms),
        timestamp_ms,
        crate::animation::Playback::Loop,
    )
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

    game.surface
        .context
        .set_fill_style_str("rgba(29, 45, 43, 0.25)");
    game.surface.context.fill_rect(x - 23.0, y - 3.0, 46.0, 6.0);
    draw_death_tomb(game, x, y);
    let elapsed_ms =
        character_animation_elapsed_ms(game.world.character_animation, game.clock.now_ms);
    let placement = || CharacterPlacement {
        anchor_x: x,
        anchor_y: y,
        scale: 1.0,
        facing_left: game.world.facing_left,
    };
    draw_setup_item(game, x, y, elapsed_ms);
    let morph_drawn = game
        .world
        .morph_definition
        .as_ref()
        .is_some_and(|definition| {
            crate::morph_render::draw_morph(
                &game.surface.context,
                &game.surface.images,
                definition,
                game.world.character_animation.animation,
                elapsed_ms,
                placement(),
            )
        });
    if !morph_drawn {
        character_render::draw_character(
            &game.surface.context,
            &game.surface.images,
            &game.world.character_sprites,
            game.world.character_animation.animation,
            elapsed_ms,
            placement(),
        );
    }
}

fn draw_setup_item(
    game: &Game,
    player_x: f64,
    player_y: f64,
    elapsed_ms: f64,
) {
    let Some(definition) = game
        .world
        .active_setup_item_id
        .and_then(|item_id| super::item_definition(game, item_id))
    else {
        return;
    };
    let frames = &definition.setup_frames;
    let Some(preferred_index) = crate::animation::frame_index(
        frames.iter().map(|frame| frame.delay_ms),
        elapsed_ms,
        crate::animation::Playback::Loop,
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
    game.surface.context.save();
    let transformed = game
        .surface
        .context
        .translate(player_x, player_y)
        .and_then(|()| {
            game.surface
                .context
                .scale(setup_horizontal_scale(game.world.facing_left), 1.0)
        });
    if transformed.is_ok() {
        let _ = game
            .surface
            .context
            .draw_image_with_html_image_element_and_dw_and_dh(
                image,
                -f64::from(frame.origin_x),
                -f64::from(frame.origin_y),
                f64::from(frame.width),
                f64::from(frame.height),
            );
    }
    game.surface.context.restore();
}

fn setup_horizontal_scale(facing_left: bool) -> f64 {
    if facing_left { 1.0 } else { -1.0 }
}

fn draw_death_tomb(
    game: &Game,
    player_x: f64,
    player_y: f64,
) {
    let Some(started_ms) = game.ui.death.started_ms else {
        return;
    };
    let frames = &game.ui.gui.death_tomb_frames;
    let elapsed_ms = (game.clock.now_ms - started_ms).max(0.0);
    let preferred_index = crate::animation::frame_index(
        frames.iter().map(|frame| frame.delay_ms),
        elapsed_ms,
        crate::animation::Playback::Once,
    )
    .or_else(|| frames.len().checked_sub(1));
    let Some(preferred_index) = preferred_index else {
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
    let duration_ms = frames
        .iter()
        .map(|frame| u64::from(frame.delay_ms.max(1)))
        .sum::<u64>();
    let anchor_y = tomb_anchor_y(player_y, elapsed_ms, duration_ms as f64);
    let Some(image) = ready_image(&game.surface.images, &frame.asset_id) else {
        return;
    };
    let _ = game
        .surface
        .context
        .draw_image_with_html_image_element_and_dw_and_dh(
            image,
            player_x - f64::from(frame.origin_x),
            anchor_y - f64::from(frame.origin_y),
            f64::from(frame.width),
            f64::from(frame.height),
        );
}

fn tomb_anchor_y(
    target_y: f64,
    elapsed_ms: f64,
    duration_ms: f64,
) -> f64 {
    if duration_ms <= 0.0 {
        return target_y;
    }
    let progress = (elapsed_ms / duration_ms).clamp(0.0, 1.0);
    target_y * progress * progress
}

#[cfg(test)]
mod tests {
    use super::setup_horizontal_scale;
    use super::tomb_anchor_y;

    #[test]
    fn setup_item_uses_the_character_facing_transform() {
        assert_eq!(setup_horizontal_scale(true), 1.0);
        assert_eq!(setup_horizontal_scale(false), -1.0);
    }

    #[test]
    fn tomb_accelerates_from_the_screen_top_and_stops_at_the_player() {
        assert_eq!(tomb_anchor_y(400.0, 0.0, 2_000.0), 0.0);
        assert_eq!(tomb_anchor_y(400.0, 1_000.0, 2_000.0), 100.0);
        assert_eq!(tomb_anchor_y(400.0, 2_000.0, 2_000.0), 400.0);
        assert_eq!(tomb_anchor_y(400.0, 3_000.0, 2_000.0), 400.0);
    }
}
