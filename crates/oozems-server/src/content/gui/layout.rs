use oozems_proto::v1::GuiLayout;
use oozems_proto::v1::GuiRegion;
use oozems_proto::v1::GuiSprite;
use oozems_proto::v1::GuiSpriteTemplate;
use oozems_proto::v1::GuiWindow;
use oozems_proto::v1::KeyActionDefinition;

use super::GuiContentError;
use super::ItemWindowSources;
use super::KeyConfigSources;
use super::NpcDialogSources;
use super::ShopWindowSources;
use super::SkillWindowSources;
use super::SourceSprite;
use super::StatWindowSources;
use super::StatusBarSources;
use super::invalid;

const GAUGE_LEFT_IN_OVERLAY: f32 = 209.0;
const GAUGE_BOTTOM_PADDING: f32 = 1.0;
const MENU_BUTTON_LEFT: f32 = 574.0;
const MENU_BUTTON_GAP: f32 = 2.0;
const MENU_BUTTON_TOP: f32 = 8.0;
const KEY_COLUMN_CENTER: f32 = 22.0;
const KEY_COLUMN_WIDTH: f32 = 35.0;
const KEY_ROW_CENTER: f32 = 22.0;
const KEY_ROW_HEIGHT: f32 = 35.0;
const STAT_WINDOW_X: f32 = 20.0;
const STAT_WINDOW_Y: f32 = 80.0;
const WINDOW_CLOSE_RIGHT: f32 = 5.0;
const WINDOW_CLOSE_TOP: f32 = 5.0;
const STAT_JOB_LEFT: f32 = 60.0;
const STAT_JOB_TOP: f32 = 57.0;
const SKILL_WINDOW_X: f32 = 20.0;
const SKILL_WINDOW_Y: f32 = 80.0;
const SKILL_CONTENT_LEFT: f32 = 17.0;
const SKILL_LIST_TOP: f32 = 94.0;
const SKILL_VISIBLE_ROWS: usize = 4;
const SKILL_TITLE_TOP: f32 = 27.0;
const SKILL_TITLE_HEIGHT: f32 = 14.0;
const SKILL_POINTS_LEFT: f32 = 82.0;
const SKILL_POINTS_TOP: f32 = 265.0;
const SKILL_POINTS_WIDTH: f32 = 30.0;
const SKILL_POINTS_HEIGHT: f32 = 14.0;
const SKILL_PAGE_TOP: f32 = 64.0;
const SKILL_PAGE_HEIGHT: f32 = 19.0;
const KEY_CONFIG_WINDOW_X: f32 = 165.0;
const KEY_CONFIG_WINDOW_Y: f32 = 60.0;
const KEY_CONFIG_CLOSE_RIGHT: f32 = 5.0;
const KEY_CONFIG_CLOSE_TOP: f32 = 6.0;
const KEY_PALETTE_LEFT: f32 = 7.0;
const KEY_PALETTE_TOP: f32 = 267.0;
const KEY_PALETTE_STEP: f32 = 34.0;
const KEY_PALETTE_COLUMNS: usize = 18;
const NPC_DIALOG_X: f32 = 135.0;
const NPC_DIALOG_Y: f32 = 100.0;
const NPC_DIALOG_CENTER_ROWS: usize = 10;
const SHOP_WINDOW_X: f32 = 168.0;
const SHOP_WINDOW_Y: f32 = 80.0;

pub(super) fn compose_npc_dialog_window(
    sources: &NpcDialogSources
) -> Result<GuiWindow, GuiContentError> {
    let center_top = sources.top.height;
    let bottom_top = center_top + sources.center.height * NPC_DIALOG_CENTER_ROWS as f32;
    let height = bottom_top + sources.bottom.height;
    let mut sprites = (0..NPC_DIALOG_CENTER_ROWS)
        .map(|row| {
            place_sprite(
                &sources.center,
                0.0,
                center_top + sources.center.height * row as f32,
                false,
            )
        })
        .collect::<Vec<_>>();
    sprites.push(place_sprite(&sources.bottom, 0.0, bottom_top, false));
    let layout = GuiLayout {
        width: sources.top.width,
        height,
        background: Some(place_sprite(&sources.top, 0.0, 0.0, false)),
        sprites,
        sprite_templates: [
            &sources.close,
            &sources.ok,
            &sources.next,
            &sources.previous,
            &sources.accept,
            &sources.decline,
            &sources.choice,
            &sources.choice_selected,
        ]
        .map(sprite_template)
        .into(),
        regions: vec![
            region("npc-portrait", 15.0, 20.0, 90.0, 155.0),
            region("npc-title", 154.0, 17.0, 345.0, 20.0),
            region("npc-text", 154.0, 42.0, 345.0, 170.0),
            region("npc-choices", 154.0, 108.0, 345.0, 100.0),
            region("npc-previous", 402.0, bottom_top + 27.0, 46.0, 20.0),
            region("npc-next", 459.0, bottom_top + 27.0, 46.0, 20.0),
            region("npc-ok", 459.0, bottom_top + 27.0, 46.0, 20.0),
            region("npc-close", 420.0, bottom_top + 27.0, 85.0, 20.0),
            region("npc-accept", 383.0, bottom_top + 27.0, 60.0, 20.0),
            region("npc-decline", 448.0, bottom_top + 27.0, 60.0, 20.0),
        ],
    };
    validate_layout(&layout)?;
    Ok(GuiWindow {
        x: NPC_DIALOG_X,
        y: NPC_DIALOG_Y,
        layout: Some(layout),
    })
}

pub(super) fn compose_shop_window(
    sources: &ShopWindowSources
) -> Result<GuiWindow, GuiContentError> {
    let layout = GuiLayout {
        width: sources.background.width,
        height: sources.background.height,
        background: Some(place_sprite(&sources.background, 0.0, 0.0, false)),
        sprites: Vec::new(),
        sprite_templates: [
            &sources.selection,
            &sources.meso,
            &sources.buy,
            &sources.sell,
            &sources.exit,
        ]
        .map(sprite_template)
        .into(),
        regions: vec![
            region("shop-stock", 7.0, 121.0, 198.0, 185.0),
            region("shop-inventory", 237.0, 121.0, 198.0, 185.0),
            region("shop-inventory-previous", 391.0, 98.0, 18.0, 18.0),
            region("shop-inventory-next", 415.0, 98.0, 18.0, 18.0),
            region("shop-buy", 125.0, 310.0, 80.0, 18.0),
            region("shop-sell", 355.0, 310.0, 80.0, 18.0),
            region("shop-close", 9.0, 310.0, 80.0, 18.0),
            region("shop-mesos", 272.0, 96.0, 160.0, 18.0),
        ],
    };
    validate_layout(&layout)?;
    Ok(GuiWindow {
        x: SHOP_WINDOW_X,
        y: SHOP_WINDOW_Y,
        layout: Some(layout),
    })
}

pub(super) fn compose_status_bar(sources: &StatusBarSources) -> Result<GuiLayout, GuiContentError> {
    let width = sources.background.width;
    let height = sources.background.height.max(sources.quick_slots.height);
    let bar_y = height - sources.background.height;
    let background = place_sprite(&sources.background, 0.0, bar_y, false);
    let overlay_x = 0.0;
    let overlay_y = bar_y;
    let quick_slots_x = width - sources.quick_slots.width;
    let mut sprites = vec![
        place_sprite(&sources.overlay, overlay_x, overlay_y, false),
        place_sprite(
            &sources.gauge,
            overlay_x + GAUGE_LEFT_IN_OVERLAY,
            height - sources.gauge.height - GAUGE_BOTTOM_PADDING,
            false,
        ),
        place_sprite(
            &sources.gauge_graduation,
            overlay_x + GAUGE_LEFT_IN_OVERLAY,
            height - sources.gauge_graduation.height - GAUGE_BOTTOM_PADDING,
            false,
        ),
        place_sprite(&sources.quick_slots, quick_slots_x, 0.0, true),
    ];
    sprites.extend(
        sources
            .key_references
            .iter()
            .enumerate()
            .map(|(index, source)| place_key_reference(source, quick_slots_x, index)),
    );
    let menu_buttons = [
        (&sources.equip, Some(&sources.equip_pressed)),
        (&sources.inventory, Some(&sources.inventory_pressed)),
        (&sources.stats, Some(&sources.stats_pressed)),
        (&sources.skills, Some(&sources.skills_pressed)),
        (&sources.key_settings, Some(&sources.key_settings_pressed)),
        (&sources.quick_slot_toggle, None),
    ];
    let mut button_x = MENU_BUTTON_LEFT;
    for (source, pressed) in menu_buttons {
        let x = button_x;
        sprites.push(place_sprite(source, x, bar_y + MENU_BUTTON_TOP, false));
        if let Some(pressed) = pressed {
            sprites.push(place_sprite(pressed, x, bar_y + MENU_BUTTON_TOP, false));
        }
        button_x += source.width + MENU_BUTTON_GAP;
    }
    let layout = GuiLayout {
        width,
        height,
        background: Some(background),
        sprites,
        sprite_templates: Vec::new(),
        regions: Vec::new(),
    };
    validate_layout(&layout)?;
    Ok(layout)
}

pub(super) fn compose_key_config(
    sources: &KeyConfigSources
) -> Result<(GuiWindow, Vec<KeyActionDefinition>), GuiContentError> {
    let width = sources.background.width;
    let height = sources.background.height;
    let mut sprites = vec![place_sprite(
        &sources.close,
        width - sources.close.width - KEY_CONFIG_CLOSE_RIGHT,
        KEY_CONFIG_CLOSE_TOP,
        false,
    )];
    let mut actions = Vec::with_capacity(sources.actions.len());
    for (spec, source) in &sources.actions {
        let column = spec.palette_index % KEY_PALETTE_COLUMNS;
        let row = spec.palette_index / KEY_PALETTE_COLUMNS;
        let icon = place_sprite(
            source,
            KEY_PALETTE_LEFT + column as f32 * KEY_PALETTE_STEP,
            KEY_PALETTE_TOP + row as f32 * KEY_PALETTE_STEP,
            false,
        );
        sprites.push(icon.clone());
        actions.push(KeyActionDefinition {
            action: spec.action as i32,
            label: spec.label.to_owned(),
            icon: Some(icon),
        });
    }
    let layout = GuiLayout {
        width,
        height,
        background: Some(place_sprite(&sources.background, 0.0, 0.0, false)),
        sprites,
        sprite_templates: Vec::new(),
        regions: Vec::new(),
    };
    validate_layout(&layout)?;
    Ok((
        GuiWindow {
            x: KEY_CONFIG_WINDOW_X,
            y: KEY_CONFIG_WINDOW_Y,
            layout: Some(layout),
        },
        actions,
    ))
}

pub(super) fn compose_item_window(
    sources: &ItemWindowSources,
    x: f32,
    y: f32,
) -> Result<GuiWindow, GuiContentError> {
    let width = sources.background.width;
    let height = sources.background.height;
    let layout = GuiLayout {
        width,
        height,
        background: Some(place_sprite(&sources.background, 0.0, 0.0, false)),
        sprites: vec![place_sprite(
            &sources.close,
            width - sources.close.width - WINDOW_CLOSE_RIGHT,
            WINDOW_CLOSE_TOP,
            false,
        )],
        sprite_templates: Vec::new(),
        regions: Vec::new(),
    };
    validate_layout(&layout)?;
    Ok(GuiWindow {
        x,
        y,
        layout: Some(layout),
    })
}

pub(super) fn compose_skill_window(
    sources: &SkillWindowSources
) -> Result<GuiWindow, GuiContentError> {
    let width = sources.background.width;
    let height = sources.background.height;
    let layout = GuiLayout {
        width,
        height,
        background: Some(place_sprite(&sources.background, 0.0, 0.0, false)),
        sprites: vec![place_sprite(
            &sources.close,
            width - sources.close.width - WINDOW_CLOSE_RIGHT,
            WINDOW_CLOSE_TOP,
            false,
        )],
        sprite_templates: [
            &sources.row,
            &sources.selected_row,
            &sources.point_up,
            &sources.point_up_hover,
            &sources.point_up_pressed,
            &sources.point_up_disabled,
        ]
        .map(sprite_template)
        .into(),
        // UIWindow.img supplies these components and their geometry, but the
        // original client supplied their destinations against the background.
        regions: vec![
            region(
                "skill-title",
                SKILL_CONTENT_LEFT,
                SKILL_TITLE_TOP,
                sources.row.width,
                SKILL_TITLE_HEIGHT,
            ),
            region(
                "skill-list",
                SKILL_CONTENT_LEFT,
                SKILL_LIST_TOP,
                sources.row.width,
                sources.row.height * SKILL_VISIBLE_ROWS as f32,
            ),
            region(
                "skill-points",
                SKILL_POINTS_LEFT,
                SKILL_POINTS_TOP,
                SKILL_POINTS_WIDTH,
                SKILL_POINTS_HEIGHT,
            ),
            region(
                "skill-page-previous",
                80.0,
                SKILL_PAGE_TOP,
                18.0,
                SKILL_PAGE_HEIGHT,
            ),
            region(
                "skill-page-label",
                98.0,
                SKILL_PAGE_TOP,
                41.0,
                SKILL_PAGE_HEIGHT,
            ),
            region(
                "skill-page-next",
                139.0,
                SKILL_PAGE_TOP,
                18.0,
                SKILL_PAGE_HEIGHT,
            ),
        ],
    };
    validate_layout(&layout)?;
    Ok(GuiWindow {
        x: SKILL_WINDOW_X,
        y: SKILL_WINDOW_Y,
        layout: Some(layout),
    })
}

pub(super) fn compose_stat_window(
    sources: &StatWindowSources
) -> Result<GuiWindow, GuiContentError> {
    let width = sources.background.width;
    let height = sources.background.height;
    let background = place_sprite(&sources.background, 0.0, 0.0, false);
    let sprites = vec![
        place_sprite(
            &sources.close,
            width - sources.close.width - WINDOW_CLOSE_RIGHT,
            WINDOW_CLOSE_TOP,
            false,
        ),
        place_sprite(&sources.job, STAT_JOB_LEFT, STAT_JOB_TOP, false),
    ];
    let layout = GuiLayout {
        width,
        height,
        background: Some(background),
        sprites,
        sprite_templates: Vec::new(),
        regions: Vec::new(),
    };
    validate_layout(&layout)?;
    Ok(GuiWindow {
        x: STAT_WINDOW_X,
        y: STAT_WINDOW_Y,
        layout: Some(layout),
    })
}

fn place_key_reference(
    source: &SourceSprite,
    quick_slots_x: f32,
    index: usize,
) -> GuiSprite {
    let column = (index % 4) as f32;
    let row = (index / 4) as f32;
    let x = quick_slots_x + KEY_COLUMN_CENTER + column * KEY_COLUMN_WIDTH - source.width / 2.0;
    let y = KEY_ROW_CENTER + row * KEY_ROW_HEIGHT - source.height / 2.0;
    place_sprite(source, x.floor(), y.floor(), true)
}

fn place_sprite(
    source: &SourceSprite,
    x: f32,
    y: f32,
    anchor_right: bool,
) -> GuiSprite {
    GuiSprite {
        name: source.name.clone(),
        asset_id: source.asset_id.clone(),
        x,
        y,
        width: source.width,
        height: source.height,
        anchor_right,
        origin_x: source.origin_x,
        origin_y: source.origin_y,
    }
}

fn sprite_template(source: &SourceSprite) -> GuiSpriteTemplate {
    GuiSpriteTemplate {
        name: source.name.clone(),
        asset_id: source.asset_id.clone(),
        width: source.width,
        height: source.height,
        origin_x: source.origin_x,
        origin_y: source.origin_y,
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

fn validate_layout(layout: &GuiLayout) -> Result<(), GuiContentError> {
    if !layout.width.is_finite()
        || !layout.height.is_finite()
        || layout.width <= 0.0
        || layout.height <= 0.0
    {
        return invalid("the GUI layout dimensions must be finite and positive");
    }
    let sprites = layout.background.iter().chain(layout.sprites.iter());
    for sprite in sprites {
        let values = [
            sprite.x,
            sprite.y,
            sprite.width,
            sprite.height,
            sprite.origin_x,
            sprite.origin_y,
            sprite.x + sprite.width,
            sprite.y + sprite.height,
        ];
        if sprite.name.is_empty()
            || sprite.asset_id.is_empty()
            || !values.iter().all(|value| value.is_finite())
            || sprite.x < 0.0
            || sprite.y < 0.0
            || sprite.width <= 0.0
            || sprite.height <= 0.0
            || sprite.x + sprite.width > layout.width
            || sprite.y + sprite.height > layout.height
        {
            return invalid(format!(
                "GUI sprite {:?} is outside the layout",
                sprite.name
            ));
        }
    }
    for template in &layout.sprite_templates {
        let values = [
            template.width,
            template.height,
            template.origin_x,
            template.origin_y,
        ];
        if template.name.is_empty()
            || template.asset_id.is_empty()
            || !values.iter().all(|value| value.is_finite())
            || template.width <= 0.0
            || template.height <= 0.0
        {
            return invalid(format!(
                "GUI sprite template {:?} is invalid",
                template.name
            ));
        }
    }
    for region in &layout.regions {
        let values = [
            region.x,
            region.y,
            region.width,
            region.height,
            region.x + region.width,
            region.y + region.height,
        ];
        if region.name.is_empty()
            || !values.iter().all(|value| value.is_finite())
            || region.x < 0.0
            || region.y < 0.0
            || region.width <= 0.0
            || region.height <= 0.0
            || region.x + region.width > layout.width
            || region.y + region.height > layout.height
        {
            return invalid(format!(
                "GUI region {:?} is outside the layout",
                region.name
            ));
        }
    }
    Ok(())
}
