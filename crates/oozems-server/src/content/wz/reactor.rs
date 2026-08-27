use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::sync::RwLock;

use oozems_proto::v1::AssetDescriptor;
use oozems_proto::v1::Platform;
use oozems_proto::v1::ReactorDefinition;
use oozems_proto::v1::ReactorFrame;
use oozems_proto::v1::ReactorSpawnPoint;
use oozems_proto::v1::ReactorStateDefinition;
use oozems_proto::v1::Vec2;
use sha2::Digest;
use sha2::Sha256;
use wz_reader::WzNodeArc;
use wz_reader::WzNodeCast;
use wz_reader::property::Vector2D;

use super::Bounds;
use super::REACTOR_ARCHIVE;
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

const DEFAULT_FRAME_DELAY_MS: u32 = 100;

pub(super) struct ReactorContent {
    _base: WzNodeArc,
    root: WzNodeArc,
    fingerprint: String,
    definitions: RwLock<HashMap<u32, LoadedReactorDefinition>>,
    assets: RwLock<HashMap<String, Arc<WzAsset>>>,
}

#[derive(Clone)]
struct LoadedReactorDefinition {
    definition: ReactorDefinition,
    assets: Vec<AssetDescriptor>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct RawReactorSpawn {
    spawn_id: u32,
    reactor_id: u32,
    x: i32,
    y: i32,
    flip_x: bool,
    respawn_seconds: u32,
}

impl ReactorContent {
    pub(super) fn open_optional(directory: &Path) -> Result<Option<Self>, WzContentError> {
        let path = directory.join(REACTOR_ARCHIVE);
        let exists = path
            .try_exists()
            .map_err(|source| WzContentError::Metadata {
                path: path.clone(),
                source,
            })?;
        if !exists {
            tracing::warn!(path = %path.display(), "Reactor.wz is absent; map reactors will not be displayed");
            return Ok(None);
        }

        let root = open_archive(&path)?;
        let base = wrap_archive_root(&root)?;
        parse(&root, format!("{} root", path.display()))?;

        tracing::info!(path = %path.display(), "WZ reactor source ready");
        Ok(Some(Self {
            _base: base,
            root,
            fingerprint: archive_fingerprint(&path)?,
            definitions: RwLock::new(HashMap::new()),
            assets: RwLock::new(HashMap::new()),
        }))
    }

    fn get_definition(
        &self,
        reactor_id: u32,
    ) -> Result<Option<LoadedReactorDefinition>, WzContentError> {
        if let Some(definition) = self
            .definitions
            .read()
            .map_err(|_| lock_error("WZ reactor definition cache"))?
            .get(&reactor_id)
            .cloned()
        {
            return Ok(Some(definition));
        }

        let image_name = format!("{reactor_id:07}.img");
        let Some(node) = child(&self.root, &image_name)? else {
            tracing::warn!(reactor_id, "reactor definition is absent from Reactor.wz");
            return Ok(None);
        };
        parse(&node, format!("reactor {reactor_id}"))?;
        let definition = build_definition(self, reactor_id, &node)?;
        self.definitions
            .write()
            .map_err(|_| lock_error("WZ reactor definition cache"))?
            .insert(reactor_id, definition.clone());
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
            .map_err(|_| lock_error("WZ reactor asset registry"))?
            .entry(id.clone())
            .or_insert(asset);
        Ok(AssetDescriptor {
            id,
            url: format!("/wz-assets/{version}.png"),
        })
    }
}

pub(super) fn read_spawn_points(map: &WzNodeArc) -> Result<Vec<RawReactorSpawn>, WzContentError> {
    let Some(reactors) = child(map, "reactor")? else {
        return Ok(Vec::new());
    };
    sorted_children(&reactors)?
        .into_iter()
        .map(|node| read_spawn_point(&node))
        .filter_map(Result::transpose)
        .collect()
}

fn read_spawn_point(node: &WzNodeArc) -> Result<Option<RawReactorSpawn>, WzContentError> {
    let values = (
        node_name(node)?.parse::<u32>().ok(),
        string_value(node, "id")?.and_then(|value| value.parse::<u32>().ok()),
        int_value(node, "x")?,
        int_value(node, "y")?,
    );
    let (Some(spawn_id), Some(reactor_id), Some(x), Some(y)) = values else {
        tracing::warn!(path = %node_path(node)?, "skipping incomplete WZ reactor spawn point");
        return Ok(None);
    };
    Ok(Some(RawReactorSpawn {
        spawn_id,
        reactor_id,
        x,
        y,
        flip_x: int_value(node, "f")?.unwrap_or_default() != 0,
        respawn_seconds: nonnegative_u32(int_value(node, "reactorTime")?.unwrap_or_default()),
    }))
}

pub(super) fn build_spawn_points(
    source: Vec<RawReactorSpawn>,
    platforms: &[Platform],
    bounds: Bounds,
) -> Vec<ReactorSpawnPoint> {
    source
        .into_iter()
        .map(|spawn| {
            let x = (spawn.x - bounds.left) as f32;
            let y = (spawn.y - bounds.top) as f32;
            let layer = attached_platform(platforms, 0, x, y).map_or(0, |(layer, _)| layer);
            ReactorSpawnPoint {
                spawn_id: spawn.spawn_id,
                reactor_id: spawn.reactor_id,
                position: Some(Vec2 { x, y }),
                flip_x: spawn.flip_x,
                layer,
                respawn_seconds: spawn.respawn_seconds,
            }
        })
        .collect()
}

pub(super) fn load_definitions(
    content: Option<&ReactorContent>,
    spawn_points: &[ReactorSpawnPoint],
    assets: &mut Vec<AssetDescriptor>,
    asset_ids: &mut HashSet<String>,
) -> Result<Vec<ReactorDefinition>, WzContentError> {
    if spawn_points.is_empty() {
        return Ok(Vec::new());
    }
    let Some(content) = content else {
        return Ok(Vec::new());
    };
    let mut reactor_ids = spawn_points
        .iter()
        .map(|spawn| spawn.reactor_id)
        .collect::<Vec<_>>();
    reactor_ids.sort_unstable();
    reactor_ids.dedup();

    let mut definitions = Vec::with_capacity(reactor_ids.len());
    for reactor_id in reactor_ids {
        let Some(loaded) = content.get_definition(reactor_id)? else {
            continue;
        };
        for asset in loaded.assets {
            if asset_ids.insert(asset.id.clone()) {
                assets.push(asset);
            }
        }
        definitions.push(loaded.definition);
    }
    Ok(definitions)
}

fn build_definition(
    content: &ReactorContent,
    reactor_id: u32,
    node: &WzNodeArc,
) -> Result<LoadedReactorDefinition, WzContentError> {
    let mut states = Vec::new();
    let mut assets = Vec::new();
    for state_node in sorted_children(node)? {
        let Ok(state) = node_name(&state_node)?.parse::<u32>() else {
            continue;
        };
        states.push(ReactorStateDefinition {
            state,
            frames: read_frames(content, &state_node, &mut assets)?,
            hit_frames: match child(&state_node, "hit")? {
                Some(hit) => read_frames(content, &hit, &mut assets)?,
                None => Vec::new(),
            },
            next_state: read_next_state(reactor_id, state, &state_node)?,
        });
    }
    states.sort_by_key(|state| state.state);
    let mut seen_assets = HashSet::new();
    assets.retain(|asset| seen_assets.insert(asset.id.clone()));
    Ok(LoadedReactorDefinition {
        definition: ReactorDefinition {
            id: reactor_id,
            states,
        },
        assets,
    })
}

fn read_next_state(
    reactor_id: u32,
    state: u32,
    state_node: &WzNodeArc,
) -> Result<Option<u32>, WzContentError> {
    let Some(events) = child(state_node, "event")? else {
        return Ok(None);
    };
    let entries = sorted_children(&events)?;
    for event in &entries {
        if int_value(event, "type")? == Some(0) {
            return Ok(int_value(event, "state")?.and_then(nonnegative_u32_option));
        }
    }
    if entries.is_empty() {
        return Ok(None);
    }
    tracing::debug!(
        reactor_id,
        state,
        "reactor state has no supported hit-triggered transition"
    );
    Ok(None)
}

fn read_frames(
    content: &ReactorContent,
    animation: &WzNodeArc,
    assets: &mut Vec<AssetDescriptor>,
) -> Result<Vec<ReactorFrame>, WzContentError> {
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
        frames.push(ReactorFrame {
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
    let read = node
        .read()
        .map_err(|_| lock_error("WZ reactor PNG geometry"))?;
    let png = read
        .try_as_png()
        .ok_or_else(|| WzContentError::InvalidAsset {
            asset_id: read.get_full_path(),
            message: "reactor frame is not a PNG property".to_owned(),
        })?;
    Ok((png.width, png.height))
}

fn nonnegative_u32(value: i32) -> u32 {
    u32::try_from(value.max(0)).unwrap_or(u32::MAX)
}

fn nonnegative_u32_option(value: i32) -> Option<u32> {
    u32::try_from(value).ok()
}

#[cfg(test)]
mod tests {
    use oozems_proto::v1::Platform;
    use wz_reader::WzNode;
    use wz_reader::property::WzString;

    use super::Bounds;
    use super::RawReactorSpawn;
    use super::build_spawn_points;
    use super::read_next_state;
    use super::read_spawn_points;

    #[test]
    fn spawn_points_keep_the_authored_anchor_and_use_the_nearest_foothold_layer() {
        let platforms = vec![Platform {
            id: 7,
            x: 0.0,
            y: 120.0,
            end_x: 200.0,
            end_y: 120.0,
            layer: 3,
        }];
        let spawn = RawReactorSpawn {
            spawn_id: 4,
            reactor_id: 2_001,
            x: 100,
            y: 116,
            flip_x: true,
            respawn_seconds: 90,
        };

        let points = build_spawn_points(
            vec![spawn],
            &platforms,
            Bounds {
                left: 0,
                top: 0,
                right: 400,
                bottom: 300,
            },
        );

        assert_eq!(points.len(), 1);
        assert_eq!(points[0].position.as_ref().expect("position").y, 116.0);
        assert_eq!(points[0].layer, 3);
        assert_eq!(points[0].respawn_seconds, 90);
    }

    #[test]
    fn synthetic_map_reactor_fields_are_parsed_without_archive_data() {
        let map = WzNode::from_str("map", 0, None).into_lock();
        let reactors = WzNode::from_str("reactor", 0, Some(&map)).into_lock();
        let spawn = WzNode::from_str("4", 0, Some(&reactors)).into_lock();
        add_string(&spawn, "id", "0002001");
        for (name, value) in [("x", 100), ("y", 116), ("f", 1), ("reactorTime", 90)] {
            add_int(&spawn, name, value);
        }
        reactors.write().expect("reactors lock").add(&spawn);
        map.write().expect("map lock").add(&reactors);

        let parsed = read_spawn_points(&map).expect("parse reactor spawns");

        assert_eq!(
            parsed,
            vec![RawReactorSpawn {
                spawn_id: 4,
                reactor_id: 2_001,
                x: 100,
                y: 116,
                flip_x: true,
                respawn_seconds: 90,
            }]
        );
    }

    #[test]
    fn only_type_zero_events_become_attack_transitions() {
        let supported = state_with_event(0, 0, 1);
        let unsupported = state_with_event(1, 2, 7);
        let terminal = WzNode::from_str("2", 0, None).into_lock();

        assert_eq!(
            read_next_state(2_001, 0, &supported).expect("supported transition"),
            Some(1)
        );
        assert_eq!(
            read_next_state(2_001, 1, &unsupported).expect("unsupported transition"),
            None
        );
        assert_eq!(
            read_next_state(2_001, 2, &terminal).expect("terminal state"),
            None
        );
    }

    fn state_with_event(
        state: u32,
        event_type: i32,
        next_state: i32,
    ) -> wz_reader::WzNodeArc {
        let state = WzNode::from_str(&state.to_string(), 0, None).into_lock();
        let events = WzNode::from_str("event", 0, Some(&state)).into_lock();
        let event = WzNode::from_str("0", 0, Some(&events)).into_lock();
        add_int(&event, "type", event_type);
        add_int(&event, "state", next_state);
        events.write().expect("events lock").add(&event);
        state.write().expect("state lock").add(&events);
        state
    }

    fn add_int(
        parent: &wz_reader::WzNodeArc,
        name: &str,
        value: i32,
    ) {
        let child = WzNode::from_str(name, value, Some(parent)).into_lock();
        parent.write().expect("parent lock").add(&child);
    }

    fn add_string(
        parent: &wz_reader::WzNodeArc,
        name: &str,
        value: &str,
    ) {
        let value = WzString::from_str(value, [0; 4]);
        let child = WzNode::from_str(name, value, Some(parent)).into_lock();
        parent.write().expect("parent lock").add(&child);
    }
}
