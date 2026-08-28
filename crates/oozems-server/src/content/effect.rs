use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use oozems_proto::v1::AnimationFrame;
use oozems_proto::v1::AssetDescriptor;
use sha2::Digest;
use sha2::Sha256;
use thiserror::Error;
use wz_reader::WzNodeArc;
use wz_reader::WzNodeCast;
use wz_reader::property::Vector2D;

use super::WzAsset;
use super::wz;
use super::wz::WzContentError;

const EFFECT_ARCHIVE: &str = "Effect.wz";
const BASIC_EFFECT_IMAGE: &str = "BasicEff.img";
const LEVEL_UP_ANIMATION: &str = "LevelUp";
const TOMB_IMAGE: &str = "Tomb.img";
const DEFAULT_FRAME_DELAY_MS: u32 = 100;

pub(super) struct EffectContent {
    _base: WzNodeArc,
    level_up_frames: Vec<AnimationFrame>,
    level_up_assets: Vec<AssetDescriptor>,
    tomb_frames: Vec<AnimationFrame>,
    tomb_assets: Vec<AssetDescriptor>,
    assets: HashMap<String, Arc<WzAsset>>,
}

#[derive(Debug, Error)]
pub enum EffectContentError {
    #[error(transparent)]
    Wz(#[from] WzContentError),
    #[error("effect WZ data is invalid: {message}")]
    Invalid { message: String },
    #[error("internal effect content lock was poisoned while accessing {context}")]
    Lock { context: &'static str },
}

impl EffectContent {
    pub(super) fn open_optional(directory: &Path) -> Result<Option<Self>, EffectContentError> {
        let path = directory.join(EFFECT_ARCHIVE);
        if !path
            .try_exists()
            .map_err(|source| WzContentError::Metadata {
                path: path.clone(),
                source,
            })?
        {
            tracing::warn!(path = %path.display(), "Effect.wz is absent; death and level-up effects are disabled");
            return Ok(None);
        }

        let root = wz::open_archive(&path)?;
        let base = wz::wrap_archive_root(&root)?;
        wz::parse(&root, format!("{} root", path.display()))?;
        let tomb = required_child(&root, TOMB_IMAGE)?;
        wz::parse(&tomb, format!("{} {TOMB_IMAGE}", path.display()))?;
        let fall = required_child(&tomb, "fall")?;
        let fingerprint = wz::archive_fingerprint(&path)?;
        let mut assets = HashMap::new();
        let level_up_frames = read_level_up_frames(&root, &path, &fingerprint, &mut assets)?;
        let level_up_assets = frame_descriptors(&level_up_frames);
        let tomb_frames = read_animation_frames(&fall, "Tomb.img/fall", &fingerprint, &mut assets)?;
        if tomb_frames.is_empty() {
            return invalid("Tomb.img/fall has no frames");
        }
        let tomb_assets = frame_descriptors(&tomb_frames);

        tracing::info!(
            path = %path.display(),
            level_up_frames = level_up_frames.len(),
            tomb_frames = tomb_frames.len(),
            "WZ effect source ready"
        );
        Ok(Some(Self {
            _base: base,
            level_up_frames,
            level_up_assets,
            tomb_frames,
            tomb_assets,
            assets,
        }))
    }

    pub(super) fn tomb_projection(&self) -> (Vec<AnimationFrame>, Vec<AssetDescriptor>) {
        (self.tomb_frames.clone(), self.tomb_assets.clone())
    }

    pub(super) fn level_up_projection(&self) -> (Vec<AnimationFrame>, Vec<AssetDescriptor>) {
        (self.level_up_frames.clone(), self.level_up_assets.clone())
    }

    pub(super) fn get_asset(
        &self,
        asset_id: &str,
    ) -> Option<Arc<WzAsset>> {
        self.assets.get(asset_id).cloned()
    }
}

fn read_level_up_frames(
    root: &WzNodeArc,
    archive_path: &Path,
    fingerprint: &str,
    assets: &mut HashMap<String, Arc<WzAsset>>,
) -> Result<Vec<AnimationFrame>, EffectContentError> {
    let Some(basic) = wz::child(root, BASIC_EFFECT_IMAGE)? else {
        tracing::warn!("Effect.wz has no {BASIC_EFFECT_IMAGE}; level-up effects are disabled");
        return Ok(Vec::new());
    };
    wz::parse(
        &basic,
        format!("{} {BASIC_EFFECT_IMAGE}", archive_path.display()),
    )?;
    let Some(level_up) = wz::child(&basic, LEVEL_UP_ANIMATION)? else {
        tracing::warn!("Effect.wz has no BasicEff.img/LevelUp; level-up effects are disabled");
        return Ok(Vec::new());
    };
    let mut animation_assets = HashMap::new();
    let frames = match read_animation_frames(
        &level_up,
        "BasicEff.img/LevelUp",
        fingerprint,
        &mut animation_assets,
    ) {
        Ok(frames) => frames,
        Err(error) => {
            tracing::warn!(%error, "could not project Effect.wz level-up animation; level-up effects are disabled");
            return Ok(Vec::new());
        }
    };
    if frames.is_empty() {
        tracing::warn!(
            "Effect.wz BasicEff.img/LevelUp has no frames; level-up effects are disabled"
        );
    } else {
        assets.extend(animation_assets);
    }
    Ok(frames)
}

fn read_animation_frames(
    animation: &WzNodeArc,
    animation_path: &str,
    fingerprint: &str,
    assets: &mut HashMap<String, Arc<WzAsset>>,
) -> Result<Vec<AnimationFrame>, EffectContentError> {
    let mut indexed = Vec::new();
    for frame in wz::sorted_children(animation)? {
        let name = wz::node_name(&frame)?;
        let Ok(index) = name.parse::<u32>() else {
            continue;
        };
        let source =
            find_png_descendant(&frame, 0)?.ok_or_else(|| EffectContentError::Invalid {
                message: format!("{animation_path} frame {name} has no PNG"),
            })?;
        let (width, height) = png_dimensions(&source)?;
        let Vector2D(origin_x, origin_y) = wz::child(&source, "origin")?
            .as_ref()
            .map(wz::vector_value)
            .transpose()?
            .flatten()
            .unwrap_or(Vector2D(0, 0));
        let delay_ms = wz::int_value(&source, "delay")?
            .or(wz::int_value(&frame, "delay")?)
            .and_then(|value| u32::try_from(value).ok())
            .filter(|value| *value > 0)
            .unwrap_or(DEFAULT_FRAME_DELAY_MS);
        let source_path = wz::node_path(&source)?;
        let asset_id = asset_id(fingerprint, &source_path);
        assets
            .entry(asset_id.clone())
            .or_insert_with(|| Arc::new(WzAsset::new(asset_id.clone(), Arc::clone(&source))));
        indexed.push((
            index,
            AnimationFrame {
                asset_id,
                width: width as f32,
                height: height as f32,
                origin_x: origin_x as f32,
                origin_y: origin_y as f32,
                delay_ms,
            },
        ));
    }
    indexed.sort_by_key(|(index, _)| *index);
    Ok(indexed.into_iter().map(|(_, frame)| frame).collect())
}

fn find_png_descendant(
    node: &WzNodeArc,
    depth: usize,
) -> Result<Option<WzNodeArc>, EffectContentError> {
    if node
        .read()
        .map_err(|_| EffectContentError::Lock {
            context: "effect animation frame",
        })?
        .try_as_png()
        .is_some()
    {
        return Ok(Some(Arc::clone(node)));
    }
    if depth >= 4 {
        return Ok(None);
    }
    for child in wz::sorted_children(node)? {
        if let Some(source) = find_png_descendant(&child, depth + 1)? {
            return Ok(Some(source));
        }
    }
    Ok(None)
}

fn png_dimensions(node: &WzNodeArc) -> Result<(u32, u32), EffectContentError> {
    let read = node.read().map_err(|_| EffectContentError::Lock {
        context: "effect animation PNG geometry",
    })?;
    let png = read
        .try_as_png()
        .ok_or_else(|| EffectContentError::Invalid {
            message: format!("{} is not a PNG", read.get_full_path()),
        })?;
    if png.width == 0 || png.height == 0 {
        return invalid(format!("{} is an empty PNG", read.get_full_path()));
    }
    Ok((png.width, png.height))
}

fn required_child(
    node: &WzNodeArc,
    name: &str,
) -> Result<WzNodeArc, EffectContentError> {
    wz::child(node, name)?.ok_or_else(|| EffectContentError::Invalid {
        message: format!("required node {name:?} is absent"),
    })
}

fn asset_id(
    fingerprint: &str,
    source_path: &str,
) -> String {
    let version = hex::encode(Sha256::digest(
        format!("effect\0{fingerprint}\0{source_path}").as_bytes(),
    ));
    format!("wz-{version}")
}

fn descriptor(asset_id: &str) -> AssetDescriptor {
    let version = asset_id
        .strip_prefix("wz-")
        .expect("effect asset IDs use the WZ prefix");
    AssetDescriptor {
        id: asset_id.to_owned(),
        url: format!("/wz-assets/{version}.png"),
    }
}

fn frame_descriptors(frames: &[AnimationFrame]) -> Vec<AssetDescriptor> {
    frames
        .iter()
        .map(|frame| descriptor(&frame.asset_id))
        .collect()
}

fn invalid<T>(message: impl Into<String>) -> Result<T, EffectContentError> {
    Err(EffectContentError::Invalid {
        message: message.into(),
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use wz_reader::WzNode;
    use wz_reader::WzNodeArc;
    use wz_reader::property::Vector2D;
    use wz_reader::property::WzPng;

    use super::read_animation_frames;
    use super::read_level_up_frames;

    #[test]
    fn missing_level_up_animation_is_an_optional_empty_projection() {
        let root = WzNode::from_str("Effect.wz", 0, None).into_lock();
        let mut assets = HashMap::new();

        let frames = read_level_up_frames(
            &root,
            std::path::Path::new("Effect.wz"),
            "fingerprint",
            &mut assets,
        )
        .expect("optional level-up frames");

        assert!(frames.is_empty());
        assert!(assets.is_empty());
    }

    #[test]
    fn malformed_level_up_animation_is_an_optional_empty_projection() {
        let root = WzNode::from_str("Effect.wz", 0, None).into_lock();
        let basic = WzNode::from_str("BasicEff.img", 0, Some(&root)).into_lock();
        add(&root, Arc::clone(&basic));
        let level_up = WzNode::from_str("LevelUp", 0, Some(&basic)).into_lock();
        add(&basic, Arc::clone(&level_up));
        let frame = WzNode::from_str("0", 0, Some(&level_up)).into_lock();
        add(&level_up, frame);
        let mut assets = HashMap::new();

        let frames = read_level_up_frames(
            &root,
            std::path::Path::new("Effect.wz"),
            "fingerprint",
            &mut assets,
        )
        .expect("optional level-up frames");

        assert!(frames.is_empty());
        assert!(assets.is_empty());
    }

    #[test]
    fn tomb_frames_preserve_numeric_order_and_native_metadata() {
        let fall = WzNode::from_str("fall", 0, None).into_lock();
        add_frame(&fall, "1", 40, 43, 20, 43, 150);
        add_frame(&fall, "0", 38, 43, 19, 43, 80);
        add(
            &fall,
            WzNode::from_str("helper", 0, Some(&fall)).into_lock(),
        );
        let mut assets = HashMap::new();

        let frames = read_animation_frames(&fall, "Tomb.img/fall", "fingerprint", &mut assets)
            .expect("tomb frames");

        assert_eq!(frames.len(), 2);
        assert_eq!(
            (
                frames[0].width,
                frames[0].height,
                frames[0].origin_x,
                frames[0].origin_y,
                frames[0].delay_ms,
            ),
            (38.0, 43.0, 19.0, 43.0, 80)
        );
        assert_eq!(frames[1].delay_ms, 150);
        assert!(
            frames
                .iter()
                .all(|frame| assets.contains_key(&frame.asset_id))
        );
    }

    #[test]
    fn tomb_frames_accept_a_nested_png() {
        let fall = WzNode::from_str("fall", 0, None).into_lock();
        let frame = WzNode::from_str("0", 0, Some(&fall)).into_lock();
        add(&fall, Arc::clone(&frame));
        add_frame(&frame, "canvas", 38, 43, 19, 43, 80);
        let mut assets = HashMap::new();

        let frames = read_animation_frames(&fall, "Tomb.img/fall", "fingerprint", &mut assets)
            .expect("tomb frames");

        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].origin_x, 19.0);
        assert_eq!(frames[0].delay_ms, 80);
    }

    fn add_frame(
        parent: &WzNodeArc,
        name: &str,
        width: u32,
        height: u32,
        origin_x: i32,
        origin_y: i32,
        delay_ms: i32,
    ) {
        let mut png = WzPng::default();
        png.width = width;
        png.height = height;
        let frame = WzNode::from_str(name, png, Some(parent)).into_lock();
        add(parent, Arc::clone(&frame));
        let origin =
            WzNode::from_str("origin", Vector2D(origin_x, origin_y), Some(&frame)).into_lock();
        add(&frame, origin);
        let delay = WzNode::from_str("delay", delay_ms, Some(&frame)).into_lock();
        add(&frame, delay);
    }

    fn add(
        parent: &WzNodeArc,
        child: WzNodeArc,
    ) {
        parent.write().expect("parent lock").add(&child);
    }
}
