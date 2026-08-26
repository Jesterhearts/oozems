use std::collections::BTreeSet;

use oozems_proto::v1::CharacterStats;
use oozems_proto::v1::EquipmentSlot;
use oozems_proto::v1::GameGui;
use oozems_proto::v1::GuiLayout;
use oozems_proto::v1::GuiRegion;
use oozems_proto::v1::GuiSprite;
use oozems_proto::v1::GuiSpriteTemplate;
use oozems_proto::v1::InventoryItemStack;
use oozems_proto::v1::InventoryState;
use oozems_proto::v1::ItemCategory;
use oozems_proto::v1::ItemDefinition;
use oozems_proto::v1::SkillBook;

mod key_config;

pub use key_config::KeyDrag;
pub use key_config::KeyIconPlacement;
pub use key_config::begin_key_drag;
pub use key_config::bound_key_icons;
pub use key_config::dragged_key_icon;
pub use key_config::finish_key_drag;
pub use key_config::move_key_drag;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InventoryTab {
    #[default]
    Equipment,
    Consume,
    Install,
    Etc,
    Cash,
}

impl InventoryTab {
    pub(crate) const ALL: [Self; 5] = [
        Self::Equipment,
        Self::Consume,
        Self::Install,
        Self::Etc,
        Self::Cash,
    ];

    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::Equipment => "equipment",
            Self::Consume => "consume",
            Self::Install => "install",
            Self::Etc => "etc",
            Self::Cash => "cash",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GuiState {
    pub stats_open: bool,
    pub equipment_open: bool,
    pub inventory_open: bool,
    pub inventory_tab: InventoryTab,
    pub key_config_open: bool,
    pub skill_page: usize,
    pub skills_open: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CanvasPoint {
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GaugeFill {
    pub source_x: f64,
    pub full_width: f64,
    pub filled_width: f64,
}

pub(crate) struct InventorySlot<'a> {
    pub inventory_index: u32,
    pub visual_index: usize,
    pub stack: &'a InventoryItemStack,
    pub definition: &'a ItemDefinition,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct CanvasRect {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

struct InventoryHit {
    inventory_index: u32,
    can_equip: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuiAction {
    OpenCashShop,
    ToggleStats,
    ToggleEquipment,
    ToggleInventory,
    ToggleKeyConfig,
    ToggleSkills,
    PreviousSkillPage,
    NextSkillPage,
    CloseStats,
    CloseEquipment,
    CloseInventory,
    SelectInventoryTab { tab: InventoryTab },
    CloseKeyConfig,
    CloseSkills,
    Equip { inventory_index: u32 },
    Unequip { slot: i32 },
    Drop { inventory_index: u32 },
    AllocateSkill { skill_id: u32 },
    UseSkill { skill_id: u32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerButton {
    Left,
    Right,
}

const INVENTORY_COLUMNS: usize = 4;
const INVENTORY_SLOT_LEFT: f32 = 7.0;
const INVENTORY_SLOT_TOP: f32 = 50.0;
const INVENTORY_SLOT_STEP: f32 = 36.0;
const ITEM_SLOT_SIZE: f32 = 32.0;
pub(crate) const INVENTORY_VISIBLE_SLOTS: usize = 24;

pub fn click_action(
    state: GuiState,
    gui: &GameGui,
    inventory: Option<&InventoryState>,
    skill_book: Option<&SkillBook>,
    viewport_width: f32,
    viewport_height: f32,
    point: CanvasPoint,
    button: PointerButton,
) -> Option<GuiAction> {
    if state.key_config_open {
        return window_action(state, gui, inventory, skill_book, point, button);
    }
    window_action(state, gui, inventory, skill_book, point, button)
        .or_else(|| status_action(gui, viewport_width, viewport_height, point, button))
}

pub fn apply_local_action(
    state: &mut GuiState,
    action: GuiAction,
) -> bool {
    match action {
        GuiAction::OpenCashShop => return false,
        GuiAction::ToggleStats => {
            state.stats_open = !state.stats_open;
            state.equipment_open = false;
            state.skills_open = false;
        }
        GuiAction::ToggleEquipment => {
            state.equipment_open = !state.equipment_open;
            state.stats_open = false;
            state.skills_open = false;
        }
        GuiAction::ToggleInventory => state.inventory_open = !state.inventory_open,
        GuiAction::ToggleSkills => {
            let opening = !state.skills_open;
            state.skills_open = opening;
            if opening {
                state.skill_page = 0;
            }
            state.stats_open = false;
            state.equipment_open = false;
        }
        GuiAction::PreviousSkillPage => {
            state.skill_page = state.skill_page.saturating_sub(1);
        }
        GuiAction::NextSkillPage => {
            state.skill_page = state.skill_page.saturating_add(1);
        }
        GuiAction::ToggleKeyConfig => {
            state.key_config_open = !state.key_config_open;
            if state.key_config_open {
                state.stats_open = false;
                state.equipment_open = false;
                state.inventory_open = false;
            }
        }
        GuiAction::CloseStats => state.stats_open = false,
        GuiAction::CloseEquipment => state.equipment_open = false,
        GuiAction::CloseInventory => state.inventory_open = false,
        GuiAction::SelectInventoryTab { tab } => state.inventory_tab = tab,
        GuiAction::CloseKeyConfig => state.key_config_open = false,
        GuiAction::CloseSkills => state.skills_open = false,
        GuiAction::Equip { .. }
        | GuiAction::Unequip { .. }
        | GuiAction::Drop { .. }
        | GuiAction::AllocateSkill { .. }
        | GuiAction::UseSkill { .. } => {
            return false;
        }
    }
    true
}

pub fn canvas_point(
    offset_x: i32,
    offset_y: i32,
    canvas_width: u32,
    canvas_height: u32,
    client_width: i32,
    client_height: i32,
) -> Option<CanvasPoint> {
    if offset_x < 0
        || offset_y < 0
        || canvas_width == 0
        || canvas_height == 0
        || client_width <= 0
        || client_height <= 0
    {
        return None;
    }
    Some(CanvasPoint {
        x: offset_x as f32 * canvas_width as f32 / client_width as f32,
        y: offset_y as f32 * canvas_height as f32 / client_height as f32,
    })
}

pub fn sprite_screen_x(
    viewport_width: f32,
    layout_width: f32,
    sprite: &GuiSprite,
) -> f32 {
    if sprite.anchor_right {
        viewport_width - (layout_width - sprite.x)
    } else {
        sprite.x
    }
}

pub fn status_bar_top(
    viewport_height: f32,
    layout_height: f32,
) -> f32 {
    (viewport_height - layout_height).max(0.0)
}

pub fn status_sprite_visible(
    state: GuiState,
    sprite: &GuiSprite,
) -> bool {
    match sprite.name.as_str() {
        "stats-pressed" => state.stats_open,
        "equip-pressed" => state.equipment_open,
        "inventory-pressed" => state.inventory_open,
        "key-settings-pressed" => state.key_config_open,
        "skills-pressed" => state.skills_open,
        _ => true,
    }
}

pub fn inventory_slot_position(index: usize) -> (f32, f32) {
    let column = (index % INVENTORY_COLUMNS) as f32;
    let row = (index / INVENTORY_COLUMNS) as f32;
    (
        INVENTORY_SLOT_LEFT + column * INVENTORY_SLOT_STEP,
        INVENTORY_SLOT_TOP + row * INVENTORY_SLOT_STEP,
    )
}

pub(crate) fn inventory_slots<'a>(
    gui: &'a GameGui,
    inventory: &'a InventoryState,
    tab: InventoryTab,
) -> Vec<InventorySlot<'a>> {
    inventory
        .stacks
        .iter()
        .enumerate()
        .filter_map(|(index, stack)| {
            let definition = gui
                .items
                .iter()
                .find(|definition| definition.item_id == stack.item_id)?;
            (item_category_tab(definition.category)? == tab).then_some((index, stack, definition))
        })
        .take(INVENTORY_VISIBLE_SLOTS)
        .enumerate()
        .filter_map(|(visual_index, (inventory_index, stack, definition))| {
            Some(InventorySlot {
                inventory_index: u32::try_from(inventory_index).ok()?,
                visual_index,
                stack,
                definition,
            })
        })
        .collect()
}

pub(crate) fn inventory_tab_template_position(
    region: &GuiRegion,
    template: &GuiSpriteTemplate,
    align_bottom: bool,
) -> (f32, f32) {
    let x = (region.x + (region.width - template.width) / 2.0).floor();
    let y = if align_bottom {
        region.y + region.height - template.height
    } else {
        region.y + (region.height - template.height) / 2.0
    }
    .floor();
    (x, y)
}

pub(crate) fn missing_item_definition_ids(
    gui: &GameGui,
    observed_item_ids: impl IntoIterator<Item = u32>,
) -> Vec<u32> {
    let known = gui
        .items
        .iter()
        .map(|definition| definition.item_id)
        .collect::<BTreeSet<_>>();
    observed_item_ids
        .into_iter()
        .filter(|item_id| *item_id > 0 && !known.contains(item_id))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

pub(crate) fn item_definition_refresh_ids(
    gui: &GameGui,
    observed_item_ids: impl IntoIterator<Item = u32>,
    required: bool,
) -> Option<Vec<u32>> {
    let observed_item_ids = observed_item_ids
        .into_iter()
        .filter(|item_id| *item_id > 0)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    (required || !missing_item_definition_ids(gui, observed_item_ids.iter().copied()).is_empty())
        .then_some(observed_item_ids)
}

pub fn equipment_slot_position(slot_value: i32) -> Option<(f32, f32)> {
    match EquipmentSlot::try_from(slot_value).ok()? {
        EquipmentSlot::Top => Some((71.0, 134.0)),
        EquipmentSlot::Bottom => Some((38.0, 167.0)),
        EquipmentSlot::Shoes => Some((71.0, 167.0)),
        EquipmentSlot::Unspecified => None,
    }
}

pub fn gauge_fills(stats: &CharacterStats) -> [GaugeFill; 3] {
    [
        gauge_fill(2.0, 105.0, u64::from(stats.hp), u64::from(stats.max_hp)),
        gauge_fill(110.0, 105.0, u64::from(stats.mp), u64::from(stats.max_mp)),
        gauge_fill(223.0, 115.0, stats.experience, stats.experience_required),
    ]
}

pub fn gauge_labels(stats: &CharacterStats) -> [String; 3] {
    [
        format!("[{}/{}]", stats.hp, stats.max_hp),
        format!("[{}/{}]", stats.mp, stats.max_mp),
        format!("[{}/{}]", stats.experience, stats.experience_required),
    ]
}

pub fn valid_layout(layout: &GuiLayout) -> bool {
    layout.width.is_finite()
        && layout.height.is_finite()
        && layout.width > 0.0
        && layout.height > 0.0
        && layout
            .background
            .as_ref()
            .is_some_and(|sprite| valid_sprite(sprite, layout.width, layout.height))
        && layout
            .sprites
            .iter()
            .all(|sprite| valid_sprite(sprite, layout.width, layout.height))
        && layout.sprite_templates.iter().all(valid_sprite_template)
        && layout
            .regions
            .iter()
            .all(|region| valid_region(region, layout.width, layout.height))
}

fn window_action(
    state: GuiState,
    gui: &GameGui,
    inventory: Option<&InventoryState>,
    skill_book: Option<&SkillBook>,
    point: CanvasPoint,
    button: PointerButton,
) -> Option<GuiAction> {
    if state.key_config_open {
        if button == PointerButton::Left
            && window_close_rect(gui.key_config_window.as_ref(), "key-config-close")
                .is_some_and(|rect| rect_contains(rect, point))
        {
            return Some(GuiAction::CloseKeyConfig);
        }
        return None;
    }
    if state.inventory_open {
        if window_close_rect(gui.inventory_window.as_ref(), "inventory-close")
            .is_some_and(|rect| rect_contains(rect, point))
        {
            return Some(GuiAction::CloseInventory);
        }
        if button == PointerButton::Left
            && let Some(tab) = inventory_tab_at(gui, point)
        {
            return Some(GuiAction::SelectInventoryTab { tab });
        }
        if let Some(hit) = inventory_item_at(gui, inventory?, state.inventory_tab, point) {
            return match button {
                PointerButton::Left
                    if state.inventory_tab == InventoryTab::Equipment && hit.can_equip =>
                {
                    Some(GuiAction::Equip {
                        inventory_index: hit.inventory_index,
                    })
                }
                PointerButton::Left => None,
                PointerButton::Right => Some(GuiAction::Drop {
                    inventory_index: hit.inventory_index,
                }),
            };
        }
    }
    if button == PointerButton::Left && state.equipment_open {
        if window_close_rect(gui.equipment_window.as_ref(), "equipment-close")
            .is_some_and(|rect| rect_contains(rect, point))
        {
            return Some(GuiAction::CloseEquipment);
        }
        if let Some(slot) = equipped_item_at(gui, inventory?, point) {
            return Some(GuiAction::Unequip { slot });
        }
    }
    if button == PointerButton::Left
        && state.skills_open
        && window_close_rect(gui.skill_window.as_ref(), "skill-close")
            .is_some_and(|rect| rect_contains(rect, point))
    {
        return Some(GuiAction::CloseSkills);
    }
    if button == PointerButton::Left && state.skills_open {
        if let Some(skill_book) = skill_book
            && let Some(action) = skill_action_at(state, gui, skill_book, point)
        {
            return Some(action);
        }
        let window = gui.skill_window.as_ref()?;
        for (name, action) in [
            ("skill-page-previous", GuiAction::PreviousSkillPage),
            ("skill-page-next", GuiAction::NextSkillPage),
        ] {
            if window_region_rect(window, name).is_some_and(|rect| rect_contains(rect, point)) {
                return Some(action);
            }
        }
    }
    if button == PointerButton::Left
        && state.stats_open
        && window_close_rect(gui.stat_window.as_ref(), "stat-close")
            .is_some_and(|rect| rect_contains(rect, point))
    {
        return Some(GuiAction::CloseStats);
    }
    None
}

fn status_action(
    gui: &GameGui,
    viewport_width: f32,
    viewport_height: f32,
    point: CanvasPoint,
    button: PointerButton,
) -> Option<GuiAction> {
    if button != PointerButton::Left {
        return None;
    }
    [
        ("cash-shop", GuiAction::OpenCashShop),
        ("equip", GuiAction::ToggleEquipment),
        ("inventory", GuiAction::ToggleInventory),
        ("stats", GuiAction::ToggleStats),
        ("skills", GuiAction::ToggleSkills),
        ("key-settings", GuiAction::ToggleKeyConfig),
    ]
    .into_iter()
    .find_map(|(name, action)| {
        status_button_rect(gui, viewport_width, viewport_height, name)
            .filter(|rect| rect_contains(*rect, point))
            .map(|_| action)
    })
}

pub fn can_allocate_skill(
    book: &SkillBook,
    skill_id: u32,
) -> bool {
    if book.available_points == 0 {
        return false;
    }
    let Some(skill) = book.skills.iter().find(|skill| {
        skill
            .definition
            .as_ref()
            .is_some_and(|definition| definition.skill_id == skill_id)
    }) else {
        return false;
    };
    let Some(definition) = skill.definition.as_ref() else {
        return false;
    };
    skill.level < maximum_skill_level(skill)
        && definition.requirements.iter().all(|requirement| {
            book.skills.iter().any(|candidate| {
                candidate.level >= requirement.level
                    && candidate
                        .definition
                        .as_ref()
                        .is_some_and(|candidate_definition| {
                            candidate_definition.skill_id == requirement.skill_id
                        })
            })
        })
}

pub(crate) fn maximum_skill_level(skill: &oozems_proto::v1::PlayerSkill) -> u32 {
    let definition_maximum = skill
        .definition
        .as_ref()
        .map_or(0, |definition| definition.max_level);
    if skill.master_level > 0 {
        definition_maximum.min(skill.master_level)
    } else {
        definition_maximum
    }
}

fn skill_action_at(
    state: GuiState,
    gui: &GameGui,
    book: &SkillBook,
    point: CanvasPoint,
) -> Option<GuiAction> {
    let (skill, row) = skill_row_at(state, gui, book, point)?;
    let definition = skill.definition.as_ref()?;
    let window = gui.skill_window.as_ref()?;
    let layout = window.layout.as_ref()?;
    let button = named_sprite_template(layout, "skill-point-up")?;
    let button_rect = CanvasRect {
        x: row.x + row.width - button.width - 2.0,
        y: row.y + (row.height - button.height) / 2.0,
        width: button.width,
        height: button.height,
    };
    if rect_contains(button_rect, point) && can_allocate_skill(book, definition.skill_id) {
        return Some(GuiAction::AllocateSkill {
            skill_id: definition.skill_id,
        });
    }
    let icon_slot = CanvasRect {
        x: row.x,
        y: row.y,
        width: row.height,
        height: row.height,
    };
    (skill.level > 0 && rect_contains(icon_slot, point)).then_some(GuiAction::UseSkill {
        skill_id: definition.skill_id,
    })
}

fn skill_at_point(
    state: GuiState,
    gui: &GameGui,
    book: &SkillBook,
    point: CanvasPoint,
) -> Option<u32> {
    let (skill, row) = skill_row_at(state, gui, book, point)?;
    let icon_slot = CanvasRect {
        x: row.x,
        y: row.y,
        width: row.height,
        height: row.height,
    };
    (skill.level > 0 && rect_contains(icon_slot, point))
        .then(|| {
            skill
                .definition
                .as_ref()
                .map(|definition| definition.skill_id)
        })
        .flatten()
}

pub fn hovered_skill<'a>(
    state: GuiState,
    gui: &GameGui,
    book: &'a SkillBook,
    point: CanvasPoint,
) -> Option<&'a oozems_proto::v1::PlayerSkill> {
    skill_row_at(state, gui, book, point).map(|(skill, _)| skill)
}

pub(crate) fn hovered_inventory_item<'a>(
    state: GuiState,
    gui: &'a GameGui,
    inventory: &'a InventoryState,
    point: CanvasPoint,
) -> Option<InventorySlot<'a>> {
    if !state.inventory_open {
        return None;
    }
    let window = gui.inventory_window.as_ref()?;
    inventory_slots(gui, inventory, state.inventory_tab)
        .into_iter()
        .find(|slot| {
            let (x, y) = inventory_slot_position(slot.visual_index);
            rect_contains(
                CanvasRect {
                    x: window.x + x,
                    y: window.y + y,
                    width: ITEM_SLOT_SIZE,
                    height: ITEM_SLOT_SIZE,
                },
                point,
            )
        })
}

fn skill_row_at<'a>(
    state: GuiState,
    gui: &GameGui,
    book: &'a SkillBook,
    point: CanvasPoint,
) -> Option<(&'a oozems_proto::v1::PlayerSkill, CanvasRect)> {
    if !state.skills_open || book.skills.is_empty() {
        return None;
    }
    let window = gui.skill_window.as_ref()?;
    let layout = window
        .layout
        .as_ref()
        .filter(|layout| valid_layout(layout))?;
    let list = named_region(layout, "skill-list")?;
    let row = named_sprite_template(layout, "skill-row")?;
    let page_size = (list.height / row.height).floor() as usize;
    if page_size == 0 {
        return None;
    }
    let page_count = book.skills.len().div_ceil(page_size);
    let page = state.skill_page % page_count;
    book.skills
        .iter()
        .skip(page * page_size)
        .take(page_size)
        .enumerate()
        .find_map(|(index, skill)| {
            let rect = CanvasRect {
                x: window.x + list.x,
                y: window.y + list.y + index as f32 * row.height,
                width: row.width,
                height: row.height,
            };
            rect_contains(rect, point).then_some((skill, rect))
        })
}

fn gauge_fill(
    source_x: f64,
    full_width: f64,
    current: u64,
    maximum: u64,
) -> GaugeFill {
    let ratio = if maximum == 0 {
        0.0
    } else {
        current.min(maximum) as f64 / maximum as f64
    };
    GaugeFill {
        source_x,
        full_width,
        filled_width: (full_width * ratio).round(),
    }
}

fn status_button_rect(
    gui: &GameGui,
    viewport_width: f32,
    viewport_height: f32,
    name: &str,
) -> Option<CanvasRect> {
    let layout = gui
        .status_bar
        .as_ref()
        .filter(|layout| valid_layout(layout))?;
    let sprite = named_sprite(layout, name)?;
    Some(CanvasRect {
        x: sprite_screen_x(viewport_width, layout.width, sprite),
        y: status_bar_top(viewport_height, layout.height) + sprite.y,
        width: sprite.width,
        height: sprite.height,
    })
}

fn window_close_rect(
    window: Option<&oozems_proto::v1::GuiWindow>,
    name: &str,
) -> Option<CanvasRect> {
    let window = window?;
    let layout = window
        .layout
        .as_ref()
        .filter(|layout| valid_layout(layout))?;
    let sprite = named_sprite(layout, name)?;
    Some(CanvasRect {
        x: window.x + sprite.x,
        y: window.y + sprite.y,
        width: sprite.width,
        height: sprite.height,
    })
}

fn window_region_rect(
    window: &oozems_proto::v1::GuiWindow,
    name: &str,
) -> Option<CanvasRect> {
    let layout = window
        .layout
        .as_ref()
        .filter(|layout| valid_layout(layout))?;
    let region = named_region(layout, name)?;
    Some(CanvasRect {
        x: window.x + region.x,
        y: window.y + region.y,
        width: region.width,
        height: region.height,
    })
}

fn inventory_item_at(
    gui: &GameGui,
    inventory: &InventoryState,
    tab: InventoryTab,
    point: CanvasPoint,
) -> Option<InventoryHit> {
    let window = gui.inventory_window.as_ref()?;
    inventory_slots(gui, inventory, tab)
        .into_iter()
        .find_map(|slot| {
            let (x, y) = inventory_slot_position(slot.visual_index);
            rect_contains(
                CanvasRect {
                    x: window.x + x,
                    y: window.y + y,
                    width: ITEM_SLOT_SIZE,
                    height: ITEM_SLOT_SIZE,
                },
                point,
            )
            .then(|| InventoryHit {
                inventory_index: slot.inventory_index,
                can_equip: slot.definition.appearance_supported
                    && EquipmentSlot::try_from(slot.definition.slot).is_ok_and(|slot| {
                        matches!(
                            slot,
                            EquipmentSlot::Top | EquipmentSlot::Bottom | EquipmentSlot::Shoes
                        )
                    }),
            })
        })
}

fn inventory_tab_at(
    gui: &GameGui,
    point: CanvasPoint,
) -> Option<InventoryTab> {
    let window = gui.inventory_window.as_ref()?;
    InventoryTab::ALL.into_iter().find(|tab| {
        window_region_rect(window, &format!("inventory-tab-{}", tab.key()))
            .is_some_and(|rect| rect_contains(rect, point))
    })
}

fn item_category_tab(category: i32) -> Option<InventoryTab> {
    match ItemCategory::try_from(category).ok()? {
        ItemCategory::Equipment => Some(InventoryTab::Equipment),
        ItemCategory::Consume => Some(InventoryTab::Consume),
        ItemCategory::Install => Some(InventoryTab::Install),
        ItemCategory::Etc => Some(InventoryTab::Etc),
        ItemCategory::Cash | ItemCategory::Pet => Some(InventoryTab::Cash),
        ItemCategory::Unspecified => None,
    }
}

fn equipped_item_at(
    gui: &GameGui,
    inventory: &InventoryState,
    point: CanvasPoint,
) -> Option<i32> {
    let window = gui.equipment_window.as_ref()?;
    inventory.equipment.iter().find_map(|equipped| {
        let (x, y) = equipment_slot_position(equipped.slot)?;
        rect_contains(
            CanvasRect {
                x: window.x + x,
                y: window.y + y,
                width: ITEM_SLOT_SIZE,
                height: ITEM_SLOT_SIZE,
            },
            point,
        )
        .then_some(equipped.slot)
    })
}

fn named_sprite<'a>(
    layout: &'a GuiLayout,
    name: &str,
) -> Option<&'a GuiSprite> {
    layout.sprites.iter().find(|sprite| sprite.name == name)
}

pub fn named_region<'a>(
    layout: &'a GuiLayout,
    name: &str,
) -> Option<&'a GuiRegion> {
    layout.regions.iter().find(|region| region.name == name)
}

pub fn named_sprite_template<'a>(
    layout: &'a GuiLayout,
    name: &str,
) -> Option<&'a GuiSpriteTemplate> {
    layout
        .sprite_templates
        .iter()
        .find(|template| template.name == name)
}

fn valid_sprite(
    sprite: &GuiSprite,
    layout_width: f32,
    layout_height: f32,
) -> bool {
    let values = [
        sprite.x,
        sprite.y,
        sprite.width,
        sprite.height,
        sprite.origin_x,
        sprite.origin_y,
    ];
    !sprite.name.is_empty()
        && !sprite.asset_id.is_empty()
        && values.iter().all(|value| value.is_finite())
        && sprite.x >= 0.0
        && sprite.y >= 0.0
        && sprite.width > 0.0
        && sprite.height > 0.0
        && sprite.x + sprite.width <= layout_width
        && sprite.y + sprite.height <= layout_height
}

fn valid_sprite_template(template: &GuiSpriteTemplate) -> bool {
    let values = [
        template.width,
        template.height,
        template.origin_x,
        template.origin_y,
    ];
    !template.name.is_empty()
        && !template.asset_id.is_empty()
        && values.iter().all(|value| value.is_finite())
        && template.width > 0.0
        && template.height > 0.0
}

fn valid_region(
    region: &GuiRegion,
    layout_width: f32,
    layout_height: f32,
) -> bool {
    let values = [region.x, region.y, region.width, region.height];
    !region.name.is_empty()
        && values.iter().all(|value| value.is_finite())
        && region.x >= 0.0
        && region.y >= 0.0
        && region.width > 0.0
        && region.height > 0.0
        && region.x + region.width <= layout_width
        && region.y + region.height <= layout_height
}

fn rect_contains(
    rect: CanvasRect,
    point: CanvasPoint,
) -> bool {
    crate::hit_test::contains(
        crate::hit_test::Rect {
            x: f64::from(rect.x),
            y: f64::from(rect.y),
            width: f64::from(rect.width),
            height: f64::from(rect.height),
        },
        crate::hit_test::Point {
            x: f64::from(point.x),
            y: f64::from(point.y),
        },
    )
}

#[cfg(test)]
mod tests {
    use oozems_proto::v1::CharacterStats;
    use oozems_proto::v1::EquipmentSlot;
    use oozems_proto::v1::EquippedItem;
    use oozems_proto::v1::GameGui;
    use oozems_proto::v1::GuiLayout;
    use oozems_proto::v1::GuiRegion;
    use oozems_proto::v1::GuiSprite;
    use oozems_proto::v1::GuiSpriteTemplate;
    use oozems_proto::v1::GuiWindow;
    use oozems_proto::v1::InventoryItemStack;
    use oozems_proto::v1::InventoryState;
    use oozems_proto::v1::ItemCategory;
    use oozems_proto::v1::ItemDefinition;
    use oozems_proto::v1::PlayerSkill;
    use oozems_proto::v1::SkillBook;
    use oozems_proto::v1::SkillDefinition;

    use super::CanvasPoint;
    use super::GuiAction;
    use super::GuiState;
    use super::InventoryTab;
    use super::PointerButton;
    use super::apply_local_action;
    use super::canvas_point;
    use super::click_action;
    use super::gauge_fills;
    use super::gauge_labels;
    use super::hovered_inventory_item;
    use super::inventory_slots;
    use super::inventory_tab_template_position;
    use super::item_definition_refresh_ids;
    use super::missing_item_definition_ids;
    use super::sprite_screen_x;
    use super::status_sprite_visible;
    use super::valid_layout;

    #[test]
    fn stat_button_toggles_the_window_and_close_hides_it() {
        let gui = gui_fixture();
        let mut state = GuiState::default();

        let action = click_action(
            state,
            &gui,
            None,
            None,
            960.0,
            600.0,
            CanvasPoint { x: 635.0, y: 540.0 },
            PointerButton::Left,
        )
        .expect("stat action");
        assert!(apply_local_action(&mut state, action));
        assert!(state.stats_open);
        let action = click_action(
            state,
            &gui,
            None,
            None,
            960.0,
            600.0,
            CanvasPoint { x: 181.0, y: 86.0 },
            PointerButton::Left,
        )
        .expect("close action");
        assert!(apply_local_action(&mut state, action));
        assert!(!state.stats_open);
    }

    #[test]
    fn item_slots_produce_server_actions() {
        let gui = gui_fixture();
        let inventory = InventoryState {
            equipment: vec![EquippedItem {
                slot: EquipmentSlot::Top as i32,
                item_id: 1_040_002,
                expires_at_unix_ms: 0,
            }],
            capacity: 24,
            stacks: vec![InventoryItemStack {
                item_id: 1_040_003,
                quantity: 1,
                expires_at_unix_ms: 0,
            }],
        };
        let state = GuiState {
            equipment_open: true,
            inventory_open: true,
            ..GuiState::default()
        };

        assert_eq!(
            click_action(
                state,
                &gui,
                Some(&inventory),
                None,
                960.0,
                600.0,
                CanvasPoint { x: 213.0, y: 131.0 },
                PointerButton::Left,
            ),
            Some(GuiAction::Equip { inventory_index: 0 })
        );
        assert_eq!(
            click_action(
                state,
                &gui,
                Some(&inventory),
                None,
                960.0,
                600.0,
                CanvasPoint { x: 213.0, y: 131.0 },
                PointerButton::Right,
            ),
            Some(GuiAction::Drop { inventory_index: 0 })
        );
        assert_eq!(
            click_action(
                state,
                &gui,
                Some(&inventory),
                None,
                960.0,
                600.0,
                CanvasPoint { x: 92.0, y: 215.0 },
                PointerButton::Left,
            ),
            Some(GuiAction::Unequip {
                slot: EquipmentSlot::Top as i32
            })
        );
    }

    #[test]
    fn inventory_tabs_filter_items_without_changing_server_indices() {
        let mut gui = gui_fixture();
        let unsupported_equipment = ItemDefinition {
            item_id: 1_300_000,
            category: ItemCategory::Equipment as i32,
            ..ItemDefinition::default()
        };
        gui.items = vec![
            item_definition(2_000_000, ItemCategory::Consume),
            item_definition(1_040_003, ItemCategory::Equipment),
            item_definition(5_000_000, ItemCategory::Pet),
            unsupported_equipment,
        ];
        let inventory = InventoryState {
            capacity: 24,
            stacks: vec![
                inventory_stack(2_000_000),
                inventory_stack(1_040_003),
                inventory_stack(5_000_000),
                inventory_stack(1_300_000),
            ],
            ..InventoryState::default()
        };
        let mut state = GuiState {
            inventory_open: true,
            ..GuiState::default()
        };

        let equipment = inventory_slots(&gui, &inventory, InventoryTab::Equipment);
        assert_eq!(equipment.len(), 2);
        assert_eq!(equipment[0].inventory_index, 1);
        assert_eq!(equipment[0].visual_index, 0);
        assert_eq!(
            click_action(
                state,
                &gui,
                Some(&inventory),
                None,
                960.0,
                600.0,
                CanvasPoint { x: 213.0, y: 131.0 },
                PointerButton::Left,
            ),
            Some(GuiAction::Equip { inventory_index: 1 })
        );

        let action = click_action(
            state,
            &gui,
            Some(&inventory),
            None,
            960.0,
            600.0,
            CanvasPoint { x: 246.0, y: 104.0 },
            PointerButton::Left,
        )
        .expect("consume tab action");
        assert_eq!(
            action,
            GuiAction::SelectInventoryTab {
                tab: InventoryTab::Consume
            }
        );
        assert!(apply_local_action(&mut state, action));
        assert_eq!(state.inventory_tab, InventoryTab::Consume);
        assert_eq!(
            click_action(
                state,
                &gui,
                Some(&inventory),
                None,
                960.0,
                600.0,
                CanvasPoint { x: 213.0, y: 131.0 },
                PointerButton::Left,
            ),
            None
        );
        assert_eq!(
            click_action(
                state,
                &gui,
                Some(&inventory),
                None,
                960.0,
                600.0,
                CanvasPoint { x: 213.0, y: 131.0 },
                PointerButton::Right,
            ),
            Some(GuiAction::Drop { inventory_index: 0 })
        );

        state.inventory_tab = InventoryTab::Equipment;
        assert_eq!(
            click_action(
                state,
                &gui,
                Some(&inventory),
                None,
                960.0,
                600.0,
                CanvasPoint { x: 249.0, y: 131.0 },
                PointerButton::Left,
            ),
            None
        );
        assert_eq!(
            click_action(
                state,
                &gui,
                Some(&inventory),
                None,
                960.0,
                600.0,
                CanvasPoint { x: 249.0, y: 131.0 },
                PointerButton::Right,
            ),
            Some(GuiAction::Drop { inventory_index: 3 })
        );

        let cash = inventory_slots(&gui, &inventory, InventoryTab::Cash);
        assert_eq!(cash.len(), 1);
        assert_eq!(cash[0].inventory_index, 2);
    }

    #[test]
    fn inventory_grid_caps_visible_items_and_hover_uses_window_offsets() {
        let mut gui = gui_fixture();
        gui.items = (0..25)
            .map(|index| item_definition(1_040_000 + index, ItemCategory::Equipment))
            .collect();
        let inventory = InventoryState {
            capacity: 24,
            stacks: (0..25)
                .map(|index| inventory_stack(1_040_000 + index))
                .collect(),
            ..InventoryState::default()
        };
        let state = GuiState {
            inventory_open: true,
            ..GuiState::default()
        };

        let slots = inventory_slots(&gui, &inventory, InventoryTab::Equipment);
        assert_eq!(slots.len(), 24);
        assert_eq!(slots[23].inventory_index, 23);
        assert_eq!(
            hovered_inventory_item(state, &gui, &inventory, CanvasPoint { x: 213.0, y: 131.0 },)
                .map(|slot| slot.definition.item_id),
            Some(1_040_000)
        );
        assert!(
            hovered_inventory_item(
                GuiState::default(),
                &gui,
                &inventory,
                CanvasPoint { x: 213.0, y: 131.0 },
            )
            .is_none()
        );
    }

    #[test]
    fn inventory_tab_art_is_snapped_to_integer_pixels() {
        let region = region("inventory-tab-consume", 37.0, 22.0, 34.0, 19.0);
        let label = template("inventory-tab-consume-active-label", 13.0, 9.0);
        let background = template("inventory-tab-consume-active-background", 34.0, 19.0);

        assert_eq!(
            inventory_tab_template_position(&region, &label, false),
            (47.0, 27.0)
        );
        assert_eq!(
            inventory_tab_template_position(&region, &background, true),
            (37.0, 22.0)
        );
    }

    #[test]
    fn missing_item_definitions_are_sorted_and_deduplicated() {
        let gui = GameGui {
            items: vec![item_definition(1, ItemCategory::Equipment)],
            ..GameGui::default()
        };

        assert_eq!(
            missing_item_definition_ids(&gui, [3, 1, 2, 3, 0]),
            vec![2, 3]
        );
    }

    #[test]
    fn item_definition_refreshes_include_every_observed_item() {
        let gui = GameGui {
            items: vec![item_definition(1, ItemCategory::Equipment)],
            ..GameGui::default()
        };

        assert_eq!(
            item_definition_refresh_ids(&gui, [3, 1, 2, 3], false),
            Some(vec![1, 2, 3])
        );
        assert_eq!(item_definition_refresh_ids(&gui, [1], false), None);
        assert_eq!(
            item_definition_refresh_ids(&GameGui::default(), [], true),
            Some(Vec::new())
        );
    }

    #[test]
    fn css_pointer_coordinates_scale_to_the_canvas() {
        assert_eq!(
            canvas_point(480, 300, 960, 600, 480, 300),
            Some(CanvasPoint { x: 960.0, y: 600.0 })
        );
        assert_eq!(canvas_point(10, 10, 960, 600, 0, 300), None);
    }

    #[test]
    fn right_anchored_sprites_follow_the_viewport_edge() {
        let sprite = GuiSprite {
            x: 649.0,
            anchor_right: true,
            ..GuiSprite::default()
        };

        assert_eq!(sprite_screen_x(960.0, 800.0, &sprite), 809.0);
    }

    #[test]
    fn pressed_stat_sprite_is_visible_only_while_the_window_is_open() {
        let normal = sprite("stats", 634.0, 17.0, 28.0, 20.0);
        let pressed = sprite("stats-pressed", 634.0, 17.0, 28.0, 20.0);

        assert!(status_sprite_visible(GuiState::default(), &normal));
        assert!(!status_sprite_visible(GuiState::default(), &pressed));
        assert!(status_sprite_visible(
            GuiState {
                stats_open: true,
                ..GuiState::default()
            },
            &pressed
        ));
    }

    #[test]
    fn gauges_use_clamped_ratios_and_bracketed_values() {
        let stats = CharacterStats {
            hp: 25,
            max_hp: 50,
            mp: 8,
            max_mp: 5,
            experience: 3,
            experience_required: 15,
            ..CharacterStats::default()
        };

        let fills = gauge_fills(&stats);

        assert_eq!(fills[0].filled_width, 53.0);
        assert_eq!(fills[1].filled_width, 105.0);
        assert_eq!(fills[2].filled_width, 23.0);
        assert_eq!(gauge_labels(&stats), ["[25/50]", "[8/5]", "[3/15]"]);
        assert!(
            gauge_fills(&CharacterStats::default())
                .iter()
                .all(|fill| fill.filled_width == 0.0)
        );
    }

    #[test]
    fn key_settings_button_opens_and_closes_the_native_editor() {
        let gui = gui_fixture();
        let mut state = GuiState::default();

        let open = click_action(
            state,
            &gui,
            None,
            None,
            960.0,
            600.0,
            CanvasPoint { x: 700.0, y: 540.0 },
            PointerButton::Left,
        )
        .expect("key settings action");
        assert_eq!(open, GuiAction::ToggleKeyConfig);
        assert!(apply_local_action(&mut state, open));
        assert!(state.key_config_open);

        let close = click_action(
            state,
            &gui,
            None,
            None,
            960.0,
            600.0,
            CanvasPoint { x: 780.0, y: 70.0 },
            PointerButton::Left,
        )
        .expect("key settings close action");
        assert_eq!(close, GuiAction::CloseKeyConfig);
        assert!(apply_local_action(&mut state, close));
        assert!(!state.key_config_open);
    }

    #[test]
    fn skill_button_opens_and_closes_the_native_skill_book() {
        let gui = gui_fixture();
        let mut state = GuiState::default();

        let open = click_action(
            state,
            &gui,
            None,
            None,
            960.0,
            600.0,
            CanvasPoint { x: 670.0, y: 540.0 },
            PointerButton::Left,
        )
        .expect("skill action");
        assert_eq!(open, GuiAction::ToggleSkills);
        assert!(apply_local_action(&mut state, open));
        assert!(state.skills_open);

        let next = click_action(
            state,
            &gui,
            None,
            None,
            960.0,
            600.0,
            CanvasPoint { x: 165.0, y: 150.0 },
            PointerButton::Left,
        )
        .expect("next skill page action");
        assert_eq!(next, GuiAction::NextSkillPage);
        assert!(apply_local_action(&mut state, next));
        assert_eq!(state.skill_page, 1);

        let close = click_action(
            state,
            &gui,
            None,
            None,
            960.0,
            600.0,
            CanvasPoint { x: 181.0, y: 86.0 },
            PointerButton::Left,
        )
        .expect("skill close action");
        assert_eq!(close, GuiAction::CloseSkills);
        assert!(apply_local_action(&mut state, close));
        assert!(!state.skills_open);
    }

    #[test]
    fn learned_skills_allocate_and_use_from_native_rows() {
        let gui = gui_fixture();
        let mut book = skill_book_fixture(0, 1);
        let state = GuiState {
            skills_open: true,
            ..GuiState::default()
        };

        assert_eq!(
            click_action(
                state,
                &gui,
                None,
                Some(&book),
                960.0,
                600.0,
                CanvasPoint { x: 165.0, y: 186.0 },
                PointerButton::Left,
            ),
            Some(GuiAction::AllocateSkill { skill_id: 1_000 })
        );
        book.skills[0].level = 1;
        assert_eq!(
            click_action(
                state,
                &gui,
                None,
                Some(&book),
                960.0,
                600.0,
                CanvasPoint { x: 40.0, y: 180.0 },
                PointerButton::Left,
            ),
            Some(GuiAction::UseSkill { skill_id: 1_000 })
        );
    }

    #[test]
    fn zero_mastery_uses_the_definition_cap() {
        let mut mastered = skill_book_fixture(2, 1);
        mastered.skills[0].master_level = 2;
        assert!(!super::can_allocate_skill(&mastered, 1_000));

        mastered.skills[0].master_level = 0;
        assert!(super::can_allocate_skill(&mastered, 1_000));
    }

    #[test]
    fn invalid_layouts_are_rejected_at_the_client_boundary() {
        let gui = gui_fixture();
        let valid = gui.status_bar.expect("status bar");
        let mut invalid = valid.clone();
        invalid.width = f32::NAN;

        assert!(valid_layout(&valid));
        assert!(!valid_layout(&invalid));
    }

    fn gui_fixture() -> GameGui {
        GameGui {
            status_bar: Some(GuiLayout {
                width: 800.0,
                height: 80.0,
                background: Some(sprite("background", 0.0, 9.0, 800.0, 71.0)),
                sprites: vec![
                    sprite("equip", 574.0, 17.0, 28.0, 20.0),
                    sprite("inventory", 604.0, 17.0, 28.0, 20.0),
                    sprite("stats", 634.0, 17.0, 28.0, 20.0),
                    sprite("skills", 664.0, 17.0, 28.0, 20.0),
                    sprite("key-settings", 694.0, 17.0, 28.0, 20.0),
                ],
                ..GuiLayout::default()
            }),
            equipment_window: Some(GuiWindow {
                x: 20.0,
                y: 80.0,
                layout: Some(GuiLayout {
                    width: 175.0,
                    height: 304.0,
                    background: Some(sprite("equipment-background", 0.0, 0.0, 175.0, 304.0)),
                    sprites: vec![sprite("equipment-close", 138.0, 5.0, 32.0, 15.0)],
                    ..GuiLayout::default()
                }),
            }),
            inventory_window: Some(GuiWindow {
                x: 205.0,
                y: 80.0,
                layout: Some(GuiLayout {
                    width: 175.0,
                    height: 289.0,
                    background: Some(sprite("inventory-background", 0.0, 0.0, 175.0, 289.0)),
                    sprites: vec![sprite("inventory-close", 138.0, 5.0, 32.0, 15.0)],
                    regions: ["equipment", "consume", "install", "etc", "cash"]
                        .into_iter()
                        .enumerate()
                        .map(|(index, name)| {
                            region(
                                &format!("inventory-tab-{name}"),
                                3.0 + index as f32 * 34.0,
                                22.0,
                                34.0,
                                19.0,
                            )
                        })
                        .collect(),
                    ..GuiLayout::default()
                }),
            }),
            stat_window: Some(GuiWindow {
                x: 20.0,
                y: 80.0,
                layout: Some(GuiLayout {
                    width: 175.0,
                    height: 347.0,
                    background: Some(sprite("stat-background", 0.0, 0.0, 175.0, 347.0)),
                    sprites: vec![sprite("stat-close", 160.0, 5.0, 10.0, 10.0)],
                    ..GuiLayout::default()
                }),
            }),
            skill_window: Some(GuiWindow {
                x: 20.0,
                y: 80.0,
                layout: Some(GuiLayout {
                    width: 175.0,
                    height: 289.0,
                    background: Some(sprite("skill-background", 0.0, 0.0, 175.0, 289.0)),
                    sprites: vec![sprite("skill-close", 160.0, 5.0, 10.0, 10.0)],
                    sprite_templates: vec![
                        template("skill-row", 141.0, 35.0),
                        template("skill-point-up", 12.0, 12.0),
                    ],
                    regions: vec![
                        region("skill-list", 17.0, 94.0, 141.0, 140.0),
                        region("skill-page-previous", 80.0, 64.0, 18.0, 19.0),
                        region("skill-page-next", 139.0, 64.0, 18.0, 19.0),
                    ],
                }),
            }),
            key_config_window: Some(GuiWindow {
                x: 165.0,
                y: 60.0,
                layout: Some(GuiLayout {
                    width: 629.0,
                    height: 373.0,
                    background: Some(sprite("key-config-background", 0.0, 0.0, 629.0, 373.0)),
                    sprites: vec![sprite("key-config-close", 612.0, 6.0, 12.0, 12.0)],
                    ..GuiLayout::default()
                }),
            }),
            items: vec![item_definition(1_040_003, ItemCategory::Equipment)],
            ..GameGui::default()
        }
    }

    fn sprite(
        name: &str,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    ) -> GuiSprite {
        GuiSprite {
            name: name.to_owned(),
            asset_id: format!("asset-{name}"),
            x,
            y,
            width,
            height,
            anchor_right: false,
            origin_x: 0.0,
            origin_y: 0.0,
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

    fn template(
        name: &str,
        width: f32,
        height: f32,
    ) -> GuiSpriteTemplate {
        GuiSpriteTemplate {
            name: name.to_owned(),
            asset_id: format!("asset-{name}"),
            width,
            height,
            origin_x: 0.0,
            origin_y: 0.0,
        }
    }

    fn item_definition(
        item_id: u32,
        category: ItemCategory,
    ) -> ItemDefinition {
        ItemDefinition {
            item_id,
            category: category as i32,
            slot: if category == ItemCategory::Equipment {
                EquipmentSlot::Top as i32
            } else {
                0
            },
            appearance_supported: category == ItemCategory::Equipment,
            ..ItemDefinition::default()
        }
    }

    fn inventory_stack(item_id: u32) -> InventoryItemStack {
        InventoryItemStack {
            item_id,
            quantity: 1,
            expires_at_unix_ms: 0,
        }
    }

    fn skill_book_fixture(
        level: u32,
        available_points: u32,
    ) -> SkillBook {
        SkillBook {
            job_id: 0,
            available_points,
            skills: vec![PlayerSkill {
                definition: Some(SkillDefinition {
                    skill_id: 1_000,
                    max_level: 3,
                    icon_asset_id: "skill-icon".to_owned(),
                    icon_width: 32.0,
                    icon_height: 32.0,
                    ..SkillDefinition::default()
                }),
                level,
                master_level: 0,
            }],
            ..SkillBook::default()
        }
    }
}
