use oozems_proto::v1::CashShopOffer;
use oozems_proto::v1::GuiLayout;
use oozems_proto::v1::GuiRegion;
use oozems_proto::v1::GuiWindow;

use super::Game;
use crate::assets::ready_image;
use crate::cash_shop_ui;
use crate::character_render;
use crate::character_render::CharacterAnimation;
use crate::character_render::CharacterPlacement;
use crate::game::character_animation_elapsed_ms;
use crate::game_gui;

pub(super) fn draw(game: &Game) {
    let viewport_width = game.surface.canvas.width() as f32;
    let viewport_height = game.surface.canvas.height() as f32;
    game.surface.context.set_fill_style_str("#15191e");
    game.surface.context.fill_rect(
        0.0,
        0.0,
        f64::from(viewport_width),
        f64::from(viewport_height),
    );

    let Some(window) = game.ui.gui.cash_shop_window.as_ref() else {
        return;
    };
    let Some(transform) = cash_shop_ui::screen_transform(window, viewport_width, viewport_height)
    else {
        return;
    };
    let Some(layout) = window.layout.as_ref() else {
        return;
    };

    game.surface.context.save();
    let transformed = game
        .surface
        .context
        .translate(f64::from(transform.origin_x), f64::from(transform.origin_y))
        .and_then(|()| {
            game.surface
                .context
                .scale(f64::from(transform.scale), f64::from(transform.scale))
        });
    if transformed.is_ok() && super::draw_window(game, window) {
        draw_character_preview(game, window, layout);
        draw_account_details(game, window, layout);
        draw_offers(game, window, layout);
        draw_request_state(game, window, layout);
    }
    game.surface.context.restore();
}

fn draw_character_preview(
    game: &Game,
    window: &GuiWindow,
    layout: &GuiLayout,
) {
    let Some(region) = game_gui::named_region(layout, "cash-shop-character") else {
        return;
    };
    character_render::draw_character(
        &game.surface.context,
        &game.surface.images,
        &game.world.character_sprites,
        CharacterAnimation::Idle,
        character_animation_elapsed_ms(game.world.character_animation, game.clock.now_ms),
        CharacterPlacement {
            anchor_x: f64::from(window.x + region.x + region.width / 2.0),
            anchor_y: f64::from(window.y + region.y + region.height),
            scale: 1.0,
            facing_left: false,
        },
    );
}

fn draw_account_details(
    game: &Game,
    window: &GuiWindow,
    layout: &GuiLayout,
) {
    game.surface.context.set_fill_style_str("#263238");
    game.surface.context.set_font("bold 12px Arial");
    draw_region_text(
        game,
        window,
        layout,
        "cash-shop-player-name",
        &game.player.name,
    );
    draw_region_text(
        game,
        window,
        layout,
        "cash-shop-balance",
        &currency_amount_label(game.player.cash_points, &game.ui.cash_shop.currency_name),
    );
}

fn draw_offers(
    game: &Game,
    window: &GuiWindow,
    layout: &GuiLayout,
) {
    let Some(offers) = game.ui.cash_shop.offers.as_ref() else {
        return;
    };
    for (index, offer) in offers.iter().take(10).enumerate() {
        let Some(buy_region) = game_gui::named_region(layout, &format!("cash-shop-buy-{index}"))
        else {
            continue;
        };
        let gift_region = game_gui::named_region(layout, &format!("cash-shop-gift-{index}"));
        let definition = super::item_definition(game, offer.item_id);
        if let (Some(definition), Some(icon_region)) = (
            definition,
            game_gui::named_region(layout, &format!("cash-shop-item-icon-{index}")),
        ) {
            super::draw_item_icon_in_region(
                game,
                definition,
                window.x + icon_region.x,
                window.y + icon_region.y,
                icon_region.width,
                icon_region.height,
            );
        }
        if let Some(name_region) =
            game_gui::named_region(layout, &format!("cash-shop-item-name-{index}"))
        {
            game.surface.context.set_fill_style_str("#25323a");
            game.surface.context.set_font("bold 11px Arial");
            let name = definition
                .map(|definition| definition.name.clone())
                .unwrap_or_else(|| format!("Item {}", offer.item_id));
            draw_text_in_region(game, window, name_region, &name);
        }
        draw_offer_details(game, window, layout, offer, index);
        draw_offer_button(
            game,
            layout,
            "cash-shop-buy",
            window.x + buy_region.x,
            window.y + buy_region.y,
        );
        if let Some(gift_region) = gift_region {
            draw_offer_button(
                game,
                layout,
                "cash-shop-gift-disabled",
                window.x + gift_region.x,
                window.y + gift_region.y,
            );
        }
    }
}

fn draw_offer_details(
    game: &Game,
    window: &GuiWindow,
    layout: &GuiLayout,
    offer: &CashShopOffer,
    index: usize,
) {
    if let Some(duration) =
        game_gui::named_region(layout, &format!("cash-shop-item-duration-{index}"))
    {
        game.surface.context.set_font("10px Arial");
        game.surface.context.set_fill_style_str("#4b5f6b");
        draw_text_in_region(game, window, duration, &duration_label(offer.duration_ms));
    }
    if let Some(price) = game_gui::named_region(layout, &format!("cash-shop-item-price-{index}")) {
        game.surface.context.set_fill_style_str("#b45d26");
        game.surface.context.set_font("bold 10px Arial");
        draw_text_in_region(
            game,
            window,
            price,
            &currency_amount_label(offer.price, &game.ui.cash_shop.currency_name),
        );
    }
}

fn draw_offer_button(
    game: &Game,
    layout: &GuiLayout,
    name: &str,
    x: f32,
    y: f32,
) {
    let Some(template) = game_gui::named_sprite_template(layout, name) else {
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
            f64::from(x + template.offset_x),
            f64::from(y + template.offset_y),
            f64::from(template.width),
            f64::from(template.height),
        );
}

fn draw_request_state(
    game: &Game,
    window: &GuiWindow,
    layout: &GuiLayout,
) {
    let message = if game.cash_shop_request_in_flight() {
        Some("Processing...")
    } else if let Some(error) = game.ui.cash_shop.load_error.as_deref() {
        Some(error)
    } else if game.ui.cash_shop.offers.as_ref().is_some_and(Vec::is_empty) {
        Some("No Cash Shop offers are configured.")
    } else {
        None
    };
    let Some(message) = message else {
        return;
    };
    let Some(panel) = game_gui::named_region(layout, "cash-shop-request-panel") else {
        return;
    };
    let Some(text) = game_gui::named_region(layout, "cash-shop-request-text") else {
        return;
    };
    game.surface
        .context
        .set_fill_style_str("rgba(245, 248, 250, 0.94)");
    game.surface.context.fill_rect(
        f64::from(window.x + panel.x),
        f64::from(window.y + panel.y),
        f64::from(panel.width),
        f64::from(panel.height),
    );
    game.surface.context.set_fill_style_str("#38464f");
    game.surface.context.set_font("bold 12px Arial");
    draw_text_in_region(game, window, text, message);
}

fn draw_region_text(
    game: &Game,
    window: &GuiWindow,
    layout: &GuiLayout,
    name: &str,
    text: &str,
) {
    if let Some(region) = game_gui::named_region(layout, name) {
        draw_text_in_region(game, window, region, text);
    }
}

fn draw_text_in_region(
    game: &Game,
    window: &GuiWindow,
    region: &GuiRegion,
    text: &str,
) {
    let _ = game.surface.context.fill_text_with_max_width(
        text,
        f64::from(window.x + region.x),
        f64::from(window.y + region.y + region.height - 3.0),
        f64::from(region.width),
    );
}

fn duration_label(duration_ms: u64) -> String {
    if duration_ms == 0 {
        return "Permanent".to_owned();
    }
    const HOUR_MS: u64 = 60 * 60 * 1_000;
    const DAY_MS: u64 = 24 * HOUR_MS;
    if duration_ms.is_multiple_of(DAY_MS) {
        return unit_label(duration_ms / DAY_MS, "day");
    }
    if duration_ms.is_multiple_of(HOUR_MS) {
        return unit_label(duration_ms / HOUR_MS, "hour");
    }
    let minutes = duration_ms.div_ceil(60 * 1_000);
    unit_label(minutes, "minute")
}

fn unit_label(
    value: u64,
    unit: &str,
) -> String {
    let suffix = if value == 1 { "" } else { "s" };
    format!("{value} {unit}{suffix}")
}

fn format_points(value: u64) -> String {
    let digits = value.to_string();
    let mut formatted = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            formatted.push(',');
        }
        formatted.push(digit);
    }
    formatted
}

fn currency_amount_label(
    value: u64,
    currency_name: &str,
) -> String {
    format!("{} {currency_name}", format_points(value))
}

#[cfg(test)]
mod tests {
    use super::currency_amount_label;
    use super::duration_label;
    use super::format_points;

    #[test]
    fn offer_duration_uses_a_compact_human_label() {
        assert_eq!(duration_label(0), "Permanent");
        assert_eq!(duration_label(24 * 60 * 60 * 1_000), "1 day");
        assert_eq!(duration_label(30 * 24 * 60 * 60 * 1_000), "30 days");
        assert_eq!(duration_label(2 * 60 * 60 * 1_000), "2 hours");
    }

    #[test]
    fn cash_points_use_grouping_separators() {
        assert_eq!(format_points(0), "0");
        assert_eq!(format_points(999), "999");
        assert_eq!(format_points(1_234_567), "1,234,567");
    }

    #[test]
    fn configured_currency_name_is_used_for_amounts() {
        assert_eq!(currency_amount_label(1_200, "Ooze"), "1,200 Ooze");
        assert_eq!(currency_amount_label(75, "Slime Tokens"), "75 Slime Tokens");
    }
}
