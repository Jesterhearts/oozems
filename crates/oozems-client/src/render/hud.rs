use oozems_proto::v1::CharacterStats;
use oozems_proto::v1::GuiRegion;
use oozems_proto::v1::GuiSprite;
use oozems_proto::v1::GuiWindow;
use oozems_proto::v1::ItemDefinition;
use web_sys::HtmlImageElement;

use crate::assets::ready_image;
use crate::game::Game;
use crate::game_gui;

const GAUGE_HEADER_HEIGHT: f64 = 15.0;
const GAUGE_FILL_TOP: f64 = 15.0;
const GAUGE_FILL_HEIGHT: f64 = 14.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ItemExpiration {
    Permanent,
    Expired,
    RemainingMinutes(u64),
}

pub(super) fn draw_cash_shop(game: &Game) {
    super::cash_shop::draw(game);
}

pub(super) fn draw(game: &Game) {
    if !draw_wz_hud(game) {
        draw_fallback_hud(game);
    }
    let active_buff_bottom = super::skill_info::draw_active_buffs(game);
    super::quest_tracker::draw(game, active_buff_bottom);
    if game.ui.gui_state.borrow().stats_open {
        draw_stat_window(game);
    }
    if game.ui.gui_state.borrow().equipment_open {
        draw_equipment_window(game);
    }
    if game.ui.gui_state.borrow().inventory_open {
        draw_inventory_window(game);
    }
    if game.ui.gui_state.borrow().skills_open {
        super::skillbook::draw(game);
    }
    if game.ui.gui_state.borrow().key_config_open {
        draw_key_config_window(game);
    }
    super::skill_info::draw_hovered_skill(game);
    super::interaction::draw(game);
}

fn draw_key_config_window(game: &Game) {
    let state = *game.ui.gui_state.borrow();
    let Some(placement) = game_gui::resolve_window(
        &game.ui.gui,
        state.window_placements,
        game_gui::WindowKind::KeyConfig,
        game.surface.canvas.width() as f32,
        game.surface.canvas.height() as f32,
    ) else {
        return;
    };
    if !draw_window_at(game, placement.window, placement.origin) {
        return;
    }
    for icon in game_gui::bound_key_icons(
        state,
        &game.ui.gui,
        &game.player.skill_book,
        &game.key_bindings(),
        game.surface.canvas.width() as f32,
        game.surface.canvas.height() as f32,
    ) {
        draw_key_icon(game, &icon);
    }
    if let Some(drag) = game.ui.key_drag.as_ref()
        && let Some(placement) =
            game_gui::dragged_key_icon(&game.ui.gui, &game.player.skill_book, drag)
    {
        draw_key_icon(game, &placement);
    }
}

fn draw_key_icon(
    game: &Game,
    placement: &game_gui::KeyIconPlacement,
) {
    let Some(image) = ready_image(&game.surface.images, &placement.asset_id) else {
        return;
    };
    let _ = game
        .surface
        .context
        .draw_image_with_html_image_element_and_dw_and_dh(
            image,
            f64::from(placement.x),
            f64::from(placement.y),
            f64::from(placement.width),
            f64::from(placement.height),
        );
}

fn draw_equipment_window(game: &Game) {
    let Some(placement) = game_gui::resolve_window(
        &game.ui.gui,
        game.ui.gui_state.borrow().window_placements,
        game_gui::WindowKind::Equipment,
        game.surface.canvas.width() as f32,
        game.surface.canvas.height() as f32,
    ) else {
        return;
    };
    if !draw_window_at(game, placement.window, placement.origin) {
        return;
    }
    let Some(inventory) = game.player.inventory.as_ref() else {
        return;
    };
    let now_unix_ms = js_sys::Date::now().max(0.0) as u64;
    for equipped in &inventory.equipment {
        let Some((x, y)) = game_gui::equipment_slot_position(equipped.slot) else {
            continue;
        };
        if let Some(definition) = item_definition(game, equipped.item_id) {
            draw_item_icon(
                game,
                definition,
                placement.origin.x + x,
                placement.origin.y + y,
            );
            draw_item_expiration(
                game,
                item_expiration(equipped.expires_at_unix_ms, now_unix_ms),
                false,
                placement.origin.x + x,
                placement.origin.y + y,
            );
        }
    }
}

fn draw_inventory_locked_slots(
    game: &Game,
    window: &GuiWindow,
    origin: game_gui::CanvasPoint,
    capacity: usize,
) {
    let Some(layout) = window.layout.as_ref() else {
        return;
    };
    let Some(template) = game_gui::named_sprite_template(layout, "inventory-locked-slot") else {
        return;
    };
    let Some(image) = ready_image(&game.surface.images, &template.asset_id) else {
        return;
    };
    let visible_capacity = capacity.min(game_gui::INVENTORY_VISIBLE_SLOTS);
    for index in visible_capacity..game_gui::INVENTORY_VISIBLE_SLOTS {
        let (x, y) = game_gui::inventory_slot_position(index);
        let _ = game
            .surface
            .context
            .draw_image_with_html_image_element_and_dw_and_dh(
                image,
                f64::from(origin.x + x + template.offset_x),
                f64::from(origin.y + y + template.offset_y),
                f64::from(template.width),
                f64::from(template.height),
            );
    }
}

fn draw_inventory_mesos(
    game: &Game,
    window: &GuiWindow,
    origin: game_gui::CanvasPoint,
) {
    let Some(layout) = window.layout.as_ref() else {
        return;
    };
    let Some(region) = game_gui::named_region(layout, "inventory-mesos") else {
        return;
    };
    let (right, center_y, maximum_width) = inventory_mesos_text_geometry(region);
    game.surface.context.set_fill_style_str("#30383b");
    game.surface.context.set_font("10px Arial");
    game.surface.context.set_text_align("right");
    game.surface.context.set_text_baseline("middle");
    let _ = game.surface.context.fill_text_with_max_width(
        &format_number(game.player.mesos),
        f64::from(origin.x + right),
        f64::from(origin.y + center_y),
        f64::from(maximum_width),
    );
    game.surface.context.set_text_baseline("alphabetic");
    game.surface.context.set_text_align("left");
}

fn inventory_mesos_text_geometry(region: &GuiRegion) -> (f32, f32, f32) {
    (
        region.x + region.width,
        region.y + region.height / 2.0,
        region.width,
    )
}

fn draw_inventory_window(game: &Game) {
    let Some(placement) = game_gui::resolve_window(
        &game.ui.gui,
        game.ui.gui_state.borrow().window_placements,
        game_gui::WindowKind::Inventory,
        game.surface.canvas.width() as f32,
        game.surface.canvas.height() as f32,
    ) else {
        return;
    };
    if !draw_window_at(game, placement.window, placement.origin) {
        return;
    }
    let selected_tab = game.ui.gui_state.borrow().inventory_tab;
    draw_inventory_tabs(game, placement.window, placement.origin, selected_tab);
    let Some(inventory) = game.player.inventory.as_ref() else {
        return;
    };
    draw_inventory_locked_slots(
        game,
        placement.window,
        placement.origin,
        inventory.capacity as usize,
    );
    draw_inventory_mesos(game, placement.window, placement.origin);
    let now_unix_ms = js_sys::Date::now().max(0.0) as u64;
    for slot in game_gui::inventory_slots(&game.ui.gui, inventory, selected_tab) {
        let (x, y) = game_gui::inventory_slot_position(slot.visual_index);
        draw_item_icon(
            game,
            slot.definition,
            placement.origin.x + x,
            placement.origin.y + y,
        );
        draw_item_quantity(
            game,
            slot.stack.quantity,
            placement.origin.x + x,
            placement.origin.y + y,
        );
        draw_item_expiration(
            game,
            item_expiration(slot.stack.expires_at_unix_ms, now_unix_ms),
            permanent_stack_needs_label(&inventory.stacks, slot.stack),
            placement.origin.x + x,
            placement.origin.y + y,
        );
    }
}

fn draw_inventory_tabs(
    game: &Game,
    window: &GuiWindow,
    origin: game_gui::CanvasPoint,
    selected: game_gui::InventoryTab,
) {
    let Some(layout) = window.layout.as_ref() else {
        return;
    };
    for tab in game_gui::InventoryTab::ALL {
        draw_inventory_tab_part(game, origin, layout, tab, tab == selected, "background");
    }
    for tab in game_gui::InventoryTab::ALL {
        draw_inventory_tab_part(game, origin, layout, tab, tab == selected, "label");
    }
}

fn draw_inventory_tab_part(
    game: &Game,
    origin: game_gui::CanvasPoint,
    layout: &oozems_proto::v1::GuiLayout,
    tab: game_gui::InventoryTab,
    active: bool,
    part: &str,
) {
    let state = if active { "active" } else { "inactive" };
    let Some(region) = game_gui::named_region(layout, &format!("inventory-tab-{}", tab.key()))
    else {
        return;
    };
    let Some(template) = game_gui::named_sprite_template(
        layout,
        &format!("inventory-tab-{}-{state}-{part}", tab.key()),
    ) else {
        return;
    };
    let Some(image) = ready_image(&game.surface.images, &template.asset_id) else {
        return;
    };
    let (x, y) = game_gui::inventory_tab_template_position(region, template, part == "background");
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

pub(super) fn draw_window(
    game: &Game,
    window: &GuiWindow,
) -> bool {
    draw_window_at(
        game,
        window,
        game_gui::CanvasPoint {
            x: window.x,
            y: window.y,
        },
    )
}

pub(super) fn draw_window_at(
    game: &Game,
    window: &GuiWindow,
    origin: game_gui::CanvasPoint,
) -> bool {
    let Some(layout) = window
        .layout
        .as_ref()
        .filter(|layout| game_gui::valid_layout(layout))
    else {
        return false;
    };
    let Some(background) = layout.background.as_ref() else {
        return false;
    };
    let Some(background_image) = ready_image(&game.surface.images, &background.asset_id) else {
        return false;
    };
    let origin_x = f64::from(origin.x);
    let origin_y = f64::from(origin.y);
    let _ = game
        .surface
        .context
        .draw_image_with_html_image_element_and_dw_and_dh(
            background_image,
            origin_x + f64::from(background.x),
            origin_y + f64::from(background.y),
            f64::from(background.width),
            f64::from(background.height),
        );
    for sprite in &layout.sprites {
        draw_window_sprite(game, sprite, origin_x, origin_y);
    }
    true
}

pub(super) fn draw_item_icon(
    game: &Game,
    definition: &ItemDefinition,
    slot_x: f32,
    slot_y: f32,
) {
    let Some(image) = ready_image(&game.surface.images, &definition.icon_asset_id) else {
        return;
    };
    let x = f64::from(slot_x + (32.0 - definition.icon_width) / 2.0);
    let y = f64::from(slot_y + (32.0 - definition.icon_height) / 2.0);
    let _ = game
        .surface
        .context
        .draw_image_with_html_image_element_and_dw_and_dh(
            image,
            x,
            y,
            f64::from(definition.icon_width),
            f64::from(definition.icon_height),
        );
}

pub(super) fn draw_item_quantity(
    game: &Game,
    quantity: u32,
    slot_x: f32,
    slot_y: f32,
) {
    if quantity <= 1 {
        return;
    }
    let label = quantity.to_string();
    game.surface.context.set_font("bold 10px Arial");
    let width = game
        .surface
        .context
        .measure_text(&label)
        .map_or(0.0, |metrics| metrics.width());
    game.surface.context.set_fill_style_str("#202020");
    let _ = game.surface.context.fill_text(
        &label,
        f64::from(slot_x + 31.0) - width,
        f64::from(slot_y + 31.0),
    );
    game.surface.context.set_fill_style_str("#ffffff");
    let _ = game.surface.context.fill_text(
        &label,
        f64::from(slot_x + 30.0) - width,
        f64::from(slot_y + 30.0),
    );
}

pub(super) fn item_expiration(
    expires_at_unix_ms: u64,
    now_unix_ms: u64,
) -> ItemExpiration {
    if expires_at_unix_ms == 0 {
        ItemExpiration::Permanent
    } else if expires_at_unix_ms <= now_unix_ms {
        ItemExpiration::Expired
    } else {
        ItemExpiration::RemainingMinutes(
            expires_at_unix_ms
                .saturating_sub(now_unix_ms)
                .div_ceil(60_000),
        )
    }
}

pub(super) fn permanent_stack_needs_label(
    stacks: &[oozems_proto::v1::InventoryItemStack],
    stack: &oozems_proto::v1::InventoryItemStack,
) -> bool {
    stack.expires_at_unix_ms == 0
        && stacks
            .iter()
            .any(|other| other.item_id == stack.item_id && other.expires_at_unix_ms != 0)
}

pub(super) fn inventory_expiration_label(
    expiration: ItemExpiration,
    show_permanent: bool,
) -> Option<String> {
    match expiration {
        ItemExpiration::Permanent => show_permanent.then(|| "PERM".to_owned()),
        ItemExpiration::Expired => Some("EXP".to_owned()),
        ItemExpiration::RemainingMinutes(minutes) => Some(format!("{minutes}m")),
    }
}

pub(super) fn item_expiration_detail(
    expiration: ItemExpiration,
    show_permanent: bool,
) -> Option<String> {
    match expiration {
        ItemExpiration::Permanent => show_permanent.then(|| "permanent".to_owned()),
        ItemExpiration::Expired => Some("expired".to_owned()),
        ItemExpiration::RemainingMinutes(minutes) => {
            let unit = if minutes == 1 { "minute" } else { "minutes" };
            Some(format!("{minutes} {unit} left"))
        }
    }
}

fn draw_item_expiration(
    game: &Game,
    expiration: ItemExpiration,
    show_permanent: bool,
    slot_x: f32,
    slot_y: f32,
) {
    let Some(label) = inventory_expiration_label(expiration, show_permanent) else {
        return;
    };
    let (background, foreground) = match expiration {
        ItemExpiration::Permanent => ("rgba(45, 67, 92, 0.9)", "#f4f8ff"),
        ItemExpiration::Expired => ("rgba(143, 39, 39, 0.92)", "#fff4f1"),
        ItemExpiration::RemainingMinutes(_) => ("rgba(89, 67, 21, 0.92)", "#fff4bf"),
    };
    game.surface.context.set_font("bold 8px Arial");
    let width = game
        .surface
        .context
        .measure_text(&label)
        .map_or(28.0, |metrics| metrics.width() + 4.0)
        .min(31.0);
    game.surface.context.set_fill_style_str(background);
    game.surface.context.fill_rect(
        f64::from(slot_x + 1.0),
        f64::from(slot_y + 1.0),
        width,
        10.0,
    );
    game.surface.context.set_fill_style_str(foreground);
    let _ = game.surface.context.fill_text_with_max_width(
        &label,
        f64::from(slot_x + 3.0),
        f64::from(slot_y + 9.0),
        27.0,
    );
}

pub(super) fn item_definition(
    game: &Game,
    item_id: u32,
) -> Option<&ItemDefinition> {
    game.ui
        .gui
        .items
        .iter()
        .find(|definition| definition.item_id == item_id)
}

fn draw_wz_hud(game: &Game) -> bool {
    let Some(layout) = game
        .ui
        .gui
        .status_bar
        .as_ref()
        .filter(|layout| game_gui::valid_layout(layout))
    else {
        return false;
    };
    let Some(background) = layout.background.as_ref() else {
        return false;
    };
    let Some(background_image) = ready_image(&game.surface.images, &background.asset_id) else {
        return false;
    };
    let viewport_width = f64::from(game.surface.canvas.width());
    let viewport_height = f64::from(game.surface.canvas.height());
    let origin_y = f64::from(game_gui::status_bar_top(
        viewport_height as f32,
        layout.height,
    ));

    let _ = game
        .surface
        .context
        .draw_image_with_html_image_element_and_dw_and_dh(
            background_image,
            0.0,
            origin_y + f64::from(background.y),
            viewport_width,
            f64::from(background.height),
        );
    let gui_state = *game.ui.gui_state.borrow();
    for sprite in layout
        .sprites
        .iter()
        .filter(|sprite| game_gui::status_sprite_visible(gui_state, sprite))
    {
        draw_gui_sprite(
            game,
            sprite,
            viewport_width,
            f64::from(layout.width),
            origin_y,
        );
    }
    let gauge_origin = layout
        .sprites
        .iter()
        .find(|sprite| sprite.name == "gauge")
        .filter(|sprite| ready_image(&game.surface.images, &sprite.asset_id).is_some())
        .map(|sprite| {
            (
                f64::from(game_gui::sprite_screen_x(
                    viewport_width as f32,
                    layout.width,
                    sprite,
                )),
                origin_y + f64::from(sprite.y),
            )
        });
    draw_status_bar_text(game, origin_y, f64::from(background.y), gauge_origin);
    true
}

fn draw_gui_sprite(
    game: &Game,
    sprite: &GuiSprite,
    viewport_width: f64,
    layout_width: f64,
    origin_y: f64,
) {
    let Some(image) = ready_image(&game.surface.images, &sprite.asset_id) else {
        return;
    };
    let destination_x = f64::from(game_gui::sprite_screen_x(
        viewport_width as f32,
        layout_width as f32,
        sprite,
    ));
    let destination_y = origin_y + f64::from(sprite.y);
    if sprite.name == "gauge"
        && let Some(stats) = game.player.stats.as_ref()
        && draw_gauge_fill(game, image, sprite, stats, destination_x, destination_y)
    {
        return;
    }
    let _ = game
        .surface
        .context
        .draw_image_with_html_image_element_and_dw_and_dh(
            image,
            destination_x,
            destination_y,
            f64::from(sprite.width),
            f64::from(sprite.height),
        );
}

fn draw_gauge_fill(
    game: &Game,
    image: &HtmlImageElement,
    sprite: &GuiSprite,
    stats: &CharacterStats,
    destination_x: f64,
    destination_y: f64,
) -> bool {
    if f64::from(sprite.width) < 340.0 || f64::from(sprite.height) < 31.0 {
        return false;
    }
    let _ = game
        .surface
        .context
        .draw_image_with_html_image_element_and_sw_and_sh_and_dx_and_dy_and_dw_and_dh(
            image,
            0.0,
            0.0,
            f64::from(sprite.width),
            GAUGE_HEADER_HEIGHT,
            destination_x,
            destination_y,
            f64::from(sprite.width),
            GAUGE_HEADER_HEIGHT,
        );
    for fill in game_gui::gauge_fills(stats) {
        if fill.filled_width == 0.0 {
            continue;
        }
        let _ = game
            .surface
            .context
            .draw_image_with_html_image_element_and_sw_and_sh_and_dx_and_dy_and_dw_and_dh(
                image,
                fill.source_x,
                GAUGE_FILL_TOP,
                fill.filled_width,
                GAUGE_FILL_HEIGHT,
                destination_x + fill.source_x,
                destination_y + GAUGE_FILL_TOP,
                fill.filled_width,
                GAUGE_FILL_HEIGHT,
            );
    }
    true
}

fn draw_status_bar_text(
    game: &Game,
    origin_y: f64,
    bar_y: f64,
    gauge_origin: Option<(f64, f64)>,
) {
    let bar_top = origin_y + bar_y;
    game.surface.context.set_fill_style_str("#e9eef2");
    game.surface.context.set_font("bold 12px Arial");
    let _ = game
        .surface
        .context
        .fill_text(&game.player.level.to_string(), 44.0, bar_top + 61.0);

    game.surface.context.set_font("bold 10px Arial");
    let job = game
        .player
        .stats
        .as_ref()
        .map_or("Beginner", |stats| job_name(stats.job_id));
    let _ = game
        .surface
        .context
        .fill_text_with_max_width(job, 84.0, bar_top + 50.0, 118.0);
    game.surface.context.set_font("11px Arial");
    let _ = game.surface.context.fill_text_with_max_width(
        &game.player.name,
        84.0,
        bar_top + 65.0,
        118.0,
    );

    game.surface.context.set_fill_style_str("#263139");
    game.surface.context.set_font("12px Arial");
    let _ = game.surface.context.fill_text_with_max_width(
        &game.world.map.name,
        10.0,
        bar_top + 22.0,
        550.0,
    );

    if let (Some(stats), Some((gauge_x, gauge_y))) = (game.player.stats.as_ref(), gauge_origin) {
        draw_gauge_text(game, stats, gauge_x, gauge_y);
    }
}

fn draw_gauge_text(
    game: &Game,
    stats: &CharacterStats,
    gauge_x: f64,
    gauge_y: f64,
) {
    let fills = game_gui::gauge_fills(stats);
    let labels = game_gui::gauge_labels(stats);
    game.surface.context.set_font("bold 10px Arial");
    game.surface.context.set_text_align("center");
    for (fill, label) in fills.into_iter().zip(labels) {
        let center_x = gauge_x + fill.source_x + fill.full_width / 2.0;
        game.surface.context.set_fill_style_str("#202020");
        let _ = game.surface.context.fill_text_with_max_width(
            &label,
            center_x + 1.0,
            gauge_y + 28.0,
            fill.full_width - 4.0,
        );
        game.surface.context.set_fill_style_str("#ffffff");
        let _ = game.surface.context.fill_text_with_max_width(
            &label,
            center_x,
            gauge_y + 27.0,
            fill.full_width - 4.0,
        );
    }
    game.surface.context.set_text_align("left");
}

fn draw_stat_window(game: &Game) {
    let Some(placement) = game_gui::resolve_window(
        &game.ui.gui,
        game.ui.gui_state.borrow().window_placements,
        game_gui::WindowKind::Stats,
        game.surface.canvas.width() as f32,
        game.surface.canvas.height() as f32,
    ) else {
        return;
    };
    let layout = placement.layout;
    let Some(background) = layout.background.as_ref() else {
        return;
    };
    let Some(background_image) = ready_image(&game.surface.images, &background.asset_id) else {
        return;
    };
    let origin_x = f64::from(placement.origin.x);
    let origin_y = f64::from(placement.origin.y);
    let _ = game
        .surface
        .context
        .draw_image_with_html_image_element_and_dw_and_dh(
            background_image,
            origin_x + f64::from(background.x),
            origin_y + f64::from(background.y),
            f64::from(background.width),
            f64::from(background.height),
        );
    let can_allocate_ability = game_gui::can_allocate_ability(game.player.stats.as_ref());
    for sprite in &layout.sprites {
        draw_window_sprite(game, sprite, origin_x, origin_y);
    }
    draw_stat_ability_buttons(game, layout, origin_x, origin_y, can_allocate_ability);
    if let Some(stats) = game.player.stats.as_ref() {
        draw_stat_values(game, stats, origin_x, origin_y);
    }
}

fn draw_stat_ability_buttons(
    game: &Game,
    layout: &oozems_proto::v1::GuiLayout,
    origin_x: f64,
    origin_y: f64,
    enabled: bool,
) {
    let template_name = if enabled {
        "stat-ability-up"
    } else {
        "stat-ability-up-disabled"
    };
    let Some(template) = game_gui::named_sprite_template(layout, template_name) else {
        return;
    };
    let Some(image) = ready_image(&game.surface.images, &template.asset_id) else {
        return;
    };
    for name in [
        "stat-strength-up",
        "stat-dexterity-up",
        "stat-intelligence-up",
        "stat-luck-up",
    ] {
        let Some(region) = game_gui::named_region(layout, name) else {
            continue;
        };
        let _ = game
            .surface
            .context
            .draw_image_with_html_image_element_and_dw_and_dh(
                image,
                origin_x + f64::from(region.x + template.offset_x),
                origin_y + f64::from(region.y + template.offset_y),
                f64::from(template.width),
                f64::from(template.height),
            );
    }
}

fn draw_window_sprite(
    game: &Game,
    sprite: &GuiSprite,
    origin_x: f64,
    origin_y: f64,
) {
    let Some(image) = ready_image(&game.surface.images, &sprite.asset_id) else {
        return;
    };
    let _ = game
        .surface
        .context
        .draw_image_with_html_image_element_and_dw_and_dh(
            image,
            origin_x + f64::from(sprite.x),
            origin_y + f64::from(sprite.y),
            f64::from(sprite.width),
            f64::from(sprite.height),
        );
}

fn draw_stat_values(
    game: &Game,
    stats: &CharacterStats,
    origin_x: f64,
    origin_y: f64,
) {
    let values = [
        (game.player.name.clone(), 45.0),
        (game.player.level.to_string(), 89.0),
        ("-".to_owned(), 106.0),
        (format!("{} / {}", stats.hp, stats.max_hp), 124.0),
        (format!("{} / {}", stats.mp, stats.max_mp), 142.0),
        (
            experience_label(stats.experience, stats.experience_required),
            160.0,
        ),
        (stats.fame.to_string(), 178.0),
    ];
    game.surface.context.set_fill_style_str("#30383b");
    game.surface.context.set_font("10px Arial");
    for (value, y) in values {
        let _ = game.surface.context.fill_text_with_max_width(
            &value,
            origin_x + 60.0,
            origin_y + y,
            106.0,
        );
    }

    let _ = game.surface.context.fill_text_with_max_width(
        &stats.ability_points.to_string(),
        origin_x + 64.0,
        origin_y + 226.0,
        22.0,
    );
    for (value, y) in [
        (stats.strength, 256.0),
        (stats.dexterity, 273.0),
        (stats.intelligence, 290.0),
        (stats.luck, 307.0),
    ] {
        let _ = game.surface.context.fill_text_with_max_width(
            &value.to_string(),
            origin_x + 60.0,
            origin_y + y,
            106.0,
        );
    }
}

fn job_name(job_id: u32) -> &'static str {
    match job_id {
        0 => "Beginner",
        100 => "Warrior",
        110 => "Fighter",
        111 => "Crusader",
        112 => "Hero",
        120 => "Page",
        121 => "White Knight",
        122 => "Paladin",
        130 => "Spearman",
        131 => "Dragon Knight",
        132 => "Dark Knight",
        200 => "Magician",
        210 => "Wizard (F/P)",
        211 => "Mage (F/P)",
        212 => "Arch Mage (F/P)",
        220 => "Wizard (I/L)",
        221 => "Mage (I/L)",
        222 => "Arch Mage (I/L)",
        230 => "Cleric",
        231 => "Priest",
        232 => "Bishop",
        300 => "Bowman",
        310 => "Hunter",
        311 => "Ranger",
        312 => "Bowmaster",
        320 => "Crossbowman",
        321 => "Sniper",
        322 => "Marksman",
        400 => "Thief",
        410 => "Assassin",
        411 => "Hermit",
        412 => "Night Lord",
        420 => "Bandit",
        421 => "Chief Bandit",
        422 => "Shadower",
        500 => "Pirate",
        510 => "Brawler",
        511 => "Marauder",
        512 => "Buccaneer",
        520 => "Gunslinger",
        521 => "Outlaw",
        522 => "Corsair",
        900 => "GM",
        910 => "SuperGM",
        1000 => "Noblesse",
        1100 => "Dawn Warrior",
        1110 => "Dawn Warrior II",
        1111 => "Dawn Warrior III",
        1112 => "Dawn Warrior IV",
        1200 => "Blaze Wizard",
        1210 => "Blaze Wizard II",
        1211 => "Blaze Wizard III",
        1212 => "Blaze Wizard IV",
        1300 => "Wind Archer",
        1310 => "Wind Archer II",
        1311 => "Wind Archer III",
        1312 => "Wind Archer IV",
        1400 => "Night Walker",
        1410 => "Night Walker II",
        1411 => "Night Walker III",
        1412 => "Night Walker IV",
        1500 => "Thunder Breaker",
        1510 => "Thunder Breaker II",
        1511 => "Thunder Breaker III",
        1512 => "Thunder Breaker IV",
        2000 => "Legend",
        2100 => "Aran",
        2110 => "Aran II",
        2111 => "Aran III",
        2112 => "Aran IV",
        _ => "Unknown",
    }
}

fn experience_label(
    experience: u64,
    required: u64,
) -> String {
    let percentage = if required == 0 {
        0.0
    } else {
        experience.min(required) as f64 * 100.0 / required as f64
    };
    format!("{} ({percentage:.2}%)", format_number(experience))
}

fn format_number(value: u64) -> String {
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

fn draw_fallback_hud(game: &Game) {
    game.surface
        .context
        .set_fill_style_str("rgba(28, 45, 44, 0.82)");
    game.surface.context.fill_rect(16.0, 16.0, 260.0, 66.0);
    game.surface.context.set_fill_style_str("#fff8d8");
    game.surface.context.set_font("bold 18px monospace");
    let _ = game.surface.context.fill_text(
        &format!("{}  Lv.{}", game.player.name, game.player.level),
        28.0,
        43.0,
    );
    game.surface.context.set_font("14px monospace");
    let _ = game
        .surface
        .context
        .fill_text(&game.world.map.name, 28.0, 66.0);
}

#[cfg(test)]
mod tests {
    use oozems_proto::v1::GuiRegion;

    use super::experience_label;
    use super::format_number;
    use super::inventory_mesos_text_geometry;

    #[test]
    fn stat_and_inventory_numbers_use_classic_compact_labels() {
        assert_eq!(format_number(1_234_567), "1,234,567");
        assert_eq!(experience_label(14, 135), "14 (10.37%)");
        assert_eq!(experience_label(10, 0), "10 (0.00%)");
    }

    #[test]
    fn inventory_mesos_geometry_uses_the_authored_region() {
        let region = GuiRegion {
            x: 26.0,
            y: 274.0,
            width: 111.0,
            height: 14.0,
            ..GuiRegion::default()
        };

        assert_eq!(
            inventory_mesos_text_geometry(&region),
            (137.0, 281.0, 111.0)
        );
    }
}
