use oozems_proto::v1::GuiLayout;
use oozems_proto::v1::GuiRegion;
use oozems_proto::v1::GuiSpriteTemplate;
use oozems_proto::v1::GuiWindow;
use oozems_proto::v1::NpcDialogChoiceKind;
use oozems_proto::v1::NpcDialogView;
use oozems_proto::v1::NpcShopView;
use oozems_proto::v1::NpcTaxiView;
use oozems_proto::v1::npc_interaction;

use crate::assets::ready_image;
use crate::game::Game;
use crate::interaction_ui::SHOP_PAGE_SIZE;
use crate::interaction_ui::visual_dialog_pages;

const DIALOG_LINE_HEIGHT: f64 = 17.0;
const CHOICE_ROW_HEIGHT: f64 = 24.0;
const SHOP_ROW_HEIGHT: f32 = 37.0;

pub(super) fn draw(game: &Game) {
    let Some(interaction) = game.interaction.interaction.as_ref() else {
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
    let Some(window) = game.gui.npc_dialog_window.as_ref() else {
        return;
    };
    if !super::draw_window(game, window) {
        return;
    }
    draw_npc_portrait(game, interaction, window);
    draw_dialog_heading(game, interaction, &dialog.title, window);
    let pages = visual_dialog_pages(dialog);
    let page = pages
        .get(game.interaction.page)
        .map(String::as_str)
        .unwrap_or_default();
    draw_wrapped_region(game, window, "npc-text", page);
    let Some(layout) = window.layout.as_ref() else {
        return;
    };
    if game.interaction.page > 0 {
        draw_template_in_region(game, window, layout, "npc-dialog-previous", "npc-previous");
    }
    if game.interaction.page + 1 < pages.len() {
        draw_template_in_region(game, window, layout, "npc-dialog-next", "npc-next");
        return;
    }
    let has_accept = dialog.choices.iter().any(|choice| {
        NpcDialogChoiceKind::try_from(choice.kind).ok() == Some(NpcDialogChoiceKind::AcceptQuest)
    });
    if has_accept {
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
    let Some(window) = game.gui.npc_dialog_window.as_ref() else {
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
        game.context.set_fill_style_str("#2f3437");
        game.context.set_font("bold 12px Arial");
        let label = format!("{}  ({} mesos)", destination.label, destination.fare);
        let _ = game.context.fill_text_with_max_width(
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
    let Some(window) = game.gui.shop_window.as_ref() else {
        return;
    };
    if !super::draw_window(game, window) {
        return;
    }
    let Some(layout) = window.layout.as_ref() else {
        return;
    };
    draw_shop_portrait(game, interaction, window);
    if let Some(index) = game
        .interaction
        .selected_offer
        .filter(|index| *index < SHOP_PAGE_SIZE)
    {
        draw_template(
            game,
            window,
            template(layout, "shop-selection"),
            42.0,
            123.0 + index as f32 * SHOP_ROW_HEIGHT,
        );
    }
    if let Some(index) = game
        .interaction
        .selected_inventory
        .and_then(|index| index.checked_sub(game.interaction.inventory_page * SHOP_PAGE_SIZE))
        .filter(|index| *index < SHOP_PAGE_SIZE)
    {
        draw_template(
            game,
            window,
            template(layout, "shop-selection"),
            273.0,
            123.0 + index as f32 * SHOP_ROW_HEIGHT,
        );
    }
    for (index, offer) in shop.offers.iter().take(SHOP_PAGE_SIZE).enumerate() {
        if let Some(definition) = super::item_definition(game, offer.item_id) {
            let y = window.y + 123.0 + index as f32 * SHOP_ROW_HEIGHT;
            super::draw_item_icon(game, definition, window.x + 8.0, y);
            draw_shop_item_text(
                game,
                window.x + 44.0,
                y,
                &definition.name,
                &format!("{} mesos", offer.buy_price),
            );
        }
    }
    if let Some(inventory) = game.player.inventory.as_ref() {
        let start = game.interaction.inventory_page * SHOP_PAGE_SIZE;
        for (index, item_id) in inventory
            .item_ids
            .iter()
            .skip(start)
            .take(SHOP_PAGE_SIZE)
            .enumerate()
        {
            if let Some(definition) = super::item_definition(game, *item_id) {
                let y = window.y + 123.0 + index as f32 * SHOP_ROW_HEIGHT;
                super::draw_item_icon(game, definition, window.x + 238.0, y);
                let price = if definition.sale_price == 0 {
                    "Cannot sell".to_owned()
                } else {
                    format!("{} mesos", definition.sale_price)
                };
                draw_shop_item_text(game, window.x + 274.0, y, &definition.name, &price);
            }
        }
        draw_inventory_page_controls(game, window, layout, inventory.item_ids.len());
    }
    draw_template_in_region(game, window, layout, "shop-buy", "shop-buy");
    draw_template_in_region(game, window, layout, "shop-sell", "shop-sell");
    draw_template_in_region(game, window, layout, "shop-exit", "shop-close");
    draw_template_in_region(game, window, layout, "shop-meso", "shop-mesos");
    game.context.set_fill_style_str("#202020");
    game.context.set_font("bold 11px Arial");
    let _ = game.context.fill_text(
        &game.player.mesos.to_string(),
        f64::from(window.x + 291.0),
        f64::from(window.y + 107.0),
    );
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
    game.context.set_fill_style_str("#eef7ff");
    game.context.set_font("bold 13px Arial");
    if game.interaction.inventory_page > 0
        && let Some(region) = region(layout, "shop-inventory-previous")
    {
        let _ = game.context.fill_text(
            "<",
            f64::from(window.x + region.x + 4.0),
            f64::from(window.y + region.y + 13.0),
        );
    }
    if game.interaction.inventory_page + 1 < page_count
        && let Some(region) = region(layout, "shop-inventory-next")
    {
        let _ = game.context.fill_text(
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
    game.context.set_fill_style_str("#202020");
    game.context.set_font("10px Arial");
    let _ = game
        .context
        .fill_text_with_max_width(name, f64::from(x), f64::from(y + 13.0), 155.0);
    game.context.set_fill_style_str("#53606a");
    let _ = game
        .context
        .fill_text_with_max_width(price, f64::from(x), f64::from(y + 27.0), 155.0);
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
    for (index, choice) in dialog.choices.iter().enumerate() {
        draw_template(
            game,
            window,
            template(layout, "npc-dialog-choice-selected"),
            region.x,
            region.y + index as f32 * CHOICE_ROW_HEIGHT as f32 + 4.0,
        );
        game.context.set_fill_style_str("#2f3437");
        game.context.set_font("bold 12px Arial");
        let _ = game.context.fill_text_with_max_width(
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
    game.context.set_fill_style_str("#2f3437");
    game.context.set_font("bold 13px Arial");
    let _ = game.context.fill_text_with_max_width(
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
    game.context.set_fill_style_str("#2f3437");
    game.context.set_font("12px Arial");
    for (index, line) in wrap_text(game, &clean_wz_text(source), f64::from(region.width))
        .into_iter()
        .take((f64::from(region.height) / DIALOG_LINE_HEIGHT) as usize)
        .enumerate()
    {
        let _ = game.context.fill_text(
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
        .map
        .npcs
        .iter()
        .find(|npc| npc.spawn_id == interaction.npc_spawn_id)
    else {
        return;
    };
    draw_portrait_frame(game, window, npc.frames.first());
}

fn draw_shop_portrait(
    game: &Game,
    interaction: &oozems_proto::v1::NpcInteraction,
    window: &GuiWindow,
) {
    let frame = game
        .map
        .npcs
        .iter()
        .find(|npc| npc.spawn_id == interaction.npc_spawn_id)
        .and_then(|npc| npc.frames.first());
    let Some(frame) = frame else {
        return;
    };
    let Some(image) = ready_image(&game.images, &frame.asset_id) else {
        return;
    };
    let scale = (90.0_f64 / f64::from(frame.width))
        .min(90.0 / f64::from(frame.height))
        .min(1.0);
    let _ = game
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
    let Some(image) = ready_image(&game.images, &frame.asset_id) else {
        return;
    };
    let scale = (90.0_f64 / f64::from(frame.width))
        .min(145.0 / f64::from(frame.height))
        .min(1.0);
    let width = f64::from(frame.width) * scale;
    let height = f64::from(frame.height) * scale;
    let _ = game
        .context
        .draw_image_with_html_image_element_and_dw_and_dh(
            image,
            f64::from(window.x + 15.0) + (90.0 - width) / 2.0,
            f64::from(window.y + 20.0) + 145.0 - height,
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
    let Some(image) = ready_image(&game.images, &template.asset_id) else {
        return;
    };
    let _ = game
        .context
        .draw_image_with_html_image_element_and_dw_and_dh(
            image,
            f64::from(window.x + x),
            f64::from(window.y + y),
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
    use super::clean_wz_text;

    #[test]
    fn dialogue_text_removes_wz_color_markers() {
        assert_eq!(
            clean_wz_text("Press #bI#k to open it."),
            "Press I to open it."
        );
    }
}
