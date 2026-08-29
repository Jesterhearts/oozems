use oozems_proto::v1::GuiRegion;

const DEFAULT_WINDOW_WIDTH: f32 = 175.0;
const DEFAULT_SKILL_ROW_HEIGHT: f32 = 35.0;
const DEFAULT_SKILL_POINT_BUTTON_SIZE: f32 = 12.0;
const TEXT_HEIGHT: f32 = 14.0;
const TEXT_BOTTOM_PADDING: f32 = 3.0;

pub(crate) fn add_missing(
    name: &str,
    width: f32,
    height: f32,
    regions: &mut Vec<GuiRegion>,
) {
    let defaults = defaults(name, width, height, regions);
    for region in defaults {
        if !regions.iter().any(|existing| existing.name == region.name) {
            regions.push(region);
        }
    }
}

pub(crate) fn defaults(
    name: &str,
    width: f32,
    height: f32,
    regions: &[GuiRegion],
) -> Vec<GuiRegion> {
    match name {
        "status-bar" => status_bar(height),
        "stats" => stats(width),
        "equipment" => equipment(width),
        "inventory" => inventory(width),
        "skills" => skills(width, regions),
        "key-config" => key_config(width),
        "npc-dialog" => npc_dialog(regions),
        "shop" => shop(regions),
        "cash-shop" => cash_shop(),
        "death-notice" => death_notice(),
        _ => Vec::new(),
    }
}

fn status_bar(height: f32) -> Vec<GuiRegion> {
    let height = positive_or(height, 80.0);
    vec![
        text_region("status-level", 44.0, height - 10.0, 32.0),
        text_region("status-job", 84.0, height - 21.0, 118.0),
        text_region("status-name", 84.0, height - 6.0, 118.0),
        text_region("status-map-name", 10.0, height - 49.0, 550.0),
        region("status-hp-gauge", 211.0, height - 17.0, 105.0, 14.0),
        region("status-mp-gauge", 319.0, height - 17.0, 105.0, 14.0),
        region("status-exp-gauge", 432.0, height - 17.0, 115.0, 14.0),
    ]
}

fn stats(width: f32) -> Vec<GuiRegion> {
    let mut regions = vec![drag_region(width, DEFAULT_WINDOW_WIDTH)];
    regions.extend([
        text_region("stat-character-name", 60.0, 45.0, 106.0),
        text_region("stat-level", 60.0, 89.0, 106.0),
        text_region("stat-guild", 60.0, 106.0, 106.0),
        text_region("stat-hp", 60.0, 124.0, 106.0),
        text_region("stat-mp", 60.0, 142.0, 106.0),
        text_region("stat-experience", 60.0, 160.0, 106.0),
        text_region("stat-fame", 60.0, 178.0, 106.0),
        text_region("stat-ability-points", 64.0, 226.0, 22.0),
        text_region("stat-strength", 60.0, 256.0, 106.0),
        text_region("stat-dexterity", 60.0, 273.0, 106.0),
        text_region("stat-intelligence", 60.0, 290.0, 106.0),
        text_region("stat-luck", 60.0, 307.0, 106.0),
    ]);
    regions
}

fn equipment(width: f32) -> Vec<GuiRegion> {
    vec![
        drag_region(width, DEFAULT_WINDOW_WIDTH),
        region("equipment-slot-top", 71.0, 134.0, 32.0, 32.0),
        region("equipment-slot-bottom", 38.0, 167.0, 32.0, 32.0),
        region("equipment-slot-shoes", 71.0, 167.0, 32.0, 32.0),
        region("equipment-slot-weapon", 104.0, 134.0, 32.0, 32.0),
    ]
}

fn inventory(width: f32) -> Vec<GuiRegion> {
    vec![
        drag_region(width, DEFAULT_WINDOW_WIDTH),
        region("inventory-slots", 7.0, 50.0, 140.0, 212.0),
        region("inventory-mesos", 26.0, 274.0, 111.0, 14.0),
    ]
}

fn skills(
    width: f32,
    regions: &[GuiRegion],
) -> Vec<GuiRegion> {
    let list = named(regions, "skill-list")
        .cloned()
        .unwrap_or_else(|| region("skill-list", 17.0, 95.0, 141.0, 140.0));
    let row_height = DEFAULT_SKILL_ROW_HEIGHT.min(list.height).max(1.0);
    let icon_size = (row_height - 3.0).max(1.0);
    let point_button_size = DEFAULT_SKILL_POINT_BUTTON_SIZE
        .min(list.width)
        .min(row_height)
        .max(1.0);
    let text_x = list.x + row_height + 2.0;
    let text_width = (list.width - row_height - 2.0).max(1.0);
    vec![
        drag_region(width, DEFAULT_WINDOW_WIDTH),
        region("skill-row-icon", list.x, list.y, icon_size, icon_size),
        region("skill-row-action", list.x, list.y, row_height, row_height),
        region("skill-row-name", text_x, list.y, text_width, 15.0),
        region("skill-row-level", text_x, list.y + 15.0, text_width, 15.0),
        region(
            "skill-row-point-button",
            list.x + (list.width - point_button_size - 2.0).max(0.0),
            list.y + (row_height - point_button_size) / 2.0,
            point_button_size,
            point_button_size,
        ),
        region(
            "skill-empty-message",
            list.x + 7.0,
            (list.y + row_height - 21.0).max(0.0),
            (list.width - 14.0).max(1.0),
            15.0,
        ),
    ]
}

fn key_config(width: f32) -> Vec<GuiRegion> {
    let mut regions = vec![drag_region(width, 629.0)];
    for (code, x, y, width) in KEY_SLOTS {
        regions.push(region(
            &format!("key-config-slot-{code}"),
            *x,
            *y,
            *width,
            32.0,
        ));
    }
    regions
}

fn npc_dialog(regions: &[GuiRegion]) -> Vec<GuiRegion> {
    let choices = named(regions, "npc-choices")
        .cloned()
        .unwrap_or_else(|| region("npc-choices", 155.0, 133.0, 345.0, 100.0));
    let text = named(regions, "npc-text")
        .cloned()
        .unwrap_or_else(|| region("npc-text", 154.0, 42.0, 345.0, 170.0));
    let mut defaults = vec![region(
        "npc-text-with-choices",
        text.x,
        text.y,
        text.width,
        (choices.y - text.y - 6.0).max(1.0),
    )];
    for index in 0..4 {
        let row_y = choices.y + index as f32 * 24.0;
        defaults.extend([
            region(
                &format!("npc-choice-row-{index}"),
                choices.x,
                row_y,
                choices.width,
                24.0,
            ),
            region(
                &format!("npc-choice-marker-{index}"),
                choices.x,
                row_y + 4.0,
                6.0,
                7.0,
            ),
            region(
                &format!("npc-choice-label-{index}"),
                choices.x + 13.0,
                row_y + 2.0,
                (choices.width - 16.0).max(1.0),
                24.0,
            ),
        ]);
    }
    defaults
}

fn shop(regions: &[GuiRegion]) -> Vec<GuiRegion> {
    let stock = named(regions, "shop-stock")
        .cloned()
        .unwrap_or_else(|| region("shop-stock", 7.0, 121.0, 198.0, 185.0));
    let inventory = named(regions, "shop-inventory")
        .cloned()
        .unwrap_or_else(|| region("shop-inventory", 237.0, 121.0, 198.0, 185.0));
    let balance = named(regions, "shop-mesos")
        .cloned()
        .unwrap_or_else(|| region("shop-mesos", 272.0, 96.0, 160.0, 18.0));
    let mut defaults = vec![
        region("shop-portrait", 69.0, 20.0, 90.0, 90.0),
        region("shop-mesos-icon", balance.x, balance.y, 12.0, 12.0),
        region(
            "shop-mesos-text",
            balance.x + 19.0,
            balance.y,
            (balance.width - 19.0).max(1.0),
            12.0,
        ),
        region(
            "shop-currency-text",
            balance.x + 5.0,
            balance.y,
            (balance.width - 10.0).max(1.0),
            12.0,
        ),
    ];
    for (prefix, list) in [("stock", stock), ("inventory", inventory)] {
        for index in 0..5 {
            let hit_y = list.y + index as f32 * 37.0;
            let content_y = hit_y + 2.0;
            defaults.extend([
                region(
                    &format!("shop-{prefix}-row-{index}"),
                    list.x,
                    hit_y,
                    list.width,
                    37.0,
                ),
                region(
                    &format!("shop-{prefix}-selection-{index}"),
                    list.x + (list.width - 163.0).max(0.0),
                    content_y,
                    list.width.min(163.0),
                    35.0,
                ),
                region(
                    &format!("shop-{prefix}-item-icon-{index}"),
                    list.x + 1.0,
                    content_y,
                    32.0,
                    32.0,
                ),
                region(
                    &format!("shop-{prefix}-item-name-{index}"),
                    list.x + 37.0,
                    content_y,
                    (list.width - 43.0).clamp(1.0, 155.0),
                    14.0,
                ),
                region(
                    &format!("shop-{prefix}-item-detail-{index}"),
                    list.x + 37.0,
                    content_y + 14.0,
                    (list.width - 43.0).clamp(1.0, 155.0),
                    14.0,
                ),
            ]);
        }
    }
    defaults
}

fn cash_shop() -> Vec<GuiRegion> {
    let mut regions = vec![
        region("cash-shop-character", 24.0, 40.0, 212.0, 133.0),
        region("cash-shop-player-name", 49.0, 213.0, 164.0, 15.0),
        region("cash-shop-balance", 49.0, 233.0, 164.0, 15.0),
        region("cash-shop-request-panel", 236.0, 270.0, 428.0, 48.0),
        region("cash-shop-request-text", 252.0, 286.0, 412.0, 16.0),
    ];
    for index in 0..10 {
        let card_x = if index % 2 == 0 { 278.0 } else { 484.0 };
        let card_y = 98.0 + (index / 2) as f32 * 81.0;
        regions.extend([
            region(
                &format!("cash-shop-item-icon-{index}"),
                card_x + 20.0,
                card_y + 24.0,
                32.0,
                32.0,
            ),
            region(
                &format!("cash-shop-item-name-{index}"),
                card_x + 78.0,
                card_y + 5.0,
                117.0,
                15.0,
            ),
            region(
                &format!("cash-shop-item-duration-{index}"),
                card_x + 78.0,
                card_y + 24.0,
                117.0,
                15.0,
            ),
            region(
                &format!("cash-shop-item-price-{index}"),
                card_x + 78.0,
                card_y + 39.0,
                117.0,
                15.0,
            ),
            region(
                &format!("cash-shop-gift-{index}"),
                card_x + 118.0,
                card_y + 57.0,
                37.0,
                19.0,
            ),
        ]);
    }
    regions
}

fn death_notice() -> Vec<GuiRegion> {
    vec![
        region("death-notice-title", 14.0, 40.0, 236.0, 21.0),
        region("death-notice-detail", 14.0, 61.0, 236.0, 24.0),
    ]
}

fn drag_region(
    width: f32,
    default_width: f32,
) -> GuiRegion {
    region(
        "window-drag-handle",
        0.0,
        0.0,
        positive_or(width, default_width),
        20.0,
    )
}

fn text_region(
    name: &str,
    x: f32,
    baseline: f32,
    width: f32,
) -> GuiRegion {
    region(
        name,
        x,
        baseline - TEXT_HEIGHT + TEXT_BOTTOM_PADDING,
        width,
        TEXT_HEIGHT,
    )
}

fn named<'a>(
    regions: &'a [GuiRegion],
    name: &str,
) -> Option<&'a GuiRegion> {
    regions.iter().find(|region| region.name == name)
}

fn positive_or(
    value: f32,
    default: f32,
) -> f32 {
    if value.is_finite() && value > 0.0 {
        value
    } else {
        default
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

const KEY_SLOTS: &[(&str, f32, f32, f32)] = &[
    ("Escape", 13.0, 28.0, 32.0),
    ("F1", 80.0, 28.0, 32.0),
    ("F2", 114.0, 28.0, 32.0),
    ("F3", 148.0, 28.0, 32.0),
    ("F4", 182.0, 28.0, 32.0),
    ("F5", 225.0, 28.0, 32.0),
    ("F6", 259.0, 28.0, 32.0),
    ("F7", 293.0, 28.0, 32.0),
    ("F8", 327.0, 28.0, 32.0),
    ("F9", 369.0, 28.0, 32.0),
    ("F10", 403.0, 28.0, 32.0),
    ("F11", 437.0, 28.0, 32.0),
    ("F12", 471.0, 28.0, 32.0),
    ("PrintScreen", 513.0, 28.0, 32.0),
    ("ScrollLock", 547.0, 28.0, 32.0),
    ("Pause", 581.0, 28.0, 32.0),
    ("Backquote", 13.0, 66.0, 32.0),
    ("Digit1", 47.0, 66.0, 32.0),
    ("Digit2", 81.0, 66.0, 32.0),
    ("Digit3", 115.0, 66.0, 32.0),
    ("Digit4", 149.0, 66.0, 32.0),
    ("Digit5", 183.0, 66.0, 32.0),
    ("Digit6", 217.0, 66.0, 32.0),
    ("Digit7", 251.0, 66.0, 32.0),
    ("Digit8", 285.0, 66.0, 32.0),
    ("Digit9", 319.0, 66.0, 32.0),
    ("Digit0", 353.0, 66.0, 32.0),
    ("Minus", 387.0, 66.0, 32.0),
    ("Equal", 421.0, 66.0, 32.0),
    ("Backspace", 455.0, 66.0, 48.0),
    ("Insert", 513.0, 66.0, 32.0),
    ("Home", 547.0, 66.0, 32.0),
    ("PageUp", 581.0, 66.0, 32.0),
    ("Tab", 13.0, 100.0, 49.0),
    ("KeyQ", 64.0, 100.0, 32.0),
    ("KeyW", 98.0, 100.0, 32.0),
    ("KeyE", 132.0, 100.0, 32.0),
    ("KeyR", 166.0, 100.0, 32.0),
    ("KeyT", 200.0, 100.0, 32.0),
    ("KeyY", 234.0, 100.0, 32.0),
    ("KeyU", 268.0, 100.0, 32.0),
    ("KeyI", 302.0, 100.0, 32.0),
    ("KeyO", 336.0, 100.0, 32.0),
    ("KeyP", 370.0, 100.0, 32.0),
    ("BracketLeft", 404.0, 100.0, 32.0),
    ("BracketRight", 438.0, 100.0, 32.0),
    ("Backslash", 472.0, 100.0, 32.0),
    ("Delete", 513.0, 100.0, 32.0),
    ("End", 547.0, 100.0, 32.0),
    ("PageDown", 581.0, 100.0, 32.0),
    ("CapsLock", 13.0, 133.0, 66.0),
    ("KeyA", 81.0, 133.0, 32.0),
    ("KeyS", 115.0, 133.0, 32.0),
    ("KeyD", 149.0, 133.0, 32.0),
    ("KeyF", 183.0, 133.0, 32.0),
    ("KeyG", 217.0, 133.0, 32.0),
    ("KeyH", 251.0, 133.0, 32.0),
    ("KeyJ", 285.0, 133.0, 32.0),
    ("KeyK", 319.0, 133.0, 32.0),
    ("KeyL", 353.0, 133.0, 32.0),
    ("Semicolon", 387.0, 133.0, 32.0),
    ("Quote", 421.0, 133.0, 32.0),
    ("Enter", 455.0, 133.0, 48.0),
    ("ShiftLeft", 13.0, 166.0, 82.0),
    ("KeyZ", 98.0, 166.0, 32.0),
    ("KeyX", 132.0, 166.0, 32.0),
    ("KeyC", 166.0, 166.0, 32.0),
    ("KeyV", 200.0, 166.0, 32.0),
    ("KeyB", 234.0, 166.0, 32.0),
    ("KeyN", 268.0, 166.0, 32.0),
    ("KeyM", 302.0, 166.0, 32.0),
    ("Comma", 336.0, 166.0, 32.0),
    ("Period", 370.0, 166.0, 32.0),
    ("Slash", 404.0, 166.0, 32.0),
    ("ShiftRight", 438.0, 166.0, 65.0),
    ("ControlLeft", 13.0, 199.0, 48.0),
    ("MetaLeft", 63.0, 199.0, 47.0),
    ("AltLeft", 112.0, 199.0, 51.0),
    ("Space", 165.0, 199.0, 167.0),
    ("AltRight", 334.0, 199.0, 53.0),
    ("ContextMenu", 390.0, 199.0, 55.0),
    ("ControlRight", 448.0, 199.0, 55.0),
];

#[cfg(test)]
mod tests {
    use super::add_missing;

    #[test]
    fn adding_defaults_preserves_authored_regions() {
        let mut regions = vec![super::region(
            "cash-shop-item-icon-0",
            10.0,
            20.0,
            30.0,
            40.0,
        )];

        add_missing("cash-shop", 800.0, 600.0, &mut regions);

        let icon = regions
            .iter()
            .find(|region| region.name == "cash-shop-item-icon-0")
            .expect("item icon region");
        assert_eq!(
            (icon.x, icon.y, icon.width, icon.height),
            (10.0, 20.0, 30.0, 40.0)
        );
        assert!(
            regions
                .iter()
                .any(|region| region.name == "cash-shop-item-icon-9")
        );
    }

    #[test]
    fn derived_defaults_follow_authored_parent_regions() {
        let mut shop = vec![
            super::region("shop-stock", 20.0, 30.0, 200.0, 190.0),
            super::region("shop-inventory", 240.0, 30.0, 200.0, 190.0),
            super::region("shop-mesos", 300.0, 10.0, 120.0, 17.0),
        ];
        add_missing("shop", 463.0, 339.0, &mut shop);
        let stock_row = super::named(&shop, "shop-stock-row-2").expect("stock row");
        let balance = super::named(&shop, "shop-mesos-text").expect("mesos text");
        assert_eq!((stock_row.x, stock_row.y), (20.0, 104.0));
        assert_eq!((balance.x, balance.y), (319.0, 10.0));

        let mut dialog = vec![
            super::region("npc-text", 100.0, 20.0, 300.0, 160.0),
            super::region("npc-choices", 110.0, 120.0, 280.0, 100.0),
        ];
        add_missing("npc-dialog", 529.0, 286.0, &mut dialog);
        let text = super::named(&dialog, "npc-text-with-choices").expect("choice text");
        let choice = super::named(&dialog, "npc-choice-row-3").expect("choice row");
        assert_eq!(
            (text.x, text.y, text.width, text.height),
            (100.0, 20.0, 300.0, 94.0)
        );
        assert_eq!((choice.x, choice.y), (110.0, 192.0));
    }
}
