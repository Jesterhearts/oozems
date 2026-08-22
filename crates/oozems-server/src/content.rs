use std::collections::HashMap;
use std::fs;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

use oozems_proto::v1::AssetDescriptor;
use oozems_proto::v1::CharacterAppearance;
use oozems_proto::v1::CharacterCreationOptions;
use oozems_proto::v1::CharacterSpriteSet;
use oozems_proto::v1::Decoration;
use oozems_proto::v1::EquippedItem;
use oozems_proto::v1::GameGui;
use oozems_proto::v1::ItemDefinition;
use oozems_proto::v1::Map;
use oozems_proto::v1::Platform;
use oozems_proto::v1::PlatformKind;
use serde::Deserialize;
use sha2::Digest;
use sha2::Sha256;
use thiserror::Error;

mod character;
mod gui;
mod wz;

use character::CharacterContent;
use character::CharacterContentError;
use gui::GuiContent;
use gui::GuiContentError;
pub(crate) use wz::WzAsset;
use wz::WzContent;
use wz::WzContentError;

pub struct ContentCatalog {
    characters: Option<CharacterContent>,
    gui: Option<GuiContent>,
    maps: HashMap<u32, Map>,
    wz: Option<WzContent>,
}

#[derive(Debug, Error)]
pub enum ContentError {
    #[error("failed to read content directory {path}")]
    ReadDirectory {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to read content file {path}")]
    ReadFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse content file {path}")]
    ParseFile {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("map {map_id} is invalid: {message}")]
    InvalidMap { map_id: u32, message: String },
    #[error("map ID {map_id} appears in more than one content file")]
    DuplicateMap { map_id: u32 },
    #[error("map content directory {path} contains no JSON maps")]
    Empty { path: PathBuf },
    #[error(transparent)]
    Wz(#[from] WzContentError),
    #[error(transparent)]
    Character(#[from] CharacterContentError),
    #[error(transparent)]
    Gui(#[from] GuiContentError),
}

#[derive(Debug, Deserialize)]
struct MapFile {
    id: u32,
    name: String,
    width: u32,
    height: u32,
    platforms: Vec<PlatformFile>,
    decorations: Vec<DecorationFile>,
    assets: Vec<AssetFile>,
}

#[derive(Debug, Deserialize)]
struct PlatformFile {
    x: f32,
    y: f32,
    width: f32,
    kind: PlatformKindFile,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum PlatformKindFile {
    Ground,
    Wood,
}

#[derive(Debug, Deserialize)]
struct DecorationFile {
    asset_id: String,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    layer: i32,
}

#[derive(Debug, Deserialize)]
struct AssetFile {
    id: String,
    path: String,
}

impl ContentCatalog {
    pub fn load(
        map_dir: &Path,
        asset_dir: &Path,
    ) -> Result<Self, ContentError> {
        let mut paths = json_files(map_dir)?;
        paths.sort();

        let mut maps = HashMap::new();
        for path in paths {
            let map_file = read_map_file(&path)?;
            let map = build_map(map_file, asset_dir)?;
            let map_id = map.id;
            if maps.insert(map_id, map).is_some() {
                return Err(ContentError::DuplicateMap { map_id });
            }
        }

        if maps.is_empty() {
            return Err(ContentError::Empty {
                path: map_dir.to_owned(),
            });
        }

        Ok(Self {
            characters: None,
            gui: None,
            maps,
            wz: None,
        })
    }

    pub fn load_with_wz(
        map_dir: &Path,
        asset_dir: &Path,
        wz_dir: &Path,
    ) -> Result<Self, ContentError> {
        let mut catalog = Self::load(map_dir, asset_dir)?;
        catalog.wz = WzContent::open_optional(wz_dir)?;
        catalog.characters = CharacterContent::open_optional(wz_dir)?;
        catalog.gui = GuiContent::open_optional(wz_dir)?;
        Ok(catalog)
    }

    pub fn get_map(
        &self,
        map_id: u32,
    ) -> Result<Option<Map>, ContentError> {
        if let Some(source) = self
            .wz
            .as_ref()
            .filter(|source| source.contains_map(map_id))
        {
            return source.get_map(map_id).map(Some).map_err(Into::into);
        }

        Ok(self.maps.get(&map_id).cloned())
    }

    pub fn get_wz_asset(
        &self,
        asset_id: &str,
    ) -> Option<std::sync::Arc<WzAsset>> {
        self.wz
            .as_ref()
            .and_then(|source| source.get_asset(asset_id))
            .or_else(|| {
                self.characters
                    .as_ref()
                    .and_then(|source| source.get_asset(asset_id))
            })
            .or_else(|| {
                self.gui
                    .as_ref()
                    .and_then(|source| source.get_asset(asset_id))
            })
    }

    pub fn game_gui(&self) -> GameGui {
        let mut gui = self
            .gui
            .as_ref()
            .map(GuiContent::game_gui)
            .unwrap_or_default();
        if let Some(characters) = &self.characters {
            gui.assets.extend(characters.item_assets());
            gui.items = characters.item_definitions();
        }
        gui
    }

    pub fn character_creation_options(&self) -> CharacterCreationOptions {
        self.characters
            .as_ref()
            .map(CharacterContent::creation_options)
            .unwrap_or_default()
    }

    pub fn item_definitions(&self) -> Vec<ItemDefinition> {
        self.characters
            .as_ref()
            .map(CharacterContent::item_definitions)
            .unwrap_or_default()
    }

    pub fn supports_character(
        &self,
        appearance: &CharacterAppearance,
    ) -> bool {
        self.characters
            .as_ref()
            .is_some_and(|source| source.supports(appearance))
    }

    pub fn get_character_sprites(
        &self,
        appearance: &CharacterAppearance,
        equipment: &[EquippedItem],
    ) -> Result<Option<CharacterSpriteSet>, ContentError> {
        self.characters
            .as_ref()
            .map(|source| source.get_sprites(appearance, equipment))
            .transpose()
            .map(|sprites| sprites.flatten())
            .map_err(Into::into)
    }
}

fn json_files(directory: &Path) -> Result<Vec<PathBuf>, ContentError> {
    let entries = fs::read_dir(directory).map_err(|source| ContentError::ReadDirectory {
        path: directory.to_owned(),
        source,
    })?;

    entries
        .filter_map(|entry| match entry {
            Ok(entry)
                if entry
                    .path()
                    .extension()
                    .is_some_and(|value| value == "json") =>
            {
                Some(Ok(entry.path()))
            }
            Ok(_) => None,
            Err(source) => Some(Err(ContentError::ReadDirectory {
                path: directory.to_owned(),
                source,
            })),
        })
        .collect()
}

fn read_map_file(path: &Path) -> Result<MapFile, ContentError> {
    let bytes = fs::read(path).map_err(|source| ContentError::ReadFile {
        path: path.to_owned(),
        source,
    })?;

    serde_json::from_slice(&bytes).map_err(|source| ContentError::ParseFile {
        path: path.to_owned(),
        source,
    })
}

fn build_map(
    source: MapFile,
    asset_dir: &Path,
) -> Result<Map, ContentError> {
    validate_map_dimensions(&source)?;

    let assets = source
        .assets
        .iter()
        .map(|asset| build_asset(source.id, asset, asset_dir))
        .collect::<Result<Vec<_>, _>>()?;
    validate_unique_assets(source.id, &assets)?;
    validate_decorations(&source, &assets)?;
    validate_platforms(&source)?;

    Ok(Map {
        id: source.id,
        name: source.name,
        width: source.width,
        height: source.height,
        platforms: source
            .platforms
            .into_iter()
            .map(|platform| Platform {
                x: platform.x,
                y: platform.y,
                width: platform.width,
                kind: platform_kind(platform.kind) as i32,
                end_x: platform.x + platform.width,
                end_y: platform.y,
                hidden: false,
            })
            .collect(),
        decorations: source
            .decorations
            .into_iter()
            .map(|decoration| Decoration {
                asset_id: decoration.asset_id,
                x: decoration.x,
                y: decoration.y,
                width: decoration.width,
                height: decoration.height,
                layer: decoration.layer,
                flip_x: false,
            })
            .collect(),
        assets,
        ladders: Vec::new(),
        portals: Vec::new(),
        dropped_items: Vec::new(),
    })
}

fn validate_map_dimensions(source: &MapFile) -> Result<(), ContentError> {
    if source.width == 0 || source.height == 0 {
        return invalid_map(source.id, "width and height must be greater than zero");
    }
    if source.name.trim().is_empty() {
        return invalid_map(source.id, "name must not be empty");
    }
    Ok(())
}

fn validate_platforms(source: &MapFile) -> Result<(), ContentError> {
    for platform in &source.platforms {
        if !values_are_finite(&[platform.x, platform.y, platform.width]) || platform.width <= 0.0 {
            return invalid_map(
                source.id,
                "platform coordinates must be finite and width must be positive",
            );
        }
    }
    Ok(())
}

fn validate_decorations(
    source: &MapFile,
    assets: &[AssetDescriptor],
) -> Result<(), ContentError> {
    for decoration in &source.decorations {
        if !assets.iter().any(|asset| asset.id == decoration.asset_id) {
            return invalid_map(
                source.id,
                format!(
                    "decoration references unknown asset {:?}",
                    decoration.asset_id
                ),
            );
        }
        if !values_are_finite(&[
            decoration.x,
            decoration.y,
            decoration.width,
            decoration.height,
        ]) || decoration.width <= 0.0
            || decoration.height <= 0.0
        {
            return invalid_map(
                source.id,
                "decoration coordinates must be finite and dimensions must be positive",
            );
        }
    }
    Ok(())
}

fn build_asset(
    map_id: u32,
    source: &AssetFile,
    asset_dir: &Path,
) -> Result<AssetDescriptor, ContentError> {
    if source.id.trim().is_empty() {
        return invalid_map(map_id, "asset ID must not be empty");
    }
    let relative_path = Path::new(&source.path);
    if !is_safe_asset_path(relative_path) {
        return invalid_map(map_id, format!("asset path {:?} is not safe", source.path));
    }

    let path = asset_dir.join(relative_path);
    let bytes = fs::read(&path).map_err(|source| ContentError::ReadFile {
        path: path.clone(),
        source,
    })?;
    let hash = hex::encode(Sha256::digest(bytes));

    Ok(AssetDescriptor {
        id: source.id.clone(),
        url: format!("/assets/{}?v={}", source.path, &hash[..16]),
        content_hash: hash,
    })
}

fn validate_unique_assets(
    map_id: u32,
    assets: &[AssetDescriptor],
) -> Result<(), ContentError> {
    let mut ids = std::collections::HashSet::new();
    for asset in assets {
        if !ids.insert(&asset.id) {
            return invalid_map(map_id, format!("duplicate asset ID {:?}", asset.id));
        }
    }
    Ok(())
}

fn is_safe_asset_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path.components().all(|component| {
            matches!(component, Component::Normal(_))
                && component.as_os_str().to_str().is_some_and(|value| {
                    value.chars().all(|character| {
                        character.is_ascii_alphanumeric()
                            || matches!(character, '/' | '_' | '-' | '.')
                    })
                })
        })
}

fn values_are_finite(values: &[f32]) -> bool {
    values.iter().all(|value| value.is_finite())
}

fn platform_kind(kind: PlatformKindFile) -> PlatformKind {
    match kind {
        PlatformKindFile::Ground => PlatformKind::Ground,
        PlatformKindFile::Wood => PlatformKind::Wood,
    }
}

fn invalid_map<T>(
    map_id: u32,
    message: impl Into<String>,
) -> Result<T, ContentError> {
    Err(ContentError::InvalidMap {
        map_id,
        message: message.into(),
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::ContentCatalog;

    #[test]
    fn bundled_maps_and_assets_are_valid() {
        let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let catalog = ContentCatalog::load(
            &manifest_dir.join("content/maps"),
            &manifest_dir.join("assets"),
        )
        .expect("bundled content should be valid");

        let map = catalog
            .get_map(100_000_000)
            .expect("map lookup should succeed")
            .expect("starter map");
        assert_eq!(map.name, "Mossy Clearing");
        assert!(!map.assets.is_empty());
    }

    #[test]
    fn local_wz_sample_loads_when_present() {
        let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let wz_dir = manifest_dir.join("../../data");
        if !wz_dir.join("Map.wz").exists() {
            return;
        }

        let catalog = ContentCatalog::load_with_wz(
            &manifest_dir.join("content/maps"),
            &manifest_dir.join("assets"),
            &wz_dir,
        )
        .expect("sample WZ archives should be valid");
        let map = catalog
            .get_map(100_000_000)
            .expect("WZ map lookup should succeed")
            .expect("Henesys should exist");

        assert_eq!(map.name, "Henesys");
        assert!(map.platforms.iter().any(|platform| platform.hidden));
        assert!(!map.decorations.is_empty());
        assert!(!map.ladders.is_empty());
        assert!(map.portals.iter().any(|portal| {
            portal.name == "east00"
                && portal.target_map_id == 100_010_000
                && portal.target_name == "west00"
                && !portal.frames.is_empty()
        }));
        if wz_dir.join("Character.wz").exists() && wz_dir.join("UI.wz").exists() {
            let gui = catalog.game_gui();
            assert_eq!(gui.items.len(), 6);
            assert!(gui.equipment_window.is_some());
            assert!(gui.inventory_window.is_some());
            assert!(gui.items.iter().all(|item| {
                gui.assets
                    .iter()
                    .any(|asset| asset.id == item.icon_asset_id)
            }));
        }
        for descriptor in &map.assets {
            let asset = catalog
                .get_wz_asset(&descriptor.id)
                .expect("map asset should be registered");
            let png = asset.png_bytes().expect("map asset should decode");
            assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
        }
    }
}
