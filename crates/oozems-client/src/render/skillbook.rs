use oozems_proto::v1::GuiLayout;
use oozems_proto::v1::GuiRegion;
use oozems_proto::v1::GuiSpriteTemplate;

use super::draw_window;
use crate::assets::ready_image;
use crate::game::Game;
use crate::game_gui;

const ICON_INSET: f32 = 3.0;
const ROW_TEXT_GAP: f32 = 2.0;
const TEXT_BOTTOM_PADDING: f32 = 3.0;

struct SkillUi<'a> {
    title: &'a GuiRegion,
    list: &'a GuiRegion,
    points: &'a GuiRegion,
    page_previous: &'a GuiRegion,
    page_label: &'a GuiRegion,
    page_next: &'a GuiRegion,
    row: &'a GuiSpriteTemplate,
    point_up: Option<&'a GuiSpriteTemplate>,
    point_up_disabled: Option<&'a GuiSpriteTemplate>,
    page_size: usize,
}

pub(super) fn draw(game: &Game) {
    let Some(window) = game.gui.skill_window.as_ref() else {
        return;
    };
    if !draw_window(game, window) {
        return;
    }
    let Some(ui) = window.layout.as_ref().and_then(resolve_skill_ui) else {
        return;
    };

    draw_header(game, window.x, window.y, &ui);
    if game.skill_book.skills.is_empty() {
        draw_empty_message(game, window.x, window.y, &ui);
        return;
    }

    let page_count = game.skill_book.skills.len().div_ceil(ui.page_size);
    let page = game.gui_state.borrow().skill_page % page_count;
    for (index, skill) in game
        .skill_book
        .skills
        .iter()
        .skip(page * ui.page_size)
        .take(ui.page_size)
        .enumerate()
    {
        let Some(definition) = skill.definition.as_ref() else {
            continue;
        };
        let row_x = window.x + ui.list.x;
        let row_y = window.y + ui.list.y + index as f32 * ui.row.height;
        draw_sprite_template(game, ui.row, row_x, row_y);
        draw_skill_icon(game, definition, row_x, row_y, ui.row.height);
        draw_skill_text(
            game,
            definition,
            skill.level,
            crate::game_gui::maximum_skill_level(skill),
            row_x,
            row_y,
            ui.row,
        );
        draw_skill_point_button(game, definition.skill_id, row_x, row_y, ui.row, &ui);
    }
    draw_page_controls(game, window.x, window.y, page, page_count, &ui);
}

fn resolve_skill_ui(layout: &GuiLayout) -> Option<SkillUi<'_>> {
    let title = game_gui::named_region(layout, "skill-title")?;
    let list = game_gui::named_region(layout, "skill-list")?;
    let points = game_gui::named_region(layout, "skill-points")?;
    let page_previous = game_gui::named_region(layout, "skill-page-previous")?;
    let page_label = game_gui::named_region(layout, "skill-page-label")?;
    let page_next = game_gui::named_region(layout, "skill-page-next")?;
    let row = game_gui::named_sprite_template(layout, "skill-row")?;
    let point_up = game_gui::named_sprite_template(layout, "skill-point-up");
    let point_up_disabled = game_gui::named_sprite_template(layout, "skill-point-up-disabled");
    if row.width > list.width || row.height > list.height {
        return None;
    }
    let page_size = (list.height / row.height).floor() as usize;
    (page_size > 0).then_some(SkillUi {
        title,
        list,
        points,
        page_previous,
        page_label,
        page_next,
        row,
        point_up,
        point_up_disabled,
        page_size,
    })
}

fn draw_skill_point_button(
    game: &Game,
    skill_id: u32,
    row_x: f32,
    row_y: f32,
    row: &GuiSpriteTemplate,
    ui: &SkillUi<'_>,
) {
    let enabled = game_gui::can_allocate_skill(&game.skill_book, skill_id);
    let template = if enabled {
        ui.point_up
    } else {
        ui.point_up_disabled.or(ui.point_up)
    };
    let Some(template) = template else {
        return;
    };
    let x = row_x + row.width - template.width - 2.0;
    let y = row_y + (row.height - template.height) / 2.0;
    draw_sprite_template(game, template, x, y);
}

fn draw_header(
    game: &Game,
    window_x: f32,
    window_y: f32,
    ui: &SkillUi<'_>,
) {
    game.context.set_fill_style_str("#30383b");
    game.context.set_font("bold 10px Arial");
    game.context.set_text_align("left");
    let _ = game.context.fill_text_with_max_width(
        &game.skill_book.name,
        f64::from(window_x + ui.title.x),
        f64::from(window_y + text_baseline(ui.title)),
        f64::from(ui.title.width),
    );
    game.context.set_text_align("center");
    let _ = game.context.fill_text_with_max_width(
        &game.skill_book.available_points.to_string(),
        f64::from(window_x + ui.points.x + ui.points.width / 2.0),
        f64::from(window_y + text_baseline(ui.points)),
        f64::from(ui.points.width),
    );
    game.context.set_text_align("left");
}

fn draw_page_controls(
    game: &Game,
    window_x: f32,
    window_y: f32,
    page: usize,
    page_count: usize,
    ui: &SkillUi<'_>,
) {
    if page_count <= 1 {
        return;
    }
    game.context.set_fill_style_str("#30383b");
    game.context.set_font("bold 11px Arial");
    game.context.set_text_align("center");
    for (text, region) in [
        ("<".to_owned(), ui.page_previous),
        (format!("{}/{}", page + 1, page_count), ui.page_label),
        (">".to_owned(), ui.page_next),
    ] {
        let _ = game.context.fill_text_with_max_width(
            &text,
            f64::from(window_x + region.x + region.width / 2.0),
            f64::from(window_y + text_baseline(region)),
            f64::from(region.width),
        );
    }
    game.context.set_text_align("left");
}

fn draw_empty_message(
    game: &Game,
    window_x: f32,
    window_y: f32,
    ui: &SkillUi<'_>,
) {
    game.context.set_fill_style_str("#596469");
    game.context.set_font("10px Arial");
    let _ = game.context.fill_text_with_max_width(
        "No skills available.",
        f64::from(window_x + ui.list.x + 7.0),
        f64::from(window_y + ui.list.y + ui.row.height - 9.0),
        f64::from(ui.list.width - 14.0),
    );
}

fn draw_sprite_template(
    game: &Game,
    template: &GuiSpriteTemplate,
    x: f32,
    y: f32,
) {
    let Some(image) = ready_image(&game.images, &template.asset_id) else {
        return;
    };
    let _ = game
        .context
        .draw_image_with_html_image_element_and_dw_and_dh(
            image,
            f64::from(x),
            f64::from(y),
            f64::from(template.width),
            f64::from(template.height),
        );
}

fn draw_skill_icon(
    game: &Game,
    definition: &oozems_proto::v1::SkillDefinition,
    row_x: f32,
    row_y: f32,
    row_height: f32,
) {
    let Some(image) = ready_image(&game.images, &definition.icon_asset_id) else {
        return;
    };
    let slot_size = row_height - ICON_INSET;
    let x = row_x + (slot_size - definition.icon_width) / 2.0;
    let y = row_y + (slot_size - definition.icon_height) / 2.0;
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
    maximum_level: u32,
    row_x: f32,
    row_y: f32,
    row: &GuiSpriteTemplate,
) {
    let text_x = row_x + row.height + ROW_TEXT_GAP;
    let text_width = row.width - row.height - ROW_TEXT_GAP;
    game.context.set_fill_style_str("#30383b");
    game.context.set_font("bold 10px Arial");
    let _ = game.context.fill_text_with_max_width(
        &definition.name,
        f64::from(text_x),
        f64::from(row_y + 12.0),
        f64::from(text_width),
    );
    game.context.set_fill_style_str("#596469");
    game.context.set_font("9px Arial");
    let _ = game.context.fill_text_with_max_width(
        &format!("Level {level}/{maximum_level}"),
        f64::from(text_x),
        f64::from(row_y + 27.0),
        f64::from(text_width),
    );
}

fn text_baseline(region: &GuiRegion) -> f32 {
    region.y + region.height - TEXT_BOTTOM_PADDING
}

#[cfg(test)]
mod tests {
    use oozems_proto::v1::GuiLayout;
    use oozems_proto::v1::GuiRegion;
    use oozems_proto::v1::GuiSpriteTemplate;

    use super::resolve_skill_ui;

    #[test]
    fn native_row_geometry_determines_skill_page_size() {
        let layout = skill_layout(35.0, 140.0);

        let ui = resolve_skill_ui(&layout).expect("valid skill UI");

        assert_eq!(ui.page_size, 4);
        assert_eq!((ui.row.width, ui.row.height), (141.0, 35.0));
    }

    #[test]
    fn row_larger_than_the_list_is_rejected() {
        let layout = skill_layout(141.0, 140.0);

        assert!(resolve_skill_ui(&layout).is_none());
    }

    fn skill_layout(
        row_height: f32,
        list_height: f32,
    ) -> GuiLayout {
        GuiLayout {
            sprite_templates: vec![GuiSpriteTemplate {
                name: "skill-row".to_owned(),
                asset_id: "skill-row-asset".to_owned(),
                width: 141.0,
                height: row_height,
                origin_x: 0.0,
                origin_y: 0.0,
            }],
            regions: vec![
                region("skill-title", 17.0, 27.0, 141.0, 14.0),
                region("skill-list", 17.0, 94.0, 141.0, list_height),
                region("skill-points", 82.0, 265.0, 30.0, 14.0),
                region("skill-page-previous", 80.0, 64.0, 18.0, 19.0),
                region("skill-page-label", 98.0, 64.0, 41.0, 19.0),
                region("skill-page-next", 139.0, 64.0, 18.0, 19.0),
            ],
            ..GuiLayout::default()
        }
    }

    fn region(
        name: &str,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    ) -> GuiRegion {
        GuiRegion {
            name: name.to_owned(),
            x,
            y,
            width,
            height,
        }
    }
}
