use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::RwLock;

use oozems_proto::v1::AssetDescriptor;
use oozems_proto::v1::Decoration;
use oozems_proto::v1::DecorationFrame;
use oozems_proto::v1::Map;
use oozems_proto::v1::Platform;
use sha2::Digest;
use sha2::Sha256;
use thiserror::Error;
use wz_reader::WzNodeArc;
use wz_reader::WzNodeCast;
use wz_reader::WzObjectType;
use wz_reader::property::Vector2D;
use wz_reader::property::WzPngParseError;
use wz_reader::util::node_util::parse_node;

mod archive;
mod asset;
mod features;
mod foothold;
mod mob;
mod movement_bounds;
mod names;
mod npc;
mod reactor;

pub(super) use archive::archive_fingerprint;
pub(super) use archive::open_archive;
pub(super) use archive::wrap_archive_root;
pub(crate) use asset::WzAsset;
use features::RawLadder;
use features::RawPortal;

use super::config::NpcFilter;

const MAP_ARCHIVE: &str = "Map.wz";
const STRING_ARCHIVE: &str = "String.wz";
const REACTOR_ARCHIVE: &str = "Reactor.wz";
const DEFAULT_DECORATION_FRAME_DELAY_MS: u32 = 100;

pub struct WzContent {
    _base: WzNodeArc,
    root: WzNodeArc,
    map_nodes: HashMap<u32, WzNodeArc>,
    map_names: HashMap<u32, String>,
    fingerprint: String,
    maps: RwLock<HashMap<u32, Map>>,
    assets: RwLock<HashMap<String, Arc<WzAsset>>>,
    mobs: Option<mob::MobContent>,
    npcs: Option<npc::NpcContent>,
    reactors: Option<reactor::ReactorContent>,
    npc_filter: NpcFilter,
}

#[derive(Debug, Error)]
pub enum WzContentError {
    #[error("failed to inspect WZ archive {path}")]
    Metadata {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("Map.wz is required at {path}")]
    MissingMapArchive { path: PathBuf },
    #[error("failed to open WZ archive {path}")]
    Open {
        path: PathBuf,
        #[source]
        source: wz_reader::node::Error,
    },
    #[error("failed to parse WZ data at {context}")]
    Parse {
        context: String,
        #[source]
        source: wz_reader::node::Error,
    },
    #[error("WZ map {map_id} has invalid geometry: {message}")]
    InvalidMap { map_id: u32, message: String },
    #[error("WZ map ID {map_id} appears more than once in {path}")]
    DuplicateMap { map_id: u32, path: PathBuf },
    #[error("internal WZ data lock was poisoned while accessing {context}")]
    Lock { context: &'static str },
    #[error("failed to decode WZ asset {asset_id}")]
    DecodeAsset {
        asset_id: String,
        #[source]
        source: WzPngParseError,
    },
    #[error("failed to encode WZ asset {asset_id} as PNG")]
    EncodeAsset {
        asset_id: String,
        #[source]
        source: image::ImageError,
    },
    #[error("WZ asset {asset_id} is invalid: {message}")]
    InvalidAsset { asset_id: String, message: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Bounds {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct MapMetadata {
    return_map_id: Option<u32>,
    town: bool,
}

#[derive(Clone, Copy, Debug)]
struct RawPlatform {
    id: u32,
    x1: i32,
    y1: i32,
    x2: i32,
    y2: i32,
    layer: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum DecorationKind {
    Object,
    Tile,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct DecorationOrder {
    layer: i32,
    kind: DecorationKind,
    primary_z: i32,
    secondary_z: i32,
}

#[derive(Clone)]
struct RawSprite {
    source: WzNodeArc,
    source_path: String,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    flip_x: bool,
}

struct RawDecorationFrame {
    sprite: RawSprite,
    delay_ms: u32,
}

struct RawDecoration {
    first_frame: RawDecorationFrame,
    remaining_frames: Vec<RawDecorationFrame>,
    order: DecorationOrder,
}

impl RawDecoration {
    fn frames(&self) -> impl Iterator<Item = &RawDecorationFrame> {
        std::iter::once(&self.first_frame).chain(&self.remaining_frames)
    }
}

impl WzContent {
    pub fn open(
        directory: &Path,
        npc_filter: NpcFilter,
    ) -> Result<Self, WzContentError> {
        let map_path = directory.join(MAP_ARCHIVE);
        let exists = map_path
            .try_exists()
            .map_err(|source| WzContentError::Metadata {
                path: map_path.clone(),
                source,
            })?;
        if !exists {
            return Err(WzContentError::MissingMapArchive { path: map_path });
        }

        let root = open_archive(&map_path)?;
        let base = wrap_archive_root(&root)?;
        parse(&root, format!("{} root", map_path.display()))?;
        let map_nodes = index_map_nodes(&root, &map_path)?;
        let map_names = names::load_map_names(&directory.join(STRING_ARCHIVE))?;
        let fingerprint = archive_fingerprint(&map_path)?;
        let mobs = mob::MobContent::open_optional(directory)?;
        let npcs = npc::NpcContent::open_optional(directory)?;
        let reactors = reactor::ReactorContent::open_optional(directory)?;

        tracing::info!(
            path = %map_path.display(),
            maps = map_nodes.len(),
            "WZ map source ready"
        );

        Ok(Self {
            _base: base,
            root,
            map_nodes,
            map_names,
            fingerprint,
            maps: RwLock::new(HashMap::new()),
            assets: RwLock::new(HashMap::new()),
            mobs,
            npcs,
            reactors,
            npc_filter,
        })
    }

    pub fn contains_map(
        &self,
        map_id: u32,
    ) -> bool {
        self.map_nodes.contains_key(&map_id)
    }

    pub fn get_map(
        &self,
        map_id: u32,
    ) -> Result<Map, WzContentError> {
        if let Some(map) = self
            .maps
            .read()
            .map_err(|_| lock_error("WZ map cache"))?
            .get(&map_id)
            .cloned()
        {
            return Ok(map);
        }

        let map_node =
            self.map_nodes
                .get(&map_id)
                .cloned()
                .ok_or_else(|| WzContentError::InvalidMap {
                    map_id,
                    message: "map is not present in the archive index".to_owned(),
                })?;
        parse(&map_node, format!("map {map_id}"))?;
        let map = build_map(self, map_id, &map_node)?;

        self.maps
            .write()
            .map_err(|_| lock_error("WZ map cache"))?
            .insert(map_id, map.clone());
        Ok(map)
    }

    pub fn get_asset(
        &self,
        asset_id: &str,
    ) -> Option<Arc<WzAsset>> {
        self.assets
            .read()
            .ok()
            .and_then(|assets| assets.get(asset_id).cloned())
            .or_else(|| self.mobs.as_ref().and_then(|mobs| mobs.get_asset(asset_id)))
            .or_else(|| self.npcs.as_ref().and_then(|npcs| npcs.get_asset(asset_id)))
            .or_else(|| {
                self.reactors
                    .as_ref()
                    .and_then(|reactors| reactors.get_asset(asset_id))
            })
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
            .map_err(|_| lock_error("WZ asset registry"))?
            .entry(id.clone())
            .or_insert(asset);

        Ok(AssetDescriptor {
            id,
            url: format!("/wz-assets/{version}.png"),
        })
    }
}

pub(super) fn parse(
    node: &WzNodeArc,
    context: String,
) -> Result<(), WzContentError> {
    parse_node(node).map_err(|source| WzContentError::Parse { context, source })
}

fn index_map_nodes(
    root: &WzNodeArc,
    path: &Path,
) -> Result<HashMap<u32, WzNodeArc>, WzContentError> {
    let map_directory = child(root, "Map")?.ok_or_else(|| WzContentError::InvalidMap {
        map_id: 0,
        message: format!("{} does not contain a Map directory", path.display()),
    })?;
    let mut maps = HashMap::new();
    index_map_directory(&map_directory, path, &mut maps)?;
    Ok(maps)
}

fn index_map_directory(
    directory: &WzNodeArc,
    path: &Path,
    maps: &mut HashMap<u32, WzNodeArc>,
) -> Result<(), WzContentError> {
    parse(directory, node_path(directory)?)?;
    for child in children(directory)? {
        let (name, is_image, is_directory) = {
            let read = child.read().map_err(|_| lock_error("WZ map index"))?;
            (
                read.name.to_string(),
                matches!(&read.object_type, WzObjectType::Image(_)),
                matches!(&read.object_type, WzObjectType::Directory(_)),
            )
        };
        if is_image {
            let Some(map_id) = name
                .strip_suffix(".img")
                .and_then(|value| value.parse::<u32>().ok())
            else {
                continue;
            };
            if maps.insert(map_id, child).is_some() {
                return Err(WzContentError::DuplicateMap {
                    map_id,
                    path: path.to_owned(),
                });
            }
        } else if is_directory {
            index_map_directory(&child, path, maps)?;
        }
    }
    Ok(())
}

fn build_map(
    source: &WzContent,
    map_id: u32,
    node: &WzNodeArc,
) -> Result<Map, WzContentError> {
    let metadata = read_map_metadata(node, map_id)?;
    let raw_platforms = read_platforms(node)?;
    let mut raw_decorations = read_decorations(&source.root, node, map_id)?;
    let raw_ladders = features::read_ladders(node)?;
    let raw_portals = features::read_portals(&source.root, node)?;
    let raw_mob_spawns = mob::read_spawn_points(node)?;
    let raw_npc_spawns =
        npc::filter_spawn_points(npc::read_spawn_points(node)?, &source.npc_filter);
    let raw_reactor_spawns = reactor::read_spawn_points(node)?;
    // zM associates an item with a foothold group. It does not control drawing.
    // The order key keeps the separate WZ draw-order fields comparable.
    raw_decorations.sort_by_key(|decoration| decoration.order);
    let bounds = read_bounds(node)?.unwrap_or_else(|| {
        derive_bounds(
            raw_platforms.iter(),
            raw_decorations.iter(),
            &raw_ladders,
            &raw_portals,
        )
        .unwrap_or(Bounds {
            left: 0,
            top: 0,
            right: 800,
            bottom: 600,
        })
    });
    validate_bounds(map_id, bounds)?;
    let portal_xs = raw_portals
        .iter()
        .map(|portal| portal.x)
        .collect::<Vec<_>>();
    let movement_bounds = movement_bounds::build(&raw_platforms, &portal_xs, bounds);

    let platforms = build_platforms(raw_platforms, bounds);
    let mut assets = Vec::new();
    let mut asset_ids = HashSet::new();
    let mut decorations = Vec::with_capacity(raw_decorations.len());
    for decoration in raw_decorations {
        decorations.push(build_decoration(
            source,
            decoration,
            bounds,
            &mut assets,
            &mut asset_ids,
        )?);
    }
    let ladders = features::build_ladders(raw_ladders, bounds);
    let portals = features::build_portals(
        source,
        raw_portals,
        &platforms,
        bounds,
        &mut assets,
        &mut asset_ids,
    )?;
    let mob_spawn_points = mob::build_spawn_points(raw_mob_spawns, &platforms, bounds);
    let mob_definitions = mob::load_definitions(
        source.mobs.as_ref(),
        &mob_spawn_points,
        &mut assets,
        &mut asset_ids,
    )?;
    let npcs = npc::build_npcs(
        source.npcs.as_ref(),
        raw_npc_spawns,
        &platforms,
        bounds,
        &mut assets,
        &mut asset_ids,
    )?;
    let reactor_spawn_points = reactor::build_spawn_points(raw_reactor_spawns, &platforms, bounds);
    let reactor_definitions = reactor::load_definitions(
        source.reactors.as_ref(),
        &reactor_spawn_points,
        &mut assets,
        &mut asset_ids,
    )?;

    Ok(Map {
        id: map_id,
        name: source
            .map_names
            .get(&map_id)
            .cloned()
            .unwrap_or_else(|| format!("Map {map_id}")),
        width: (bounds.right - bounds.left) as u32,
        height: (bounds.bottom - bounds.top) as u32,
        platforms,
        decorations,
        assets,
        ladders,
        portals,
        dropped_items: Vec::new(),
        mob_spawn_points,
        mob_definitions,
        mobs: Vec::new(),
        mob_projectiles: Vec::new(),
        simulation_sequence: 0,
        npcs,
        movement_bounds: Some(movement_bounds),
        return_map_id: metadata.return_map_id,
        town: metadata.town,
        reactor_spawn_points,
        reactor_definitions,
        reactors: Vec::new(),
    })
}

fn read_map_metadata(
    node: &WzNodeArc,
    map_id: u32,
) -> Result<MapMetadata, WzContentError> {
    let Some(info) = child(node, "info")? else {
        return Ok(MapMetadata::default());
    };
    let return_map_id = int_value(&info, "returnMap")?
        .map(|value| {
            u32::try_from(value).map_err(|_| WzContentError::InvalidMap {
                map_id,
                message: format!("returnMap must be nonnegative, not {value}"),
            })
        })
        .transpose()?;
    let town = match int_value(&info, "town")? {
        None | Some(0) => false,
        Some(1) => true,
        Some(value) => {
            return Err(WzContentError::InvalidMap {
                map_id,
                message: format!("town must be 0 or 1, not {value}"),
            });
        }
    };
    Ok(MapMetadata {
        return_map_id,
        town,
    })
}

fn read_platforms(node: &WzNodeArc) -> Result<Vec<RawPlatform>, WzContentError> {
    let Some(footholds) = child(node, "foothold")? else {
        return Ok(Vec::new());
    };
    let mut output = Vec::new();
    for layer_index in 0..=7 {
        let Some(layer) = child(&footholds, &layer_index.to_string())? else {
            continue;
        };
        collect_platforms(&layer, layer_index, &mut output)?;
    }
    Ok(output)
}

fn collect_platforms(
    node: &WzNodeArc,
    layer: i32,
    output: &mut Vec<RawPlatform>,
) -> Result<(), WzContentError> {
    let values = ["x1", "y1", "x2", "y2"]
        .map(|name| int_value(node, name))
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?;
    if let [Some(x1), Some(y1), Some(x2), Some(y2)] = values.as_slice() {
        output.push(RawPlatform {
            id: node_name(node)?.parse::<u32>().unwrap_or_default(),
            x1: *x1,
            y1: *y1,
            x2: *x2,
            y2: *y2,
            layer,
        });
        return Ok(());
    }

    for child in children(node)? {
        collect_platforms(&child, layer, output)?;
    }
    Ok(())
}

fn read_decorations(
    root: &WzNodeArc,
    map: &WzNodeArc,
    map_id: u32,
) -> Result<Vec<RawDecoration>, WzContentError> {
    let mut output = Vec::new();
    for layer_index in 0..=7 {
        let Some(layer) = child(map, &layer_index.to_string())? else {
            continue;
        };
        read_tiles(root, &layer, map_id, layer_index, &mut output)?;
        read_objects(root, &layer, layer_index, &mut output)?;
    }
    Ok(output)
}

fn read_tiles(
    root: &WzNodeArc,
    layer: &WzNodeArc,
    map_id: u32,
    layer_index: i32,
    output: &mut Vec<RawDecoration>,
) -> Result<(), WzContentError> {
    let tile_set = match child(layer, "info")? {
        Some(info) => string_value(&info, "tS")?,
        None => None,
    };
    let (Some(tile_set), Some(tiles)) = (tile_set, child(layer, "tile")?) else {
        return Ok(());
    };

    for tile in sorted_children(&tiles)? {
        let (Some(unit), Some(number), Some(x), Some(y)) = (
            string_value(&tile, "u")?,
            int_value(&tile, "no")?,
            int_value(&tile, "x")?,
            int_value(&tile, "y")?,
        ) else {
            continue;
        };
        let path = format!("Tile/{tile_set}.img/{unit}/{number}");
        let Some(source) = resolve_png_source(root, &path)? else {
            tracing::debug!(%path, "skipping WZ tile without a PNG source");
            continue;
        };
        let source_z = int_value(&source, "z")?.unwrap_or_default();
        let instance_z =
            node_name(&tile)?
                .parse::<i32>()
                .map_err(|error| WzContentError::InvalidMap {
                    map_id,
                    message: format!("tile instance has a non-numeric name: {error}"),
                })?;
        output.push(raw_decoration(
            source,
            x,
            y,
            tile_order(layer_index, source_z, instance_z),
            int_value(&tile, "f")?.unwrap_or_default() != 0,
        )?);
    }
    Ok(())
}

fn read_objects(
    root: &WzNodeArc,
    layer: &WzNodeArc,
    layer_index: i32,
    output: &mut Vec<RawDecoration>,
) -> Result<(), WzContentError> {
    let Some(objects) = child(layer, "obj")? else {
        return Ok(());
    };
    for object in sorted_children(&objects)? {
        if int_value(&object, "hide")?.unwrap_or_default() != 0 {
            continue;
        }
        let parts = ["oS", "l0", "l1", "l2"]
            .map(|name| string_value(&object, name))
            .into_iter()
            .collect::<Result<Vec<_>, _>>()?;
        let [Some(object_set), Some(level0), Some(level1), Some(level2)] = parts.as_slice() else {
            continue;
        };
        let (Some(x), Some(y)) = (int_value(&object, "x")?, int_value(&object, "y")?) else {
            continue;
        };
        let base_path = format!("Obj/{object_set}.img/{level0}/{level1}/{level2}");
        let sources = resolve_png_frames(root, &base_path)?;
        if sources.is_empty() {
            tracing::debug!(path = %base_path, "skipping WZ object without a PNG source");
            continue;
        }
        output.push(raw_animated_decoration(
            sources,
            x,
            y,
            object_order(layer_index, int_value(&object, "z")?.unwrap_or_default()),
            int_value(&object, "f")?.unwrap_or_default() != 0,
        )?);
    }
    Ok(())
}

fn resolve_png_source(
    root: &WzNodeArc,
    path: &str,
) -> Result<Option<WzNodeArc>, WzContentError> {
    let Some(node) = node_at_path(root, path)? else {
        return Ok(None);
    };
    find_png_descendant(&node, 0)
}

fn resolve_png_frames(
    root: &WzNodeArc,
    path: &str,
) -> Result<Vec<WzNodeArc>, WzContentError> {
    let Some(node) = node_at_path(root, path)? else {
        return Ok(Vec::new());
    };
    let mut frames = Vec::new();
    for (name, child) in sorted_named_children(&node)? {
        if name.parse::<u32>().is_err() {
            continue;
        }
        if let Some(source) = find_png_descendant(&child, 0)? {
            frames.push(source);
        }
    }
    if frames.is_empty()
        && let Some(source) = find_png_descendant(&node, 0)?
    {
        frames.push(source);
    }
    Ok(frames)
}

fn node_at_path(
    root: &WzNodeArc,
    path: &str,
) -> Result<Option<WzNodeArc>, WzContentError> {
    match root
        .read()
        .map_err(|_| lock_error("WZ archive root"))?
        .at_path_parsed(path)
    {
        Ok(node) => Ok(Some(node)),
        Err(wz_reader::node::Error::NodeNotFound) => Ok(None),
        Err(source) => Err(WzContentError::Parse {
            context: path.to_owned(),
            source,
        }),
    }
}

pub(super) fn find_png_descendant(
    node: &WzNodeArc,
    depth: usize,
) -> Result<Option<WzNodeArc>, WzContentError> {
    if node
        .read()
        .map_err(|_| lock_error("WZ PNG source"))?
        .try_as_png()
        .is_some()
    {
        return Ok(Some(Arc::clone(node)));
    }
    if depth >= 8 {
        return Ok(None);
    }
    for child in sorted_children(node)? {
        if let Some(found) = find_png_descendant(&child, depth + 1)? {
            return Ok(Some(found));
        }
    }
    Ok(None)
}

fn raw_decoration(
    source: WzNodeArc,
    anchor_x: i32,
    anchor_y: i32,
    order: DecorationOrder,
    flip_x: bool,
) -> Result<RawDecoration, WzContentError> {
    raw_animated_decoration(vec![source], anchor_x, anchor_y, order, flip_x)
}

fn raw_animated_decoration(
    sources: Vec<WzNodeArc>,
    anchor_x: i32,
    anchor_y: i32,
    order: DecorationOrder,
    flip_x: bool,
) -> Result<RawDecoration, WzContentError> {
    let mut frames = sources
        .into_iter()
        .map(|source| raw_decoration_frame(source, anchor_x, anchor_y, flip_x))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter();
    let first_frame = frames.next().ok_or_else(|| WzContentError::InvalidMap {
        map_id: 0,
        message: "map decoration has no image frames".to_owned(),
    })?;
    Ok(RawDecoration {
        first_frame,
        remaining_frames: frames.collect(),
        order,
    })
}

fn raw_decoration_frame(
    source: WzNodeArc,
    anchor_x: i32,
    anchor_y: i32,
    flip_x: bool,
) -> Result<RawDecorationFrame, WzContentError> {
    let delay_ms = int_value(&source, "delay")?
        .and_then(|delay| u32::try_from(delay).ok())
        .filter(|delay| *delay > 0)
        .unwrap_or(DEFAULT_DECORATION_FRAME_DELAY_MS);
    Ok(RawDecorationFrame {
        sprite: raw_sprite(source, anchor_x, anchor_y, flip_x)?,
        delay_ms,
    })
}

fn raw_sprite(
    source: WzNodeArc,
    anchor_x: i32,
    anchor_y: i32,
    flip_x: bool,
) -> Result<RawSprite, WzContentError> {
    let (width, height) = {
        let read = source.read().map_err(|_| lock_error("WZ PNG geometry"))?;
        let png = read
            .try_as_png()
            .ok_or_else(|| WzContentError::InvalidMap {
                map_id: 0,
                message: format!("{} is not a PNG property", read.get_full_path()),
            })?;
        (png.width, png.height)
    };
    let origin = match child(&source, "origin")? {
        Some(origin) => vector_value(&origin)?,
        None => None,
    };
    let Vector2D(origin_x, origin_y) = origin.unwrap_or(Vector2D(0, 0));
    let width_i32 = i32::try_from(width).unwrap_or(i32::MAX);
    let x = if flip_x {
        anchor_x.saturating_sub(width_i32.saturating_sub(origin_x))
    } else {
        anchor_x.saturating_sub(origin_x)
    };

    Ok(RawSprite {
        source_path: node_path(&source)?,
        source,
        x,
        y: anchor_y.saturating_sub(origin_y),
        width,
        height,
        flip_x,
    })
}

fn object_order(
    layer: i32,
    instance_z: i32,
) -> DecorationOrder {
    DecorationOrder {
        layer,
        kind: DecorationKind::Object,
        primary_z: instance_z,
        secondary_z: 0,
    }
}

fn tile_order(
    layer: i32,
    source_z: i32,
    instance_z: i32,
) -> DecorationOrder {
    DecorationOrder {
        layer,
        kind: DecorationKind::Tile,
        primary_z: source_z,
        secondary_z: instance_z,
    }
}

fn read_bounds(node: &WzNodeArc) -> Result<Option<Bounds>, WzContentError> {
    if let Some(info) = child(node, "info")? {
        let values = ["VRLeft", "VRTop", "VRRight", "VRBottom"]
            .map(|name| int_value(&info, name))
            .into_iter()
            .collect::<Result<Vec<_>, _>>()?;
        if let [Some(left), Some(top), Some(right), Some(bottom)] = values.as_slice() {
            return Ok(Some(Bounds {
                left: *left,
                top: *top,
                right: *right,
                bottom: *bottom,
            }));
        }
    }

    let Some(minimap) = child(node, "miniMap")? else {
        return Ok(None);
    };
    let values = ["centerX", "centerY", "width", "height"]
        .map(|name| int_value(&minimap, name))
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?;
    let [Some(center_x), Some(center_y), Some(width), Some(height)] = values.as_slice() else {
        return Ok(None);
    };
    Ok(Some(Bounds {
        left: -*center_x,
        top: -*center_y,
        right: width.saturating_sub(*center_x),
        bottom: height.saturating_sub(*center_y),
    }))
}

fn derive_bounds<'a>(
    platforms: impl Iterator<Item = &'a RawPlatform>,
    decorations: impl Iterator<Item = &'a RawDecoration>,
    ladders: &[RawLadder],
    portals: &[RawPortal],
) -> Option<Bounds> {
    let mut points = platforms
        .flat_map(|platform| [(platform.x1, platform.y1), (platform.x2, platform.y2)])
        .chain(decorations.flat_map(|decoration| {
            decoration.frames().flat_map(|frame| {
                [
                    (frame.sprite.x, frame.sprite.y),
                    (
                        frame.sprite.x.saturating_add(frame.sprite.width as i32),
                        frame.sprite.y.saturating_add(frame.sprite.height as i32),
                    ),
                ]
            })
        }))
        .chain(
            ladders
                .iter()
                .flat_map(|ladder| [(ladder.x, ladder.y1), (ladder.x, ladder.y2)]),
        )
        .chain(portals.iter().flat_map(|portal| {
            std::iter::once((portal.x, portal.y)).chain(portal.frames.iter().flat_map(|frame| {
                [
                    (frame.sprite.x, frame.sprite.y),
                    (
                        frame.sprite.x.saturating_add(frame.sprite.width as i32),
                        frame.sprite.y.saturating_add(frame.sprite.height as i32),
                    ),
                ]
            }))
        }));
    let (first_x, first_y) = points.next()?;
    let (mut left, mut top, mut right, mut bottom) = (first_x, first_y, first_x, first_y);
    for (x, y) in points {
        left = left.min(x);
        top = top.min(y);
        right = right.max(x);
        bottom = bottom.max(y);
    }
    Some(Bounds {
        left: left.saturating_sub(100),
        top: top.saturating_sub(100),
        right: right.saturating_add(100),
        bottom: bottom.saturating_add(100),
    })
}

fn validate_bounds(
    map_id: u32,
    bounds: Bounds,
) -> Result<(), WzContentError> {
    if bounds.right <= bounds.left || bounds.bottom <= bounds.top {
        return Err(WzContentError::InvalidMap {
            map_id,
            message: format!("invalid bounds {bounds:?}"),
        });
    }
    Ok(())
}

fn build_platform(
    source: RawPlatform,
    bounds: Bounds,
) -> Platform {
    let x1 = (source.x1 - bounds.left) as f32;
    let y1 = (source.y1 - bounds.top) as f32;
    let x2 = (source.x2 - bounds.left) as f32;
    let y2 = (source.y2 - bounds.top) as f32;
    Platform {
        x: x1,
        y: y1,
        end_x: x2,
        end_y: y2,
        layer: source.layer,
        id: source.id,
    }
}

fn build_platforms(
    sources: Vec<RawPlatform>,
    bounds: Bounds,
) -> Vec<Platform> {
    sources
        .into_iter()
        .map(|source| build_platform(source, bounds))
        .collect()
}

fn build_decoration(
    content: &WzContent,
    source: RawDecoration,
    bounds: Bounds,
    assets: &mut Vec<AssetDescriptor>,
    asset_ids: &mut HashSet<String>,
) -> Result<Decoration, WzContentError> {
    let is_animated = !source.remaining_frames.is_empty();
    let layer = source.order.layer;
    let flip_x = source.first_frame.sprite.flip_x;
    let mut frames = Vec::with_capacity(source.remaining_frames.len() + 1);
    for frame in source.frames() {
        let asset = content.register_asset(&frame.sprite.source_path, &frame.sprite.source)?;
        if asset_ids.insert(asset.id.clone()) {
            assets.push(asset.clone());
        }
        frames.push(DecorationFrame {
            asset_id: asset.id,
            x: (frame.sprite.x - bounds.left) as f32,
            y: (frame.sprite.y - bounds.top) as f32,
            width: frame.sprite.width as f32,
            height: frame.sprite.height as f32,
            delay_ms: frame.delay_ms,
        });
    }
    let first = frames[0].clone();
    Ok(Decoration {
        asset_id: first.asset_id,
        x: first.x,
        y: first.y,
        width: first.width,
        height: first.height,
        layer,
        flip_x,
        frames: if is_animated { frames } else { Vec::new() },
    })
}

pub(super) fn child(
    node: &WzNodeArc,
    name: &str,
) -> Result<Option<WzNodeArc>, WzContentError> {
    Ok(node
        .read()
        .map_err(|_| lock_error("WZ child node"))?
        .at(name))
}

pub(super) fn children(node: &WzNodeArc) -> Result<Vec<WzNodeArc>, WzContentError> {
    Ok(node
        .read()
        .map_err(|_| lock_error("WZ child nodes"))?
        .children
        .values()
        .cloned()
        .collect())
}

pub(super) fn sorted_children(node: &WzNodeArc) -> Result<Vec<WzNodeArc>, WzContentError> {
    Ok(sorted_named_children(node)?
        .into_iter()
        .map(|(_, child)| child)
        .collect())
}

fn sorted_named_children(node: &WzNodeArc) -> Result<Vec<(String, WzNodeArc)>, WzContentError> {
    let mut children = node
        .read()
        .map_err(|_| lock_error("WZ child nodes"))?
        .children
        .iter()
        .map(|(name, child)| (name.to_string(), Arc::clone(child)))
        .collect::<Vec<_>>();
    children.sort_by_key(|(name, _)| (name.parse::<u32>().unwrap_or(u32::MAX), name.clone()));
    Ok(children)
}

pub(super) fn node_name(node: &WzNodeArc) -> Result<String, WzContentError> {
    Ok(node
        .read()
        .map_err(|_| lock_error("WZ node name"))?
        .name
        .to_string())
}

pub(super) fn node_path(node: &WzNodeArc) -> Result<String, WzContentError> {
    Ok(node
        .read()
        .map_err(|_| lock_error("WZ node path"))?
        .get_full_path())
}

pub(super) fn int_value(
    node: &WzNodeArc,
    name: &str,
) -> Result<Option<i32>, WzContentError> {
    let Some(child) = child(node, name)? else {
        return Ok(None);
    };
    let read = child.read().map_err(|_| lock_error("WZ integer value"))?;
    Ok(read
        .try_as_int()
        .copied()
        .or_else(|| read.try_as_short().map(|value| i32::from(*value))))
}

pub(super) fn string_value(
    node: &WzNodeArc,
    name: &str,
) -> Result<Option<String>, WzContentError> {
    let Some(child) = child(node, name)? else {
        return Ok(None);
    };
    let read = child.read().map_err(|_| lock_error("WZ string value"))?;
    Ok(read
        .try_as_string()
        .and_then(|value| value.get_string().ok()))
}

pub(super) fn vector_value(node: &WzNodeArc) -> Result<Option<Vector2D>, WzContentError> {
    Ok(node
        .read()
        .map_err(|_| lock_error("WZ vector value"))?
        .try_as_vector2d()
        .copied())
}

fn lock_error(context: &'static str) -> WzContentError {
    WzContentError::Lock { context }
}

#[cfg(test)]
mod tests {
    use super::Bounds;
    use super::RawDecoration;
    use super::RawPlatform;
    use super::build_platform;
    use super::build_platforms;
    use super::derive_bounds;
    use super::object_order;
    use super::read_bounds;
    use super::read_map_metadata;
    use super::sorted_named_children;
    use super::tile_order;
    #[test]
    fn derived_bounds_include_geometry_and_padding() {
        let platforms = [RawPlatform {
            id: 1,
            x1: -40,
            y1: 20,
            x2: 80,
            y2: 30,
            layer: 0,
        }];
        let decorations: [RawDecoration; 0] = [];

        assert_eq!(
            derive_bounds(platforms.iter(), decorations.iter(), &[], &[]),
            Some(Bounds {
                left: -140,
                top: -80,
                right: 180,
                bottom: 130,
            })
        );
    }

    #[test]
    fn platform_keeps_its_wz_layer() {
        let platform = build_platform(
            RawPlatform {
                id: 1,
                x1: 10,
                y1: 20,
                x2: 30,
                y2: 20,
                layer: 3,
            },
            Bounds {
                left: 0,
                top: 0,
                right: 100,
                bottom: 100,
            },
        );

        assert_eq!(platform.layer, 3);
    }

    #[test]
    fn platform_collection_keeps_vertical_wz_footholds() {
        let platforms = build_platforms(
            vec![RawPlatform {
                id: 1,
                x1: 30,
                y1: 20,
                x2: 30,
                y2: 80,
                layer: 3,
            }],
            Bounds {
                left: 0,
                top: 0,
                right: 100,
                bottom: 100,
            },
        );

        assert_eq!(platforms.len(), 1);
        assert_eq!(platforms[0].x, platforms[0].end_x);
        assert_eq!(platforms[0].y, 20.0);
        assert_eq!(platforms[0].end_y, 80.0);
    }

    #[test]
    fn decoration_order_matches_wz_layer_semantics() {
        let mut orders = vec![
            tile_order(1, 4, 3),
            object_order(1, 20),
            tile_order(0, 10, 0),
            object_order(1, 2),
            tile_order(1, 3, 9),
            tile_order(1, 3, 1),
        ];

        orders.sort();

        assert_eq!(
            orders,
            vec![
                tile_order(0, 10, 0),
                object_order(1, 2),
                object_order(1, 20),
                tile_order(1, 3, 1),
                tile_order(1, 3, 9),
                tile_order(1, 4, 3),
            ]
        );
    }

    #[test]
    fn child_sorting_preserves_frame_keys_when_nodes_reference_other_frames() {
        let parent = wz_reader::WzNode::from_str("animation", 0, None).into_lock();
        let first = wz_reader::WzNode::from_str("target-2", 0, Some(&parent)).into_lock();
        let second = wz_reader::WzNode::from_str("target-0", 0, Some(&parent)).into_lock();
        let _ = parent
            .write()
            .expect("animation lock")
            .children
            .insert("2".into(), first);
        let _ = parent
            .write()
            .expect("animation lock")
            .children
            .insert("0".into(), second);

        let keys = sorted_named_children(&parent)
            .expect("sorted children")
            .into_iter()
            .map(|(name, _)| name)
            .collect::<Vec<_>>();

        assert_eq!(keys, vec!["0", "2"]);
    }

    #[test]
    fn map_metadata_preserves_the_death_return_town() {
        let map = wz_reader::WzNode::from_str("map", 0, None).into_lock();
        let info = wz_reader::WzNode::from_str("info", 0, Some(&map)).into_lock();
        for (name, value) in [("returnMap", 100_000_000), ("town", 1)] {
            let child = wz_reader::WzNode::from_str(name, value, Some(&info)).into_lock();
            info.write().expect("info lock").add(&child);
        }
        map.write().expect("map lock").add(&info);

        let metadata = read_map_metadata(&map, 100_000_100).expect("map metadata");

        assert_eq!(metadata.return_map_id, Some(100_000_000));
        assert!(metadata.town);
    }

    #[test]
    fn minimap_bounds_do_not_scale_with_magnification() {
        let map = wz_reader::WzNode::from_str("map", 0, None).into_lock();
        let minimap = wz_reader::WzNode::from_str("miniMap", 0, Some(&map)).into_lock();
        for (name, value) in [
            ("centerX", 1068),
            ("centerY", 661),
            ("width", 7444),
            ("height", 1391),
            ("mag", 4),
        ] {
            let child = wz_reader::WzNode::from_str(name, value, Some(&minimap)).into_lock();
            minimap.write().expect("minimap lock").add(&child);
        }
        map.write().expect("map lock").add(&minimap);

        assert_eq!(
            read_bounds(&map).expect("bounds"),
            Some(Bounds {
                left: -1068,
                top: -661,
                right: 6376,
                bottom: 730,
            })
        );
    }
}
