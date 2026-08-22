use super::draw_window;
use crate::assets::ready_image;
use crate::game::Game;

const ROW_LEFT: f32 = 17.0;
const ROW_TOP: f32 = 94.0;
const ROW_HEIGHT: f32 = 35.0;
const ICON_SIZE: f32 = 32.0;
const PAGE_SIZE: usize = 4;
const TITLE_BASELINE: f32 = 38.0;
const PAGE_BASELINE: f32 = 80.0;
const FOOTER_BASELINE: f32 = 276.0;

pub(super) fn draw(game: &Game) {
    let Some(window) = game.gui.skill_window.as_ref() else {
        return;
    };
    if !draw_window(game, window) {
        return;
    }

    draw_header(game, window.x, window.y);
    if game.skill_book.skills.is_empty() {
        draw_empty_message(game, window.x, window.y);
        return;
    }
    let page_count = game.skill_book.skills.len().div_ceil(PAGE_SIZE);
    let page = game.gui_state.borrow().skill_page % page_count;
    for (index, skill) in game
        .skill_book
        .skills
        .iter()
        .skip(page * PAGE_SIZE)
        .take(PAGE_SIZE)
        .enumerate()
    {
        let Some(definition) = skill.definition.as_ref() else {
            continue;
        };
        let row_y = window.y + ROW_TOP + index as f32 * ROW_HEIGHT;
        draw_skill_icon(game, definition, window.x + ROW_LEFT, row_y);
        draw_skill_text(game, definition, skill.level, window.x, row_y);
    }
    draw_page_controls(game, window.x, window.y, page, page_count);
}

fn draw_header(
    game: &Game,
    window_x: f32,
    window_y: f32,
) {
    game.context.set_fill_style_str("#30383b");
    game.context.set_font("bold 10px Arial");
    let _ = game.context.fill_text_with_max_width(
        &game.skill_book.name,
        f64::from(window_x + 17.0),
        f64::from(window_y + TITLE_BASELINE),
        141.0,
    );
    let _ = game.context.fill_text_with_max_width(
        &game.skill_book.available_points.to_string(),
        f64::from(window_x + 94.0),
        f64::from(window_y + FOOTER_BASELINE),
        28.0,
    );
}

fn draw_page_controls(
    game: &Game,
    window_x: f32,
    window_y: f32,
    page: usize,
    page_count: usize,
) {
    if page_count <= 1 {
        return;
    }
    game.context.set_fill_style_str("#30383b");
    game.context.set_font("bold 11px Arial");
    let _ = game.context.fill_text(
        "<",
        f64::from(window_x + 84.0),
        f64::from(window_y + PAGE_BASELINE),
    );
    let _ = game.context.fill_text_with_max_width(
        &format!("{}/{}", page + 1, page_count),
        f64::from(window_x + 105.0),
        f64::from(window_y + PAGE_BASELINE),
        24.0,
    );
    let _ = game.context.fill_text(
        ">",
        f64::from(window_x + 143.0),
        f64::from(window_y + PAGE_BASELINE),
    );
}

fn draw_empty_message(
    game: &Game,
    window_x: f32,
    window_y: f32,
) {
    game.context.set_fill_style_str("#596469");
    game.context.set_font("10px Arial");
    let _ = game.context.fill_text(
        "No skills available.",
        f64::from(window_x + 24.0),
        f64::from(window_y + ROW_TOP + 26.0),
    );
}

fn draw_skill_icon(
    game: &Game,
    definition: &oozems_proto::v1::SkillDefinition,
    slot_x: f32,
    slot_y: f32,
) {
    let Some(image) = ready_image(&game.images, &definition.icon_asset_id) else {
        return;
    };
    let x = slot_x + (ICON_SIZE - definition.icon_width) / 2.0;
    let y = slot_y + (ICON_SIZE - definition.icon_height) / 2.0;
    let _ = game
        .context
        .draw_image_with_html_image_element_and_dw_and_dh(
            image,
            f64::from(x),
            f64::from(y),
            f64::from(definition.icon_width),
            f64::from(definition.icon_height),
        );
}

fn draw_skill_text(
    game: &Game,
    definition: &oozems_proto::v1::SkillDefinition,
    level: u32,
    window_x: f32,
    row_y: f32,
) {
    game.context.set_fill_style_str("#30383b");
    game.context.set_font("bold 10px Arial");
    let _ = game.context.fill_text_with_max_width(
        &definition.name,
        f64::from(window_x + 54.0),
        f64::from(row_y + 12.0),
        104.0,
    );
    game.context.set_fill_style_str("#596469");
    game.context.set_font("9px Arial");
    let _ = game.context.fill_text_with_max_width(
        &format!("Level {level}/{}", definition.max_level),
        f64::from(window_x + 54.0),
        f64::from(row_y + 27.0),
        104.0,
    );
}
