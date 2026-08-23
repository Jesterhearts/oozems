use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::sync::RwLock;

use oozems_proto::v1::AssetDescriptor;
use oozems_proto::v1::MobAnimation;
use oozems_proto::v1::MobDefinition;
use oozems_proto::v1::MobFrame;
use oozems_proto::v1::MobSpawnPoint;
use oozems_proto::v1::Platform;
use oozems_proto::v1::Vec2;
use sha2::Digest;
use sha2::Sha256;
use wz_reader::WzNodeArc;
use wz_reader::WzNodeCast;
use wz_reader::property::Vector2D;

use super::Bounds;
use super::WzAsset;
use super::WzContentError;
use super::archive_fingerprint;
use super::child;
use super::find_png_descendant;
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

const MOB_ARCHIVE: &str = "Mob.wz";
const DEFAULT_FRAME_DELAY_MS: u32 = 100;

pub(super) struct MobContent {
    _base: WzNodeArc,
    root: WzNodeArc,
    fingerprint: String,
    definitions: RwLock<HashMap<u32, LoadedMobDefinition>>,
    assets: RwLock<HashMap<String, Arc<WzAsset>>>,
}

#[derive(Clone)]
pub(super) struct LoadedMobDefinition {
    pub(super) definition: MobDefinition,
    pub(super) assets: Vec<AssetDescriptor>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct RawMobSpawn {
    spawn_id: u32,
    mob_id: u32,
    x: i32,
    y: i32,
    center_y: i32,
    roam_left: i32,
    roam_right: i32,
    flip_x: bool,
    respawn_seconds: u32,
    foothold_id: u32,
}

impl MobContent {
    pub(super) fn open_optional(directory: &Path) -> Result<Option<Self>, WzContentError> {
        let path = directory.join(MOB_ARCHIVE);
        if !archive_exists(&path)? {
            tracing::warn!(path = %path.display(), "Mob.wz is absent; map mobs will not spawn");
            return Ok(None);
        }

        let root = open_archive(&path)?;
        let base = wrap_archive_root(&root)?;
        parse(&root, format!("{} root", path.display()))?;
        let fingerprint = archive_fingerprint(&path)?;

        tracing::info!(path = %path.display(), "WZ mob source ready");
        Ok(Some(Self {
            _base: base,
            root,
            fingerprint,
            definitions: RwLock::new(HashMap::new()),
            assets: RwLock::new(HashMap::new()),
        }))
    }

    pub(super) fn get_definition(
        &self,
        mob_id: u32,
    ) -> Result<Option<LoadedMobDefinition>, WzContentError> {
        if let Some(definition) = self
            .definitions
            .read()
            .map_err(|_| lock_error("WZ mob definition cache"))?
            .get(&mob_id)
            .cloned()
        {
            return Ok(Some(definition));
        }

        let image_name = format!("{mob_id:07}.img");
        let Some(node) = child(&self.root, &image_name)? else {
            tracing::warn!(mob_id, "mob definition is absent from Mob.wz");
            return Ok(None);
        };
        parse(&node, format!("mob {mob_id}"))?;
        let definition = build_definition(self, mob_id, &node)?;
        self.definitions
            .write()
            .map_err(|_| lock_error("WZ mob definition cache"))?
            .insert(mob_id, definition.clone());
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
            .map_err(|_| lock_error("WZ mob asset registry"))?
            .entry(id.clone())
            .or_insert(asset);
        Ok(AssetDescriptor {
            id,
            url: format!("/wz-assets/{version}.png"),
            content_hash: version,
        })
    }
}

fn archive_exists(path: &Path) -> Result<bool, WzContentError> {
    path.try_exists()
        .map_err(|source| WzContentError::Metadata {
            path: path.to_owned(),
            source,
        })
}

pub(super) fn read_spawn_points(map: &WzNodeArc) -> Result<Vec<RawMobSpawn>, WzContentError> {
    let Some(life) = child(map, "life")? else {
        return Ok(Vec::new());
    };
    sorted_children(&life)?
        .into_iter()
        .map(|node| read_spawn_point(&node))
        .filter_map(Result::transpose)
        .collect()
}

fn read_spawn_point(node: &WzNodeArc) -> Result<Option<RawMobSpawn>, WzContentError> {
    if string_value(node, "type")?.as_deref() != Some("m")
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
    let (Some(spawn_id), Some(mob_id), Some(x), Some(y)) = values else {
        tracing::warn!(path = %node_path(node)?, "skipping incomplete WZ mob spawn point");
        return Ok(None);
    };

    Ok(Some(RawMobSpawn {
        spawn_id,
        mob_id,
        x,
        y,
        center_y: int_value(node, "cy")?.unwrap_or(y),
        roam_left: int_value(node, "rx0")?.unwrap_or(x),
        roam_right: int_value(node, "rx1")?.unwrap_or(x),
        flip_x: int_value(node, "f")?.unwrap_or_default() != 0,
        respawn_seconds: nonnegative_u32(int_value(node, "mobTime")?.unwrap_or_default()),
        foothold_id: nonnegative_u32(int_value(node, "fh")?.unwrap_or_default()),
    }))
}

pub(super) fn build_spawn_points(
    source: Vec<RawMobSpawn>,
    platforms: &[Platform],
    bounds: Bounds,
) -> Vec<MobSpawnPoint> {
    source
        .into_iter()
        .map(|spawn| build_spawn_point(spawn, platforms, bounds))
        .collect()
}

pub(super) fn load_definitions(
    content: Option<&MobContent>,
    spawn_points: &[MobSpawnPoint],
    assets: &mut Vec<AssetDescriptor>,
    asset_ids: &mut HashSet<String>,
) -> Result<Vec<MobDefinition>, WzContentError> {
    let Some(content) = content else {
        return Ok(Vec::new());
    };
    let mut mob_ids = spawn_points
        .iter()
        .map(|spawn| spawn.mob_id)
        .collect::<Vec<_>>();
    mob_ids.sort_unstable();
    mob_ids.dedup();

    let mut definitions = Vec::with_capacity(mob_ids.len());
    for mob_id in mob_ids {
        let Some(loaded) = content.get_definition(mob_id)? else {
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

fn build_spawn_point(
    source: RawMobSpawn,
    platforms: &[Platform],
    bounds: Bounds,
) -> MobSpawnPoint {
    let x = (source.x - bounds.left) as f32;
    let y = (source.y - bounds.top) as f32;
    let center_y = (source.center_y - bounds.top) as f32;
    let (layer, surface_y) =
        attached_platform(platforms, source.foothold_id, x, center_y).unwrap_or((0, y));
    MobSpawnPoint {
        spawn_id: source.spawn_id,
        mob_id: source.mob_id,
        position: Some(Vec2 { x, y: surface_y }),
        roam_left: (source.roam_left.min(source.roam_right) - bounds.left) as f32,
        roam_right: (source.roam_left.max(source.roam_right) - bounds.left) as f32,
        flip_x: source.flip_x,
        layer,
        respawn_seconds: source.respawn_seconds,
        foothold_id: source.foothold_id,
    }
}

fn attached_platform(
    platforms: &[Platform],
    foothold_id: u32,
    x: f32,
    reference_y: f32,
) -> Option<(i32, f32)> {
    let attached = platforms
        .iter()
        .find(|platform| foothold_id != 0 && platform.id == foothold_id)
        .and_then(|platform| {
            platform_surface_at_x(platform, x).map(|surface| (platform.layer, surface))
        });
    if attached.is_some() {
        return attached;
    }
    platforms
        .iter()
        .filter_map(|platform| {
            let surface = platform_surface_at_x(platform, x)?;
            Some((platform.layer, surface, (surface - reference_y).abs()))
        })
        .min_by(|left, right| left.2.total_cmp(&right.2))
        .map(|(layer, surface, _)| (layer, surface))
}

fn platform_surface_at_x(
    platform: &Platform,
    x: f32,
) -> Option<f32> {
    let minimum_x = platform.x.min(platform.end_x);
    let maximum_x = platform.x.max(platform.end_x);
    if !(minimum_x..=maximum_x).contains(&x) {
        return None;
    }
    let delta_x = platform.end_x - platform.x;
    if delta_x.abs() < f32::EPSILON {
        return None;
    }
    let progress = (x - platform.x) / delta_x;
    Some(platform.y + progress * (platform.end_y - platform.y))
}

fn build_definition(
    content: &MobContent,
    mob_id: u32,
    node: &WzNodeArc,
) -> Result<LoadedMobDefinition, WzContentError> {
    let mut assets = Vec::new();
    let animations = read_animations(content, node, &mut assets)?;
    let can_jump = has_jump_animation(&animations);
    let info = child(node, "info")?;
    let definition = match info {
        Some(info) => MobDefinition {
            id: mob_id,
            level: positive_u32(numeric_value(&info, "level")?.unwrap_or_default()),
            max_hp: positive_u64(numeric_value(&info, "maxHP")?.unwrap_or(1)),
            max_mp: positive_u64(numeric_value(&info, "maxMP")?.unwrap_or_default()),
            experience: positive_u64(numeric_value(&info, "exp")?.unwrap_or_default()),
            physical_attack: bounded_i32(numeric_value(&info, "PADamage")?),
            physical_defense: bounded_i32(numeric_value(&info, "PDDamage")?),
            magic_attack: bounded_i32(numeric_value(&info, "MADamage")?),
            magic_defense: bounded_i32(numeric_value(&info, "MDDamage")?),
            accuracy: bounded_i32(numeric_value(&info, "acc")?),
            avoidability: bounded_i32(numeric_value(&info, "eva")?),
            speed: bounded_i32(numeric_value(&info, "speed")?),
            body_attack: flag_value(&info, "bodyAttack")?,
            boss: flag_value(&info, "boss")?,
            undead: flag_value(&info, "undead")?,
            animations,
            can_jump,
        },
        None => MobDefinition {
            id: mob_id,
            max_hp: 1,
            animations,
            can_jump,
            ..MobDefinition::default()
        },
    };
    Ok(LoadedMobDefinition { definition, assets })
}

fn has_jump_animation(animations: &[MobAnimation]) -> bool {
    animations
        .iter()
        .any(|animation| animation.name == "jump" && !animation.frames.is_empty())
}

fn read_animations(
    content: &MobContent,
    mob: &WzNodeArc,
    assets: &mut Vec<AssetDescriptor>,
) -> Result<Vec<MobAnimation>, WzContentError> {
    let mut animations = Vec::new();
    for node in sorted_children(mob)? {
        let name = node_name(&node)?;
        if name == "info" {
            continue;
        }
        let frames = read_animation_frames(content, &node, assets)?;
        if !frames.is_empty() {
            animations.push(MobAnimation { name, frames });
        }
    }
    Ok(animations)
}

fn read_animation_frames(
    content: &MobContent,
    animation: &WzNodeArc,
    assets: &mut Vec<AssetDescriptor>,
) -> Result<Vec<MobFrame>, WzContentError> {
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
        frames.push(MobFrame {
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
    let read = node.read().map_err(|_| lock_error("WZ mob PNG geometry"))?;
    let png = read
        .try_as_png()
        .ok_or_else(|| WzContentError::InvalidAsset {
            asset_id: read.get_full_path(),
            message: "mob frame is not a PNG property".to_owned(),
        })?;
    Ok((png.width, png.height))
}

fn numeric_value(
    node: &WzNodeArc,
    name: &str,
) -> Result<Option<i64>, WzContentError> {
    let Some(value) = child(node, name)? else {
        return Ok(None);
    };
    let read = value
        .read()
        .map_err(|_| lock_error("WZ mob numeric value"))?;
    Ok(read
        .try_as_long()
        .copied()
        .or_else(|| read.try_as_int().map(|value| i64::from(*value)))
        .or_else(|| read.try_as_short().map(|value| i64::from(*value))))
}

fn flag_value(
    node: &WzNodeArc,
    name: &str,
) -> Result<bool, WzContentError> {
    Ok(numeric_value(node, name)?.unwrap_or_default() != 0)
}

fn positive_u32(value: i64) -> u32 {
    u32::try_from(value.max(0)).unwrap_or(u32::MAX)
}

fn positive_u64(value: i64) -> u64 {
    u64::try_from(value.max(0)).unwrap_or(u64::MAX)
}

fn nonnegative_u32(value: i32) -> u32 {
    u32::try_from(value.max(0)).unwrap_or(u32::MAX)
}

fn bounded_i32(value: Option<i64>) -> i32 {
    let value = value.unwrap_or_default();
    i32::try_from(value).unwrap_or_else(|_| {
        if value.is_negative() {
            i32::MIN
        } else {
            i32::MAX
        }
    })
}

#[cfg(test)]
mod tests {
    use oozems_proto::v1::MobAnimation;
    use oozems_proto::v1::MobFrame;
    use oozems_proto::v1::Platform;

    use super::Bounds;
    use super::RawMobSpawn;
    use super::build_spawn_points;
    use super::has_jump_animation;

    #[test]
    fn spawn_points_snap_to_their_attached_foothold() {
        let platforms = vec![
            Platform {
                id: 1,
                x: 0.0,
                y: 100.0,
                end_x: 200.0,
                end_y: 120.0,
                layer: 2,
                ..Platform::default()
            },
            Platform {
                id: 7,
                x: 0.0,
                y: 300.0,
                end_x: 200.0,
                end_y: 300.0,
                layer: 5,
                ..Platform::default()
            },
        ];
        let spawn = RawMobSpawn {
            spawn_id: 4,
            mob_id: 100_101,
            x: 100,
            y: 270,
            center_y: 110,
            roam_left: 20,
            roam_right: 180,
            flip_x: true,
            respawn_seconds: 12,
            foothold_id: 7,
        };

        let points = build_spawn_points(
            vec![spawn],
            &platforms,
            Bounds {
                left: 0,
                top: 0,
                right: 400,
                bottom: 400,
            },
        );

        assert_eq!(points.len(), 1);
        assert_eq!(points[0].position.as_ref().expect("position").y, 300.0);
        assert_eq!(points[0].layer, 5);
        assert_eq!(points[0].roam_left, 20.0);
        assert_eq!(points[0].roam_right, 180.0);
    }

    #[test]
    fn a_nonempty_jump_animation_marks_a_mob_as_jump_capable() {
        let jump = MobAnimation {
            name: "jump".to_owned(),
            frames: vec![MobFrame::default()],
        };
        let empty_jump = MobAnimation {
            name: "jump".to_owned(),
            frames: Vec::new(),
        };

        assert!(has_jump_animation(&[jump]));
        assert!(!has_jump_animation(&[empty_jump]));
    }
}
