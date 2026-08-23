use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::sync::RwLock;

use oozems_proto::v1::AssetDescriptor;
use oozems_proto::v1::GameGui;
use oozems_proto::v1::KeySlot;
use sha2::Digest;
use sha2::Sha256;
use thiserror::Error;
use wz_reader::WzNodeArc;
use wz_reader::WzNodeCast;

use super::WzAsset;
use super::wz::WzContentError;
use super::wz::archive_fingerprint;
use super::wz::child;
use super::wz::open_archive;
use super::wz::parse;
use super::wz::vector_value;
use super::wz::wrap_archive_root;

mod layout;

use layout::compose_item_window;
use layout::compose_key_config;
use layout::compose_skill_window;
use layout::compose_stat_window;
use layout::compose_status_bar;

const GUI_ARCHIVE: &str = "UI.wz";
const STATUS_BAR_IMAGE: &str = "StatusBar.img";
const UI_WINDOW_IMAGE: &str = "UIWindow.img";
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
    key_references: Vec<SourceSprite>,
}

struct StatWindowSources {
    background: SourceSprite,
    close: SourceSprite,
    job: SourceSprite,
}

struct ItemWindowSources {
    background: SourceSprite,
    close: SourceSprite,
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
}

struct KeyConfigSources {
    background: SourceSprite,
    close: SourceSprite,
    actions: Vec<(crate::keymap::KeyActionSpec, SourceSprite)>,
}

impl GuiContent {
    pub fn open_optional(directory: &Path) -> Result<Option<Self>, GuiContentError> {
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

        let mut content = Self {
            _base: base,
            gui: GameGui::default(),
            fingerprint: archive_fingerprint(&path)?,
            assets: RwLock::new(HashMap::new()),
        };
        content.gui = build_game_gui(&content, &status_bar, &ui_window)?;

        tracing::info!(
            path = %path.display(),
            assets = content.gui.assets.len(),
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
            content_hash: version,
        })
    }
}

fn build_game_gui(
    content: &GuiContent,
    status_bar: &WzNodeArc,
    ui_window: &WzNodeArc,
) -> Result<GameGui, GuiContentError> {
    let (status_sources, mut assets) = load_status_bar_sources(content, status_bar)?;
    let status_bar = compose_status_bar(&status_sources)?;
    let (stat_sources, stat_assets) = load_stat_window_sources(content, ui_window)?;
    let stat_window = compose_stat_window(&stat_sources)?;
    assets.extend(stat_assets);
    let (equipment_sources, equipment_assets) =
        load_item_window_sources(content, ui_window, "equipment", "Equip")?;
    let equipment_window =
        compose_item_window(&equipment_sources, EQUIPMENT_WINDOW_X, EQUIPMENT_WINDOW_Y)?;
    assets.extend(equipment_assets);
    let (inventory_sources, inventory_assets) =
        load_item_window_sources(content, ui_window, "inventory", "Item")?;
    let inventory_window =
        compose_item_window(&inventory_sources, INVENTORY_WINDOW_X, INVENTORY_WINDOW_Y)?;
    assets.extend(inventory_assets);
    let (skill_sources, skill_assets) = load_skill_window_sources(content, ui_window)?;
    let skill_window = compose_skill_window(&skill_sources)?;
    assets.extend(skill_assets);
    let (key_config_sources, key_config_assets) = load_key_config_sources(content, ui_window)?;
    let (key_config_window, key_actions) = compose_key_config(&key_config_sources)?;
    assets.extend(key_config_assets);
    let mut asset_ids = HashSet::new();
    assets.retain(|asset| asset_ids.insert(asset.id.clone()));
    Ok(GameGui {
        status_bar: Some(status_bar),
        assets,
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
    })
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

fn load_item_window_sources(
    content: &GuiContent,
    ui_window: &WzNodeArc,
    name: &str,
    wz_name: &str,
) -> Result<(ItemWindowSources, Vec<AssetDescriptor>), GuiContentError> {
    let mut assets = Vec::new();
    let background = load_source(
        content,
        ui_window,
        UI_WINDOW_IMAGE,
        &format!("{name}-background"),
        &[wz_name, "backgrnd"],
        &mut assets,
    )?;
    let close = load_source(
        content,
        ui_window,
        UI_WINDOW_IMAGE,
        &format!("{name}-close"),
        &["BtUIClose", "normal", "0"],
        &mut assets,
    )?;
    Ok((ItemWindowSources { background, close }, assets))
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

    Ok((
        StatWindowSources {
            background,
            close,
            job,
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
    use std::path::Path;

    use super::GuiContent;
    use super::SkillWindowSources;
    use super::SourceSprite;
    use super::StatWindowSources;
    use super::StatusBarSources;
    use super::compose_skill_window;
    use super::compose_stat_window;
    use super::compose_status_bar;

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
    fn stat_sources_form_a_clickable_window_layout() {
        let sources = StatWindowSources {
            background: source("stat-background", 175.0, 347.0),
            close: source("stat-close", 10.0, 10.0),
            job: source("stat-job", 50.0, 7.0),
        };

        let window = compose_stat_window(&sources).expect("valid stat window");
        let layout = window.layout.expect("stat window layout");

        assert_eq!((window.x, window.y), (20.0, 80.0));
        assert_eq!((layout.width, layout.height), (175.0, 347.0));
        assert_eq!(sprite_position(&layout, "stat-close"), Some((160.0, 5.0)));
        assert_eq!(sprite_position(&layout, "stat-job"), Some((60.0, 57.0)));
    }

    #[test]
    fn skill_sources_form_a_component_driven_layout() {
        let sources = SkillWindowSources {
            background: source("skill-background", 175.0, 289.0),
            close: source("skill-close", 10.0, 10.0),
            row: source("skill-row", 141.0, 35.0),
            selected_row: source("skill-row-selected", 141.0, 35.0),
            point_up: source("skill-point-up", 12.0, 12.0),
            point_up_hover: source("skill-point-up-hover", 12.0, 12.0),
            point_up_pressed: source("skill-point-up-pressed", 12.0, 12.0),
            point_up_disabled: source("skill-point-up-disabled", 12.0, 12.0),
        };

        let window = compose_skill_window(&sources).expect("valid skill window");
        let layout = window.layout.expect("skill window layout");

        assert_eq!((window.x, window.y), (20.0, 80.0));
        assert_eq!((layout.width, layout.height), (175.0, 289.0));
        assert_eq!(sprite_position(&layout, "skill-close"), Some((160.0, 5.0)));
        assert_eq!(template_size(&layout, "skill-row"), Some((141.0, 35.0)));
        assert_eq!(
            region_geometry(&layout, "skill-list"),
            Some((17.0, 94.0, 141.0, 140.0))
        );
        assert_eq!(
            region_geometry(&layout, "skill-title"),
            Some((17.0, 27.0, 141.0, 14.0))
        );
        assert_eq!(
            region_geometry(&layout, "skill-points"),
            Some((82.0, 265.0, 30.0, 14.0))
        );
    }

    #[test]
    fn local_ui_archive_builds_a_native_status_bar_when_present() {
        let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let directory = manifest_dir.join("../../data");
        if !directory.join("UI.wz").exists() {
            return;
        }

        let content = GuiContent::open_optional(&directory)
            .expect("sample UI.wz should be valid")
            .expect("sample UI.wz should be present");
        let gui = content.game_gui();
        let status_bar = gui.status_bar.expect("status bar layout");

        assert_eq!(status_bar.width, 800.0);
        assert_eq!(status_bar.height, 80.0);
        assert_eq!(
            status_bar
                .background
                .as_ref()
                .map(|sprite| sprite.name.as_str()),
            Some("background")
        );
        assert!(status_bar.sprites.iter().any(|sprite| {
            sprite.name == "quick-slots" && sprite.x == 649.0 && sprite.anchor_right
        }));
        let stat_window = gui.stat_window.as_ref().expect("stat window");
        assert_eq!(
            stat_window.layout.as_ref().map(|layout| layout.height),
            Some(347.0)
        );
        assert_eq!(gui.assets.len(), 46);
        assert_eq!(gui.key_actions.len(), crate::keymap::ACTIONS.len());
        assert_eq!(gui.key_slots.len(), crate::keymap::SLOTS.len());
        assert_eq!(
            gui.key_config_window
                .as_ref()
                .and_then(|window| window.layout.as_ref())
                .map(|layout| (layout.width, layout.height)),
            Some((629.0, 373.0))
        );
        assert_eq!(
            gui.equipment_window
                .as_ref()
                .and_then(|window| window.layout.as_ref())
                .map(|layout| (layout.width, layout.height)),
            Some((175.0, 304.0))
        );
        assert_eq!(
            gui.inventory_window
                .as_ref()
                .and_then(|window| window.layout.as_ref())
                .map(|layout| (layout.width, layout.height)),
            Some((175.0, 289.0))
        );
        assert_eq!(
            gui.skill_window
                .as_ref()
                .and_then(|window| window.layout.as_ref())
                .map(|layout| (layout.width, layout.height)),
            Some((175.0, 289.0))
        );
        let skill_layout = gui
            .skill_window
            .as_ref()
            .and_then(|window| window.layout.as_ref())
            .expect("skill window layout");
        assert_eq!(
            template_size(skill_layout, "skill-row"),
            Some((141.0, 35.0))
        );
        assert_eq!(
            region_geometry(skill_layout, "skill-list"),
            Some((17.0, 94.0, 141.0, 140.0))
        );

        for descriptor in &gui.assets {
            let asset = content
                .get_asset(&descriptor.id)
                .expect("GUI asset should be registered");
            let png = asset.png_bytes().expect("GUI asset should decode");
            assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
        }
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
