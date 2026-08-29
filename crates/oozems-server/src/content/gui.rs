use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::sync::RwLock;

use oozems_proto::v1::AnimationFrame;
use oozems_proto::v1::AssetDescriptor;
use oozems_proto::v1::GameGui;
use oozems_proto::v1::GuiLayout;
use oozems_proto::v1::GuiSpriteSource;
use oozems_proto::v1::GuiWindow;
use oozems_proto::v1::GuiWindowDefinition;
use oozems_proto::v1::KeySlot;
use oozems_proto::v1::MouseCursor;
use oozems_proto::v1::NpcFrame;
use sha2::Digest;
use sha2::Sha256;
use thiserror::Error;
use wz_reader::WzNodeArc;
use wz_reader::WzNodeCast;

use super::WzAsset;
use super::wz::WzContentError;
use super::wz::archive_fingerprint;
use super::wz::child;
use super::wz::int_value;
use super::wz::node_name;
use super::wz::open_archive;
use super::wz::parse;
use super::wz::sorted_children;
use super::wz::vector_value;
use super::wz::wrap_archive_root;

mod layout;

use layout::compose_cash_shop_window;
use layout::compose_death_notice_window;
use layout::compose_equipment_window;
use layout::compose_inventory_window;
use layout::compose_key_config;
use layout::compose_npc_dialog_window;
use layout::compose_shop_window;
use layout::compose_skill_window;
use layout::compose_stat_window;
use layout::compose_status_bar;
use layout::place_sprite;
use layout::sprite_template;
use layout::validate_layout;

const GUI_ARCHIVE: &str = "UI.wz";
const BASIC_IMAGE: &str = "Basic.img";
const NORMAL_CURSOR_ID: &str = "0";
const INTERACTIVE_CURSOR_ID: &str = "1";
const STATUS_BAR_IMAGE: &str = "StatusBar.img";
const CASH_SHOP_IMAGE: &str = "CashShop.img";
const UI_WINDOW_IMAGE: &str = "UIWindow.img";
const DEFAULT_CURSOR_FRAME_DELAY_MS: u32 = 100;
const DEFAULT_QUEST_ICON_FRAME_DELAY_MS: u32 = 100;
const QUEST_AVAILABLE_ICON_ID: &str = "0";
const QUEST_READY_ICON_ID: &str = "2";
const EQUIPMENT_WINDOW_X: f32 = 20.0;
const EQUIPMENT_WINDOW_Y: f32 = 80.0;
const INVENTORY_WINDOW_X: f32 = 205.0;
const INVENTORY_WINDOW_Y: f32 = 80.0;

pub struct GuiContent {
    _base: WzNodeArc,
    gui: GameGui,
    fingerprint: String,
    assets: RwLock<HashMap<String, Arc<WzAsset>>>,
}

#[derive(Debug, Error)]
pub enum GuiContentError {
    #[error(transparent)]
    Wz(#[from] WzContentError),
    #[error(transparent)]
    Layout(#[from] oozems_ui_layout::LayoutError),
    #[error("UI.wz is invalid: {message}")]
    Invalid { message: String },
    #[error("internal GUI content lock was poisoned while accessing {context}")]
    Lock { context: &'static str },
}

#[derive(Clone, Debug)]
struct SourceSprite {
    name: String,
    asset_id: String,
    width: f32,
    height: f32,
    origin_x: f32,
    origin_y: f32,
}

struct StatusBarSources {
    background: SourceSprite,
    overlay: SourceSprite,
    gauge: SourceSprite,
    gauge_graduation: SourceSprite,
    quick_slots: SourceSprite,
    equip: SourceSprite,
    equip_pressed: SourceSprite,
    inventory: SourceSprite,
    inventory_pressed: SourceSprite,
    stats: SourceSprite,
    stats_pressed: SourceSprite,
    skills: SourceSprite,
    skills_pressed: SourceSprite,
    key_settings: SourceSprite,
    key_settings_pressed: SourceSprite,
    quick_slot_toggle: SourceSprite,
    cash_shop: SourceSprite,
    key_references: Vec<SourceSprite>,
}

struct StatWindowSources {
    background: SourceSprite,
    close: SourceSprite,
    job: SourceSprite,
    ability_up: SourceSprite,
    ability_up_disabled: SourceSprite,
    auto_assign: SourceSprite,
    detail: SourceSprite,
}

struct EquipmentWindowSources {
    background: SourceSprite,
    close: SourceSprite,
    detail: SourceSprite,
}

struct InventoryTabSources {
    active_background: SourceSprite,
    inactive_background: SourceSprite,
    active_label: SourceSprite,
    inactive_label: SourceSprite,
}

struct InventoryWindowSources {
    background: SourceSprite,
    close: SourceSprite,
    tabs: Vec<InventoryTabSources>,
    gather: SourceSprite,
    sort: SourceSprite,
    expand: SourceSprite,
    locked_slot: SourceSprite,
}

struct SkillTabSources {
    enabled: SourceSprite,
    disabled: SourceSprite,
}

struct SkillWindowSources {
    background: SourceSprite,
    close: SourceSprite,
    row: SourceSprite,
    selected_row: SourceSprite,
    point_up: SourceSprite,
    point_up_hover: SourceSprite,
    point_up_pressed: SourceSprite,
    point_up_disabled: SourceSprite,
    tabs: Vec<SkillTabSources>,
}

struct KeyConfigSources {
    background: SourceSprite,
    close: SourceSprite,
    actions: Vec<(crate::keymap::KeyActionSpec, SourceSprite)>,
}

struct NpcDialogSources {
    top: SourceSprite,
    center: SourceSprite,
    bottom: SourceSprite,
    close: SourceSprite,
    ok: SourceSprite,
    next: SourceSprite,
    previous: SourceSprite,
    accept: SourceSprite,
    decline: SourceSprite,
    choice: SourceSprite,
    choice_selected: SourceSprite,
}

struct ShopWindowSources {
    background: SourceSprite,
    selection: SourceSprite,
    meso: SourceSprite,
    buy: SourceSprite,
    sell: SourceSprite,
    exit: SourceSprite,
}

struct CashShopWindowSources {
    background: SourceSprite,
    preview: SourceSprite,
    category_tab: SourceSprite,
    item_card: SourceSprite,
    buy: SourceSprite,
    gift_disabled: SourceSprite,
    exit: SourceSprite,
}

struct DeathNoticeSources {
    background: SourceSprite,
    ok: SourceSprite,
}

impl GuiContent {
    pub fn open_optional(
        directory: &Path,
        layout_directory: &Path,
    ) -> Result<Option<Self>, GuiContentError> {
        let path = directory.join(GUI_ARCHIVE);
        if !path
            .try_exists()
            .map_err(|source| WzContentError::Metadata {
                path: path.clone(),
                source,
            })?
        {
            tracing::warn!(path = %path.display(), "UI.wz is absent; the client will use its fallback HUD");
            return Ok(None);
        }

        let root = open_archive(&path)?;
        let base = wrap_archive_root(&root)?;
        parse(&root, format!("{} root", path.display()))?;
        let status_bar = required_child(&root, STATUS_BAR_IMAGE)?;
        parse(
            &status_bar,
            format!("{} {STATUS_BAR_IMAGE}", path.display()),
        )?;
        let ui_window = required_child(&root, UI_WINDOW_IMAGE)?;
        parse(&ui_window, format!("{} {UI_WINDOW_IMAGE}", path.display()))?;
        let cash_shop = required_child(&root, CASH_SHOP_IMAGE)?;
        parse(&cash_shop, format!("{} {CASH_SHOP_IMAGE}", path.display()))?;
        let basic = required_child(&root, BASIC_IMAGE)?;
        parse(&basic, format!("{} {BASIC_IMAGE}", path.display()))?;

        let definitions = oozems_ui_layout::load_directory(layout_directory)?;
        let mut content = Self {
            _base: base,
            gui: GameGui::default(),
            fingerprint: archive_fingerprint(&path)?,
            assets: RwLock::new(HashMap::new()),
        };
        content.gui = build_game_gui(
            &content,
            &root,
            &status_bar,
            &ui_window,
            &cash_shop,
            &basic,
            &definitions,
        )?;

        tracing::info!(
            path = %path.display(),
            assets = content.gui.assets.len(),
            authored_layouts = definitions.len(),
            "WZ GUI source ready"
        );
        Ok(Some(content))
    }

    pub fn game_gui(&self) -> GameGui {
        self.gui.clone()
    }

    pub fn get_asset(
        &self,
        asset_id: &str,
    ) -> Option<Arc<WzAsset>> {
        self.assets.read().ok()?.get(asset_id).cloned()
    }

    fn register_asset(
        &self,
        source_path: &str,
        node: &WzNodeArc,
    ) -> Result<AssetDescriptor, GuiContentError> {
        let version = hex::encode(Sha256::digest(
            format!("gui\0{}\0{source_path}", self.fingerprint).as_bytes(),
        ));
        let id = format!("wz-{version}");
        let asset = Arc::new(WzAsset::new(id.clone(), Arc::clone(node)));
        self.assets
            .write()
            .map_err(|_| lock_error("GUI asset registry"))?
            .entry(id.clone())
            .or_insert(asset);

        Ok(AssetDescriptor {
            id,
            url: format!("/wz-assets/{version}.png"),
        })
    }
}

fn build_game_gui(
    content: &GuiContent,
    root: &WzNodeArc,
    status_bar: &WzNodeArc,
    ui_window: &WzNodeArc,
    cash_shop: &WzNodeArc,
    basic: &WzNodeArc,
    definitions: &[GuiWindowDefinition],
) -> Result<GameGui, GuiContentError> {
    let (status_sources, mut assets) = load_status_bar_sources(content, status_bar)?;
    let (normal_cursor, normal_cursor_assets) =
        load_mouse_cursor(content, basic, NORMAL_CURSOR_ID)?;
    assets.extend(normal_cursor_assets);
    let (interactive_cursor, interactive_cursor_assets) =
        load_mouse_cursor(content, basic, INTERACTIVE_CURSOR_ID)?;
    assets.extend(interactive_cursor_assets);
    let mut status_bar = compose_status_bar(&status_sources)?;
    add_runtime_regions("status-bar", &mut status_bar);
    let (stat_sources, stat_assets) = load_stat_window_sources(content, ui_window)?;
    let mut stat_window = compose_stat_window(&stat_sources)?;
    add_window_runtime_regions("stats", &mut stat_window);
    assets.extend(stat_assets);
    let (equipment_sources, equipment_assets) = load_equipment_window_sources(content, ui_window)?;
    let mut equipment_window =
        compose_equipment_window(&equipment_sources, EQUIPMENT_WINDOW_X, EQUIPMENT_WINDOW_Y)?;
    add_window_runtime_regions("equipment", &mut equipment_window);
    assets.extend(equipment_assets);
    let (inventory_sources, inventory_assets) = load_inventory_window_sources(content, ui_window)?;
    let mut inventory_window =
        compose_inventory_window(&inventory_sources, INVENTORY_WINDOW_X, INVENTORY_WINDOW_Y)?;
    add_window_runtime_regions("inventory", &mut inventory_window);
    assets.extend(inventory_assets);
    let (skill_sources, skill_assets) = load_skill_window_sources(content, ui_window)?;
    let mut skill_window = compose_skill_window(&skill_sources)?;
    add_window_runtime_regions("skills", &mut skill_window);
    assets.extend(skill_assets);
    let (key_config_sources, key_config_assets) = load_key_config_sources(content, ui_window)?;
    let (mut key_config_window, key_actions) = compose_key_config(&key_config_sources)?;
    add_window_runtime_regions("key-config", &mut key_config_window);
    assets.extend(key_config_assets);
    let (npc_dialog_sources, npc_dialog_assets) = load_npc_dialog_sources(content, ui_window)?;
    let mut npc_dialog_window = compose_npc_dialog_window(&npc_dialog_sources)?;
    add_window_runtime_regions("npc-dialog", &mut npc_dialog_window);
    assets.extend(npc_dialog_assets);
    let (shop_sources, shop_assets) = load_shop_window_sources(content, ui_window)?;
    let mut shop_window = compose_shop_window(&shop_sources)?;
    add_window_runtime_regions("shop", &mut shop_window);
    assets.extend(shop_assets);
    let (cash_shop_sources, cash_shop_assets) = load_cash_shop_window_sources(content, cash_shop)?;
    let mut cash_shop_window = compose_cash_shop_window(&cash_shop_sources)?;
    add_window_runtime_regions("cash-shop", &mut cash_shop_window);
    assets.extend(cash_shop_assets);
    let (death_notice_sources, death_notice_assets) = load_death_notice_sources(content, basic)?;
    let mut death_notice_window = compose_death_notice_window(&death_notice_sources)?;
    add_window_runtime_regions("death-notice", &mut death_notice_window);
    assets.extend(death_notice_assets);
    let (quest_available_frames, quest_available_assets) =
        load_quest_icon_frames(content, ui_window, QUEST_AVAILABLE_ICON_ID)?;
    assets.extend(quest_available_assets);
    let (quest_ready_frames, quest_ready_assets) =
        load_quest_icon_frames(content, ui_window, QUEST_READY_ICON_ID)?;
    assets.extend(quest_ready_assets);
    let mut gui = GameGui {
        status_bar: Some(status_bar),
        assets: Vec::new(),
        stat_window: Some(stat_window),
        equipment_window: Some(equipment_window),
        inventory_window: Some(inventory_window),
        items: Vec::new(),
        key_config_window: Some(key_config_window),
        key_actions,
        key_slots: crate::keymap::SLOTS
            .iter()
            .map(|slot| KeySlot {
                code: slot.code.to_owned(),
                x: slot.x,
                y: slot.y,
                width: slot.width,
                height: slot.height,
            })
            .collect(),
        skill_window: Some(skill_window),
        npc_dialog_window: Some(npc_dialog_window),
        shop_window: Some(shop_window),
        cash_shop_window: Some(cash_shop_window),
        quest_available_frames,
        quest_ready_frames,
        death_notice_window: Some(death_notice_window),
        death_tomb_frames: Vec::new(),
        level_up_frames: Vec::new(),
        quest_tracker: Vec::new(),
        normal_cursor,
        interactive_cursor,
    };
    for definition in definitions {
        let (window, definition_assets) = compose_window_definition(content, root, definition)?;
        assets.extend(definition_assets);
        install_window_definition(&mut gui, definition, window);
    }
    let mut asset_ids = HashSet::new();
    assets.retain(|asset| asset_ids.insert(asset.id.clone()));
    gui.assets = assets;
    Ok(gui)
}

fn compose_window_definition(
    content: &GuiContent,
    root: &WzNodeArc,
    definition: &GuiWindowDefinition,
) -> Result<(GuiWindow, Vec<AssetDescriptor>), GuiContentError> {
    let mut assets = Vec::new();
    let background_definition =
        definition
            .background
            .as_ref()
            .ok_or_else(|| GuiContentError::Invalid {
                message: format!(
                    "authored GUI window {:?} has no background",
                    definition.name
                ),
            })?;
    let background_source =
        load_authored_source(content, root, background_definition, &mut assets)?;
    let background = place_sprite(
        &background_source,
        background_definition.x,
        background_definition.y,
        background_definition.anchor_right,
    );
    let width = if definition.width > 0.0 {
        definition.width
    } else {
        background.width
    };
    let height = if definition.height > 0.0 {
        definition.height
    } else {
        background.height
    };
    let sprites = definition
        .sprites
        .iter()
        .map(|sprite| {
            let source = load_authored_source(content, root, sprite, &mut assets)?;
            let x = if sprite.pin_right {
                width - source.width - sprite.right
            } else {
                sprite.x
            };
            let y = if sprite.pin_bottom {
                height - source.height - sprite.bottom
            } else {
                sprite.y
            };
            Ok(place_sprite(&source, x, y, sprite.anchor_right))
        })
        .collect::<Result<Vec<_>, GuiContentError>>()?;
    let templates = definition
        .sprite_templates
        .iter()
        .map(|template| {
            let source_definition = GuiSpriteSource {
                name: template.name.clone(),
                wz_path: template.wz_path.clone(),
                ..GuiSpriteSource::default()
            };
            load_authored_source(content, root, &source_definition, &mut assets).map(|source| {
                let mut runtime_template = sprite_template(&source);
                runtime_template.offset_x = template.offset_x;
                runtime_template.offset_y = template.offset_y;
                runtime_template
            })
        })
        .collect::<Result<Vec<_>, GuiContentError>>()?;
    let layout = GuiLayout {
        width,
        height,
        background: Some(background),
        sprites,
        sprite_templates: templates,
        regions: definition.regions.clone(),
    };
    validate_layout(&layout)?;
    Ok((
        GuiWindow {
            x: definition.x,
            y: definition.y,
            layout: Some(layout),
        },
        assets,
    ))
}

fn add_window_runtime_regions(
    name: &str,
    window: &mut GuiWindow,
) {
    if let Some(layout) = window.layout.as_mut() {
        add_runtime_regions(name, layout);
    }
}

fn add_runtime_regions(
    name: &str,
    layout: &mut GuiLayout,
) {
    oozems_ui_layout::add_missing_runtime_regions(
        name,
        layout.width,
        layout.height,
        &mut layout.regions,
    );
}

fn load_authored_source(
    content: &GuiContent,
    root: &WzNodeArc,
    definition: &GuiSpriteSource,
    assets: &mut Vec<AssetDescriptor>,
) -> Result<SourceSprite, GuiContentError> {
    let (image_name, path) =
        definition
            .wz_path
            .split_once('/')
            .ok_or_else(|| GuiContentError::Invalid {
                message: format!(
                    "authored GUI element {:?} has invalid WZ path {:?}",
                    definition.name, definition.wz_path
                ),
            })?;
    let image = required_child(root, image_name)?;
    parse(&image, format!("UI.wz {image_name}"))?;
    let path = path.split('/').collect::<Vec<_>>();
    load_source(content, &image, image_name, &definition.name, &path, assets)
}

fn install_window_definition(
    gui: &mut GameGui,
    definition: &GuiWindowDefinition,
    window: GuiWindow,
) {
    match definition.name.as_str() {
        "status-bar" => gui.status_bar = window.layout,
        "stats" => gui.stat_window = Some(window),
        "equipment" => gui.equipment_window = Some(window),
        "inventory" => gui.inventory_window = Some(window),
        "skills" => gui.skill_window = Some(window),
        "key-config" => {
            if let Some(layout) = window.layout.as_ref() {
                for action in &mut gui.key_actions {
                    let Some(icon) = action.icon.as_mut() else {
                        continue;
                    };
                    if let Some(sprite) = layout
                        .sprites
                        .iter()
                        .find(|sprite| sprite.name == icon.name)
                    {
                        *icon = sprite.clone();
                    }
                }
                for slot in &mut gui.key_slots {
                    let region_name = format!("key-config-slot-{}", slot.code);
                    let Some(region) = layout
                        .regions
                        .iter()
                        .find(|region| region.name == region_name)
                    else {
                        continue;
                    };
                    slot.x = region.x;
                    slot.y = region.y;
                    slot.width = region.width;
                    slot.height = region.height;
                }
            }
            gui.key_config_window = Some(window);
        }
        "npc-dialog" => gui.npc_dialog_window = Some(window),
        "shop" => gui.shop_window = Some(window),
        "cash-shop" => gui.cash_shop_window = Some(window),
        "death-notice" => gui.death_notice_window = Some(window),
        _ => unreachable!("validated GUI window name"),
    }
}

fn load_quest_icon_frames(
    content: &GuiContent,
    ui_window: &WzNodeArc,
    icon_id: &str,
) -> Result<(Vec<NpcFrame>, Vec<AssetDescriptor>), GuiContentError> {
    let animation = required_path(ui_window, &["QuestIcon", icon_id])?;
    let mut frames = Vec::new();
    let mut assets = Vec::new();
    for node in sorted_children(&animation)? {
        let frame_name = node_name(&node)?;
        if frame_name.parse::<u32>().is_err() {
            continue;
        }
        let path = ["QuestIcon", icon_id, frame_name.as_str()];
        let source = load_source(
            content,
            ui_window,
            UI_WINDOW_IMAGE,
            &format!("quest-icon-{icon_id}-{frame_name}"),
            &path,
            &mut assets,
        )?;
        let delay_ms = int_value(&node, "delay")?
            .and_then(|delay| u32::try_from(delay).ok())
            .filter(|delay| *delay > 0)
            .unwrap_or(DEFAULT_QUEST_ICON_FRAME_DELAY_MS);
        frames.push(NpcFrame {
            asset_id: source.asset_id,
            width: source.width,
            height: source.height,
            origin_x: source.origin_x,
            origin_y: source.origin_y,
            delay_ms,
        });
    }
    if frames.is_empty() {
        return invalid(format!(
            "{UI_WINDOW_IMAGE}/QuestIcon/{icon_id} has no animation frames"
        ));
    }
    Ok((frames, assets))
}

fn load_npc_dialog_sources(
    content: &GuiContent,
    ui_window: &WzNodeArc,
) -> Result<(NpcDialogSources, Vec<AssetDescriptor>), GuiContentError> {
    let mut assets = Vec::new();
    let mut load = |name: &str, path: &[&str]| {
        load_source(content, ui_window, UI_WINDOW_IMAGE, name, path, &mut assets)
    };
    let sources = NpcDialogSources {
        top: load("npc-dialog-top", &["UtilDlgEx", "t"])?,
        center: load("npc-dialog-center", &["UtilDlgEx", "c"])?,
        bottom: load("npc-dialog-bottom", &["UtilDlgEx", "s"])?,
        close: load("npc-dialog-close", &["UtilDlgEx", "BtClose", "normal", "0"])?,
        ok: load("npc-dialog-ok", &["UtilDlgEx", "BtOK", "normal", "0"])?,
        next: load("npc-dialog-next", &["UtilDlgEx", "BtNext", "normal", "0"])?,
        previous: load(
            "npc-dialog-previous",
            &["UtilDlgEx", "BtPrev", "normal", "0"],
        )?,
        accept: load("npc-dialog-accept", &["UtilDlgEx", "BtQYes", "normal", "0"])?,
        decline: load("npc-dialog-decline", &["UtilDlgEx", "BtQNo", "normal", "0"])?,
        choice: load("npc-dialog-choice", &["UtilDlgEx", "dot0"])?,
        choice_selected: load("npc-dialog-choice-selected", &["UtilDlgEx", "dot1"])?,
    };
    Ok((sources, assets))
}

fn load_shop_window_sources(
    content: &GuiContent,
    ui_window: &WzNodeArc,
) -> Result<(ShopWindowSources, Vec<AssetDescriptor>), GuiContentError> {
    let mut assets = Vec::new();
    let mut load = |name: &str, path: &[&str]| {
        load_source(content, ui_window, UI_WINDOW_IMAGE, name, path, &mut assets)
    };
    let sources = ShopWindowSources {
        background: load("shop-background", &["Shop", "backgrnd"])?,
        selection: load("shop-selection", &["Shop", "select"])?,
        meso: load("shop-meso", &["Shop", "meso"])?,
        buy: load("shop-buy", &["Shop", "BtBuy", "normal", "0"])?,
        sell: load("shop-sell", &["Shop", "BtSell", "normal", "0"])?,
        exit: load("shop-exit", &["Shop", "BtExit", "normal", "0"])?,
    };
    Ok((sources, assets))
}

fn load_cash_shop_window_sources(
    content: &GuiContent,
    cash_shop: &WzNodeArc,
) -> Result<(CashShopWindowSources, Vec<AssetDescriptor>), GuiContentError> {
    let mut assets = Vec::new();
    let mut load = |name: &str, path: &[&str]| {
        load_source(content, cash_shop, CASH_SHOP_IMAGE, name, path, &mut assets)
    };
    let sources = CashShopWindowSources {
        category_tab: load("cash-shop-category-tab", &["CSTab", "Tab", "1"])?,
        gift_disabled: load(
            "cash-shop-gift-disabled",
            &["CSList", "BtGift", "disabled", "0"],
        )?,
        background: load("cash-shop-background", &["Base", "backgrnd"])?,
        preview: load("cash-shop-preview", &["Base", "Preview", "0"])?,
        item_card: load("cash-shop-item-card", &["CSList", "Base"])?,
        buy: load("cash-shop-buy", &["CSList", "BtBuy", "normal", "0"])?,
        exit: load("cash-shop-exit", &["CSStatus", "BtExit", "normal", "0"])?,
    };
    Ok((sources, assets))
}

fn load_death_notice_sources(
    content: &GuiContent,
    basic: &WzNodeArc,
) -> Result<(DeathNoticeSources, Vec<AssetDescriptor>), GuiContentError> {
    let mut assets = Vec::new();
    let mut load = |name: &str, path: &[&str]| {
        load_source(content, basic, BASIC_IMAGE, name, path, &mut assets)
    };
    let sources = DeathNoticeSources {
        background: load("death-notice-background", &["Notice", "backgrnd"])?,
        ok: load("death-notice-ok", &["BtOK", "normal", "0"])?,
    };
    Ok((sources, assets))
}

fn load_status_bar_sources(
    content: &GuiContent,
    status_bar: &WzNodeArc,
) -> Result<(StatusBarSources, Vec<AssetDescriptor>), GuiContentError> {
    let mut assets = Vec::new();
    let background = load_status_source(
        content,
        status_bar,
        "background",
        &["base", "backgrnd"],
        &mut assets,
    )?;
    let overlay = load_status_source(
        content,
        status_bar,
        "status-overlay",
        &["base", "backgrnd2"],
        &mut assets,
    )?;
    let gauge = load_status_source(content, status_bar, "gauge", &["gauge", "bar"], &mut assets)?;
    let gauge_graduation = load_status_source(
        content,
        status_bar,
        "gauge-graduation",
        &["gauge", "graduation"],
        &mut assets,
    )?;
    let quick_slots = load_status_source(
        content,
        status_bar,
        "quick-slots",
        &["base", "quickSlot"],
        &mut assets,
    )?;
    let equip = load_normal_button(content, status_bar, "equip", "EquipKey", &mut assets)?;
    let equip_pressed = load_button_state(
        content,
        status_bar,
        "equip-pressed",
        "EquipKey",
        "pressed",
        &mut assets,
    )?;
    let inventory = load_normal_button(content, status_bar, "inventory", "InvenKey", &mut assets)?;
    let inventory_pressed = load_button_state(
        content,
        status_bar,
        "inventory-pressed",
        "InvenKey",
        "pressed",
        &mut assets,
    )?;
    let stats = load_normal_button(content, status_bar, "stats", "StatKey", &mut assets)?;
    let stats_pressed = load_button_state(
        content,
        status_bar,
        "stats-pressed",
        "StatKey",
        "pressed",
        &mut assets,
    )?;
    let skills = load_normal_button(content, status_bar, "skills", "SkillKey", &mut assets)?;
    let skills_pressed = load_button_state(
        content,
        status_bar,
        "skills-pressed",
        "SkillKey",
        "pressed",
        &mut assets,
    )?;
    let key_settings =
        load_normal_button(content, status_bar, "key-settings", "KeySet", &mut assets)?;
    let key_settings_pressed = load_button_state(
        content,
        status_bar,
        "key-settings-pressed",
        "KeySet",
        "pressed",
        &mut assets,
    )?;
    let quick_slot_toggle = load_normal_button(
        content,
        status_bar,
        "quick-slot-toggle",
        "QuickSlotD",
        &mut assets,
    )?;
    let cash_shop = load_normal_button(content, status_bar, "cash-shop", "BtShop", &mut assets)?;
    let mut key_references = Vec::with_capacity(8);
    for index in 0..8 {
        let index = index.to_string();
        key_references.push(load_status_source(
            content,
            status_bar,
            &format!("key-{index}"),
            &["key", &index],
            &mut assets,
        )?);
    }

    Ok((
        StatusBarSources {
            background,
            overlay,
            gauge,
            gauge_graduation,
            quick_slots,
            equip,
            equip_pressed,
            inventory,
            inventory_pressed,
            stats,
            stats_pressed,
            skills,
            skills_pressed,
            key_settings,
            key_settings_pressed,
            quick_slot_toggle,
            cash_shop,
            key_references,
        },
        assets,
    ))
}

fn load_key_config_sources(
    content: &GuiContent,
    ui_window: &WzNodeArc,
) -> Result<(KeyConfigSources, Vec<AssetDescriptor>), GuiContentError> {
    let mut assets = Vec::new();
    let background = load_source(
        content,
        ui_window,
        UI_WINDOW_IMAGE,
        "key-config-background",
        &["KeyConfig", "backgrnd"],
        &mut assets,
    )?;
    let close = load_source(
        content,
        ui_window,
        UI_WINDOW_IMAGE,
        "key-config-close",
        &["KeyConfig", "BtClose", "normal", "0"],
        &mut assets,
    )?;
    let mut actions = Vec::with_capacity(crate::keymap::ACTIONS.len());
    for spec in crate::keymap::ACTIONS {
        let source = load_source(
            content,
            ui_window,
            UI_WINDOW_IMAGE,
            &format!("key-action-{}", spec.icon_id),
            &["KeyConfig", "icon", spec.icon_id],
            &mut assets,
        )?;
        actions.push((*spec, source));
    }
    Ok((
        KeyConfigSources {
            background,
            close,
            actions,
        },
        assets,
    ))
}

fn load_equipment_window_sources(
    content: &GuiContent,
    ui_window: &WzNodeArc,
) -> Result<(EquipmentWindowSources, Vec<AssetDescriptor>), GuiContentError> {
    let mut assets = Vec::new();
    let background = load_source(
        content,
        ui_window,
        UI_WINDOW_IMAGE,
        "equipment-background",
        &["Equip", "backgrnd"],
        &mut assets,
    )?;
    let close = load_source(
        content,
        ui_window,
        UI_WINDOW_IMAGE,
        "equipment-close",
        &["BtUIClose", "normal", "0"],
        &mut assets,
    )?;
    let detail = load_source(
        content,
        ui_window,
        UI_WINDOW_IMAGE,
        "equipment-detail-disabled",
        &["Equip", "BtDetail", "disabled", "0"],
        &mut assets,
    )?;
    Ok((
        EquipmentWindowSources {
            background,
            close,
            detail,
        },
        assets,
    ))
}

fn load_inventory_window_sources(
    content: &GuiContent,
    ui_window: &WzNodeArc,
) -> Result<(InventoryWindowSources, Vec<AssetDescriptor>), GuiContentError> {
    let mut assets = Vec::new();
    let (background, close, gather, sort, expand, locked_slot) = {
        let mut load = |name: &str, path: &[&str]| {
            load_source(content, ui_window, UI_WINDOW_IMAGE, name, path, &mut assets)
        };
        (
            load("inventory-background", &["Item", "backgrnd"])?,
            load("inventory-close", &["BtUIClose", "normal", "0"])?,
            load(
                "inventory-gather-disabled",
                &["Item", "BtGather", "disabled", "0"],
            )?,
            load(
                "inventory-sort-disabled",
                &["Item", "BtSort", "disabled", "0"],
            )?,
            load(
                "inventory-expand-disabled",
                &["Item", "BtFull", "disabled", "0"],
            )?,
            load("inventory-locked-slot", &["Item", "disabled"])?,
        )
    };
    let mut tabs = Vec::with_capacity(5);
    for (index, name) in ["equipment", "consume", "install", "etc", "cash"]
        .into_iter()
        .enumerate()
    {
        let index = index.to_string();
        let mut load = |part: &str, path: &[&str]| {
            load_source(
                content,
                ui_window,
                UI_WINDOW_IMAGE,
                &format!("inventory-tab-{name}-{part}"),
                path,
                &mut assets,
            )
        };
        tabs.push(InventoryTabSources {
            active_background: load("active-background", &["Item", "New", "Tab1", &index])?,
            inactive_background: load("inactive-background", &["Item", "New", "Tab0", &index])?,
            active_label: load("active-label", &["Item", "Tab", "enabled", &index])?,
            inactive_label: load("inactive-label", &["Item", "Tab", "disabled", &index])?,
        });
    }
    Ok((
        InventoryWindowSources {
            background,
            close,
            tabs,
            gather,
            sort,
            expand,
            locked_slot,
        },
        assets,
    ))
}

fn load_skill_window_sources(
    content: &GuiContent,
    ui_window: &WzNodeArc,
) -> Result<(SkillWindowSources, Vec<AssetDescriptor>), GuiContentError> {
    let mut assets = Vec::new();
    let background = load_source(
        content,
        ui_window,
        UI_WINDOW_IMAGE,
        "skill-background",
        &["Skill", "backgrnd"],
        &mut assets,
    )?;
    let close = load_source(
        content,
        ui_window,
        UI_WINDOW_IMAGE,
        "skill-close",
        &["BtUIClose", "normal", "0"],
        &mut assets,
    )?;
    let row = load_source(
        content,
        ui_window,
        UI_WINDOW_IMAGE,
        "skill-row",
        &["Skill", "skill0"],
        &mut assets,
    )?;
    let selected_row = load_source(
        content,
        ui_window,
        UI_WINDOW_IMAGE,
        "skill-row-selected",
        &["Skill", "skill1"],
        &mut assets,
    )?;
    let point_up = load_source(
        content,
        ui_window,
        UI_WINDOW_IMAGE,
        "skill-point-up",
        &["Skill", "BtSpUp", "normal", "0"],
        &mut assets,
    )?;
    let point_up_hover = load_source(
        content,
        ui_window,
        UI_WINDOW_IMAGE,
        "skill-point-up-hover",
        &["Skill", "BtSpUp", "mouseOver", "0"],
        &mut assets,
    )?;
    let point_up_pressed = load_source(
        content,
        ui_window,
        UI_WINDOW_IMAGE,
        "skill-point-up-pressed",
        &["Skill", "BtSpUp", "pressed", "0"],
        &mut assets,
    )?;
    let point_up_disabled = load_source(
        content,
        ui_window,
        UI_WINDOW_IMAGE,
        "skill-point-up-disabled",
        &["Skill", "BtSpUp", "disabled", "0"],
        &mut assets,
    )?;
    let mut tabs = Vec::with_capacity(5);
    for index in 0..5 {
        let index = index.to_string();
        let enabled = load_source(
            content,
            ui_window,
            UI_WINDOW_IMAGE,
            &format!("skill-job-tab-{index}-enabled"),
            &["Skill", "Tab", "enabled", &index],
            &mut assets,
        )?;
        let disabled = load_source(
            content,
            ui_window,
            UI_WINDOW_IMAGE,
            &format!("skill-job-tab-{index}-disabled"),
            &["Skill", "Tab", "disabled", &index],
            &mut assets,
        )?;
        tabs.push(SkillTabSources { enabled, disabled });
    }
    Ok((
        SkillWindowSources {
            background,
            close,
            row,
            selected_row,
            point_up,
            point_up_hover,
            point_up_pressed,
            point_up_disabled,
            tabs,
        },
        assets,
    ))
}

fn load_stat_window_sources(
    content: &GuiContent,
    ui_window: &WzNodeArc,
) -> Result<(StatWindowSources, Vec<AssetDescriptor>), GuiContentError> {
    let mut assets = Vec::new();
    let background = load_source(
        content,
        ui_window,
        UI_WINDOW_IMAGE,
        "stat-background",
        &["Stat", "backgrnd"],
        &mut assets,
    )?;
    let close = load_source(
        content,
        ui_window,
        UI_WINDOW_IMAGE,
        "stat-close",
        &["BtUIClose", "normal", "0"],
        &mut assets,
    )?;
    let job = load_source(
        content,
        ui_window,
        UI_WINDOW_IMAGE,
        "stat-job",
        &["Stat", "Job", "main", "0"],
        &mut assets,
    )?;
    let ability_up_disabled = load_source(
        content,
        ui_window,
        UI_WINDOW_IMAGE,
        "stat-ability-up-disabled",
        &["Stat", "BtApUp", "disabled", "0"],
        &mut assets,
    )?;
    let ability_up = load_source(
        content,
        ui_window,
        UI_WINDOW_IMAGE,
        "stat-ability-up",
        &["Stat", "BtApUp", "normal", "0"],
        &mut assets,
    )?;
    let auto_assign = load_source(
        content,
        ui_window,
        UI_WINDOW_IMAGE,
        "stat-auto-assign-disabled",
        &["Stat", "BtAuto", "disabled", "0"],
        &mut assets,
    )?;
    let detail = load_source(
        content,
        ui_window,
        UI_WINDOW_IMAGE,
        "stat-detail-disabled",
        &["Stat", "BtDetail", "disabled", "0"],
        &mut assets,
    )?;

    Ok((
        StatWindowSources {
            background,
            close,
            job,
            ability_up,
            ability_up_disabled,
            auto_assign,
            detail,
        },
        assets,
    ))
}

fn load_normal_button(
    content: &GuiContent,
    status_bar: &WzNodeArc,
    name: &str,
    path: &str,
    assets: &mut Vec<AssetDescriptor>,
) -> Result<SourceSprite, GuiContentError> {
    load_button_state(content, status_bar, name, path, "normal", assets)
}

fn load_mouse_cursor(
    content: &GuiContent,
    basic: &WzNodeArc,
    cursor_id: &str,
) -> Result<(Option<MouseCursor>, Vec<AssetDescriptor>), GuiContentError> {
    let cursor_path = ["Cursor", cursor_id];
    let Some(cursor_node) = optional_path(basic, &cursor_path)? else {
        return Ok((None, Vec::new()));
    };

    let mut frames = Vec::new();
    let mut assets = Vec::new();
    for node in sorted_children(&cursor_node)? {
        let frame_name = node_name(&node)?;
        if frame_name.parse::<u32>().is_err() {
            continue;
        }
        let path = ["Cursor", cursor_id, frame_name.as_str()];
        let geometry = match png_geometry(&node, &path) {
            Ok(geometry) => geometry,
            Err(error @ GuiContentError::Invalid { .. }) => {
                tracing::warn!(
                    path = %format!("{BASIC_IMAGE}/{}", path.join("/")),
                    %error,
                    "skipping malformed optional WZ cursor frame"
                );
                continue;
            }
            Err(error) => return Err(error),
        };
        let source_path = format!("{BASIC_IMAGE}/{}", path.join("/"));
        let descriptor = content.register_asset(&source_path, &node)?;
        let delay_ms = int_value(&node, "delay")?
            .and_then(|delay| u32::try_from(delay).ok())
            .filter(|delay| *delay > 0)
            .unwrap_or(DEFAULT_CURSOR_FRAME_DELAY_MS);
        frames.push(AnimationFrame {
            asset_id: descriptor.id.clone(),
            width: geometry.width as f32,
            height: geometry.height as f32,
            origin_x: geometry.origin_x as f32,
            origin_y: geometry.origin_y as f32,
            delay_ms,
        });
        assets.push(descriptor);
    }
    if frames.is_empty() {
        tracing::warn!(
            path = %format!("{BASIC_IMAGE}/{}", cursor_path.join("/")),
            "WZ cursor has no frames; the client will use its native cursor"
        );
        return Ok((None, Vec::new()));
    }
    Ok((Some(MouseCursor { frames }), assets))
}

fn load_button_state(
    content: &GuiContent,
    status_bar: &WzNodeArc,
    name: &str,
    path: &str,
    state: &str,
    assets: &mut Vec<AssetDescriptor>,
) -> Result<SourceSprite, GuiContentError> {
    load_status_source(content, status_bar, name, &[path, state, "0"], assets)
}

fn load_status_source(
    content: &GuiContent,
    status_bar: &WzNodeArc,
    name: &str,
    path: &[&str],
    assets: &mut Vec<AssetDescriptor>,
) -> Result<SourceSprite, GuiContentError> {
    load_source(content, status_bar, STATUS_BAR_IMAGE, name, path, assets)
}

fn load_source(
    content: &GuiContent,
    root: &WzNodeArc,
    image_name: &str,
    name: &str,
    path: &[&str],
    assets: &mut Vec<AssetDescriptor>,
) -> Result<SourceSprite, GuiContentError> {
    let node = required_path(root, path)?;
    let geometry = png_geometry(&node, path)?;
    let source_path = format!("{image_name}/{}", path.join("/"));
    let descriptor = content.register_asset(&source_path, &node)?;
    let source = SourceSprite {
        name: name.to_owned(),
        asset_id: descriptor.id.clone(),
        width: geometry.width as f32,
        height: geometry.height as f32,
        origin_x: geometry.origin_x as f32,
        origin_y: geometry.origin_y as f32,
    };
    assets.push(descriptor);
    Ok(source)
}

fn required_path(
    root: &WzNodeArc,
    path: &[&str],
) -> Result<WzNodeArc, GuiContentError> {
    let mut node = Arc::clone(root);
    for name in path {
        node = required_child(&node, name)?;
    }
    Ok(node)
}

fn optional_path(
    root: &WzNodeArc,
    path: &[&str],
) -> Result<Option<WzNodeArc>, GuiContentError> {
    let mut node = Arc::clone(root);
    for name in path {
        let Some(child) = child(&node, name)? else {
            return Ok(None);
        };
        node = child;
    }
    Ok(Some(node))
}

fn required_child(
    node: &WzNodeArc,
    name: &str,
) -> Result<WzNodeArc, GuiContentError> {
    child(node, name)?.ok_or_else(|| GuiContentError::Invalid {
        message: format!("required node {name:?} is missing"),
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SpriteGeometry {
    width: u32,
    height: u32,
    origin_x: i32,
    origin_y: i32,
}

fn png_geometry(
    node: &WzNodeArc,
    path: &[&str],
) -> Result<SpriteGeometry, GuiContentError> {
    let (width, height) = {
        let read = node
            .read()
            .map_err(|_| lock_error("GUI sprite dimensions"))?;
        let png = read.try_as_png().ok_or_else(|| GuiContentError::Invalid {
            message: format!("{} is not a PNG sprite", path.join("/")),
        })?;
        (png.width, png.height)
    };
    if width == 0 || height == 0 {
        return invalid(format!("{} has an empty PNG sprite", path.join("/")));
    }
    let (origin_x, origin_y) = match child(node, "origin")? {
        Some(origin) => vector_value(&origin)?
            .map(|origin| (origin.0, origin.1))
            .unwrap_or_default(),
        None => (0, 0),
    };
    Ok(SpriteGeometry {
        width,
        height,
        origin_x,
        origin_y,
    })
}

fn lock_error(context: &'static str) -> GuiContentError {
    GuiContentError::Lock { context }
}

fn invalid<T>(message: impl Into<String>) -> Result<T, GuiContentError> {
    Err(GuiContentError::Invalid {
        message: message.into(),
    })
}

#[cfg(test)]
mod tests {
    use oozems_proto::v1::GameGui;
    use oozems_proto::v1::GuiLayout;
    use oozems_proto::v1::GuiSprite;
    use oozems_proto::v1::GuiWindow;
    use oozems_proto::v1::GuiWindowDefinition;
    use oozems_proto::v1::KeyActionDefinition;
    use oozems_proto::v1::KeySlot;

    use super::CashShopWindowSources;
    use super::EquipmentWindowSources;
    use super::InventoryTabSources;
    use super::InventoryWindowSources;
    use super::KeyConfigSources;
    use super::NpcDialogSources;
    use super::SkillTabSources;
    use super::SkillWindowSources;
    use super::SourceSprite;
    use super::StatWindowSources;
    use super::StatusBarSources;
    use super::add_runtime_regions;
    use super::compose_cash_shop_window;
    use super::compose_equipment_window;
    use super::compose_inventory_window;
    use super::compose_key_config;
    use super::compose_npc_dialog_window;
    use super::compose_skill_window;
    use super::compose_stat_window;
    use super::compose_status_bar;
    use super::install_window_definition;

    #[test]
    fn status_bar_sources_form_the_native_layout() {
        let sources = StatusBarSources {
            background: source("background", 800.0, 71.0),
            overlay: source("status-overlay", 570.0, 71.0),
            gauge: source("gauge", 340.0, 31.0),
            gauge_graduation: source("gauge-graduation", 340.0, 31.0),
            quick_slots: source("quick-slots", 151.0, 80.0),
            equip: source("equip", 28.0, 20.0),
            equip_pressed: source("equip-pressed", 28.0, 20.0),
            inventory: source("inventory", 28.0, 20.0),
            inventory_pressed: source("inventory-pressed", 28.0, 20.0),
            stats: source("stats", 28.0, 20.0),
            stats_pressed: source("stats-pressed", 28.0, 20.0),
            skills: source("skills", 28.0, 20.0),
            skills_pressed: source("skills-pressed", 28.0, 20.0),
            key_settings: source("key-settings", 28.0, 20.0),
            key_settings_pressed: source("key-settings-pressed", 28.0, 20.0),
            quick_slot_toggle: source("quick-slot-toggle", 28.0, 20.0),
            cash_shop: source("cash-shop", 54.0, 34.0),
            key_references: [
                (28.0, 11.0),
                (18.0, 11.0),
                (19.0, 11.0),
                (22.0, 13.0),
                (24.0, 11.0),
                (19.0, 11.0),
                (21.0, 11.0),
                (22.0, 11.0),
            ]
            .into_iter()
            .enumerate()
            .map(|(index, (width, height))| source(&format!("key-{index}"), width, height))
            .collect(),
        };

        let layout = compose_status_bar(&sources).expect("valid native layout");

        assert_eq!((layout.width, layout.height), (800.0, 80.0));
        assert_eq!(
            layout
                .background
                .as_ref()
                .map(|sprite| (sprite.x, sprite.y)),
            Some((0.0, 9.0))
        );
        assert_eq!(sprite_position(&layout, "status-overlay"), Some((0.0, 9.0)));
        assert_eq!(sprite_position(&layout, "gauge"), Some((209.0, 48.0)));
        assert_eq!(sprite_position(&layout, "quick-slots"), Some((649.0, 0.0)));
        assert_eq!(sprite_position(&layout, "cash-shop"), Some((593.0, 46.0)));
        assert!(
            layout
                .sprites
                .iter()
                .find(|sprite| sprite.name == "cash-shop")
                .is_some_and(|sprite| sprite.anchor_right)
        );
        assert_eq!(sprite_position(&layout, "key-0"), Some((657.0, 16.0)));
        assert_eq!(sprite_position(&layout, "stats"), Some((634.0, 17.0)));
        assert_eq!(
            sprite_position(&layout, "stats-pressed"),
            Some((634.0, 17.0))
        );
        assert_eq!(sprite_position(&layout, "skills"), Some((664.0, 17.0)));
        assert_eq!(
            sprite_position(&layout, "skills-pressed"),
            Some((664.0, 17.0))
        );
        assert_eq!(
            sprite_position(&layout, "quick-slot-toggle"),
            Some((724.0, 17.0))
        );
    }

    #[test]
    fn cash_shop_sources_form_the_fixed_classic_screen() {
        let window = compose_cash_shop_window(&CashShopWindowSources {
            background: source("cash-shop-background", 800.0, 600.0),
            preview: source("cash-shop-preview", 212.0, 165.0),
            category_tab: source("cash-shop-category-tab", 508.0, 78.0),
            item_card: source("cash-shop-item-card", 200.0, 80.0),
            buy: source("cash-shop-buy", 37.0, 19.0),
            gift_disabled: source("cash-shop-gift-disabled", 37.0, 19.0),
            exit: source("cash-shop-exit", 168.0, 49.0),
        })
        .expect("valid cash-shop screen");
        let layout = window.layout.expect("cash-shop layout");

        assert_eq!((layout.width, layout.height), (800.0, 600.0));
        assert_eq!(
            sprite_position(&layout, "cash-shop-preview"),
            Some((24.0, 40.0))
        );
        assert_eq!(
            sprite_position(&layout, "cash-shop-category-tab"),
            Some((273.0, 16.0))
        );
        assert_eq!(
            sprite_position(&layout, "cash-shop-item-card-0"),
            Some((278.0, 98.0))
        );
        assert_eq!(
            sprite_position(&layout, "cash-shop-item-card-9"),
            Some((484.0, 422.0))
        );
        assert_eq!(
            region_geometry(&layout, "cash-shop-gift-0"),
            Some((396.0, 155.0, 37.0, 19.0))
        );
        assert_eq!(
            region_geometry(&layout, "cash-shop-buy-0"),
            Some((355.0, 155.0, 37.0, 19.0))
        );
        assert_eq!(
            region_geometry(&layout, "cash-shop-exit"),
            Some((632.0, 535.0, 168.0, 49.0))
        );
    }

    #[test]
    fn npc_decision_footer_keeps_back_clear_of_accept_and_decline() {
        let window = compose_npc_dialog_window(&NpcDialogSources {
            top: source("npc-top", 529.0, 46.0),
            center: source("npc-center", 529.0, 18.0),
            bottom: source("npc-bottom", 529.0, 60.0),
            close: source("npc-dialog-close", 85.0, 15.0),
            ok: source("npc-dialog-ok", 44.0, 15.0),
            next: source("npc-dialog-next", 44.0, 15.0),
            previous: source("npc-dialog-previous", 44.0, 15.0),
            accept: source("npc-dialog-accept", 55.0, 15.0),
            decline: source("npc-dialog-decline", 55.0, 15.0),
            choice: source("npc-dialog-choice", 345.0, 20.0),
            choice_selected: source("npc-dialog-choice-selected", 345.0, 20.0),
        })
        .expect("valid NPC dialog");
        let layout = window.layout.expect("NPC dialog layout");
        let previous = named_region(&layout, "npc-previous");
        let decision_previous = named_region(&layout, "npc-decision-previous");
        let accept = named_region(&layout, "npc-accept");
        let decline = named_region(&layout, "npc-decline");

        assert_eq!((previous.x, decision_previous.x), (402.0, 329.0));
        assert_eq!((decision_previous.y, accept.y), (accept.y, decline.y));
        assert!(decision_previous.x + decision_previous.width <= accept.x);
        assert!(accept.x + accept.width <= decline.x);
    }

    #[test]
    fn stat_sources_form_a_clickable_window_layout() {
        let sources = StatWindowSources {
            background: source("stat-background", 175.0, 347.0),
            close: source("stat-close", 10.0, 10.0),
            job: source("stat-job", 50.0, 7.0),
            ability_up: source("stat-ability-up", 12.0, 12.0),
            ability_up_disabled: source("stat-ability-up-disabled", 12.0, 12.0),
            auto_assign: source("stat-auto-assign-disabled", 73.0, 35.0),
            detail: source("stat-detail-disabled", 47.0, 18.0),
        };

        let window = compose_stat_window(&sources).expect("valid stat window");
        let layout = window.layout.expect("stat window layout");

        assert_eq!((window.x, window.y), (20.0, 80.0));
        assert_eq!((layout.width, layout.height), (175.0, 347.0));
        assert_eq!(sprite_position(&layout, "stat-close"), Some((160.0, 5.0)));
        assert_eq!(sprite_position(&layout, "stat-job"), Some((60.0, 57.0)));
        assert_eq!(
            sprite_position(&layout, "stat-auto-assign-disabled"),
            Some((95.0, 197.0))
        );
        assert_eq!(
            sprite_position(&layout, "stat-ability-up-disabled"),
            Some((153.0, 117.0))
        );
        assert_eq!(
            sprite_position(&layout, "stat-detail-disabled"),
            Some((113.0, 324.0))
        );
        assert_eq!(
            layout
                .sprites
                .iter()
                .filter(|sprite| sprite.name == "stat-ability-up-disabled")
                .count(),
            2
        );
        for name in ["stat-ability-up", "stat-ability-up-disabled"] {
            assert!(
                layout
                    .sprite_templates
                    .iter()
                    .any(|template| template.name == name)
            );
        }
        for name in [
            "stat-strength-up",
            "stat-dexterity-up",
            "stat-intelligence-up",
            "stat-luck-up",
        ] {
            assert!(layout.regions.iter().any(|region| region.name == name));
        }
    }

    #[test]
    fn skill_sources_form_a_component_driven_layout() {
        let mut sources = SkillWindowSources {
            background: source("skill-background", 175.0, 289.0),
            close: source("skill-close", 10.0, 10.0),
            row: source("skill-row", 141.0, 35.0),
            selected_row: source("skill-row-selected", 141.0, 35.0),
            point_up: source("skill-point-up", 12.0, 12.0),
            point_up_hover: source("skill-point-up-hover", 12.0, 12.0),
            point_up_pressed: source("skill-point-up-pressed", 12.0, 12.0),
            point_up_disabled: source("skill-point-up-disabled", 12.0, 12.0),
            tabs: (0..5)
                .map(|index| SkillTabSources {
                    enabled: source(&format!("skill-job-tab-{index}-enabled"), 10.0, 12.0),
                    disabled: source(&format!("skill-job-tab-{index}-disabled"), 10.0, 12.0),
                })
                .collect(),
        };

        let window = compose_skill_window(&sources).expect("valid skill window");
        let layout = window.layout.expect("skill window layout");

        assert_eq!((window.x, window.y), (20.0, 80.0));
        assert_eq!((layout.width, layout.height), (175.0, 289.0));
        assert_eq!(sprite_position(&layout, "skill-close"), Some((160.0, 5.0)));
        assert_eq!(template_size(&layout, "skill-row"), Some((141.0, 35.0)));
        assert_eq!(
            template_size(&layout, "skill-row-selected"),
            Some((141.0, 35.0))
        );
        assert_eq!(
            region_geometry(&layout, "skill-list"),
            Some((17.0, 94.0, 141.0, 140.0))
        );
        assert_eq!(
            region_geometry(&layout, "skill-title"),
            Some((47.0, 61.0, 116.0, 15.0))
        );
        assert_eq!(
            region_geometry(&layout, "skill-job-tab-4"),
            Some((135.0, 26.0, 20.0, 14.0))
        );
        assert_eq!(
            region_geometry(&layout, "skill-points"),
            Some((82.0, 265.0, 30.0, 14.0))
        );
        sources.tabs.pop();
        assert!(compose_skill_window(&sources).is_err());
    }

    #[test]
    fn inventory_sources_form_five_native_tabs() {
        let sources = inventory_sources();

        let window =
            compose_inventory_window(&sources, 205.0, 80.0).expect("valid inventory window");
        let layout = window.layout.expect("inventory window layout");

        assert_eq!((layout.width, layout.height), (175.0, 289.0));
        assert_eq!(layout.sprite_templates.len(), 21);
        assert_eq!(
            template_size(&layout, "inventory-tab-equipment-active-background"),
            Some((34.0, 19.0))
        );
        assert_eq!(
            region_geometry(&layout, "inventory-tab-equipment"),
            Some((3.0, 22.0, 34.0, 19.0))
        );
        assert_eq!(
            region_geometry(&layout, "inventory-tab-cash"),
            Some((139.0, 22.0, 34.0, 19.0))
        );
        assert_eq!(
            region_geometry(&layout, "inventory-mesos"),
            Some((26.0, 274.0, 111.0, 14.0))
        );
        assert_eq!(
            sprite_position(&layout, "inventory-gather-disabled"),
            Some((96.0, 6.0))
        );
        assert_eq!(
            sprite_position(&layout, "inventory-sort-disabled"),
            Some((110.0, 6.0))
        );
        assert_eq!(
            sprite_position(&layout, "inventory-expand-disabled"),
            Some((124.0, 6.0))
        );
    }

    #[test]
    fn item_and_key_config_sources_form_native_windows() {
        let item = EquipmentWindowSources {
            background: source("equipment-background", 175.0, 304.0),
            close: source("equipment-close", 10.0, 10.0),
            detail: source("equipment-detail-disabled", 54.0, 18.0),
        };
        let equipment =
            compose_equipment_window(&item, 20.0, 80.0).expect("valid equipment window");
        assert_eq!(
            equipment
                .layout
                .as_ref()
                .map(|layout| (layout.width, layout.height)),
            Some((175.0, 304.0))
        );
        assert_eq!(
            equipment
                .layout
                .as_ref()
                .and_then(|layout| sprite_position(layout, "equipment-detail-disabled")),
            Some((100.0, 281.0))
        );

        let key_config = KeyConfigSources {
            background: source("key-config-background", 629.0, 373.0),
            close: source("key-config-close", 10.0, 10.0),
            actions: crate::keymap::ACTIONS
                .iter()
                .map(|spec| (*spec, source(spec.icon_id, 32.0, 32.0)))
                .collect(),
        };
        let (window, actions) = compose_key_config(&key_config).expect("valid key config window");
        assert_eq!(
            window
                .layout
                .as_ref()
                .map(|layout| (layout.width, layout.height)),
            Some((629.0, 373.0))
        );
        assert_eq!(actions.len(), crate::keymap::ACTIONS.len());
    }

    #[test]
    fn authored_windows_install_in_their_runtime_fields() {
        for name in oozems_ui_layout::SUPPORTED_WINDOWS {
            let definition = GuiWindowDefinition {
                name: (*name).to_owned(),
                ..GuiWindowDefinition::default()
            };
            let window = GuiWindow {
                x: 13.0,
                y: 17.0,
                layout: Some(GuiLayout {
                    width: 211.0,
                    height: 223.0,
                    ..GuiLayout::default()
                }),
            };
            let expected = window.clone();
            let mut gui = GameGui::default();

            install_window_definition(&mut gui, &definition, window);

            match name {
                "status-bar" => assert_eq!(gui.status_bar, expected.layout),
                "stats" => assert_eq!(gui.stat_window, Some(expected)),
                "equipment" => assert_eq!(gui.equipment_window, Some(expected)),
                "inventory" => assert_eq!(gui.inventory_window, Some(expected)),
                "skills" => assert_eq!(gui.skill_window, Some(expected)),
                "key-config" => assert_eq!(gui.key_config_window, Some(expected)),
                "npc-dialog" => assert_eq!(gui.npc_dialog_window, Some(expected)),
                "shop" => assert_eq!(gui.shop_window, Some(expected)),
                "cash-shop" => assert_eq!(gui.cash_shop_window, Some(expected)),
                "death-notice" => assert_eq!(gui.death_notice_window, Some(expected)),
                _ => unreachable!("supported GUI window"),
            }
        }
    }

    #[test]
    fn authored_key_config_replaces_key_action_icons() {
        let mut gui = GameGui {
            key_actions: vec![KeyActionDefinition {
                icon: Some(GuiSprite {
                    name: "key-action-53".to_owned(),
                    asset_id: "old-asset".to_owned(),
                    ..GuiSprite::default()
                }),
                ..KeyActionDefinition::default()
            }],
            key_slots: vec![KeySlot {
                code: "KeyA".to_owned(),
                x: 1.0,
                y: 2.0,
                width: 3.0,
                height: 4.0,
            }],
            ..GameGui::default()
        };
        let window = GuiWindow {
            layout: Some(GuiLayout {
                sprites: vec![GuiSprite {
                    name: "key-action-53".to_owned(),
                    asset_id: "authored-asset".to_owned(),
                    x: 47.0,
                    y: 53.0,
                    ..GuiSprite::default()
                }],
                regions: vec![oozems_proto::v1::GuiRegion {
                    name: "key-config-slot-KeyA".to_owned(),
                    x: 11.0,
                    y: 13.0,
                    width: 17.0,
                    height: 19.0,
                }],
                ..GuiLayout::default()
            }),
            ..GuiWindow::default()
        };
        let definition = GuiWindowDefinition {
            name: "key-config".to_owned(),
            ..GuiWindowDefinition::default()
        };

        install_window_definition(&mut gui, &definition, window);

        let icon = gui.key_actions[0].icon.as_ref().expect("key action icon");
        assert_eq!(icon.asset_id, "authored-asset");
        assert_eq!((icon.x, icon.y), (47.0, 53.0));
        let slot = &gui.key_slots[0];
        assert_eq!(
            (slot.x, slot.y, slot.width, slot.height),
            (11.0, 13.0, 17.0, 19.0)
        );
    }

    #[test]
    fn runtime_region_injection_adds_dynamic_cash_shop_content() {
        let mut layout = GuiLayout {
            width: 800.0,
            height: 600.0,
            regions: vec![oozems_proto::v1::GuiRegion {
                name: "cash-shop-item-icon-0".to_owned(),
                x: 1.0,
                y: 2.0,
                width: 3.0,
                height: 4.0,
            }],
            ..GuiLayout::default()
        };

        add_runtime_regions("cash-shop", &mut layout);

        assert_eq!(
            region_geometry(&layout, "cash-shop-item-icon-0"),
            Some((1.0, 2.0, 3.0, 4.0))
        );
        assert_eq!(
            region_geometry(&layout, "cash-shop-item-icon-9"),
            Some((504.0, 446.0, 32.0, 32.0))
        );
        assert!(region_geometry(&layout, "cash-shop-item-name-9").is_some());
    }

    fn source(
        name: &str,
        width: f32,
        height: f32,
    ) -> SourceSprite {
        SourceSprite {
            name: name.to_owned(),
            asset_id: format!("asset-{name}"),
            width,
            height,
            origin_x: 0.0,
            origin_y: 0.0,
        }
    }

    fn inventory_sources() -> InventoryWindowSources {
        let tabs = ["equipment", "consume", "install", "etc", "cash"]
            .into_iter()
            .map(|name| InventoryTabSources {
                active_background: source(
                    &format!("inventory-tab-{name}-active-background"),
                    34.0,
                    19.0,
                ),
                inactive_background: source(
                    &format!("inventory-tab-{name}-inactive-background"),
                    34.0,
                    18.0,
                ),
                active_label: source(&format!("inventory-tab-{name}-active-label"), 20.0, 10.0),
                inactive_label: source(&format!("inventory-tab-{name}-inactive-label"), 20.0, 10.0),
            })
            .collect();
        InventoryWindowSources {
            background: source("inventory-background", 175.0, 289.0),
            close: source("inventory-close", 10.0, 10.0),
            tabs,
            gather: source("inventory-gather-disabled", 12.0, 12.0),
            sort: source("inventory-sort-disabled", 12.0, 12.0),
            expand: source("inventory-expand-disabled", 12.0, 12.0),
            locked_slot: source("inventory-locked-slot", 32.0, 32.0),
        }
    }

    fn template_size(
        layout: &oozems_proto::v1::GuiLayout,
        name: &str,
    ) -> Option<(f32, f32)> {
        layout
            .sprite_templates
            .iter()
            .find(|template| template.name == name)
            .map(|template| (template.width, template.height))
    }

    fn region_geometry(
        layout: &oozems_proto::v1::GuiLayout,
        name: &str,
    ) -> Option<(f32, f32, f32, f32)> {
        layout
            .regions
            .iter()
            .find(|region| region.name == name)
            .map(|region| (region.x, region.y, region.width, region.height))
    }

    fn named_region<'a>(
        layout: &'a oozems_proto::v1::GuiLayout,
        name: &str,
    ) -> &'a oozems_proto::v1::GuiRegion {
        layout
            .regions
            .iter()
            .find(|region| region.name == name)
            .expect("named GUI region")
    }

    fn sprite_position(
        layout: &oozems_proto::v1::GuiLayout,
        name: &str,
    ) -> Option<(f32, f32)> {
        layout
            .sprites
            .iter()
            .find(|sprite| sprite.name == name)
            .map(|sprite| (sprite.x, sprite.y))
    }
}
