use oozems_proto::v1::GuiRegion;
use oozems_proto::v1::GuiSpriteSource;
use oozems_proto::v1::GuiSpriteTemplateSource;
use oozems_proto::v1::GuiWindowDefinition;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpriteDimensions {
    pub width: f32,
    pub height: f32,
}

pub fn builtin_definition<E>(
    name: &str,
    mut dimensions: impl FnMut(&str) -> Result<SpriteDimensions, E>,
) -> Result<Option<GuiWindowDefinition>, E> {
    let definition = match name {
        "status-bar" => status_bar(&mut dimensions)?,
        "stats" => stats(&mut dimensions)?,
        "equipment" => equipment(&mut dimensions)?,
        "inventory" => inventory(&mut dimensions)?,
        "skills" => skills(&mut dimensions)?,
        "key-config" => key_config(&mut dimensions)?,
        "npc-dialog" => npc_dialog(&mut dimensions)?,
        "shop" => shop(&mut dimensions)?,
        "cash-shop" => cash_shop(&mut dimensions)?,
        _ => return Ok(None),
    };
    Ok(Some(definition))
}

fn status_bar<E>(
    dimensions: &mut impl FnMut(&str) -> Result<SpriteDimensions, E>
) -> Result<GuiWindowDefinition, E> {
    let background_path = "StatusBar.img/base/backgrnd";
    let quick_slots_path = "StatusBar.img/base/quickSlot";
    let background_size = dimensions(background_path)?;
    let quick_slots_size = dimensions(quick_slots_path)?;
    let width = background_size.width;
    let height = background_size.height.max(quick_slots_size.height);
    let bar_y = height - background_size.height;
    let quick_slots_x = width - quick_slots_size.width;
    let mut definition = window(
        "status-bar",
        0.0,
        0.0,
        width,
        height,
        source("background", background_path, 0.0, bar_y),
    );
    definition.sprites.extend([
        source("status-overlay", "StatusBar.img/base/backgrnd2", 0.0, bar_y),
        pinned_bottom("gauge", "StatusBar.img/gauge/bar", 209.0, 1.0),
        pinned_bottom(
            "gauge-graduation",
            "StatusBar.img/gauge/graduation",
            209.0,
            1.0,
        ),
        pinned_right("quick-slots", quick_slots_path, 0.0, 0.0, true),
        pinned_right_bottom(
            "cash-shop",
            "StatusBar.img/BtShop/normal/0",
            quick_slots_size.width + 2.0,
            0.0,
            true,
        ),
    ]);
    for index in 0..8 {
        let path = format!("StatusBar.img/key/{index}");
        let size = dimensions(&path)?;
        let column = (index % 4) as f32;
        let row = (index / 4) as f32;
        let x = (quick_slots_x + 22.0 + column * 35.0 - size.width / 2.0).floor();
        let y = (22.0 + row * 35.0 - size.height / 2.0).floor();
        let mut key = source(&format!("key-{index}"), &path, x, y);
        key.anchor_right = true;
        definition.sprites.push(key);
    }
    let mut menu_x = 574.0;
    for (name, path, pressed_name, pressed_path) in [
        (
            "equip",
            "StatusBar.img/EquipKey/normal/0",
            Some("equip-pressed"),
            Some("StatusBar.img/EquipKey/pressed/0"),
        ),
        (
            "inventory",
            "StatusBar.img/InvenKey/normal/0",
            Some("inventory-pressed"),
            Some("StatusBar.img/InvenKey/pressed/0"),
        ),
        (
            "stats",
            "StatusBar.img/StatKey/normal/0",
            Some("stats-pressed"),
            Some("StatusBar.img/StatKey/pressed/0"),
        ),
        (
            "skills",
            "StatusBar.img/SkillKey/normal/0",
            Some("skills-pressed"),
            Some("StatusBar.img/SkillKey/pressed/0"),
        ),
        (
            "key-settings",
            "StatusBar.img/KeySet/normal/0",
            Some("key-settings-pressed"),
            Some("StatusBar.img/KeySet/pressed/0"),
        ),
        (
            "quick-slot-toggle",
            "StatusBar.img/QuickSlotD/normal/0",
            None,
            None,
        ),
    ] {
        definition
            .sprites
            .push(source(name, path, menu_x, bar_y + 8.0));
        if let (Some(pressed_name), Some(pressed_path)) = (pressed_name, pressed_path) {
            definition
                .sprites
                .push(source(pressed_name, pressed_path, menu_x, bar_y + 8.0));
        }
        menu_x += dimensions(path)?.width + 2.0;
    }
    Ok(definition)
}

fn stats<E>(
    dimensions: &mut impl FnMut(&str) -> Result<SpriteDimensions, E>
) -> Result<GuiWindowDefinition, E> {
    let background_path = "UIWindow.img/Stat/backgrnd";
    let background_size = dimensions(background_path)?;
    let ability_path = "UIWindow.img/Stat/BtApUp/normal/0";
    let ability_size = dimensions(ability_path)?;
    let disabled_path = "UIWindow.img/Stat/BtApUp/disabled/0";
    let mut definition = window(
        "stats",
        20.0,
        80.0,
        background_size.width,
        background_size.height,
        source("stat-background", background_path, 0.0, 0.0),
    );
    definition.sprites.extend([
        pinned_right(
            "stat-close",
            "UIWindow.img/BtUIClose/normal/0",
            5.0,
            5.0,
            false,
        ),
        source("stat-job", "UIWindow.img/Stat/Job/main/0", 60.0, 57.0),
        source(
            "stat-auto-assign-disabled",
            "UIWindow.img/Stat/BtAuto/disabled/0",
            95.0,
            197.0,
        ),
        source(
            "stat-detail-disabled",
            "UIWindow.img/Stat/BtDetail/disabled/0",
            113.0,
            324.0,
        ),
        source("stat-hp-up-disabled", disabled_path, 153.0, 117.0),
        source("stat-mp-up-disabled", disabled_path, 153.0, 135.0),
    ]);
    definition.sprite_templates.extend([
        template("stat-ability-up", ability_path),
        template("stat-ability-up-disabled", disabled_path),
    ]);
    definition.regions.extend(
        [
            ("stat-strength-up", 246.0),
            ("stat-dexterity-up", 264.0),
            ("stat-intelligence-up", 282.0),
            ("stat-luck-up", 300.0),
        ]
        .map(|(name, y)| region(name, 153.0, y, ability_size.width, ability_size.height)),
    );
    Ok(definition)
}

fn equipment<E>(
    dimensions: &mut impl FnMut(&str) -> Result<SpriteDimensions, E>
) -> Result<GuiWindowDefinition, E> {
    let background_path = "UIWindow.img/Equip/backgrnd";
    let size = dimensions(background_path)?;
    let mut definition = window(
        "equipment",
        20.0,
        80.0,
        size.width,
        size.height,
        source("equipment-background", background_path, 0.0, 0.0),
    );
    definition.sprites.extend([
        pinned_right(
            "equipment-close",
            "UIWindow.img/BtUIClose/normal/0",
            5.0,
            5.0,
            false,
        ),
        source(
            "equipment-detail-disabled",
            "UIWindow.img/Equip/BtDetail/disabled/0",
            100.0,
            281.0,
        ),
    ]);
    Ok(definition)
}

fn inventory<E>(
    dimensions: &mut impl FnMut(&str) -> Result<SpriteDimensions, E>
) -> Result<GuiWindowDefinition, E> {
    let background_path = "UIWindow.img/Item/backgrnd";
    let size = dimensions(background_path)?;
    let gather_path = "UIWindow.img/Item/BtGather/disabled/0";
    let sort_path = "UIWindow.img/Item/BtSort/disabled/0";
    let gather_width = dimensions(gather_path)?.width;
    let sort_x = 96.0 + gather_width + 2.0;
    let expand_x = sort_x + dimensions(sort_path)?.width + 2.0;
    let mut definition = window(
        "inventory",
        205.0,
        80.0,
        size.width,
        size.height,
        source("inventory-background", background_path, 0.0, 0.0),
    );
    definition.sprites.extend([
        source("inventory-gather-disabled", gather_path, 96.0, 6.0),
        source("inventory-sort-disabled", sort_path, sort_x, 6.0),
        source(
            "inventory-expand-disabled",
            "UIWindow.img/Item/BtFull/disabled/0",
            expand_x,
            6.0,
        ),
        pinned_right(
            "inventory-close",
            "UIWindow.img/BtUIClose/normal/0",
            5.0,
            5.0,
            false,
        ),
    ]);
    for (index, name) in ["equipment", "consume", "install", "etc", "cash"]
        .into_iter()
        .enumerate()
    {
        definition.sprite_templates.extend([
            template(
                &format!("inventory-tab-{name}-active-background"),
                &format!("UIWindow.img/Item/New/Tab1/{index}"),
            ),
            template(
                &format!("inventory-tab-{name}-inactive-background"),
                &format!("UIWindow.img/Item/New/Tab0/{index}"),
            ),
            template(
                &format!("inventory-tab-{name}-active-label"),
                &format!("UIWindow.img/Item/Tab/enabled/{index}"),
            ),
            template(
                &format!("inventory-tab-{name}-inactive-label"),
                &format!("UIWindow.img/Item/Tab/disabled/{index}"),
            ),
        ]);
        definition.regions.push(region(
            &format!("inventory-tab-{name}"),
            3.0 + index as f32 * 34.0,
            22.0,
            34.0,
            19.0,
        ));
    }
    definition.sprite_templates.push(template(
        "inventory-locked-slot",
        "UIWindow.img/Item/disabled",
    ));
    Ok(definition)
}

fn skills<E>(
    dimensions: &mut impl FnMut(&str) -> Result<SpriteDimensions, E>
) -> Result<GuiWindowDefinition, E> {
    let background_path = "UIWindow.img/Skill/backgrnd";
    let size = dimensions(background_path)?;
    let row_size = dimensions("UIWindow.img/Skill/skill0")?;
    let mut definition = window(
        "skills",
        20.0,
        80.0,
        size.width,
        size.height,
        source("skill-background", background_path, 0.0, 0.0),
    );
    definition.sprites.push(pinned_right(
        "skill-close",
        "UIWindow.img/BtUIClose/normal/0",
        5.0,
        5.0,
        false,
    ));
    definition.sprite_templates.extend([
        template("skill-row", "UIWindow.img/Skill/skill0"),
        template("skill-row-selected", "UIWindow.img/Skill/skill1"),
        template("skill-point-up", "UIWindow.img/Skill/BtSpUp/normal/0"),
        template(
            "skill-point-up-hover",
            "UIWindow.img/Skill/BtSpUp/mouseOver/0",
        ),
        template(
            "skill-point-up-pressed",
            "UIWindow.img/Skill/BtSpUp/pressed/0",
        ),
        template(
            "skill-point-up-disabled",
            "UIWindow.img/Skill/BtSpUp/disabled/0",
        ),
    ]);
    for index in 0..5 {
        definition.sprite_templates.extend([
            template(
                &format!("skill-job-tab-{index}-enabled"),
                &format!("UIWindow.img/Skill/Tab/enabled/{index}"),
            ),
            template(
                &format!("skill-job-tab-{index}-disabled"),
                &format!("UIWindow.img/Skill/Tab/disabled/{index}"),
            ),
        ]);
    }
    definition.regions.extend([
        region("skill-title", 47.0, 61.0, 116.0, 15.0),
        region(
            "skill-list",
            17.0,
            94.0,
            row_size.width,
            row_size.height * 4.0,
        ),
        region("skill-points", 82.0, 265.0, 30.0, 14.0),
        region("skill-page-previous", 66.0, 237.0, 18.0, 18.0),
        region("skill-page-label", 84.0, 237.0, 49.0, 18.0),
        region("skill-page-next", 133.0, 237.0, 18.0, 18.0),
    ]);
    definition.regions.extend((0..5).map(|index| {
        region(
            &format!("skill-job-tab-{index}"),
            47.0 + index as f32 * 22.0,
            26.0,
            20.0,
            14.0,
        )
    }));
    Ok(definition)
}

fn key_config<E>(
    dimensions: &mut impl FnMut(&str) -> Result<SpriteDimensions, E>
) -> Result<GuiWindowDefinition, E> {
    let background_path = "UIWindow.img/KeyConfig/backgrnd";
    let size = dimensions(background_path)?;
    let mut definition = window(
        "key-config",
        165.0,
        60.0,
        size.width,
        size.height,
        source("key-config-background", background_path, 0.0, 0.0),
    );
    definition.sprites.push(pinned_right(
        "key-config-close",
        "UIWindow.img/KeyConfig/BtClose/normal/0",
        5.0,
        6.0,
        false,
    ));
    for (index, icon_id) in ["53", "50", "2", "0", "1", "9", "3", "52"]
        .into_iter()
        .enumerate()
    {
        definition.sprites.push(source(
            &format!("key-action-{icon_id}"),
            &format!("UIWindow.img/KeyConfig/icon/{icon_id}"),
            7.0 + index as f32 * 34.0,
            267.0,
        ));
    }
    Ok(definition)
}

fn npc_dialog<E>(
    dimensions: &mut impl FnMut(&str) -> Result<SpriteDimensions, E>
) -> Result<GuiWindowDefinition, E> {
    let top_path = "UIWindow.img/UtilDlgEx/t";
    let center_path = "UIWindow.img/UtilDlgEx/c";
    let bottom_path = "UIWindow.img/UtilDlgEx/s";
    let top = dimensions(top_path)?;
    let center = dimensions(center_path)?;
    let bottom = dimensions(bottom_path)?;
    let bottom_top = top.height + center.height * 10.0;
    let mut definition = window(
        "npc-dialog",
        135.0,
        100.0,
        top.width,
        bottom_top + bottom.height,
        source("npc-dialog-top", top_path, 0.0, 0.0),
    );
    definition.sprites.extend((0..10).map(|row| {
        source(
            &format!("npc-dialog-center-{row}"),
            center_path,
            0.0,
            top.height + center.height * row as f32,
        )
    }));
    definition
        .sprites
        .push(source("npc-dialog-bottom", bottom_path, 0.0, bottom_top));
    definition.sprite_templates.extend([
        template(
            "npc-dialog-close",
            "UIWindow.img/UtilDlgEx/BtClose/normal/0",
        ),
        template("npc-dialog-ok", "UIWindow.img/UtilDlgEx/BtOK/normal/0"),
        template("npc-dialog-next", "UIWindow.img/UtilDlgEx/BtNext/normal/0"),
        template(
            "npc-dialog-previous",
            "UIWindow.img/UtilDlgEx/BtPrev/normal/0",
        ),
        template(
            "npc-dialog-accept",
            "UIWindow.img/UtilDlgEx/BtQYes/normal/0",
        ),
        template(
            "npc-dialog-decline",
            "UIWindow.img/UtilDlgEx/BtQNo/normal/0",
        ),
        template("npc-dialog-choice", "UIWindow.img/UtilDlgEx/dot0"),
        template("npc-dialog-choice-selected", "UIWindow.img/UtilDlgEx/dot1"),
    ]);
    let footer_y = bottom_top + 27.0;
    definition.regions.extend([
        region("npc-portrait", 15.0, 20.0, 90.0, 145.0),
        region("npc-title", 154.0, 17.0, 345.0, 20.0),
        region("npc-text", 154.0, 42.0, 345.0, 170.0),
        region("npc-choices", 154.0, 108.0, 345.0, 100.0),
        region("npc-previous", 402.0, footer_y, 46.0, 20.0),
        region("npc-decision-previous", 329.0, footer_y, 46.0, 20.0),
        region("npc-next", 459.0, footer_y, 46.0, 20.0),
        region("npc-ok", 459.0, footer_y, 46.0, 20.0),
        region("npc-close", 420.0, footer_y, 85.0, 20.0),
        region("npc-accept", 383.0, footer_y, 60.0, 20.0),
        region("npc-decline", 448.0, footer_y, 60.0, 20.0),
    ]);
    Ok(definition)
}

fn shop<E>(
    dimensions: &mut impl FnMut(&str) -> Result<SpriteDimensions, E>
) -> Result<GuiWindowDefinition, E> {
    let background_path = "UIWindow.img/Shop/backgrnd";
    let size = dimensions(background_path)?;
    let mut definition = window(
        "shop",
        168.0,
        80.0,
        size.width,
        size.height,
        source("shop-background", background_path, 0.0, 0.0),
    );
    definition.sprite_templates.extend([
        template("shop-selection", "UIWindow.img/Shop/select"),
        template("shop-meso", "UIWindow.img/Shop/meso"),
        template("shop-buy", "UIWindow.img/Shop/BtBuy/normal/0"),
        template("shop-sell", "UIWindow.img/Shop/BtSell/normal/0"),
        template("shop-exit", "UIWindow.img/Shop/BtExit/normal/0"),
    ]);
    definition.regions.extend([
        region("shop-stock", 7.0, 121.0, 198.0, 185.0),
        region("shop-inventory", 237.0, 121.0, 198.0, 185.0),
        region("shop-inventory-previous", 391.0, 98.0, 18.0, 18.0),
        region("shop-inventory-next", 415.0, 98.0, 18.0, 18.0),
        region("shop-buy", 125.0, 310.0, 80.0, 18.0),
        region("shop-sell", 355.0, 310.0, 80.0, 18.0),
        region("shop-close", 9.0, 310.0, 80.0, 18.0),
        region("shop-mesos", 272.0, 96.0, 160.0, 18.0),
    ]);
    Ok(definition)
}

fn cash_shop<E>(
    dimensions: &mut impl FnMut(&str) -> Result<SpriteDimensions, E>
) -> Result<GuiWindowDefinition, E> {
    let background_path = "CashShop.img/Base/backgrnd";
    let size = dimensions(background_path)?;
    let buy_path = "CashShop.img/CSList/BtBuy/normal/0";
    let gift_path = "CashShop.img/CSList/BtGift/disabled/0";
    let exit_path = "CashShop.img/CSStatus/BtExit/normal/0";
    let buy_size = dimensions(buy_path)?;
    let gift_size = dimensions(gift_path)?;
    let exit_size = dimensions(exit_path)?;
    let mut definition = window(
        "cash-shop",
        0.0,
        0.0,
        size.width,
        size.height,
        source("cash-shop-background", background_path, 0.0, 0.0),
    );
    definition.sprites.extend([
        source(
            "cash-shop-category-tab",
            "CashShop.img/CSTab/Tab/1",
            273.0,
            16.0,
        ),
        source(
            "cash-shop-preview",
            "CashShop.img/Base/Preview/0",
            24.0,
            40.0,
        ),
    ]);
    for (index, (x, y)) in [98.0, 179.0, 260.0, 341.0, 422.0]
        .into_iter()
        .flat_map(|y| [278.0, 484.0].map(move |x| (x, y)))
        .enumerate()
    {
        definition.sprites.push(source(
            &format!("cash-shop-item-card-{index}"),
            "CashShop.img/CSList/Base",
            x,
            y,
        ));
        definition.regions.extend([
            region(
                &format!("cash-shop-buy-{index}"),
                x + 77.0,
                y + 57.0,
                buy_size.width,
                buy_size.height,
            ),
            region(
                &format!("cash-shop-gift-{index}"),
                x + 118.0,
                y + 57.0,
                gift_size.width,
                gift_size.height,
            ),
        ]);
    }
    definition
        .sprites
        .push(source("cash-shop-exit", exit_path, 632.0, 535.0));
    definition.sprite_templates.extend([
        template("cash-shop-buy", buy_path),
        template("cash-shop-gift-disabled", gift_path),
    ]);
    definition.regions.push(region(
        "cash-shop-exit",
        632.0,
        535.0,
        exit_size.width,
        exit_size.height,
    ));
    Ok(definition)
}

fn window(
    name: &str,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    background: GuiSpriteSource,
) -> GuiWindowDefinition {
    GuiWindowDefinition {
        name: name.to_owned(),
        x,
        y,
        background: Some(background),
        sprites: Vec::new(),
        sprite_templates: Vec::new(),
        regions: Vec::new(),
        width,
        height,
    }
}

fn source(
    name: &str,
    wz_path: &str,
    x: f32,
    y: f32,
) -> GuiSpriteSource {
    GuiSpriteSource {
        name: name.to_owned(),
        wz_path: wz_path.to_owned(),
        x,
        y,
        ..GuiSpriteSource::default()
    }
}

fn pinned_right(
    name: &str,
    wz_path: &str,
    right: f32,
    y: f32,
    anchor_right: bool,
) -> GuiSpriteSource {
    GuiSpriteSource {
        name: name.to_owned(),
        wz_path: wz_path.to_owned(),
        y,
        anchor_right,
        pin_right: true,
        right,
        ..GuiSpriteSource::default()
    }
}

fn pinned_bottom(
    name: &str,
    wz_path: &str,
    x: f32,
    bottom: f32,
) -> GuiSpriteSource {
    GuiSpriteSource {
        name: name.to_owned(),
        wz_path: wz_path.to_owned(),
        x,
        pin_bottom: true,
        bottom,
        ..GuiSpriteSource::default()
    }
}

fn pinned_right_bottom(
    name: &str,
    wz_path: &str,
    right: f32,
    bottom: f32,
    anchor_right: bool,
) -> GuiSpriteSource {
    GuiSpriteSource {
        name: name.to_owned(),
        wz_path: wz_path.to_owned(),
        anchor_right,
        pin_right: true,
        right,
        pin_bottom: true,
        bottom,
        ..GuiSpriteSource::default()
    }
}

fn template(
    name: &str,
    wz_path: &str,
) -> GuiSpriteTemplateSource {
    GuiSpriteTemplateSource {
        name: name.to_owned(),
        wz_path: wz_path.to_owned(),
        ..GuiSpriteTemplateSource::default()
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

#[cfg(test)]
mod tests {
    use super::SpriteDimensions;
    use super::builtin_definition;
    use crate::SUPPORTED_WINDOWS;
    use crate::validate;

    #[test]
    fn every_supported_window_has_a_valid_builtin_source_recipe() {
        for name in SUPPORTED_WINDOWS {
            let definition = builtin_definition(name, |path| {
                Ok::<_, std::convert::Infallible>(dimensions(path))
            })
            .expect("infallible dimensions")
            .expect("supported definition");

            validate(&definition)
                .unwrap_or_else(|error| panic!("invalid {name} built-in recipe: {error}"));
        }
    }

    #[test]
    fn composite_windows_use_explicit_canvas_dimensions() {
        let status = builtin_definition("status-bar", |path| {
            Ok::<_, std::convert::Infallible>(dimensions(path))
        })
        .expect("infallible dimensions")
        .expect("status bar");
        let dialog = builtin_definition("npc-dialog", |path| {
            Ok::<_, std::convert::Infallible>(dimensions(path))
        })
        .expect("infallible dimensions")
        .expect("NPC dialog");

        assert_eq!((status.width, status.height), (800.0, 80.0));
        assert_eq!(status.background.expect("background").y, 9.0);
        assert_eq!((dialog.width, dialog.height), (529.0, 286.0));
        assert_eq!(dialog.sprites.len(), 11);
    }

    fn dimensions(path: &str) -> SpriteDimensions {
        let (width, height) = match path {
            "StatusBar.img/base/backgrnd" => (800.0, 71.0),
            "StatusBar.img/base/quickSlot" => (151.0, 80.0),
            "UIWindow.img/Stat/backgrnd" => (175.0, 347.0),
            "UIWindow.img/Equip/backgrnd" => (175.0, 304.0),
            "UIWindow.img/Item/backgrnd" => (175.0, 289.0),
            "UIWindow.img/Skill/backgrnd" => (175.0, 289.0),
            "UIWindow.img/Skill/skill0" => (141.0, 35.0),
            "UIWindow.img/KeyConfig/backgrnd" => (629.0, 373.0),
            "UIWindow.img/UtilDlgEx/t" => (529.0, 46.0),
            "UIWindow.img/UtilDlgEx/c" => (529.0, 18.0),
            "UIWindow.img/UtilDlgEx/s" => (529.0, 60.0),
            "UIWindow.img/Shop/backgrnd" => (445.0, 333.0),
            "CashShop.img/Base/backgrnd" => (800.0, 600.0),
            "CashShop.img/CSStatus/BtExit/normal/0" => (168.0, 49.0),
            _ => (20.0, 20.0),
        };
        SpriteDimensions { width, height }
    }
}
