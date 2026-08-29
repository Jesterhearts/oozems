use crate::death_ui;
use crate::game::Game;
use crate::game_gui;

const TEXT_COLOR: &str = "#303438";

pub(super) fn draw(game: &Game) {
    if !death_ui::is_open(game.ui.death) {
        return;
    }
    let native_drawn = game
        .ui
        .gui
        .death_notice_window
        .as_ref()
        .is_some_and(|window| super::draw_window(game, window));
    if native_drawn {
        draw_native_text(game);
    } else {
        draw_fallback(game);
    }
}

fn draw_native_text(game: &Game) {
    let Some(window) = game.ui.gui.death_notice_window.as_ref() else {
        return;
    };
    let Some(layout) = window.layout.as_ref() else {
        return;
    };
    let Some(title) = game_gui::named_region(layout, "death-notice-title") else {
        return;
    };
    let Some(detail) = game_gui::named_region(layout, "death-notice-detail") else {
        return;
    };
    let context = &game.surface.context;
    context.set_fill_style_str(TEXT_COLOR);
    context.set_text_align("center");
    context.set_text_baseline("middle");
    context.set_font("bold 12px Arial");
    draw_native_line(game, window, title, "You have died.");
    context.set_font("11px Arial");
    let message = if game.ui.death.respawn_requested {
        "Returning to the nearest town..."
    } else {
        "You will be revived in the nearest town."
    };
    draw_native_line(game, window, detail, message);
    context.set_text_baseline("alphabetic");
    context.set_text_align("left");
}

fn draw_native_line(
    game: &Game,
    window: &oozems_proto::v1::GuiWindow,
    region: &oozems_proto::v1::GuiRegion,
    text: &str,
) {
    let _ = game.surface.context.fill_text_with_max_width(
        text,
        f64::from(window.x + region.x + region.width / 2.0),
        f64::from(window.y + region.y + region.height / 2.0),
        f64::from(region.width),
    );
}

fn draw_fallback(game: &Game) {
    let context = &game.surface.context;
    let x = f64::from(death_ui::FALLBACK_WINDOW_X);
    let y = f64::from(death_ui::FALLBACK_WINDOW_Y);
    let width = f64::from(death_ui::FALLBACK_WINDOW_WIDTH);
    let height = f64::from(death_ui::FALLBACK_WINDOW_HEIGHT);
    context.set_fill_style_str("#f5f5f5");
    context.fill_rect(x, y, width, height);
    context.set_stroke_style_str("#646b70");
    context.stroke_rect(x + 0.5, y + 0.5, width - 1.0, height - 1.0);
    draw_message(game, x + 14.0, y + 40.0, width - 28.0);

    let button_x = x + f64::from(death_ui::FALLBACK_BUTTON_X);
    let button_y = y + f64::from(death_ui::FALLBACK_BUTTON_Y);
    let button_width = f64::from(death_ui::FALLBACK_BUTTON_WIDTH);
    let button_height = f64::from(death_ui::FALLBACK_BUTTON_HEIGHT);
    context.set_fill_style_str("#d8dcdf");
    context.fill_rect(button_x, button_y, button_width, button_height);
    context.set_stroke_style_str("#777d81");
    context.stroke_rect(
        button_x + 0.5,
        button_y + 0.5,
        button_width - 1.0,
        button_height - 1.0,
    );
    context.set_fill_style_str(TEXT_COLOR);
    context.set_font("bold 11px Arial");
    context.set_text_align("center");
    let _ = context.fill_text("OK", button_x + button_width / 2.0, button_y + 16.0);
    context.set_text_align("left");
}

fn draw_message(
    game: &Game,
    x: f64,
    y: f64,
    width: f64,
) {
    let context = &game.surface.context;
    context.set_fill_style_str(TEXT_COLOR);
    context.set_text_align("center");
    context.set_font("bold 12px Arial");
    let _ = context.fill_text_with_max_width("You have died.", x + width / 2.0, y + 16.0, width);
    context.set_font("11px Arial");
    let detail = if game.ui.death.respawn_requested {
        "Returning to the nearest town..."
    } else {
        "You will be revived in the nearest town."
    };
    let _ = context.fill_text_with_max_width(detail, x + width / 2.0, y + 37.0, width);
    context.set_text_align("left");
}
