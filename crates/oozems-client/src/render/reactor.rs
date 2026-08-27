use oozems_proto::v1::ReactorDefinition;
use oozems_proto::v1::ReactorFrame;

use crate::assets;
use crate::game::Game;

pub(super) fn draw(
    game: &Game,
    camera_x: f64,
    camera_y: f64,
    layer: i32,
) {
    for reactor in &game.world.map.reactors {
        let Some(spawn) = game
            .world
            .map
            .reactor_spawn_points
            .iter()
            .find(|spawn| spawn.spawn_id == reactor.spawn_id && spawn.layer == layer)
        else {
            continue;
        };
        let Some(position) = spawn.position else {
            continue;
        };
        let Some(definition) = definition(game, reactor.definition_id) else {
            continue;
        };
        let Some((frames, preferred_index)) = crate::reactor_render::frame_selection(
            &game.world.reactor_render,
            reactor,
            definition,
            game.clock.now_ms,
        ) else {
            continue;
        };
        let Some(index) = assets::ready_or_fallback_index(
            &game.surface.images,
            frames.iter().map(|frame| frame.asset_id.as_str()),
            preferred_index,
        ) else {
            continue;
        };
        draw_frame(
            game,
            &frames[index],
            position.x,
            position.y,
            spawn.flip_x,
            camera_x,
            camera_y,
        );
    }
}

fn definition(
    game: &Game,
    definition_id: u32,
) -> Option<&ReactorDefinition> {
    game.world
        .map
        .reactor_definitions
        .iter()
        .find(|definition| definition.id == definition_id)
}

fn draw_frame(
    game: &Game,
    frame: &ReactorFrame,
    anchor_x: f32,
    anchor_y: f32,
    flip_x: bool,
    camera_x: f64,
    camera_y: f64,
) {
    let x = if flip_x {
        anchor_x - (frame.width - frame.origin_x)
    } else {
        anchor_x - frame.origin_x
    };
    super::draw_sprite(
        game,
        &frame.asset_id,
        x,
        anchor_y - frame.origin_y,
        frame.width,
        frame.height,
        flip_x,
        camera_x,
        camera_y,
    );
}
