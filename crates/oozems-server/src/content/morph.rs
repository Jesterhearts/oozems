use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use oozems_proto::v1::AssetDescriptor;
use oozems_proto::v1::MorphAnimation;
use oozems_proto::v1::MorphDefinition;
use oozems_proto::v1::MorphFrame;
use sha2::Digest;
use sha2::Sha256;
use thiserror::Error;
use wz_reader::WzNodeArc;
use wz_reader::WzNodeCast;
use wz_reader::property::Vector2D;

use super::WzAsset;
use super::wz;
use super::wz::WzContentError;

const MORPH_ARCHIVE: &str = "Morph.wz";
const REQUIRED_MORPH_IDS: [u32; 2] = [4, 40];
const REQUIRED_ANIMATIONS: [&str; 4] = ["stand", "walk", "jump", "prone"];
const OPTIONAL_ANIMATIONS: [&str; 3] = ["fly", "ladder", "rope"];
const DEFAULT_FRAME_DELAY_MS: u32 = 100;

pub(super) struct MorphContent {
    _base: WzNodeArc,
    definitions: BTreeMap<u32, MorphDefinition>,
    assets: HashMap<String, Arc<WzAsset>>,
}

#[derive(Debug, Error)]
pub enum MorphContentError {
    #[error(transparent)]
    Wz(#[from] WzContentError),
    #[error("morph WZ data is invalid: {message}")]
    Invalid { message: String },
    #[error("internal morph content lock was poisoned while accessing {context}")]
    Lock { context: &'static str },
}

impl MorphContent {
    pub(super) fn load(directory: &Path) -> Result<Self, MorphContentError> {
        let path = directory.join(MORPH_ARCHIVE);
        if !path
            .try_exists()
            .map_err(|source| WzContentError::Metadata {
                path: path.clone(),
                source,
            })?
        {
            return invalid(format!(
                "{} is required by the supported consume effects",
                path.display()
            ));
        }
        let root = wz::open_archive(&path)?;
        let base = wz::wrap_archive_root(&root)?;
        wz::parse(&root, format!("{} root", path.display()))?;
        let fingerprint = wz::archive_fingerprint(&path)?;
        let mut definitions = BTreeMap::new();
        let mut assets = HashMap::new();
        for morph_id in REQUIRED_MORPH_IDS {
            let name = format!("{morph_id:04}.img");
            let node = wz::child(&root, &name)?.ok_or_else(|| MorphContentError::Invalid {
                message: format!("required morph {morph_id} ({name}) is absent"),
            })?;
            wz::parse(&node, format!("{MORPH_ARCHIVE}/{name}"))?;
            let definition = read_definition(morph_id, &node, &fingerprint, &mut assets)?;
            definitions.insert(morph_id, definition);
        }
        Ok(Self {
            _base: base,
            definitions,
            assets,
        })
    }

    pub(super) fn definition(
        &self,
        morph_id: u32,
    ) -> Option<MorphDefinition> {
        self.definitions.get(&morph_id).cloned()
    }

    pub(super) fn ids(&self) -> impl Iterator<Item = u32> + '_ {
        self.definitions.keys().copied()
    }

    pub(super) fn get_asset(
        &self,
        asset_id: &str,
    ) -> Option<Arc<WzAsset>> {
        self.assets.get(asset_id).cloned()
    }
}

fn read_definition(
    morph_id: u32,
    node: &WzNodeArc,
    fingerprint: &str,
    assets: &mut HashMap<String, Arc<WzAsset>>,
) -> Result<MorphDefinition, MorphContentError> {
    let mut animations = Vec::new();
    for name in REQUIRED_ANIMATIONS.into_iter().chain(OPTIONAL_ANIMATIONS) {
        let Some(animation) = wz::child(node, name)? else {
            if REQUIRED_ANIMATIONS.contains(&name) {
                return invalid(format!("required morph {morph_id} has no {name} animation"));
            }
            continue;
        };
        let frames = read_frames(morph_id, name, &animation, fingerprint, assets)?;
        if frames.is_empty() {
            return invalid(format!(
                "required morph {morph_id} animation {name:?} has no frames"
            ));
        }
        animations.push(MorphAnimation {
            name: name.to_owned(),
            frames,
        });
    }
    let info = wz::child(node, "info")?.ok_or_else(|| MorphContentError::Invalid {
        message: format!("required morph {morph_id} has no info node"),
    })?;
    let no_cancel_damage = match wz::int_value(&info, "noCancelDamage")? {
        None | Some(0) => false,
        Some(1) => true,
        Some(value) => {
            return invalid(format!(
                "required morph {morph_id} noCancelDamage must be 0 or 1, not {value}"
            ));
        }
    };
    let descriptors = animations
        .iter()
        .flat_map(|animation| &animation.frames)
        .map(|frame| descriptor(&frame.asset_id))
        .collect::<Vec<_>>();
    let mut seen = HashSet::new();
    let assets = descriptors
        .into_iter()
        .filter(|asset| seen.insert(asset.id.clone()))
        .collect();
    Ok(MorphDefinition {
        morph_id,
        animations,
        assets,
        no_cancel_damage,
    })
}

fn read_frames(
    morph_id: u32,
    animation_name: &str,
    animation: &WzNodeArc,
    fingerprint: &str,
    assets: &mut HashMap<String, Arc<WzAsset>>,
) -> Result<Vec<MorphFrame>, MorphContentError> {
    let mut indexed = Vec::new();
    for frame in wz::sorted_children(animation)? {
        let name = wz::node_name(&frame)?;
        let index = name
            .parse::<u32>()
            .map_err(|_| MorphContentError::Invalid {
                message: format!(
                    "required morph {morph_id} animation {animation_name:?} has nonnumeric frame \
                     {name:?}"
                ),
            })?;
        let source = find_png_descendant(&frame, 0)?.ok_or_else(|| MorphContentError::Invalid {
            message: format!(
                "required morph {morph_id} animation {animation_name:?} frame {name} has no PNG"
            ),
        })?;
        let source_path = wz::node_path(&source)?;
        let asset_id = asset_id(fingerprint, &source_path);
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
        assets
            .entry(asset_id.clone())
            .or_insert_with(|| Arc::new(WzAsset::new(asset_id.clone(), Arc::clone(&source))));
        indexed.push((
            index,
            MorphFrame {
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
) -> Result<Option<WzNodeArc>, MorphContentError> {
    if node
        .read()
        .map_err(|_| MorphContentError::Lock {
            context: "morph frame",
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

fn png_dimensions(node: &WzNodeArc) -> Result<(u32, u32), MorphContentError> {
    let read = node.read().map_err(|_| MorphContentError::Lock {
        context: "morph PNG geometry",
    })?;
    let png = read
        .try_as_png()
        .ok_or_else(|| MorphContentError::Invalid {
            message: format!("{} is not a PNG", read.get_full_path()),
        })?;
    if png.width == 0 || png.height == 0 {
        return invalid(format!("{} is an empty PNG", read.get_full_path()));
    }
    Ok((png.width, png.height))
}

fn asset_id(
    fingerprint: &str,
    source_path: &str,
) -> String {
    let version = hex::encode(Sha256::digest(
        format!("morph\0{fingerprint}\0{source_path}").as_bytes(),
    ));
    format!("wz-{version}")
}

fn descriptor(asset_id: &str) -> AssetDescriptor {
    let version = asset_id
        .strip_prefix("wz-")
        .expect("morph asset IDs use the WZ prefix");
    AssetDescriptor {
        id: asset_id.to_owned(),
        url: format!("/wz-assets/{version}.png"),
        content_hash: version.to_owned(),
    }
}

fn invalid<T>(message: impl Into<String>) -> Result<T, MorphContentError> {
    Err(MorphContentError::Invalid {
        message: message.into(),
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::MorphContent;

    #[test]
    fn local_required_morphs_preserve_audited_animations_and_metadata() {
        let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data");
        if !directory.join("Morph.wz").exists() {
            return;
        }
        let content = MorphContent::load(&directory).expect("local morph content");
        let four = content.definition(4).expect("morph 4");
        let forty = content.definition(40).expect("morph 40");
        assert_eq!(
            four.animations
                .iter()
                .map(|animation| animation.name.as_str())
                .collect::<Vec<_>>(),
            ["stand", "walk", "jump", "prone"]
        );
        assert_eq!(
            forty
                .animations
                .iter()
                .map(|animation| animation.name.as_str())
                .collect::<Vec<_>>(),
            ["stand", "walk", "jump", "prone", "fly", "ladder", "rope"]
        );
        assert!(!four.no_cancel_damage);
        assert!(forty.no_cancel_damage);
        for definition in [four, forty] {
            assert!(definition.animations.iter().all(|animation| {
                !animation.frames.is_empty()
                    && animation.frames.iter().all(|frame| {
                        frame.delay_ms > 0
                            && frame.width > 0.0
                            && frame.height > 0.0
                            && definition
                                .assets
                                .iter()
                                .any(|asset| asset.id == frame.asset_id)
                    })
            }));
            assert!(
                definition
                    .assets
                    .iter()
                    .all(|asset| content.get_asset(&asset.id).is_some())
            );
        }
    }
}
