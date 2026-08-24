use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::sync::RwLock;

use oozems_proto::v1::AssetDescriptor;
use oozems_proto::v1::Npc;
use oozems_proto::v1::NpcAnimation;
use oozems_proto::v1::NpcFrame;
use oozems_proto::v1::Platform;
use oozems_proto::v1::Vec2;
use sha2::Digest;
use sha2::Sha256;
use wz_reader::WzNodeArc;
use wz_reader::WzNodeCast;
use wz_reader::property::Vector2D;

use super::super::config::NpcFilter;
use super::Bounds;
use super::WzAsset;
use super::WzContentError;
use super::archive_fingerprint;
use super::child;
use super::find_png_descendant;
use super::foothold::attached_platform;
use super::int_value;
use super::lock_error;
use super::node_name;
use super::node_path;
use super::open_archive;
use super::parse;
use super::sorted_children;
use super::string_value;
use super::vector_value;
use super::wrap_archive_root;

const NPC_ARCHIVE: &str = "Npc.wz";
const STRING_ARCHIVE: &str = "String.wz";
const DEFAULT_FRAME_DELAY_MS: u32 = 100;

pub(super) struct NpcContent {
    _base: WzNodeArc,
    _string_base: Option<WzNodeArc>,
    root: WzNodeArc,
    strings: Option<WzNodeArc>,
    fingerprint: String,
    definitions: RwLock<HashMap<u32, LoadedNpcDefinition>>,
    assets: RwLock<HashMap<String, Arc<WzAsset>>>,
}

#[derive(Clone)]
struct LoadedNpcDefinition {
    animations: Vec<NpcAnimation>,
    assets: Vec<AssetDescriptor>,
    name: String,
    function: String,
    ambient_lines: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct RawNpcSpawn {
    spawn_id: u32,
    npc_id: u32,
    x: i32,
    y: i32,
    center_y: i32,
    flip_x: bool,
    foothold_id: u32,
    limited_name: Option<String>,
}

impl NpcContent {
    pub(super) fn open_optional(directory: &Path) -> Result<Option<Self>, WzContentError> {
        let path = directory.join(NPC_ARCHIVE);
        let exists = path
            .try_exists()
            .map_err(|source| WzContentError::Metadata {
                path: path.clone(),
                source,
            })?;
        if !exists {
            tracing::warn!(path = %path.display(), "Npc.wz is absent; map NPCs will not be displayed");
            return Ok(None);
        }

        let root = open_archive(&path)?;
        let base = wrap_archive_root(&root)?;
        parse(&root, format!("{} root", path.display()))?;
        let fingerprint = archive_fingerprint(&path)?;
        let (string_base, strings) = open_npc_strings(directory)?;

        tracing::info!(path = %path.display(), "WZ NPC source ready");
        Ok(Some(Self {
            _base: base,
            _string_base: string_base,
            root,
            strings,
            fingerprint,
            definitions: RwLock::new(HashMap::new()),
            assets: RwLock::new(HashMap::new()),
        }))
    }

    fn get_definition(
        &self,
        npc_id: u32,
    ) -> Result<Option<LoadedNpcDefinition>, WzContentError> {
        self.load_definition(npc_id, &mut HashSet::new())
    }

    fn load_definition(
        &self,
        npc_id: u32,
        visited: &mut HashSet<u32>,
    ) -> Result<Option<LoadedNpcDefinition>, WzContentError> {
        if let Some(definition) = self
            .definitions
            .read()
            .map_err(|_| lock_error("WZ NPC definition cache"))?
            .get(&npc_id)
            .cloned()
        {
            return Ok(Some(definition));
        }
        if !visited.insert(npc_id) {
            tracing::warn!(npc_id, "NPC definition link contains a cycle");
            return Ok(None);
        }

        let image_name = format!("{npc_id:07}.img");
        let Some(node) = child(&self.root, &image_name)? else {
            tracing::warn!(npc_id, "NPC definition is absent from Npc.wz");
            return Ok(None);
        };
        parse(&node, format!("NPC {npc_id}"))?;
        if let Some(linked_id) = linked_definition_id(&node)?
            && linked_id != npc_id
            && let Some(definition) = self.load_definition(linked_id, visited)?
        {
            let definition = with_presentation(self, npc_id, &node, definition)?;
            self.definitions
                .write()
                .map_err(|_| lock_error("WZ NPC definition cache"))?
                .insert(npc_id, definition.clone());
            return Ok(Some(definition));
        }
        let definition = with_presentation(self, npc_id, &node, build_definition(self, &node)?)?;
        if definition.animations.is_empty() {
            tracing::warn!(npc_id, "NPC definition has no displayable animation frames");
            return Ok(None);
        }
        self.definitions
            .write()
            .map_err(|_| lock_error("WZ NPC definition cache"))?
            .insert(npc_id, definition.clone());
        Ok(Some(definition))
    }

    pub(super) fn get_asset(
        &self,
        asset_id: &str,
    ) -> Option<Arc<WzAsset>> {
        self.assets.read().ok()?.get(asset_id).cloned()
    }

    fn register_asset(
        &self,
        source_path: &str,
        node: &WzNodeArc,
    ) -> Result<AssetDescriptor, WzContentError> {
        let version = hex::encode(Sha256::digest(
            format!("{}\0{source_path}", self.fingerprint).as_bytes(),
        ));
        let id = format!("wz-{version}");
        let asset = Arc::new(WzAsset::new(id.clone(), Arc::clone(node)));
        self.assets
            .write()
            .map_err(|_| lock_error("WZ NPC asset registry"))?
            .entry(id.clone())
            .or_insert(asset);
        Ok(AssetDescriptor {
            id,
            url: format!("/wz-assets/{version}.png"),
        })
    }
}

fn open_npc_strings(
    directory: &Path
) -> Result<(Option<WzNodeArc>, Option<WzNodeArc>), WzContentError> {
    let path = directory.join(STRING_ARCHIVE);
    if !path
        .try_exists()
        .map_err(|source| WzContentError::Metadata {
            path: path.clone(),
            source,
        })?
    {
        tracing::warn!(path = %path.display(), "String.wz is absent; NPC names and speech will be unavailable");
        return Ok((None, None));
    }
    let root = open_archive(&path)?;
    let base = wrap_archive_root(&root)?;
    parse(&root, format!("{} root", path.display()))?;
    let Some(strings) = child(&root, "Npc.img")? else {
        tracing::warn!(path = %path.display(), "String.wz has no Npc.img; NPC names and speech will be unavailable");
        return Ok((Some(base), None));
    };
    parse(&strings, format!("{} Npc.img", path.display()))?;
    Ok((Some(base), Some(strings)))
}

fn with_presentation(
    content: &NpcContent,
    npc_id: u32,
    node: &WzNodeArc,
    mut definition: LoadedNpcDefinition,
) -> Result<LoadedNpcDefinition, WzContentError> {
    definition.name = format!("NPC {npc_id}");
    definition.function.clear();
    definition.ambient_lines.clear();
    let Some(strings) = &content.strings else {
        return Ok(definition);
    };
    let Some(entry) = child(strings, &npc_id.to_string())? else {
        return Ok(definition);
    };
    definition.name = string_value(&entry, "name")?
        .filter(|name| !name.is_empty())
        .unwrap_or(definition.name);
    definition.function = string_value(&entry, "func")?.unwrap_or_default();
    let Some(info) = child(node, "info")? else {
        return Ok(definition);
    };
    let Some(speak) = child(&info, "speak")? else {
        return Ok(definition);
    };
    for key_node in sorted_children(&speak)? {
        let child_name = node_name(&key_node)?;
        let Some(key) = string_value(&speak, &child_name)? else {
            continue;
        };
        if let Some(line) = string_value(&entry, &key)?.filter(|line| !line.is_empty()) {
            definition.ambient_lines.push(line);
        }
    }
    Ok(definition)
}

fn linked_definition_id(node: &WzNodeArc) -> Result<Option<u32>, WzContentError> {
    let Some(info) = child(node, "info")? else {
        return Ok(None);
    };
    if let Some(value) = int_value(&info, "link")? {
        return Ok(u32::try_from(value).ok());
    }
    Ok(string_value(&info, "link")?.and_then(|value| value.parse::<u32>().ok()))
}

pub(super) fn read_spawn_points(map: &WzNodeArc) -> Result<Vec<RawNpcSpawn>, WzContentError> {
    let Some(life) = child(map, "life")? else {
        return Ok(Vec::new());
    };
    sorted_children(&life)?
        .into_iter()
        .map(|node| read_spawn_point(&node))
        .filter_map(Result::transpose)
        .collect()
}

fn read_spawn_point(node: &WzNodeArc) -> Result<Option<RawNpcSpawn>, WzContentError> {
    if string_value(node, "type")?.as_deref() != Some("n")
        || int_value(node, "hide")?.unwrap_or_default() != 0
    {
        return Ok(None);
    }
    let values = (
        node_name(node)?.parse::<u32>().ok(),
        string_value(node, "id")?.and_then(|value| value.parse::<u32>().ok()),
        int_value(node, "x")?,
        int_value(node, "y")?,
    );
    let (Some(spawn_id), Some(npc_id), Some(x), Some(y)) = values else {
        tracing::warn!(path = %node_path(node)?, "skipping incomplete WZ NPC spawn point");
        return Ok(None);
    };

    let limited_name = if child(node, "limitedname")?.is_some() {
        Some(string_value(node, "limitedname")?.unwrap_or_default())
    } else {
        None
    };
    Ok(Some(RawNpcSpawn {
        spawn_id,
        npc_id,
        x,
        y,
        center_y: int_value(node, "cy")?.unwrap_or(y),
        flip_x: int_value(node, "f")?.unwrap_or_default() != 0,
        foothold_id: nonnegative_u32(int_value(node, "fh")?.unwrap_or_default()),
        limited_name,
    }))
}

pub(super) fn filter_spawn_points(
    spawn_points: Vec<RawNpcSpawn>,
    filter: &NpcFilter,
) -> Vec<RawNpcSpawn> {
    spawn_points
        .into_iter()
        .filter(|spawn| filter.allows(spawn.npc_id, spawn.limited_name.as_deref()))
        .collect()
}

pub(super) fn build_npcs(
    content: Option<&NpcContent>,
    spawn_points: Vec<RawNpcSpawn>,
    platforms: &[Platform],
    bounds: Bounds,
    assets: &mut Vec<AssetDescriptor>,
    asset_ids: &mut HashSet<String>,
) -> Result<Vec<Npc>, WzContentError> {
    let Some(content) = content else {
        return Ok(Vec::new());
    };
    let mut npcs = Vec::with_capacity(spawn_points.len());
    for spawn in spawn_points {
        let Some(definition) = content.get_definition(spawn.npc_id)? else {
            continue;
        };
        for asset in definition.assets {
            if asset_ids.insert(asset.id.clone()) {
                assets.push(asset);
            }
        }
        npcs.push(build_npc(
            spawn,
            definition.animations,
            definition.name,
            definition.function,
            definition.ambient_lines,
            platforms,
            bounds,
        ));
    }
    Ok(npcs)
}

fn build_npc(
    source: RawNpcSpawn,
    animations: Vec<NpcAnimation>,
    name: String,
    function: String,
    ambient_lines: Vec<String>,
    platforms: &[Platform],
    bounds: Bounds,
) -> Npc {
    let x = (source.x - bounds.left) as f32;
    let y = (source.y - bounds.top) as f32;
    let center_y = (source.center_y - bounds.top) as f32;
    let (layer, surface_y) =
        attached_platform(platforms, source.foothold_id, x, center_y).unwrap_or((0, y));
    Npc {
        spawn_id: source.spawn_id,
        npc_id: source.npc_id,
        position: Some(Vec2 { x, y: surface_y }),
        flip_x: source.flip_x,
        layer,
        name,
        function,
        ambient_lines,
        animations,
    }
}

fn build_definition(
    content: &NpcContent,
    node: &WzNodeArc,
) -> Result<LoadedNpcDefinition, WzContentError> {
    let mut assets = Vec::new();
    let animations = read_animations(content, node, &mut assets)?;
    let mut asset_ids = HashSet::new();
    assets.retain(|asset| asset_ids.insert(asset.id.clone()));
    Ok(LoadedNpcDefinition {
        animations,
        assets,
        name: String::new(),
        function: String::new(),
        ambient_lines: Vec::new(),
    })
}

fn read_animations(
    content: &NpcContent,
    npc: &WzNodeArc,
    assets: &mut Vec<AssetDescriptor>,
) -> Result<Vec<NpcAnimation>, WzContentError> {
    let mut animations = Vec::new();
    let mut names = HashSet::new();
    for animation in sorted_children(npc)? {
        let name = node_name(&animation)?;
        if name == "info" || !has_numeric_frames(&animation)? {
            continue;
        }
        if name.is_empty() {
            return Err(WzContentError::InvalidAsset {
                asset_id: node_path(&animation)?,
                message: "NPC animation name is empty".to_owned(),
            });
        }
        if !names.insert(name.clone()) {
            return Err(WzContentError::InvalidAsset {
                asset_id: node_path(&animation)?,
                message: format!("NPC animation name {name:?} appears more than once"),
            });
        }
        let frames = read_animation_frames(content, &animation, assets)?;
        if !frames.is_empty() {
            animations.push(NpcAnimation { name, frames });
        }
    }
    Ok(animations)
}

fn has_numeric_frames(animation: &WzNodeArc) -> Result<bool, WzContentError> {
    Ok(sorted_children(animation)?
        .into_iter()
        .any(|frame| node_name(&frame).is_ok_and(|name| name.parse::<u32>().is_ok())))
}

fn read_animation_frames(
    content: &NpcContent,
    animation: &WzNodeArc,
    assets: &mut Vec<AssetDescriptor>,
) -> Result<Vec<NpcFrame>, WzContentError> {
    let mut frames = Vec::new();
    for frame in sorted_children(animation)? {
        if node_name(&frame)?.parse::<u32>().is_err() {
            continue;
        }
        let Some(source) = find_png_descendant(&frame, 0)? else {
            continue;
        };
        let asset = content.register_asset(&node_path(&source)?, &source)?;
        let (width, height) = png_dimensions(&source)?;
        let origin = match child(&source, "origin")? {
            Some(origin) => vector_value(&origin)?,
            None => None,
        };
        let Vector2D(origin_x, origin_y) = origin.unwrap_or(Vector2D(0, 0));
        let delay_ms = int_value(&source, "delay")?
            .and_then(|value| u32::try_from(value).ok())
            .filter(|value| *value > 0)
            .unwrap_or(DEFAULT_FRAME_DELAY_MS);
        assets.push(asset.clone());
        frames.push(NpcFrame {
            asset_id: asset.id,
            width: width as f32,
            height: height as f32,
            origin_x: origin_x as f32,
            origin_y: origin_y as f32,
            delay_ms,
        });
    }
    Ok(frames)
}

fn png_dimensions(node: &WzNodeArc) -> Result<(u32, u32), WzContentError> {
    let read = node.read().map_err(|_| lock_error("WZ NPC PNG geometry"))?;
    let png = read
        .try_as_png()
        .ok_or_else(|| WzContentError::InvalidAsset {
            asset_id: read.get_full_path(),
            message: "NPC frame is not a PNG property".to_owned(),
        })?;
    Ok((png.width, png.height))
}

fn nonnegative_u32(value: i32) -> u32 {
    u32::try_from(value.max(0)).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use oozems_proto::v1::NpcFrame;
    use oozems_proto::v1::Platform;

    use super::Bounds;
    use super::RawNpcSpawn;
    use super::build_npc;
    use super::linked_definition_id;

    #[test]
    fn linked_definition_ids_accept_numeric_wz_values() {
        let npc = wz_reader::WzNode::from_str("npc", 0, None).into_lock();
        let info = wz_reader::WzNode::from_str("info", 0, Some(&npc)).into_lock();
        let link = wz_reader::WzNode::from_str("link", 1_012_000, Some(&info)).into_lock();
        info.write().expect("info lock").add(&link);
        npc.write().expect("NPC lock").add(&info);

        assert_eq!(
            linked_definition_id(&npc).expect("linked NPC ID"),
            Some(1_012_000)
        );
    }

    #[test]
    fn npc_uses_its_attached_foothold_and_layer() {
        let platforms = vec![Platform {
            id: 7,
            x: 0.0,
            y: 100.0,
            end_x: 200.0,
            end_y: 120.0,
            layer: 3,
        }];
        let npc = build_npc(
            RawNpcSpawn {
                spawn_id: 4,
                npc_id: 1_012_000,
                x: 100,
                y: 90,
                center_y: 110,
                flip_x: true,
                foothold_id: 7,
                limited_name: None,
            },
            vec![oozems_proto::v1::NpcAnimation {
                name: "stand".to_owned(),
                frames: vec![NpcFrame::default()],
            }],
            "Regular Cab".to_owned(),
            String::new(),
            Vec::new(),
            &platforms,
            Bounds {
                left: 0,
                top: 0,
                right: 400,
                bottom: 400,
            },
        );

        assert_eq!(npc.position.expect("position").y, 110.0);
        assert_eq!(npc.layer, 3);
        assert!(npc.flip_x);
        assert_eq!(npc.animations.len(), 1);
        assert_eq!(npc.animations[0].name, "stand");
        assert_eq!(npc.animations[0].frames.len(), 1);
        assert_eq!(npc.name, "Regular Cab");
    }
}
