use std::collections::HashMap;
use std::collections::HashSet;
use std::fs;
use std::io::Cursor;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::RwLock;

use image::ImageFormat;
use oozems_proto::v1::AssetDescriptor;
use oozems_proto::v1::Decoration;
use oozems_proto::v1::Map;
use oozems_proto::v1::Platform;
use oozems_proto::v1::PlatformKind;
use sha2::Digest;
use sha2::Sha256;
use thiserror::Error;
use wz_reader::WzFile;
use wz_reader::WzNode;
use wz_reader::WzNodeArc;
use wz_reader::WzNodeCast;
use wz_reader::WzObjectType;
use wz_reader::property::Vector2D;
use wz_reader::property::WzPngParseError;
use wz_reader::property::png::get_image;
use wz_reader::util::node_util::parse_node;

mod features;

use features::RawLadder;
use features::RawPortal;

const MAP_ARCHIVE: &str = "Map.wz";
const STRING_ARCHIVE: &str = "String.wz";
const PLAYER_LAYER: i32 = 4;

pub struct WzContent {
    _base: WzNodeArc,
    root: WzNodeArc,
    map_nodes: HashMap<u32, WzNodeArc>,
    map_names: HashMap<u32, String>,
    fingerprint: String,
    maps: RwLock<HashMap<u32, Map>>,
    assets: RwLock<HashMap<String, Arc<WzAsset>>>,
}

pub(crate) struct WzAsset {
    id: String,
    node: WzNodeArc,
    png: OnceLock<Arc<[u8]>>,
}

#[derive(Debug, Error)]
pub enum WzContentError {
    #[error("failed to inspect WZ archive {path}")]
    Metadata {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Bounds {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[derive(Clone, Copy, Debug)]
struct RawPlatform {
    x1: i32,
    y1: i32,
    x2: i32,
    y2: i32,
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

struct RawDecoration {
    sprite: RawSprite,
    order: DecorationOrder,
}

impl WzContent {
    pub fn open_optional(directory: &Path) -> Result<Option<Self>, WzContentError> {
        let map_path = directory.join(MAP_ARCHIVE);
        let exists = map_path
            .try_exists()
            .map_err(|source| WzContentError::Metadata {
                path: map_path.clone(),
                source,
            })?;
        if !exists {
            return Ok(None);
        }

        let root = open_archive(&map_path)?;
        let base = wrap_archive_root(&root)?;
        parse(&root, format!("{} root", map_path.display()))?;
        let map_nodes = index_map_nodes(&root, &map_path)?;
        let map_names = load_map_names(&directory.join(STRING_ARCHIVE))?;
        let fingerprint = archive_fingerprint(&map_path)?;

        tracing::info!(
            path = %map_path.display(),
            maps = map_nodes.len(),
            "WZ map source ready"
        );

        Ok(Some(Self {
            _base: base,
            root,
            map_nodes,
            map_names,
            fingerprint,
            maps: RwLock::new(HashMap::new()),
            assets: RwLock::new(HashMap::new()),
        }))
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
        let asset = Arc::new(WzAsset {
            id: id.clone(),
            node: Arc::clone(node),
            png: OnceLock::new(),
        });
        self.assets
            .write()
            .map_err(|_| lock_error("WZ asset registry"))?
            .entry(id.clone())
            .or_insert(asset);

        Ok(AssetDescriptor {
            id,
            url: format!("/wz-assets/{version}.png"),
            content_hash: version,
        })
    }
}

impl WzAsset {
    pub fn png_bytes(&self) -> Result<Arc<[u8]>, WzContentError> {
        if let Some(bytes) = self.png.get() {
            return Ok(Arc::clone(bytes));
        }

        let image = get_image(&self.node).map_err(|source| WzContentError::DecodeAsset {
            asset_id: self.id.clone(),
            source,
        })?;
        let mut output = Cursor::new(Vec::new());
        image
            .write_to(&mut output, ImageFormat::Png)
            .map_err(|source| WzContentError::EncodeAsset {
                asset_id: self.id.clone(),
                source,
            })?;
        let bytes: Arc<[u8]> = output.into_inner().into();
        let _ = self.png.set(Arc::clone(&bytes));
        Ok(self.png.get().cloned().unwrap_or(bytes))
    }
}

fn open_archive(path: &Path) -> Result<WzNodeArc, WzContentError> {
    WzNode::from_wz_file(path, None)
        .map(Into::into)
        .map_err(|source| WzContentError::Open {
            path: path.to_owned(),
            source,
        })
}

fn wrap_archive_root(root: &WzNodeArc) -> Result<WzNodeArc, WzContentError> {
    let base = WzNode::from_str("Base", WzFile::default(), None).into_lock();
    root.write()
        .map_err(|_| lock_error("WZ archive parent"))?
        .parent = Arc::downgrade(&base);
    base.write()
        .map_err(|_| lock_error("WZ synthetic Base root"))?
        .add(root);
    Ok(base)
}

fn parse(
    node: &WzNodeArc,
    context: String,
) -> Result<(), WzContentError> {
    parse_node(node).map_err(|source| WzContentError::Parse { context, source })
}

fn archive_fingerprint(path: &Path) -> Result<String, WzContentError> {
    let metadata = fs::metadata(path).map_err(|source| WzContentError::Metadata {
        path: path.to_owned(),
        source,
    })?;
    let modified = metadata
        .modified()
        .map_err(|source| WzContentError::Metadata {
            path: path.to_owned(),
            source,
        })?
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    Ok(hex::encode(Sha256::digest(
        format!("{}:{modified}", metadata.len()).as_bytes(),
    )))
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

fn load_map_names(path: &Path) -> Result<HashMap<u32, String>, WzContentError> {
    if !path
        .try_exists()
        .map_err(|source| WzContentError::Metadata {
            path: path.to_owned(),
            source,
        })?
    {
        tracing::warn!(path = %path.display(), "String.wz is absent; using numeric map names");
        return Ok(HashMap::new());
    }

    let root = open_archive(path)?;
    parse(&root, format!("{} root", path.display()))?;
    let map_image = child(&root, "Map.img")?.ok_or_else(|| WzContentError::InvalidMap {
        map_id: 0,
        message: format!("{} does not contain Map.img", path.display()),
    })?;
    parse(&map_image, format!("{} Map.img", path.display()))?;
    let mut names = HashMap::new();
    collect_map_names(&map_image, &mut names)?;
    Ok(names)
}

fn collect_map_names(
    node: &WzNodeArc,
    names: &mut HashMap<u32, String>,
) -> Result<(), WzContentError> {
    let name = node_name(node)?;
    if let Ok(map_id) = name.parse::<u32>()
        && let Some(map_name) = string_value(node, "mapName")?.filter(|value| !value.is_empty())
    {
        names.insert(map_id, map_name);
    }
    for child in children(node)? {
        collect_map_names(&child, names)?;
    }
    Ok(())
}

fn build_map(
    source: &WzContent,
    map_id: u32,
    node: &WzNodeArc,
) -> Result<Map, WzContentError> {
    let raw_platforms = read_platforms(node)?;
    let mut raw_decorations = read_decorations(&source.root, node, map_id)?;
    let raw_ladders = features::read_ladders(node)?;
    let raw_portals = features::read_portals(&source.root, node)?;
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

    let platforms = raw_platforms
        .into_iter()
        .filter(|platform| platform.x1 != platform.x2)
        .map(|platform| build_platform(platform, bounds))
        .collect();
    let mut assets = Vec::new();
    let mut asset_ids = HashSet::new();
    let mut decorations = Vec::with_capacity(raw_decorations.len());
    for decoration in raw_decorations {
        let asset =
            source.register_asset(&decoration.sprite.source_path, &decoration.sprite.source)?;
        if asset_ids.insert(asset.id.clone()) {
            assets.push(asset.clone());
        }
        decorations.push(build_decoration(decoration, &asset.id, bounds));
    }
    let ladders = features::build_ladders(raw_ladders, bounds);
    let portals =
        features::build_portals(source, raw_portals, bounds, &mut assets, &mut asset_ids)?;

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
    })
}

fn read_platforms(node: &WzNodeArc) -> Result<Vec<RawPlatform>, WzContentError> {
    let Some(footholds) = child(node, "foothold")? else {
        return Ok(Vec::new());
    };
    let mut output = Vec::new();
    collect_platforms(&footholds, &mut output)?;
    Ok(output)
}

fn collect_platforms(
    node: &WzNodeArc,
    output: &mut Vec<RawPlatform>,
) -> Result<(), WzContentError> {
    let values = ["x1", "y1", "x2", "y2"]
        .map(|name| int_value(node, name))
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?;
    if let [Some(x1), Some(y1), Some(x2), Some(y2)] = values.as_slice() {
        output.push(RawPlatform {
            x1: *x1,
            y1: *y1,
            x2: *x2,
            y2: *y2,
        });
        return Ok(());
    }

    for child in children(node)? {
        collect_platforms(&child, output)?;
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
        let source = match resolve_png_source(root, &format!("{base_path}/0"))? {
            Some(source) => Some(source),
            None => resolve_png_source(root, &base_path)?,
        };
        let Some(source) = source else {
            tracing::debug!(path = %base_path, "skipping WZ object without a PNG source");
            continue;
        };
        output.push(raw_decoration(
            source,
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
    let node = match root
        .read()
        .map_err(|_| lock_error("WZ archive root"))?
        .at_path_parsed(path)
    {
        Ok(node) => node,
        Err(wz_reader::node::Error::NodeNotFound) => return Ok(None),
        Err(source) => {
            return Err(WzContentError::Parse {
                context: path.to_owned(),
                source,
            });
        }
    };
    find_png_descendant(&node, 0)
}

fn find_png_descendant(
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
    Ok(RawDecoration {
        sprite: raw_sprite(source, anchor_x, anchor_y, flip_x)?,
        order,
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
            [
                (decoration.sprite.x, decoration.sprite.y),
                (
                    decoration
                        .sprite
                        .x
                        .saturating_add(decoration.sprite.width as i32),
                    decoration
                        .sprite
                        .y
                        .saturating_add(decoration.sprite.height as i32),
                ),
            ]
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
        width: (x2 - x1).abs(),
        kind: PlatformKind::Unspecified as i32,
        end_x: x2,
        end_y: y2,
        hidden: true,
    }
}

fn build_decoration(
    source: RawDecoration,
    asset_id: &str,
    bounds: Bounds,
) -> Decoration {
    Decoration {
        asset_id: asset_id.to_owned(),
        x: (source.sprite.x - bounds.left) as f32,
        y: (source.sprite.y - bounds.top) as f32,
        width: source.sprite.width as f32,
        height: source.sprite.height as f32,
        layer: source.order.layer - PLAYER_LAYER,
        flip_x: source.sprite.flip_x,
    }
}

fn child(
    node: &WzNodeArc,
    name: &str,
) -> Result<Option<WzNodeArc>, WzContentError> {
    Ok(node
        .read()
        .map_err(|_| lock_error("WZ child node"))?
        .at(name))
}

fn children(node: &WzNodeArc) -> Result<Vec<WzNodeArc>, WzContentError> {
    Ok(node
        .read()
        .map_err(|_| lock_error("WZ child nodes"))?
        .children
        .values()
        .cloned()
        .collect())
}

fn sorted_children(node: &WzNodeArc) -> Result<Vec<WzNodeArc>, WzContentError> {
    let mut children = children(node)?;
    children.sort_by_key(|child| {
        let name = child
            .read()
            .map(|read| read.name.to_string())
            .unwrap_or_default();
        (name.parse::<u32>().unwrap_or(u32::MAX), name)
    });
    Ok(children)
}

fn node_name(node: &WzNodeArc) -> Result<String, WzContentError> {
    Ok(node
        .read()
        .map_err(|_| lock_error("WZ node name"))?
        .name
        .to_string())
}

fn node_path(node: &WzNodeArc) -> Result<String, WzContentError> {
    Ok(node
        .read()
        .map_err(|_| lock_error("WZ node path"))?
        .get_full_path())
}

fn int_value(
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

fn string_value(
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

fn vector_value(node: &WzNodeArc) -> Result<Option<Vector2D>, WzContentError> {
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
    use super::derive_bounds;
    use super::object_order;
    use super::read_bounds;
    use super::tile_order;

    #[test]
    fn derived_bounds_include_geometry_and_padding() {
        let platforms = [RawPlatform {
            x1: -40,
            y1: 20,
            x2: 80,
            y2: 30,
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
