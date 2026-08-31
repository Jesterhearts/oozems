use oozems_proto::v1::GuiLayout;
use oozems_proto::v1::GuiSpriteTemplate;
use oozems_proto::v1::QuestTrackerEntry;

use crate::assets::ready_image;
use crate::game::Game;
use crate::game_gui;
use crate::game_gui::QuestJournalTab;

const DETAIL_LINE_HEIGHT: f64 = 16.0;

pub(super) fn draw(game: &Game) {
    let state = *game.ui.gui_state.borrow();
    let Some(placement) = game_gui::resolve_window(
        &game.ui.gui,
        state.window_placements,
        game_gui::WindowKind::QuestJournal,
        game.surface.canvas.width() as f32,
        game.surface.canvas.height() as f32,
    ) else {
        return;
    };
    if !super::draw_window_at(game, placement.window, placement.origin) {
        return;
    }
    let entries = crate::quest_journal::entries(
        state.quest_journal_tab,
        &game.ui.gui,
        &game.player.state,
        &game.player.active_buffs,
    );
    let page_count = crate::quest_journal::page_count(entries.len());
    let page = state.quest_journal_page.min(page_count - 1);
    let page_start = page * crate::quest_journal::QUESTS_PER_PAGE;
    let selected = if state.quest_journal_selection >= page_start
        && state.quest_journal_selection < entries.len()
    {
        state.quest_journal_selection
    } else {
        page_start
    };

    draw_tabs(
        game,
        placement.layout,
        placement.origin,
        state.quest_journal_tab,
    );
    draw_entries(
        game,
        placement.layout,
        placement.origin,
        &entries,
        page_start,
        selected,
    );
    draw_page_controls(game, placement.layout, placement.origin, page, page_count);
    if let Some(entry) = entries.get(selected) {
        draw_detail(
            game,
            placement.layout,
            placement.origin,
            entry,
            state.quest_journal_tab,
        );
    } else {
        draw_empty_detail(
            game,
            placement.layout,
            placement.origin,
            state.quest_journal_tab,
        );
    }
}

fn draw_tabs(
    game: &Game,
    layout: &GuiLayout,
    origin: game_gui::CanvasPoint,
    selected: QuestJournalTab,
) {
    for tab in QuestJournalTab::ALL {
        let state = if tab == selected {
            "active"
        } else {
            "inactive"
        };
        draw_template_in_region(
            game,
            layout,
            origin,
            &format!("quest-journal-tab-{}-{state}", tab.key()),
            &format!("quest-journal-tab-{}", tab.key()),
        );
    }
}

fn draw_entries(
    game: &Game,
    layout: &GuiLayout,
    origin: game_gui::CanvasPoint,
    entries: &[QuestTrackerEntry],
    page_start: usize,
    selected: usize,
) {
    let Some(list) = game_gui::named_region(layout, "quest-journal-list") else {
        return;
    };
    let row_height = f64::from(list.height) / crate::quest_journal::QUESTS_PER_PAGE as f64;
    for (row, (index, entry)) in entries
        .iter()
        .enumerate()
        .skip(page_start)
        .take(crate::quest_journal::QUESTS_PER_PAGE)
        .enumerate()
    {
        let y = f64::from(origin.y + list.y) + row as f64 * row_height;
        let is_selected = index == selected;
        if is_selected {
            game.surface
                .context
                .set_fill_style_str("rgba(112, 157, 190, 0.28)");
            game.surface.context.fill_rect(
                f64::from(origin.x + list.x),
                y,
                f64::from(list.width),
                row_height,
            );
        }
        draw_template(
            game,
            origin,
            game_gui::named_sprite_template(
                layout,
                if is_selected {
                    "quest-journal-entry-selected"
                } else {
                    "quest-journal-entry"
                },
            ),
            list.x + 3.0,
            list.y + row as f32 * row_height as f32 + 4.0,
        );
        game.surface
            .context
            .set_fill_style_str(if entry.ready { "#3f7d52" } else { "#34393d" });
        game.surface.context.set_font(if is_selected {
            "bold 11px Arial"
        } else {
            "11px Arial"
        });
        game.surface.context.set_text_baseline("middle");
        let _ = game.surface.context.fill_text_with_max_width(
            &entry.title,
            f64::from(origin.x + list.x + 23.0),
            y + row_height / 2.0,
            f64::from((list.width - 27.0).max(1.0)),
        );
    }
    game.surface.context.set_text_baseline("alphabetic");
}

fn draw_page_controls(
    game: &Game,
    layout: &GuiLayout,
    origin: game_gui::CanvasPoint,
    page: usize,
    page_count: usize,
) {
    game.surface.context.set_fill_style_str("#4a5054");
    game.surface.context.set_font("bold 12px Arial");
    game.surface.context.set_text_align("center");
    if page > 0 {
        draw_centered_region_text(game, layout, origin, "quest-journal-page-previous", "<");
    }
    draw_centered_region_text(
        game,
        layout,
        origin,
        "quest-journal-page-label",
        &format!("{} / {}", page + 1, page_count),
    );
    if page + 1 < page_count {
        draw_centered_region_text(game, layout, origin, "quest-journal-page-next", ">");
    }
    game.surface.context.set_text_align("left");
}

fn draw_detail(
    game: &Game,
    layout: &GuiLayout,
    origin: game_gui::CanvasPoint,
    entry: &QuestTrackerEntry,
    tab: QuestJournalTab,
) {
    draw_region_text(
        game,
        layout,
        origin,
        "quest-journal-detail-title",
        &entry.title,
        true,
    );
    let summary = if entry.summary.is_empty() {
        match tab {
            QuestJournalTab::Available => "This quest is available to start.",
            QuestJournalTab::InProgress if entry.ready => "All objectives are complete.",
            QuestJournalTab::InProgress => "Complete the objectives below.",
            QuestJournalTab::Completed => "This quest has been completed.",
        }
    } else {
        &entry.summary
    };
    draw_wrapped_region(
        game,
        layout,
        origin,
        "quest-journal-detail-summary",
        summary,
        "11px Arial",
    );
    draw_objectives(game, layout, origin, entry, tab);
}

fn draw_objectives(
    game: &Game,
    layout: &GuiLayout,
    origin: game_gui::CanvasPoint,
    entry: &QuestTrackerEntry,
    tab: QuestJournalTab,
) {
    let Some(region) = game_gui::named_region(layout, "quest-journal-detail-objectives") else {
        return;
    };
    game.surface.context.set_fill_style_str("#34393d");
    game.surface.context.set_font("bold 11px Arial");
    let heading = match tab {
        QuestJournalTab::Available => "AVAILABLE",
        QuestJournalTab::InProgress => "OBJECTIVES",
        QuestJournalTab::Completed => "COMPLETED",
    };
    let _ = game.surface.context.fill_text(
        heading,
        f64::from(origin.x + region.x),
        f64::from(origin.y + region.y + 12.0),
    );
    game.surface.context.set_font("11px Arial");
    for (index, objective) in entry.objectives.iter().take(8).enumerate() {
        let marker = if objective.complete { "[x]" } else { "[ ]" };
        let counter = if objective.show_counter {
            format!("  {}/{}", objective.current, objective.required)
        } else {
            String::new()
        };
        let text = format!("{marker} {}{counter}", objective.label);
        let _ = game.surface.context.fill_text_with_max_width(
            &text,
            f64::from(origin.x + region.x),
            f64::from(origin.y + region.y + 32.0) + index as f64 * DETAIL_LINE_HEIGHT,
            f64::from(region.width),
        );
    }
}

fn draw_empty_detail(
    game: &Game,
    layout: &GuiLayout,
    origin: game_gui::CanvasPoint,
    tab: QuestJournalTab,
) {
    let message = match tab {
        QuestJournalTab::Available => "No quests are currently available.",
        QuestJournalTab::InProgress => "No quests are in progress.",
        QuestJournalTab::Completed => "No quests have been completed.",
    };
    draw_region_text(
        game,
        layout,
        origin,
        "quest-journal-detail-summary",
        message,
        false,
    );
}

fn draw_region_text(
    game: &Game,
    layout: &GuiLayout,
    origin: game_gui::CanvasPoint,
    region_name: &str,
    text: &str,
    bold: bool,
) {
    let Some(region) = game_gui::named_region(layout, region_name) else {
        return;
    };
    game.surface.context.set_fill_style_str("#30363a");
    game.surface.context.set_font(if bold {
        "bold 13px Arial"
    } else {
        "11px Arial"
    });
    let _ = game.surface.context.fill_text_with_max_width(
        text,
        f64::from(origin.x + region.x),
        f64::from(origin.y + region.y + 13.0),
        f64::from(region.width),
    );
}

fn draw_wrapped_region(
    game: &Game,
    layout: &GuiLayout,
    origin: game_gui::CanvasPoint,
    region_name: &str,
    source: &str,
    font: &str,
) {
    let Some(region) = game_gui::named_region(layout, region_name) else {
        return;
    };
    game.surface.context.set_fill_style_str("#30363a");
    game.surface.context.set_font(font);
    let source = super::interaction::clean_wz_text(source);
    for (index, line) in super::interaction::wrap_text(game, &source, f64::from(region.width))
        .into_iter()
        .take((f64::from(region.height) / DETAIL_LINE_HEIGHT) as usize)
        .enumerate()
    {
        let _ = game.surface.context.fill_text(
            &line,
            f64::from(origin.x + region.x),
            f64::from(origin.y + region.y + 12.0) + index as f64 * DETAIL_LINE_HEIGHT,
        );
    }
}

fn draw_centered_region_text(
    game: &Game,
    layout: &GuiLayout,
    origin: game_gui::CanvasPoint,
    region_name: &str,
    text: &str,
) {
    let Some(region) = game_gui::named_region(layout, region_name) else {
        return;
    };
    game.surface.context.set_text_baseline("middle");
    let _ = game.surface.context.fill_text(
        text,
        f64::from(origin.x + region.x + region.width / 2.0),
        f64::from(origin.y + region.y + region.height / 2.0),
    );
    game.surface.context.set_text_baseline("alphabetic");
}

fn draw_template_in_region(
    game: &Game,
    layout: &GuiLayout,
    origin: game_gui::CanvasPoint,
    template_name: &str,
    region_name: &str,
) {
    let Some(region) = game_gui::named_region(layout, region_name) else {
        return;
    };
    let template = game_gui::named_sprite_template(layout, template_name);
    let Some(template) = template else {
        return;
    };
    draw_template(
        game,
        origin,
        Some(template),
        region.x + (region.width - template.width) / 2.0,
        region.y + (region.height - template.height) / 2.0,
    );
}

fn draw_template(
    game: &Game,
    origin: game_gui::CanvasPoint,
    template: Option<&GuiSpriteTemplate>,
    x: f32,
    y: f32,
) {
    let Some(template) = template else {
        return;
    };
    let Some(image) = ready_image(&game.surface.images, &template.asset_id) else {
        return;
    };
    let _ = game
        .surface
        .context
        .draw_image_with_html_image_element_and_dw_and_dh(
            image,
            f64::from(origin.x + x),
            f64::from(origin.y + y),
            f64::from(template.width),
            f64::from(template.height),
        );
}
