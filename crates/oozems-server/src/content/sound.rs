use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::RwLock;

use oozems_proto::v1::AssetDescriptor;
use oozems_proto::v1::MapAudio;
use sha2::Digest;
use sha2::Sha256;
use wz_reader::WzNodeArc;
use wz_reader::WzNodeCast;
use wz_reader::property::WzSoundType;

use super::WzAsset;
use super::wz;
use super::wz::WzContentError;

const SOUND_ARCHIVE: &str = "Sound.wz";

pub(super) struct SoundContent {
    _base: WzNodeArc,
    root: WzNodeArc,
    fingerprint: String,
    descriptors: RwLock<HashMap<String, Option<AssetDescriptor>>>,
    assets: RwLock<HashMap<String, Arc<WzAsset>>>,
}

impl SoundContent {
    pub(super) fn open_optional(directory: &Path) -> Result<Option<Arc<Self>>, WzContentError> {
        let path = directory.join(SOUND_ARCHIVE);
        if !path
            .try_exists()
            .map_err(|source| WzContentError::Metadata {
                path: path.clone(),
                source,
            })?
        {
            tracing::warn!(path = %path.display(), "Sound.wz is absent; music and sound effects will be silent");
            return Ok(None);
        }

        let root = wz::open_archive(&path)?;
        let base = wz::wrap_archive_root(&root)?;
        wz::parse(&root, format!("{} root", path.display()))?;
        let fingerprint = wz::archive_fingerprint(&path)?;
        tracing::info!(path = %path.display(), "WZ sound source ready");
        Ok(Some(Arc::new(Self {
            _base: base,
            root,
            fingerprint,
            descriptors: RwLock::new(HashMap::new()),
            assets: RwLock::new(HashMap::new()),
        })))
    }

    pub(super) fn map_audio(
        &self,
        bgm_reference: Option<&str>,
    ) -> MapAudio {
        MapAudio {
            bgm: bgm_reference.and_then(|reference| self.optional_descriptor(reference)),
            jump: self.optional_descriptor("Game/Jump"),
            portal: self.optional_descriptor("Game/Portal"),
            pick_up_item: self.optional_descriptor("Game/PickUpItem"),
            drop_item: self.optional_descriptor("Game/DropItem"),
            use_item: self.optional_descriptor("Game/UseShopItem"),
            tombstone: self.optional_descriptor("Game/Tombstone"),
            level_up: self.optional_descriptor("Game/LevelUp"),
            quest_clear: self.optional_descriptor("Game/QuestClear"),
        }
    }

    pub(super) fn skill_use(
        &self,
        skill_id: u32,
    ) -> Result<Option<AssetDescriptor>, WzContentError> {
        self.descriptor(&skill_sound_reference(skill_id))
    }

    pub(super) fn mob_damage(
        &self,
        mob_id: u32,
    ) -> Option<AssetDescriptor> {
        self.optional_descriptor(&mob_sound_reference(mob_id, "Damage"))
    }

    pub(super) fn mob_death(
        &self,
        mob_id: u32,
    ) -> Option<AssetDescriptor> {
        self.optional_descriptor(&mob_sound_reference(mob_id, "Die"))
    }

    pub(super) fn get_asset(
        &self,
        asset_id: &str,
    ) -> Option<Arc<WzAsset>> {
        self.assets.read().ok()?.get(asset_id).cloned()
    }

    fn descriptor(
        &self,
        reference: &str,
    ) -> Result<Option<AssetDescriptor>, WzContentError> {
        let Some(path) = normalize_sound_reference(reference) else {
            return Ok(None);
        };
        if let Some(descriptor) = self
            .descriptors
            .read()
            .map_err(|_| lock_error("sound descriptor cache"))?
            .get(&path)
            .cloned()
        {
            return Ok(descriptor);
        }

        let descriptor = match resolve_sound_node(&self.root, &path)? {
            Some(node) if sound_is_supported(&node)? => {
                let source_path = wz::node_path(&node)?;
                Some(self.register_sound(&source_path, &node)?)
            }
            Some(_) => {
                tracing::warn!(path, "skipping WZ sound with an unsupported format");
                None
            }
            None => {
                tracing::warn!(path, "WZ sound reference is absent");
                None
            }
        };
        self.descriptors
            .write()
            .map_err(|_| lock_error("sound descriptor cache"))?
            .insert(path, descriptor.clone());
        Ok(descriptor)
    }

    fn optional_descriptor(
        &self,
        reference: &str,
    ) -> Option<AssetDescriptor> {
        match self.descriptor(reference) {
            Ok(descriptor) => descriptor,
            Err(error) => {
                tracing::warn!(reference, %error, "could not project optional WZ sound");
                None
            }
        }
    }

    fn register_sound(
        &self,
        source_path: &str,
        node: &WzNodeArc,
    ) -> Result<AssetDescriptor, WzContentError> {
        let version = hex::encode(Sha256::digest(
            format!("sound\0{}\0{source_path}", self.fingerprint).as_bytes(),
        ));
        let id = format!("wz-{version}");
        let asset = Arc::new(WzAsset::new_sound(id.clone(), Arc::clone(node))?);
        let extension = asset.extension();
        self.assets
            .write()
            .map_err(|_| lock_error("sound asset registry"))?
            .entry(id.clone())
            .or_insert(asset);
        Ok(AssetDescriptor {
            id,
            url: format!("/wz-assets/{version}.{extension}"),
        })
    }
}

fn normalize_sound_reference(reference: &str) -> Option<String> {
    let mut parts = reference
        .split('/')
        .map(str::trim)
        .filter(|part| !part.is_empty());
    let image = parts.next()?;
    let image = image.strip_suffix(".img").unwrap_or(image);
    if image.is_empty() {
        return None;
    }
    let properties = parts.collect::<Vec<_>>();
    if properties.is_empty() {
        return None;
    }
    Some(format!("{image}.img/{}", properties.join("/")))
}

fn skill_sound_reference(skill_id: u32) -> String {
    format!("Skill/{skill_id:07}/Use")
}

fn mob_sound_reference(
    mob_id: u32,
    cue: &str,
) -> String {
    format!("Mob/{mob_id:07}/{cue}")
}

fn resolve_sound_node(
    root: &WzNodeArc,
    path: &str,
) -> Result<Option<WzNodeArc>, WzContentError> {
    let mut parts = path.split('/');
    let Some(image_name) = parts.next() else {
        return Ok(None);
    };
    let Some(mut node) = find_child(root, image_name, false)? else {
        return Ok(None);
    };
    wz::parse(&node, format!("{SOUND_ARCHIVE}/{image_name}"))?;
    for part in parts {
        let allow_prefix = part.eq_ignore_ascii_case("Use");
        let Some(child) = find_child(&node, part, allow_prefix)? else {
            return Ok(None);
        };
        node = child;
    }
    Ok(Some(node))
}

fn find_child(
    parent: &WzNodeArc,
    name: &str,
    allow_prefix: bool,
) -> Result<Option<WzNodeArc>, WzContentError> {
    let children = wz::children(parent)?;
    if let Some(child) = children
        .iter()
        .find(|child| wz::node_name(child).is_ok_and(|child_name| child_name == name))
    {
        return Ok(Some(Arc::clone(child)));
    }
    let requested = name.to_ascii_lowercase();
    Ok(children.into_iter().find(|child| {
        wz::node_name(child).is_ok_and(|child_name| {
            let child_name = child_name.to_ascii_lowercase();
            child_name == requested || (allow_prefix && child_name.starts_with(&requested))
        })
    }))
}

fn sound_is_supported(node: &WzNodeArc) -> Result<bool, WzContentError> {
    let read = node.read().map_err(|_| lock_error("WZ sound property"))?;
    Ok(read
        .try_as_sound()
        .is_some_and(|sound| matches!(sound.sound_type, WzSoundType::Mp3 | WzSoundType::Wav)))
}

fn lock_error(context: &'static str) -> WzContentError {
    WzContentError::Lock { context }
}

#[cfg(test)]
mod tests {
    use super::mob_sound_reference;
    use super::normalize_sound_reference;
    use super::skill_sound_reference;

    #[test]
    fn sound_references_add_the_wz_image_suffix() {
        assert_eq!(
            normalize_sound_reference("Bgm00/FloralLife"),
            Some("Bgm00.img/FloralLife".to_owned())
        );
        assert_eq!(
            normalize_sound_reference("/Game.img/Jump/"),
            Some("Game.img/Jump".to_owned())
        );
    }

    #[test]
    fn sound_references_require_an_image_and_property() {
        assert_eq!(normalize_sound_reference(""), None);
        assert_eq!(normalize_sound_reference("Game"), None);
    }

    #[test]
    fn skill_sound_references_preserve_seven_digit_ids() {
        assert_eq!(skill_sound_reference(1_003), "Skill/0001003/Use");
        assert_eq!(skill_sound_reference(2_321_003), "Skill/2321003/Use");
    }

    #[test]
    fn mob_sound_references_preserve_seven_digit_ids() {
        assert_eq!(mob_sound_reference(100_100, "Damage"), "Mob/0100100/Damage");
        assert_eq!(mob_sound_reference(100_100, "Die"), "Mob/0100100/Die");
    }
}
