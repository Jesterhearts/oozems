use oozems_proto::v1::GuiLayout;
use oozems_proto::v1::GuiRegion;
use oozems_proto::v1::GuiSpriteTemplate;

use super::draw_window_at;
use crate::assets::ready_image;
use crate::game::Game;
use crate::game_gui;

const TEXT_BOTTOM_PADDING: f32 = 3.0;

struct SkillUi<'a> {
    title: &'a GuiRegion,
    list: &'a GuiRegion,
    points: &'a GuiRegion,
    page_previous: &'a GuiRegion,
    page_label: &'a GuiRegion,
    page_next: &'a GuiRegion,
    row_icon: &'a GuiRegion,
    row_name: &'a GuiRegion,
    row_level: &'a GuiRegion,
    row_point_button: &'a GuiRegion,
    empty_message: &'a GuiRegion,
    row: &'a GuiSpriteTemplate,
    selected_row: Option<&'a GuiSpriteTemplate>,
    point_up: Option<&'a GuiSpriteTemplate>,
    point_up_disabled: Option<&'a GuiSpriteTemplate>,
    tabs: Vec<SkillTabUi<'a>>,
    page_size: usize,
}

struct SkillTabUi<'a> {
    region: &'a GuiRegion,
    enabled: &'a GuiSpriteTemplate,
    disabled: &'a GuiSpriteTemplate,
}

pub(super) fn draw(game: &Game) {
    let Some(placement) = game_gui::resolve_window(
        &game.ui.gui,
        game.ui.gui_state.borrow().window_placements,
        game_gui::WindowKind::Skills,
        game.surface.canvas.width() as f32,
        game.surface.canvas.height() as f32,
    ) else {
        return;
    };
    if !draw_window_at(game, placement.window, placement.origin) {
        return;
    }
    let Some(ui) = resolve_skill_ui(placement.layout) else {
        return;
    };
    let window_x = placement.origin.x;
    let window_y = placement.origin.y;

    draw_job_tabs(game, window_x, window_y, &ui);
    draw_header(game, window_x, window_y, &ui);
    if game.player.skill_book.skills.is_empty() {
        draw_empty_message(game, window_x, window_y, &ui);
        return;
    }

    let page_count = game.player.skill_book.skills.len().div_ceil(ui.page_size);
    let page = game.ui.gui_state.borrow().skill_page % page_count;
    for (index, skill) in game
        .player
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
        let row_x = window_x + ui.list.x;
        let row_y = window_y + ui.list.y + index as f32 * ui.row.height;
        let row = if game.ui.pointer.is_some_and(|point| {
            point.x >= row_x
                && point.x < row_x + ui.row.width
                && point.y >= row_y
                && point.y < row_y + ui.row.height
        }) {
            ui.selected_row.unwrap_or(ui.row)
        } else {
            ui.row
        };
        draw_sprite_template(game, row, row_x, row_y);
        let icon = repeated_region(ui.row_icon, index, ui.row.height);
        let name = repeated_region(ui.row_name, index, ui.row.height);
        let level = repeated_region(ui.row_level, index, ui.row.height);
        let point_button = repeated_region(ui.row_point_button, index, ui.row.height);
        draw_skill_icon(game, definition, window_x, window_y, icon);
        draw_skill_text(
            game,
            definition,
            skill.level,
            crate::game_gui::maximum_skill_level(skill),
            window_x,
            window_y,
            name,
            level,
        );
        draw_skill_point_button(
            game,
            definition.skill_id,
            window_x,
            window_y,
            point_button,
            &ui,
        );
    }
    draw_page_controls(game, window_x, window_y, page, page_count, &ui);
}

fn resolve_skill_ui(layout: &GuiLayout) -> Option<SkillUi<'_>> {
    let title = game_gui::named_region(layout, "skill-title")?;
    let list = game_gui::named_region(layout, "skill-list")?;
    let points = game_gui::named_region(layout, "skill-points")?;
    let page_previous = game_gui::named_region(layout, "skill-page-previous")?;
    let page_label = game_gui::named_region(layout, "skill-page-label")?;
    let page_next = game_gui::named_region(layout, "skill-page-next")?;
    let row_icon = game_gui::named_region(layout, "skill-row-icon")?;
    let row_name = game_gui::named_region(layout, "skill-row-name")?;
    let row_level = game_gui::named_region(layout, "skill-row-level")?;
    let row_point_button = game_gui::named_region(layout, "skill-row-point-button")?;
    let empty_message = game_gui::named_region(layout, "skill-empty-message")?;
    let row = game_gui::named_sprite_template(layout, "skill-row")?;
    let selected_row = game_gui::named_sprite_template(layout, "skill-row-selected");
    let point_up = game_gui::named_sprite_template(layout, "skill-point-up");
    let point_up_disabled = game_gui::named_sprite_template(layout, "skill-point-up-disabled");
    let tabs = (0..5)
        .filter_map(|index| {
            Some(SkillTabUi {
                region: game_gui::named_region(layout, &format!("skill-job-tab-{index}"))?,
                enabled: game_gui::named_sprite_template(
                    layout,
                    &format!("skill-job-tab-{index}-enabled"),
                )?,
                disabled: game_gui::named_sprite_template(
                    layout,
                    &format!("skill-job-tab-{index}-disabled"),
                )?,
            })
        })
        .collect();
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
        row_icon,
        row_name,
        row_level,
        row_point_button,
        empty_message,
        row,
        selected_row,
        point_up,
        point_up_disabled,
        tabs,
        page_size,
    })
}

fn draw_job_tabs(
    game: &Game,
    window_x: f32,
    window_y: f32,
    ui: &SkillUi<'_>,
) {
    let selected = game
        .player
        .stats
        .as_ref()
        .map_or(0, |stats| job_tab_index(stats.job_id));
    for (index, tab) in ui.tabs.iter().enumerate() {
        let template = if index == selected {
            tab.enabled
        } else {
            tab.disabled
        };
        draw_sprite_template(
            game,
            template,
            window_x + (tab.region.x + (tab.region.width - template.width) / 2.0).floor(),
            window_y + (tab.region.y + (tab.region.height - template.height) / 2.0).floor(),
        );
    }
}

fn job_tab_index(job_id: u32) -> usize {
    if job_id.is_multiple_of(1_000) {
        return 0;
    }
    let advancement = job_id % 100;
    match advancement {
        0 => 1,
        value if value.is_multiple_of(10) => 2,
        value if value % 10 == 1 => 3,
        _ => 4,
    }
}

fn draw_skill_point_button(
    game: &Game,
    skill_id: u32,
    window_x: f32,
    window_y: f32,
    region: GuiRegion,
    ui: &SkillUi<'_>,
) {
    let enabled = game_gui::can_allocate_skill(&game.player.skill_book, skill_id);
    let template = if enabled {
        ui.point_up
    } else {
        ui.point_up_disabled.or(ui.point_up)
    };
    let Some(template) = template else {
        return;
    };
    let x = window_x + region.x + (region.width - template.width) / 2.0;
    let y = window_y + region.y + (region.height - template.height) / 2.0;
    draw_sprite_template(game, template, x, y);
}

fn draw_header(
    game: &Game,
    window_x: f32,
    window_y: f32,
    ui: &SkillUi<'_>,
) {
    game.surface.context.set_fill_style_str("#1c252a");
    game.surface.context.set_font("bold 10px Arial");
    game.surface.context.set_text_align("left");
    let _ = game.surface.context.fill_text_with_max_width(
        &game.player.skill_book.name,
        f64::from(window_x + ui.title.x + 1.0),
        f64::from(window_y + text_baseline(ui.title) + 1.0),
        f64::from(ui.title.width),
    );
    game.surface.context.set_fill_style_str("#ffffff");
    let _ = game.surface.context.fill_text_with_max_width(
        &game.player.skill_book.name,
        f64::from(window_x + ui.title.x),
        f64::from(window_y + text_baseline(ui.title)),
        f64::from(ui.title.width),
    );
    game.surface.context.set_fill_style_str("#30383b");
    game.surface.context.set_text_align("center");
    let _ = game.surface.context.fill_text_with_max_width(
        &game.player.skill_book.available_points.to_string(),
        f64::from(window_x + ui.points.x + ui.points.width / 2.0),
        f64::from(window_y + text_baseline(ui.points)),
        f64::from(ui.points.width),
    );
    game.surface.context.set_text_align("left");
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
    game.surface.context.set_fill_style_str("#30383b");
    game.surface.context.set_font("bold 11px Arial");
    game.surface.context.set_text_align("center");
    for (text, region) in [
        ("<".to_owned(), ui.page_previous),
        (format!("{}/{}", page + 1, page_count), ui.page_label),
        (">".to_owned(), ui.page_next),
    ] {
        let _ = game.surface.context.fill_text_with_max_width(
            &text,
            f64::from(window_x + region.x + region.width / 2.0),
            f64::from(window_y + text_baseline(region)),
            f64::from(region.width),
        );
    }
    game.surface.context.set_text_align("left");
}

fn draw_empty_message(
    game: &Game,
    window_x: f32,
    window_y: f32,
    ui: &SkillUi<'_>,
) {
    game.surface.context.set_fill_style_str("#596469");
    game.surface.context.set_font("10px Arial");
    let _ = game.surface.context.fill_text_with_max_width(
        "No skills available.",
        f64::from(window_x + ui.empty_message.x),
        f64::from(window_y + text_baseline(ui.empty_message)),
        f64::from(ui.empty_message.width),
    );
}

fn draw_sprite_template(
    game: &Game,
    template: &GuiSpriteTemplate,
    x: f32,
    y: f32,
) {
    let Some(image) = ready_image(&game.surface.images, &template.asset_id) else {
        return;
    };
    let _ = game
        .surface
        .context
        .draw_image_with_html_image_element_and_dw_and_dh(
            image,
            f64::from(x + template.offset_x),
            f64::from(y + template.offset_y),
            f64::from(template.width),
            f64::from(template.height),
        );
}

fn draw_skill_icon(
    game: &Game,
    definition: &oozems_proto::v1::SkillDefinition,
    window_x: f32,
    window_y: f32,
    region: GuiRegion,
) {
    let Some(image) = ready_image(&game.surface.images, &definition.icon_asset_id) else {
        return;
    };
    let x = window_x + region.x + (region.width - definition.icon_width) / 2.0;
    let y = window_y + region.y + (region.height - definition.icon_height) / 2.0;
    let _ = game
        .surface
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
    window_x: f32,
    window_y: f32,
    name: GuiRegion,
    level_region: GuiRegion,
) {
    game.surface.context.set_fill_style_str("#30383b");
    game.surface.context.set_font("bold 10px Arial");
    let _ = game.surface.context.fill_text_with_max_width(
        &definition.name,
        f64::from(window_x + name.x),
        f64::from(window_y + text_baseline(&name)),
        f64::from(name.width),
    );
    game.surface.context.set_fill_style_str("#596469");
    game.surface.context.set_font("9px Arial");
    let _ = game.surface.context.fill_text_with_max_width(
        &format!("Level {level}/{maximum_level}"),
        f64::from(window_x + level_region.x),
        f64::from(window_y + text_baseline(&level_region)),
        f64::from(level_region.width),
    );
}

fn repeated_region(
    prototype: &GuiRegion,
    index: usize,
    step_y: f32,
) -> GuiRegion {
    GuiRegion {
        y: prototype.y + index as f32 * step_y,
        ..prototype.clone()
    }
}

fn text_baseline(region: &GuiRegion) -> f32 {
    region.y + region.height - TEXT_BOTTOM_PADDING
}

#[cfg(test)]
mod tests {
    use oozems_proto::v1::GuiLayout;
    use oozems_proto::v1::GuiRegion;
    use oozems_proto::v1::GuiSpriteTemplate;

    use super::job_tab_index;
    use super::resolve_skill_ui;

    #[test]
    fn native_row_geometry_determines_skill_page_size() {
        let layout = skill_layout(35.0, 140.0);

        let ui = resolve_skill_ui(&layout).expect("valid skill UI");

        assert_eq!(ui.page_size, 4);
        assert_eq!((ui.row.width, ui.row.height), (141.0, 35.0));
        assert!(ui.selected_row.is_some());
    }

    #[test]
    fn row_larger_than_the_list_is_rejected() {
        let layout = skill_layout(141.0, 140.0);

        assert!(resolve_skill_ui(&layout).is_none());
    }

    #[test]
    fn job_advancement_selects_the_matching_tab() {
        assert_eq!(job_tab_index(0), 0);
        assert_eq!(job_tab_index(400), 1);
        assert_eq!(job_tab_index(410), 2);
        assert_eq!(job_tab_index(411), 3);
        assert_eq!(job_tab_index(412), 4);
        assert_eq!(job_tab_index(2112), 4);
    }

    fn skill_layout(
        row_height: f32,
        list_height: f32,
    ) -> GuiLayout {
        let mut layout = GuiLayout {
            sprite_templates: vec![
                GuiSpriteTemplate {
                    name: "skill-row".to_owned(),
                    asset_id: "skill-row-asset".to_owned(),
                    width: 141.0,
                    height: row_height,
                    origin_x: 0.0,
                    origin_y: 0.0,
                    offset_x: 0.0,
                    offset_y: 0.0,
                },
                GuiSpriteTemplate {
                    name: "skill-row-selected".to_owned(),
                    asset_id: "skill-row-selected-asset".to_owned(),
                    width: 141.0,
                    height: row_height,
                    origin_x: 0.0,
                    origin_y: 0.0,
                    offset_x: 0.0,
                    offset_y: 0.0,
                },
            ],
            regions: vec![
                region("skill-title", 17.0, 27.0, 141.0, 14.0),
                region("skill-list", 17.0, 94.0, 141.0, list_height),
                region("skill-points", 82.0, 265.0, 30.0, 14.0),
                region("skill-page-previous", 80.0, 64.0, 18.0, 19.0),
                region("skill-page-label", 98.0, 64.0, 41.0, 19.0),
                region("skill-page-next", 139.0, 64.0, 18.0, 19.0),
                region("skill-row-icon", 17.0, 94.0, 32.0, 32.0),
                region("skill-row-name", 54.0, 94.0, 104.0, 15.0),
                region("skill-row-level", 54.0, 109.0, 104.0, 15.0),
                region("skill-row-point-button", 144.0, 105.5, 12.0, 12.0),
                region("skill-empty-message", 24.0, 108.0, 127.0, 15.0),
            ],
            ..GuiLayout::default()
        };
        for index in 0..5 {
            layout
                .sprite_templates
                .extend([true, false].map(|enabled| GuiSpriteTemplate {
                    name: format!(
                        "skill-job-tab-{index}-{}",
                        if enabled { "enabled" } else { "disabled" }
                    ),
                    asset_id: format!("skill-job-tab-{index}-{enabled}-asset"),
                    width: 10.0,
                    height: 12.0,
                    origin_x: 0.0,
                    origin_y: 0.0,
                    offset_x: 0.0,
                    offset_y: 0.0,
                }));
            layout.regions.push(region(
                &format!("skill-job-tab-{index}"),
                47.0 + index as f32 * 22.0,
                26.0,
                20.0,
                14.0,
            ));
        }
        layout
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
