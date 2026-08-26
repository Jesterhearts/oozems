use oozems_proto::v1::GuiLayout;
use oozems_proto::v1::GuiRegion;
use oozems_proto::v1::GuiSpriteTemplate;
use oozems_proto::v1::GuiWindow;
use oozems_proto::v1::NpcDialogView;
use oozems_proto::v1::NpcShopView;
use oozems_proto::v1::NpcTaxiView;
use oozems_proto::v1::npc_interaction;

use crate::assets::ready_image;
use crate::game::Game;
use crate::interaction_ui::DIALOG_CHOICE_PAGE_SIZE;
use crate::interaction_ui::SHOP_PAGE_SIZE;
use crate::interaction_ui::dialog_previous_region;
use crate::interaction_ui::is_quest_decision;
use crate::interaction_ui::visual_dialog_pages;

const DIALOG_LINE_HEIGHT: f64 = 17.0;
const CHOICE_ROW_HEIGHT: f64 = 24.0;
const SHOP_ROW_HEIGHT: f32 = 37.0;
const SHOP_ROW_TOP: f32 = 2.0;
const SHOP_ICON_LEFT: f32 = 1.0;
const SHOP_TEXT_LEFT: f32 = 37.0;

pub(super) fn draw(game: &Game) {
    let Some(interaction) = game.ui.interaction.interaction.as_ref() else {
        return;
    };
    match interaction.view.as_ref() {
        Some(npc_interaction::View::Dialog(dialog)) => {
            draw_dialog(game, interaction, dialog);
        }
        Some(npc_interaction::View::Shop(shop)) => draw_shop(game, interaction, shop),
        Some(npc_interaction::View::Taxi(taxi)) => draw_taxi(game, interaction, taxi),
        None => {}
    }
}

fn draw_dialog(
    game: &Game,
    interaction: &oozems_proto::v1::NpcInteraction,
    dialog: &NpcDialogView,
) {
    let Some(window) = game.ui.gui.npc_dialog_window.as_ref() else {
        return;
    };
    if !super::draw_window(game, window) {
        return;
    }
    draw_npc_portrait(game, interaction, window);
    draw_dialog_heading(game, interaction, &dialog.title, window);
    let pages = visual_dialog_pages(dialog);
    let page = pages
        .get(game.ui.interaction.page)
        .map(String::as_str)
        .unwrap_or_default();
    draw_wrapped_region(game, window, "npc-text", page);
    let Some(layout) = window.layout.as_ref() else {
        return;
    };
    let choice_page_count = dialog
        .choices
        .len()
        .max(1)
        .div_ceil(DIALOG_CHOICE_PAGE_SIZE);
    let previous_region = dialog_previous_region(&game.ui.interaction, dialog, pages.len());
    if game.ui.interaction.choice_page > 0 || game.ui.interaction.page > 0 {
        draw_template_in_region(game, window, layout, "npc-dialog-previous", previous_region);
    }
    if game.ui.interaction.page + 1 < pages.len() {
        draw_template_in_region(game, window, layout, "npc-dialog-next", "npc-next");
        return;
    }
    if game.ui.interaction.choice_page + 1 < choice_page_count {
        draw_template_in_region(game, window, layout, "npc-dialog-next", "npc-next");
    }
    if is_quest_decision(dialog) {
        draw_template_in_region(game, window, layout, "npc-dialog-accept", "npc-accept");
        draw_template_in_region(game, window, layout, "npc-dialog-decline", "npc-decline");
    } else if !dialog.choices.is_empty() {
        draw_dialog_choices(game, window, layout, dialog);
    } else {
        draw_template_in_region(game, window, layout, "npc-dialog-ok", "npc-ok");
    }
}

fn draw_taxi(
    game: &Game,
    interaction: &oozems_proto::v1::NpcInteraction,
    taxi: &NpcTaxiView,
) {
    let Some(window) = game.ui.gui.npc_dialog_window.as_ref() else {
        return;
    };
    if !super::draw_window(game, window) {
        return;
    }
    draw_npc_portrait(game, interaction, window);
    draw_dialog_heading(game, interaction, "Destinations", window);
    let Some(layout) = window.layout.as_ref() else {
        return;
    };
    let Some(region) = region(layout, "npc-choices") else {
        return;
    };
    for (index, destination) in taxi.destinations.iter().enumerate() {
        let y = f64::from(window.y + region.y) + 14.0 + index as f64 * CHOICE_ROW_HEIGHT;
        draw_template(
            game,
            window,
            template(layout, "npc-dialog-choice-selected"),
            region.x,
            region.y + index as f32 * CHOICE_ROW_HEIGHT as f32 + 4.0,
        );
        game.surface.context.set_fill_style_str("#2f3437");
        game.surface.context.set_font("bold 12px Arial");
        let label = format!("{}  ({} mesos)", destination.label, destination.fare);
        let _ = game.surface.context.fill_text_with_max_width(
            &label,
            f64::from(window.x + region.x + 13.0),
            y,
            f64::from(region.width - 16.0),
        );
    }
    draw_template_in_region(game, window, layout, "npc-dialog-close", "npc-close");
}

fn draw_shop(
    game: &Game,
    interaction: &oozems_proto::v1::NpcInteraction,
    shop: &NpcShopView,
) {
    let Some(window) = game.ui.gui.shop_window.as_ref() else {
        return;
    };
    if !super::draw_window(game, window) {
        return;
    }
    let Some(layout) = window.layout.as_ref() else {
        return;
    };
    let Some(stock_region) = region(layout, "shop-stock") else {
        return;
    };
    let Some(inventory_region) = region(layout, "shop-inventory") else {
        return;
    };
    let Some(balance_region) = region(layout, "shop-mesos") else {
        return;
    };
    let cash_point_shop = crate::interaction_ui::is_cash_point_shop(shop);
    let now_unix_ms = js_sys::Date::now().max(0.0) as u64;
    draw_shop_portrait(game, interaction, window);
    if let Some(index) = game
        .ui
        .interaction
        .selected_offer
        .filter(|index| *index < SHOP_PAGE_SIZE)
    {
        draw_shop_selection(game, window, layout, stock_region, index);
    }
    if !cash_point_shop
        && let Some(index) = game
            .ui
            .interaction
            .selected_inventory
            .and_then(|index| {
                index.checked_sub(game.ui.interaction.inventory_page * SHOP_PAGE_SIZE)
            })
            .filter(|index| *index < SHOP_PAGE_SIZE)
    {
        draw_shop_selection(game, window, layout, inventory_region, index);
    }
    for (index, offer) in shop.offers.iter().take(SHOP_PAGE_SIZE).enumerate() {
        if let Some(definition) = super::item_definition(game, offer.item_id) {
            let y = window.y + shop_row_y(stock_region, index);
            super::draw_item_icon(
                game,
                definition,
                window.x + stock_region.x + SHOP_ICON_LEFT,
                y,
            );
            draw_shop_item_text(
                game,
                window.x + stock_region.x + SHOP_TEXT_LEFT,
                y,
                &definition.name,
                &shop_price_label(offer.buy_price, &shop.currency_name),
            );
        }
    }
    if !cash_point_shop && let Some(inventory) = game.player.inventory.as_ref() {
        let start = game.ui.interaction.inventory_page * SHOP_PAGE_SIZE;
        for (index, stack) in inventory
            .stacks
            .iter()
            .skip(start)
            .take(SHOP_PAGE_SIZE)
            .enumerate()
        {
            if let Some(definition) = super::item_definition(game, stack.item_id) {
                let y = window.y + shop_row_y(inventory_region, index);
                let icon_x = window.x + inventory_region.x + SHOP_ICON_LEFT;
                super::draw_item_icon(game, definition, icon_x, y);
                super::draw_item_quantity(game, stack.quantity, icon_x, y);
                let price = if definition.sale_price == 0 {
                    "Cannot sell".to_owned()
                } else {
                    format!("{} mesos", definition.sale_price)
                };
                let detail = super::item_expiration_detail(
                    super::item_expiration(stack.expires_at_unix_ms, now_unix_ms),
                    super::permanent_stack_needs_label(&inventory.stacks, stack),
                )
                .map(|expiration| format!("{price}, {expiration}"))
                .unwrap_or(price);
                draw_shop_item_text(
                    game,
                    window.x + inventory_region.x + SHOP_TEXT_LEFT,
                    y,
                    &definition.name,
                    &detail,
                );
            }
        }
        draw_inventory_page_controls(game, window, layout, inventory.stacks.len());
    }
    draw_template_in_region(game, window, layout, "shop-buy", "shop-buy");
    if !cash_point_shop {
        draw_template_in_region(game, window, layout, "shop-sell", "shop-sell");
    }
    draw_template_in_region(game, window, layout, "shop-exit", "shop-close");
    game.surface.context.set_fill_style_str("#202020");
    game.surface.context.set_font("bold 11px Arial");
    if cash_point_shop {
        let _ = game.surface.context.fill_text_with_max_width(
            &format!("{}: {}", shop.currency_name, game.player.cash_points),
            f64::from(window.x + balance_region.x + 5.0),
            f64::from(window.y + balance_region.y + 11.0),
            f64::from((balance_region.width - 10.0).max(0.0)),
        );
    } else {
        draw_template_in_region(game, window, layout, "shop-meso", "shop-mesos");
        let _ = game.surface.context.fill_text(
            &game.player.mesos.to_string(),
            f64::from(window.x + balance_region.x + 19.0),
            f64::from(window.y + balance_region.y + 11.0),
        );
    }
}

fn draw_shop_selection(
    game: &Game,
    window: &GuiWindow,
    layout: &GuiLayout,
    list_region: &GuiRegion,
    index: usize,
) {
    let Some(selection) = template(layout, "shop-selection") else {
        return;
    };
    draw_template(
        game,
        window,
        Some(selection),
        shop_selection_x(list_region, selection),
        shop_row_y(list_region, index),
    );
}

fn shop_selection_x(
    region: &GuiRegion,
    selection: &GuiSpriteTemplate,
) -> f32 {
    region.x + (region.width - selection.width).max(0.0)
}

fn shop_row_y(
    region: &GuiRegion,
    index: usize,
) -> f32 {
    region.y + SHOP_ROW_TOP + index as f32 * SHOP_ROW_HEIGHT
}

fn shop_price_label(
    price: u64,
    currency_name: &str,
) -> String {
    format!("{price} {currency_name}")
}

fn draw_inventory_page_controls(
    game: &Game,
    window: &GuiWindow,
    layout: &GuiLayout,
    item_count: usize,
) {
    let page_count = item_count.max(1).div_ceil(SHOP_PAGE_SIZE);
    if page_count <= 1 {
        return;
    }
    game.surface.context.set_fill_style_str("#eef7ff");
    game.surface.context.set_font("bold 13px Arial");
    if game.ui.interaction.inventory_page > 0
        && let Some(region) = region(layout, "shop-inventory-previous")
    {
        let _ = game.surface.context.fill_text(
            "<",
            f64::from(window.x + region.x + 4.0),
            f64::from(window.y + region.y + 13.0),
        );
    }
    if game.ui.interaction.inventory_page + 1 < page_count
        && let Some(region) = region(layout, "shop-inventory-next")
    {
        let _ = game.surface.context.fill_text(
            ">",
            f64::from(window.x + region.x + 4.0),
            f64::from(window.y + region.y + 13.0),
        );
    }
}

fn draw_shop_item_text(
    game: &Game,
    x: f32,
    y: f32,
    name: &str,
    price: &str,
) {
    game.surface.context.set_fill_style_str("#202020");
    game.surface.context.set_font("10px Arial");
    let _ = game.surface.context.fill_text_with_max_width(
        name,
        f64::from(x),
        f64::from(y + 13.0),
        155.0,
    );
    game.surface.context.set_fill_style_str("#53606a");
    let _ = game.surface.context.fill_text_with_max_width(
        price,
        f64::from(x),
        f64::from(y + 27.0),
        155.0,
    );
}

fn draw_dialog_choices(
    game: &Game,
    window: &GuiWindow,
    layout: &GuiLayout,
    dialog: &NpcDialogView,
) {
    let Some(region) = region(layout, "npc-choices") else {
        return;
    };
    for (index, choice) in dialog
        .choices
        .iter()
        .skip(game.ui.interaction.choice_page * DIALOG_CHOICE_PAGE_SIZE)
        .take(DIALOG_CHOICE_PAGE_SIZE)
        .enumerate()
    {
        draw_template(
            game,
            window,
            template(layout, "npc-dialog-choice-selected"),
            region.x,
            region.y + index as f32 * CHOICE_ROW_HEIGHT as f32 + 4.0,
        );
        game.surface.context.set_fill_style_str("#2f3437");
        game.surface.context.set_font("bold 12px Arial");
        let _ = game.surface.context.fill_text_with_max_width(
            &choice.label,
            f64::from(window.x + region.x + 13.0),
            f64::from(window.y + region.y) + 14.0 + index as f64 * CHOICE_ROW_HEIGHT,
            f64::from(region.width - 16.0),
        );
    }
}

fn draw_dialog_heading(
    game: &Game,
    interaction: &oozems_proto::v1::NpcInteraction,
    title: &str,
    window: &GuiWindow,
) {
    let Some(layout) = window.layout.as_ref() else {
        return;
    };
    let Some(region) = region(layout, "npc-title") else {
        return;
    };
    let heading = if title.is_empty() {
        interaction.npc_name.clone()
    } else {
        format!("{} - {title}", interaction.npc_name)
    };
    game.surface.context.set_fill_style_str("#2f3437");
    game.surface.context.set_font("bold 13px Arial");
    let _ = game.surface.context.fill_text_with_max_width(
        &heading,
        f64::from(window.x + region.x),
        f64::from(window.y + region.y + 13.0),
        f64::from(region.width),
    );
}

fn draw_wrapped_region(
    game: &Game,
    window: &GuiWindow,
    region_name: &str,
    source: &str,
) {
    let Some(layout) = window.layout.as_ref() else {
        return;
    };
    let Some(region) = region(layout, region_name) else {
        return;
    };
    game.surface.context.set_fill_style_str("#2f3437");
    game.surface.context.set_font("12px Arial");
    for (index, line) in wrap_text(game, &clean_wz_text(source), f64::from(region.width))
        .into_iter()
        .take((f64::from(region.height) / DIALOG_LINE_HEIGHT) as usize)
        .enumerate()
    {
        let _ = game.surface.context.fill_text(
            &line,
            f64::from(window.x + region.x),
            f64::from(window.y + region.y) + 13.0 + index as f64 * DIALOG_LINE_HEIGHT,
        );
    }
}

fn draw_npc_portrait(
    game: &Game,
    interaction: &oozems_proto::v1::NpcInteraction,
    window: &GuiWindow,
) {
    let Some(npc) = game
        .world
        .map
        .npcs
        .iter()
        .find(|npc| npc.spawn_id == interaction.npc_spawn_id)
    else {
        return;
    };
    let frame = super::npc::standing_frames(npc).and_then(|frames| frames.first());
    draw_portrait_frame(game, window, frame);
}

fn draw_shop_portrait(
    game: &Game,
    interaction: &oozems_proto::v1::NpcInteraction,
    window: &GuiWindow,
) {
    let frame = game
        .world
        .map
        .npcs
        .iter()
        .find(|npc| npc.spawn_id == interaction.npc_spawn_id)
        .and_then(super::npc::standing_frames)
        .and_then(|frames| frames.first());
    let Some(frame) = frame else {
        return;
    };
    let Some(image) = ready_image(&game.surface.images, &frame.asset_id) else {
        return;
    };
    let scale = (90.0_f64 / f64::from(frame.width))
        .min(90.0 / f64::from(frame.height))
        .min(1.0);
    let _ = game
        .surface
        .context
        .draw_image_with_html_image_element_and_dw_and_dh(
            image,
            f64::from(window.x + 69.0),
            f64::from(window.y + 20.0),
            f64::from(frame.width) * scale,
            f64::from(frame.height) * scale,
        );
}

fn draw_portrait_frame(
    game: &Game,
    window: &GuiWindow,
    frame: Option<&oozems_proto::v1::NpcFrame>,
) {
    let Some(frame) = frame else {
        return;
    };
    let Some(image) = ready_image(&game.surface.images, &frame.asset_id) else {
        return;
    };
    let Some(portrait) = window
        .layout
        .as_ref()
        .and_then(|layout| region(layout, "npc-portrait"))
    else {
        return;
    };
    let scale = (f64::from(portrait.width) / f64::from(frame.width))
        .min(f64::from(portrait.height) / f64::from(frame.height))
        .min(1.0);
    let width = f64::from(frame.width) * scale;
    let height = f64::from(frame.height) * scale;
    let _ = game
        .surface
        .context
        .draw_image_with_html_image_element_and_dw_and_dh(
            image,
            f64::from(window.x + portrait.x) + (f64::from(portrait.width) - width) / 2.0,
            f64::from(window.y + portrait.y + portrait.height) - height,
            width,
            height,
        );
}

fn draw_template_in_region(
    game: &Game,
    window: &GuiWindow,
    layout: &GuiLayout,
    template_name: &str,
    region_name: &str,
) {
    let Some(region) = region(layout, region_name) else {
        return;
    };
    draw_template(
        game,
        window,
        template(layout, template_name),
        region.x,
        region.y,
    );
}

fn draw_template(
    game: &Game,
    window: &GuiWindow,
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
            f64::from(window.x + x + template.offset_x),
            f64::from(window.y + y + template.offset_y),
            f64::from(template.width),
            f64::from(template.height),
        );
}

fn region<'a>(
    layout: &'a GuiLayout,
    name: &str,
) -> Option<&'a GuiRegion> {
    layout.regions.iter().find(|region| region.name == name)
}

fn template<'a>(
    layout: &'a GuiLayout,
    name: &str,
) -> Option<&'a GuiSpriteTemplate> {
    layout
        .sprite_templates
        .iter()
        .find(|template| template.name == name)
}

fn clean_wz_text(source: &str) -> String {
    ["#b", "#r", "#k", "#n"]
        .into_iter()
        .fold(source.to_owned(), |text, marker| text.replace(marker, ""))
}

fn wrap_text(
    game: &Game,
    source: &str,
    maximum_width: f64,
) -> Vec<String> {
    let mut output = Vec::new();
    for paragraph in source.lines() {
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            let candidate = if line.is_empty() {
                word.to_owned()
            } else {
                format!("{line} {word}")
            };
            let width = game
                .surface
                .context
                .measure_text(&candidate)
                .map_or(candidate.chars().count() as f64 * 6.0, |metrics| {
                    metrics.width()
                });
            if !line.is_empty() && width > maximum_width {
                output.push(std::mem::take(&mut line));
                line.push_str(word);
            } else {
                line = candidate;
            }
        }
        output.push(line);
    }
    output
}

#[cfg(test)]
mod tests {
    use oozems_proto::v1::GuiRegion;
    use oozems_proto::v1::GuiSpriteTemplate;

    use super::clean_wz_text;
    use super::shop_price_label;
    use super::shop_row_y;
    use super::shop_selection_x;

    #[test]
    fn dialogue_text_removes_wz_color_markers() {
        assert_eq!(
            clean_wz_text("Press #bI#k to open it."),
            "Press I to open it."
        );
    }

    #[test]
    fn shop_prices_use_the_authoritative_currency() {
        assert_eq!(shop_price_label(250, "mesos"), "250 mesos");
        assert_eq!(shop_price_label(250, "Ooze"), "250 Ooze");
    }

    #[test]
    fn shop_row_art_follows_the_authored_list_region() {
        let region = GuiRegion {
            x: 107.0,
            y: 221.0,
            width: 198.0,
            height: 185.0,
            ..GuiRegion::default()
        };
        let selection = GuiSpriteTemplate {
            width: 163.0,
            ..GuiSpriteTemplate::default()
        };

        assert_eq!(shop_selection_x(&region, &selection), 142.0);
        assert_eq!(shop_row_y(&region, 2), 297.0);
    }
}
