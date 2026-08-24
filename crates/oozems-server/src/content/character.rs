use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::sync::RwLock;

use oozems_proto::v1::AssetDescriptor;
use oozems_proto::v1::CharacterAppearance;
use oozems_proto::v1::CharacterCreationOptions;
use oozems_proto::v1::CharacterFrame;
use oozems_proto::v1::CharacterGender;
use oozems_proto::v1::CharacterLayer;
use oozems_proto::v1::CharacterSpriteSet;
use oozems_proto::v1::CharacterStyleOption;
use oozems_proto::v1::EquipmentSlot;
use oozems_proto::v1::EquippedItem;
use sha2::Digest;
use sha2::Sha256;
use thiserror::Error;
use wz_reader::WzNodeArc;
use wz_reader::WzNodeCast;
use wz_reader::WzObjectType;
use wz_reader::property::Vector2D;

use super::WzAsset;
use super::wz::WzContentError;
use super::wz::archive_fingerprint;
use super::wz::child;
use super::wz::children;
use super::wz::int_value;
use super::wz::node_name;
use super::wz::node_path;
use super::wz::open_archive;
use super::wz::parse;
use super::wz::sorted_children;
use super::wz::string_value;
use super::wz::vector_value;
use super::wz::wrap_archive_root;

const CHARACTER_ARCHIVE: &str = "Character.wz";
const MAXIMUM_STYLE_CHOICES: usize = 12;
const MALE_PAJAMA_TOP_ID: u32 = 1_042_023;
const MALE_PAJAMA_BOTTOM_ID: u32 = 1_062_025;
const FEMALE_PAJAMA_TOP_ID: u32 = 1_041_113;
const FEMALE_PAJAMA_BOTTOM_ID: u32 = 1_061_112;

pub struct CharacterContent {
    _base: WzNodeArc,
    root: WzNodeArc,
    bodies: HashMap<u32, WzNodeArc>,
    heads: HashMap<u32, WzNodeArc>,
    faces: HashMap<u32, WzNodeArc>,
    hairs: HashMap<u32, WzNodeArc>,
    equipment: HashMap<u32, WzNodeArc>,
    pajamas: PajamaSources,
    options: CharacterCreationOptions,
    fingerprint: String,
    sprites: RwLock<HashMap<CharacterKey, CharacterSpriteSet>>,
    assets: RwLock<HashMap<String, Arc<WzAsset>>>,
}

#[derive(Debug, Error)]
pub enum CharacterContentError {
    #[error(transparent)]
    Wz(#[from] WzContentError),
    #[error("Character.wz is invalid: {message}")]
    Invalid { message: String },
    #[error("internal character content lock was poisoned while accessing {context}")]
    Lock { context: &'static str },
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct AppearanceKey {
    gender: CharacterGender,
    skin_id: u32,
    face_id: u32,
    hair_id: u32,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct CharacterKey {
    appearance: AppearanceKey,
    equipment: Vec<(i32, u32)>,
}

struct PajamaSources {
    male_top: WzNodeArc,
    male_bottom: WzNodeArc,
    female_top: WzNodeArc,
    female_bottom: WzNodeArc,
}

#[derive(Clone, Debug)]
struct PlacedLayer {
    source: WzNodeArc,
    source_path: String,
    z: String,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

#[derive(Clone, Copy)]
struct CharacterParts<'a> {
    body: &'a WzNodeArc,
    head: &'a WzNodeArc,
    face: &'a WzNodeArc,
    hair: &'a WzNodeArc,
    equipment: &'a [WzNodeArc],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HeadView {
    Front,
    Back,
}

impl CharacterContent {
    pub fn open_optional(directory: &Path) -> Result<Option<Self>, CharacterContentError> {
        let path = directory.join(CHARACTER_ARCHIVE);
        if !path
            .try_exists()
            .map_err(|source| WzContentError::Metadata {
                path: path.clone(),
                source,
            })?
        {
            tracing::warn!(path = %path.display(), "Character.wz is absent; character creation is unavailable");
            return Ok(None);
        }

        let root = open_archive(&path)?;
        let base = wrap_archive_root(&root)?;
        parse(&root, format!("{} root", path.display()))?;

        let root_images = index_images(&root)?;
        let bodies = retain_ids(&root_images, 2_000, 3_000);
        let heads = retain_ids(&root_images, 12_000, 13_000);
        let faces = index_directory(&root, "Face")?;
        let hairs = index_directory(&root, "Hair")?;
        let coats = index_directory(&root, "Coat")?;
        let pants = index_directory(&root, "Pants")?;
        let shoes = index_directory(&root, "Shoes")?;
        let equipment = index_supported_equipment(&coats, &pants, &shoes)?;
        let pajamas = load_pajama_sources(&coats, &pants)?;
        let options = build_creation_options(&bodies, &heads, &faces, &hairs);
        validate_options(&options)?;

        tracing::info!(
            path = %path.display(),
            skins = options.skins.len(),
            faces = options.faces.len(),
            hairs = options.hairs.len(),
            "WZ character source ready"
        );

        let content = Self {
            _base: base,
            root,
            bodies,
            heads,
            faces,
            hairs,
            equipment,
            pajamas,
            options,
            fingerprint: archive_fingerprint(&path)?,
            sprites: RwLock::new(HashMap::new()),
            assets: RwLock::new(HashMap::new()),
        };
        Ok(Some(content))
    }

    pub fn creation_options(&self) -> CharacterCreationOptions {
        self.options.clone()
    }

    pub fn supports(
        &self,
        appearance: &CharacterAppearance,
    ) -> bool {
        AppearanceKey::parse(appearance).is_some_and(|key| self.supports_key(key))
    }

    pub fn get_sprites(
        &self,
        appearance: &CharacterAppearance,
        equipment: &[EquippedItem],
    ) -> Result<Option<CharacterSpriteSet>, CharacterContentError> {
        let Some(appearance) =
            AppearanceKey::parse(appearance).filter(|key| self.supports_key(*key))
        else {
            return Ok(None);
        };
        let Some(equipment) = normalize_equipment(equipment) else {
            return Ok(None);
        };
        let key = CharacterKey {
            appearance,
            equipment,
        };
        if let Some(sprites) = self
            .sprites
            .read()
            .map_err(|_| lock_error("character sprite cache"))?
            .get(&key)
            .cloned()
        {
            return Ok(Some(sprites));
        }

        let sprites = build_sprite_set(self, key.clone())?;
        self.sprites
            .write()
            .map_err(|_| lock_error("character sprite cache"))?
            .insert(key, sprites.clone());
        Ok(Some(sprites))
    }

    pub fn get_asset(
        &self,
        asset_id: &str,
    ) -> Option<Arc<WzAsset>> {
        self.assets.read().ok()?.get(asset_id).cloned()
    }

    pub(super) fn item_source(&self) -> (&WzNodeArc, &str) {
        (&self.root, &self.fingerprint)
    }

    fn supports_key(
        &self,
        key: AppearanceKey,
    ) -> bool {
        self.bodies.contains_key(&key.skin_id)
            && self.heads.contains_key(&head_id(key.skin_id))
            && option_supports(&self.options.faces, key.face_id, key.gender)
            && option_supports(&self.options.hairs, key.hair_id, key.gender)
    }

    fn register_asset(
        &self,
        source_path: &str,
        node: &WzNodeArc,
    ) -> Result<AssetDescriptor, CharacterContentError> {
        let version = hex::encode(Sha256::digest(
            format!("character\0{}\0{source_path}", self.fingerprint).as_bytes(),
        ));
        let id = format!("wz-{version}");
        let asset = Arc::new(WzAsset::new(id.clone(), Arc::clone(node)));
        self.assets
            .write()
            .map_err(|_| lock_error("character asset registry"))?
            .entry(id.clone())
            .or_insert(asset);

        Ok(AssetDescriptor {
            id,
            url: format!("/wz-assets/{version}.png"),
        })
    }
}

impl AppearanceKey {
    fn parse(appearance: &CharacterAppearance) -> Option<Self> {
        Some(Self {
            gender: CharacterGender::try_from(appearance.gender)
                .ok()
                .filter(|gender| {
                    matches!(gender, CharacterGender::Male | CharacterGender::Female)
                })?,
            skin_id: appearance.skin_id,
            face_id: appearance.face_id,
            hair_id: appearance.hair_id,
        })
    }
}

fn index_directory(
    root: &WzNodeArc,
    name: &str,
) -> Result<HashMap<u32, WzNodeArc>, CharacterContentError> {
    let directory = child(root, name)?.ok_or_else(|| CharacterContentError::Invalid {
        message: format!("the {name} directory is missing"),
    })?;
    parse(&directory, format!("Character.wz {name}"))?;
    index_images(&directory)
}

fn index_images(node: &WzNodeArc) -> Result<HashMap<u32, WzNodeArc>, CharacterContentError> {
    let mut images = HashMap::new();
    for child in children(node)? {
        let (name, is_image) = {
            let read = child
                .read()
                .map_err(|_| lock_error("character image index"))?;
            (
                read.name.to_string(),
                matches!(&read.object_type, WzObjectType::Image(_)),
            )
        };
        let Some(id) = is_image
            .then(|| name.strip_suffix(".img")?.parse::<u32>().ok())
            .flatten()
        else {
            continue;
        };
        images.insert(id, child);
    }
    Ok(images)
}

fn retain_ids(
    images: &HashMap<u32, WzNodeArc>,
    start: u32,
    end: u32,
) -> HashMap<u32, WzNodeArc> {
    images
        .iter()
        .filter(|(id, _)| (start..end).contains(id))
        .map(|(id, node)| (*id, Arc::clone(node)))
        .collect()
}

fn required_style(
    styles: &HashMap<u32, WzNodeArc>,
    id: u32,
    label: &str,
) -> Result<WzNodeArc, CharacterContentError> {
    styles
        .get(&id)
        .cloned()
        .ok_or_else(|| CharacterContentError::Invalid {
            message: format!("{label} {id:08} is missing"),
        })
}

fn load_pajama_sources(
    coats: &HashMap<u32, WzNodeArc>,
    pants: &HashMap<u32, WzNodeArc>,
) -> Result<PajamaSources, CharacterContentError> {
    Ok(PajamaSources {
        male_top: required_style(coats, MALE_PAJAMA_TOP_ID, "male pajama top")?,
        male_bottom: required_style(pants, MALE_PAJAMA_BOTTOM_ID, "male pajama bottom")?,
        female_top: required_style(coats, FEMALE_PAJAMA_TOP_ID, "female pajama top")?,
        female_bottom: required_style(pants, FEMALE_PAJAMA_BOTTOM_ID, "female pajama bottom")?,
    })
}

fn character_clothing_sources(
    content: &CharacterContent,
    key: &CharacterKey,
) -> Result<Vec<WzNodeArc>, CharacterContentError> {
    let mut sources = key
        .equipment
        .iter()
        .map(|(_, item_id)| required_style(&content.equipment, *item_id, "equipped item"))
        .collect::<Result<Vec<_>, _>>()?;
    let (pajama_top, pajama_bottom) = match key.appearance.gender {
        CharacterGender::Male => (&content.pajamas.male_top, &content.pajamas.male_bottom),
        CharacterGender::Female => (&content.pajamas.female_top, &content.pajamas.female_bottom),
        CharacterGender::Unspecified => {
            return Err(CharacterContentError::Invalid {
                message: "a character with unspecified gender cannot select pajamas".to_owned(),
            });
        }
    };
    if !has_equipment_slot(&key.equipment, EquipmentSlot::Top) {
        sources.push(Arc::clone(pajama_top));
    }
    if !has_equipment_slot(&key.equipment, EquipmentSlot::Bottom) {
        sources.push(Arc::clone(pajama_bottom));
    }
    Ok(sources)
}

fn has_equipment_slot(
    equipment: &[(i32, u32)],
    slot: EquipmentSlot,
) -> bool {
    equipment
        .iter()
        .any(|(equipped_slot, _)| *equipped_slot == slot as i32)
}

fn index_supported_equipment(
    coats: &HashMap<u32, WzNodeArc>,
    pants: &HashMap<u32, WzNodeArc>,
    shoes: &HashMap<u32, WzNodeArc>,
) -> Result<HashMap<u32, WzNodeArc>, CharacterContentError> {
    equipment_specs()
        .into_iter()
        .map(|(item_id, slot)| {
            let source = match slot {
                EquipmentSlot::Top => required_style(coats, item_id, "equipment top")?,
                EquipmentSlot::Bottom => required_style(pants, item_id, "equipment bottom")?,
                EquipmentSlot::Shoes => required_style(shoes, item_id, "equipment shoes")?,
                EquipmentSlot::Unspecified => unreachable!("item specifications use valid slots"),
            };
            Ok((item_id, source))
        })
        .collect()
}

fn equipment_specs() -> [(u32, EquipmentSlot); 6] {
    [
        (crate::items::STARTER_TOP_ID, EquipmentSlot::Top),
        (crate::items::STARTER_BOTTOM_ID, EquipmentSlot::Bottom),
        (crate::items::STARTER_SHOES_ID, EquipmentSlot::Shoes),
        (crate::items::SPARE_TOP_ID, EquipmentSlot::Top),
        (crate::items::SPARE_BOTTOM_ID, EquipmentSlot::Bottom),
        (crate::items::SPARE_SHOES_ID, EquipmentSlot::Shoes),
    ]
}

pub(super) fn supported_equipment_ids() -> [u32; 6] {
    equipment_specs().map(|(item_id, _)| item_id)
}

pub(super) fn supported_equipment_slot(item_id: u32) -> Option<EquipmentSlot> {
    equipment_specs()
        .into_iter()
        .find_map(|(candidate, slot)| (candidate == item_id).then_some(slot))
}

fn normalize_equipment(equipment: &[EquippedItem]) -> Option<Vec<(i32, u32)>> {
    let mut normalized = Vec::with_capacity(equipment.len());
    for equipped in equipment {
        let expected_slot = supported_equipment_slot(equipped.item_id)?;
        let slot = EquipmentSlot::try_from(equipped.slot).ok()?;
        if slot == EquipmentSlot::Unspecified
            || expected_slot != slot
            || normalized
                .iter()
                .any(|(equipped_slot, _)| *equipped_slot == equipped.slot)
        {
            return None;
        }
        normalized.push((equipped.slot, equipped.item_id));
    }
    normalized.sort_unstable();
    Some(normalized)
}

fn build_creation_options(
    bodies: &HashMap<u32, WzNodeArc>,
    heads: &HashMap<u32, WzNodeArc>,
    faces: &HashMap<u32, WzNodeArc>,
    hairs: &HashMap<u32, WzNodeArc>,
) -> CharacterCreationOptions {
    let mut skin_ids = bodies
        .keys()
        .copied()
        .filter(|id| heads.contains_key(&head_id(*id)))
        .collect::<Vec<_>>();
    skin_ids.sort_unstable();
    skin_ids.truncate(MAXIMUM_STYLE_CHOICES);

    CharacterCreationOptions {
        skins: style_options(
            skin_ids,
            CharacterGender::Unspecified,
            "Skin",
            MAXIMUM_STYLE_CHOICES,
        ),
        faces: gendered_style_options(faces, 20_000, 21_000, "Face"),
        hairs: gendered_style_options(hairs, 30_000, 31_000, "Hair"),
    }
}

fn gendered_style_options(
    styles: &HashMap<u32, WzNodeArc>,
    male_start: u32,
    female_start: u32,
    label: &str,
) -> Vec<CharacterStyleOption> {
    let male = sorted_ids(styles, male_start, female_start);
    let female = sorted_ids(styles, female_start, female_start + 1_000);
    style_options(male, CharacterGender::Male, label, MAXIMUM_STYLE_CHOICES)
        .into_iter()
        .chain(style_options(
            female,
            CharacterGender::Female,
            label,
            MAXIMUM_STYLE_CHOICES,
        ))
        .collect()
}

fn sorted_ids(
    styles: &HashMap<u32, WzNodeArc>,
    start: u32,
    end: u32,
) -> Vec<u32> {
    let mut ids = styles
        .keys()
        .copied()
        .filter(|id| (start..end).contains(id))
        .collect::<Vec<_>>();
    ids.sort_unstable();
    ids
}

fn style_options(
    ids: Vec<u32>,
    gender: CharacterGender,
    label: &str,
    maximum: usize,
) -> Vec<CharacterStyleOption> {
    ids.into_iter()
        .take(maximum)
        .enumerate()
        .map(|(index, id)| CharacterStyleOption {
            id,
            label: format!("{label} {}", index + 1),
            gender: gender as i32,
        })
        .collect()
}

fn validate_options(options: &CharacterCreationOptions) -> Result<(), CharacterContentError> {
    let has_gender = |options: &[CharacterStyleOption], gender: CharacterGender| {
        options.iter().any(|option| option.gender == gender as i32)
    };
    if options.skins.is_empty()
        || !has_gender(&options.faces, CharacterGender::Male)
        || !has_gender(&options.faces, CharacterGender::Female)
        || !has_gender(&options.hairs, CharacterGender::Male)
        || !has_gender(&options.hairs, CharacterGender::Female)
    {
        return Err(CharacterContentError::Invalid {
            message: "the archive does not contain complete male and female starter styles"
                .to_owned(),
        });
    }
    Ok(())
}

fn option_supports(
    options: &[CharacterStyleOption],
    id: u32,
    gender: CharacterGender,
) -> bool {
    options
        .iter()
        .any(|option| option.id == id && option.gender == gender as i32)
}

fn head_id(skin_id: u32) -> u32 {
    skin_id + 10_000
}

fn build_sprite_set(
    source: &CharacterContent,
    key: CharacterKey,
) -> Result<CharacterSpriteSet, CharacterContentError> {
    let appearance = key.appearance;
    let body = required_style(&source.bodies, appearance.skin_id, "body")?;
    let head = required_style(&source.heads, head_id(appearance.skin_id), "head")?;
    let face = required_style(&source.faces, appearance.face_id, "face")?;
    let hair = required_style(&source.hairs, appearance.hair_id, "hair")?;
    for (label, node) in [
        ("body", &body),
        ("head", &head),
        ("face", &face),
        ("hair", &hair),
    ] {
        parse(node, format!("Character.wz {label} {}", appearance.skin_id))?;
    }
    let equipment = character_clothing_sources(source, &key)?;
    for node in &equipment {
        parse(node, node_path(node)?)?;
    }

    let mut assets = Vec::new();
    let mut asset_ids = HashSet::new();
    let parts = CharacterParts {
        body: &body,
        head: &head,
        face: &face,
        hair: &hair,
        equipment: &equipment,
    };
    let idle_frames = build_animation(source, parts, "stand1", &mut assets, &mut asset_ids)?;
    let walk_frames = build_animation(source, parts, "walk1", &mut assets, &mut asset_ids)?;
    let jump_frames = build_animation(source, parts, "jump", &mut assets, &mut asset_ids)?;
    let ladder_frames = build_animation(source, parts, "ladder", &mut assets, &mut asset_ids)?;
    let rope_frames = build_animation(source, parts, "rope", &mut assets, &mut asset_ids)?;
    let attack_frames = build_animation(source, parts, "swingO1", &mut assets, &mut asset_ids)?;

    Ok(CharacterSpriteSet {
        idle_frames,
        assets,
        walk_frames,
        jump_frames,
        ladder_frames,
        rope_frames,
        attack_frames,
    })
}

fn build_animation(
    source: &CharacterContent,
    parts: CharacterParts<'_>,
    animation_name: &str,
    assets: &mut Vec<AssetDescriptor>,
    asset_ids: &mut HashSet<String>,
) -> Result<Vec<CharacterFrame>, CharacterContentError> {
    let animation =
        child(parts.body, animation_name)?.ok_or_else(|| CharacterContentError::Invalid {
            message: format!("the selected body does not contain {animation_name}"),
        })?;
    let mut frames = Vec::new();
    for frame in sorted_children(&animation)? {
        let frame_name = node_name(&frame)?;
        if frame_name.parse::<u32>().is_err() {
            continue;
        }
        frames.push(build_frame(
            source,
            &frame,
            &frame_name,
            parts,
            animation_name,
            assets,
            asset_ids,
        )?);
    }
    if frames.is_empty() {
        return Err(CharacterContentError::Invalid {
            message: format!("the selected body has no {animation_name} frames"),
        });
    }
    Ok(frames)
}

fn build_frame(
    source: &CharacterContent,
    body_frame: &WzNodeArc,
    frame_name: &str,
    parts: CharacterParts<'_>,
    animation_name: &str,
    assets: &mut Vec<AssetDescriptor>,
    asset_ids: &mut HashSet<String>,
) -> Result<CharacterFrame, CharacterContentError> {
    let mut bones = HashMap::new();
    let mut layers = Vec::new();

    let body_layer = child(body_frame, "body")?.ok_or_else(|| CharacterContentError::Invalid {
        message: format!("{animation_name} frame {frame_name} has no body layer"),
    })?;
    layers.push(place_layer(&body_layer, None, &mut bones)?);
    add_direct_layers(
        body_frame,
        &["navel"],
        Some("body"),
        &mut bones,
        &mut layers,
    )?;

    let head_view = head_view(body_frame)?;
    let head_frame_name = match head_view {
        HeadView::Front => "front",
        HeadView::Back => "back",
    };
    let head_frame =
        child(parts.head, head_frame_name)?.ok_or_else(|| CharacterContentError::Invalid {
            message: format!("the selected head has no {head_frame_name} frame"),
        })?;
    add_direct_layers(&head_frame, &["neck"], None, &mut bones, &mut layers)?;

    if head_view == HeadView::Front {
        let face_frame =
            child(parts.face, "default")?.ok_or_else(|| CharacterContentError::Invalid {
                message: "the selected face has no default frame".to_owned(),
            })?;
        add_direct_layers(&face_frame, &["brow"], None, &mut bones, &mut layers)?;
    }

    let hair_frame = hair_frame(parts.hair, animation_name, frame_name, head_view)?;
    add_direct_layers(&hair_frame, &["brow"], None, &mut bones, &mut layers)?;

    for clothing in parts.equipment {
        if let Some(frame) = animation_frame(clothing, animation_name, frame_name)? {
            add_direct_layers(&frame, &["navel"], None, &mut bones, &mut layers)?;
        }
    }

    layers.sort_by(|left, right| {
        z_rank(&left.z)
            .cmp(&z_rank(&right.z))
            .then_with(|| left.source_path.cmp(&right.source_path))
    });
    let layers = layers
        .into_iter()
        .map(|layer| build_layer(source, layer, assets, asset_ids))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(CharacterFrame {
        layers,
        delay_ms: int_value(body_frame, "delay")?
            .and_then(|value| u32::try_from(value).ok())
            .unwrap_or(500)
            .max(1),
    })
}

fn head_view(body_frame: &WzNodeArc) -> Result<HeadView, CharacterContentError> {
    Ok(if int_value(body_frame, "face")?.unwrap_or(1) == 0 {
        HeadView::Back
    } else {
        HeadView::Front
    })
}

fn hair_frame(
    hair: &WzNodeArc,
    animation_name: &str,
    frame_name: &str,
    head_view: HeadView,
) -> Result<WzNodeArc, CharacterContentError> {
    if let Some(frame) = animation_frame(hair, animation_name, frame_name)? {
        return Ok(frame);
    }

    let fallback_animation = match head_view {
        HeadView::Front => "stand1",
        HeadView::Back => "ladder",
    };
    if let Some(frame) = animation_frame(hair, fallback_animation, "0")? {
        return Ok(frame);
    }

    child(hair, "default")?.ok_or_else(|| CharacterContentError::Invalid {
        message: format!(
            "the selected hair has no {animation_name} frame {frame_name}, {fallback_animation} \
             fallback, or default frame"
        ),
    })
}

fn animation_frame(
    item: &WzNodeArc,
    animation: &str,
    frame_name: &str,
) -> Result<Option<WzNodeArc>, CharacterContentError> {
    let Some(animation) = child(item, animation)? else {
        return Ok(None);
    };
    Ok(child(&animation, frame_name)?.or(child(&animation, "0")?))
}

fn add_direct_layers(
    container: &WzNodeArc,
    anchors: &[&str],
    skip_name: Option<&str>,
    bones: &mut HashMap<String, Vector2D>,
    output: &mut Vec<PlacedLayer>,
) -> Result<(), CharacterContentError> {
    for node in sorted_children(container)? {
        if skip_name.is_some_and(|name| node_name(&node).is_ok_and(|node_name| node_name == name)) {
            continue;
        }
        let is_png = node
            .read()
            .map_err(|_| lock_error("character PNG layer"))?
            .try_as_png()
            .is_some();
        if is_png {
            output.push(place_layer(&node, Some(anchors), bones)?);
        }
    }
    Ok(())
}

fn place_layer(
    source: &WzNodeArc,
    attachment_order: Option<&[&str]>,
    bones: &mut HashMap<String, Vector2D>,
) -> Result<PlacedLayer, CharacterContentError> {
    let local_anchors = read_anchors(source)?;
    let translation = attachment_order
        .and_then(|names| attachment_translation(bones, &local_anchors, names))
        .unwrap_or(Vector2D(0, 0));
    for (name, Vector2D(x, y)) in &local_anchors {
        bones
            .entry(name.clone())
            .or_insert(Vector2D(translation.0 + x, translation.1 + y));
    }
    let Vector2D(origin_x, origin_y) = child(source, "origin")?
        .as_ref()
        .map(vector_value)
        .transpose()?
        .flatten()
        .unwrap_or(Vector2D(0, 0));
    let (width, height) = {
        let read = source
            .read()
            .map_err(|_| lock_error("character PNG geometry"))?;
        let png = read
            .try_as_png()
            .ok_or_else(|| CharacterContentError::Invalid {
                message: format!("{} is not a PNG layer", read.get_full_path()),
            })?;
        (png.width, png.height)
    };

    Ok(PlacedLayer {
        source: Arc::clone(source),
        source_path: node_path(source)?,
        z: string_value(source, "z")?.unwrap_or_else(|| node_name(source).unwrap_or_default()),
        x: translation.0 - origin_x,
        y: translation.1 - origin_y,
        width,
        height,
    })
}

fn read_anchors(source: &WzNodeArc) -> Result<HashMap<String, Vector2D>, CharacterContentError> {
    let Some(map) = child(source, "map")? else {
        return Ok(HashMap::new());
    };
    let mut anchors = HashMap::new();
    for anchor in children(&map)? {
        if let Some(vector) = vector_value(&anchor)? {
            anchors.insert(node_name(&anchor)?, vector);
        }
    }
    Ok(anchors)
}

fn attachment_translation(
    bones: &HashMap<String, Vector2D>,
    local_anchors: &HashMap<String, Vector2D>,
    order: &[&str],
) -> Option<Vector2D> {
    order.iter().find_map(|name| {
        let global = bones.get(*name)?;
        let local = local_anchors.get(*name)?;
        Some(Vector2D(global.0 - local.0, global.1 - local.1))
    })
}

fn build_layer(
    source: &CharacterContent,
    layer: PlacedLayer,
    assets: &mut Vec<AssetDescriptor>,
    asset_ids: &mut HashSet<String>,
) -> Result<CharacterLayer, CharacterContentError> {
    let asset = source.register_asset(&layer.source_path, &layer.source)?;
    if asset_ids.insert(asset.id.clone()) {
        assets.push(asset.clone());
    }
    Ok(CharacterLayer {
        asset_id: asset.id,
        x: layer.x as f32,
        y: layer.y as f32,
        width: layer.width as f32,
        height: layer.height as f32,
    })
}

fn z_rank(z: &str) -> u16 {
    match z {
        "backBody" => 0,
        "backMailChestBelowPants" => 1,
        "backPantsBelowShoes" => 2,
        "backShoesBelowPants" => 3,
        "backPants" => 4,
        "backShoes" => 5,
        "backPantsOverShoesBelowMailChest" => 6,
        "backMailChest" => 7,
        "backPantsOverMailChest" => 8,
        "backMailChestOverPants" => 9,
        "backHead" | "hair" => 10,
        "backHairBelowCap" => 11,
        "backHair" => 15,
        "body" => 20,
        "pants" | "pantsBelowShoes" => 30,
        "shoes" | "shoesOverPants" => 40,
        "mailChest" => 50,
        "arm" => 60,
        "mailArm" => 70,
        "glove" | "gloveWrist" | "hand" => 80,
        "head" | "ear" => 90,
        "face" => 100,
        "hairOverHead" | "hairBelowCap" => 110,
        _ => 75,
    }
}

fn lock_error(context: &'static str) -> CharacterContentError {
    CharacterContentError::Lock { context }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::RwLock;

    use oozems_proto::v1::CharacterCreationOptions;
    use oozems_proto::v1::CharacterGender;
    use oozems_proto::v1::EquipmentSlot;
    use wz_reader::WzNode;
    use wz_reader::WzNodeArc;
    use wz_reader::property::Vector2D;

    use super::AppearanceKey;
    use super::CharacterContent;
    use super::CharacterKey;
    use super::HeadView;
    use super::PajamaSources;
    use super::attachment_translation;
    use super::character_clothing_sources;
    use super::hair_frame;
    use super::head_view;
    use super::z_rank;

    #[test]
    fn attachment_aligns_the_shared_anchor() {
        let bones = HashMap::from([("neck".to_owned(), Vector2D(-4, -32))]);
        let local = HashMap::from([("neck".to_owned(), Vector2D(0, 15))]);

        assert_eq!(
            attachment_translation(&bones, &local, &["neck"]),
            Some(Vector2D(-4, -47))
        );
    }

    #[test]
    fn front_hair_is_drawn_after_the_face() {
        assert!(z_rank("hair") < z_rank("body"));
        assert!(z_rank("face") < z_rank("hairOverHead"));
    }

    #[test]
    fn back_hair_is_drawn_between_the_back_head_and_body() {
        assert!(z_rank("backHead") < z_rank("backHairBelowCap"));
        assert!(z_rank("backHairBelowCap") < z_rank("backHair"));
        assert!(z_rank("backHair") < z_rank("body"));
    }

    #[test]
    fn back_body_is_drawn_behind_back_facing_clothes_and_head() {
        assert!(z_rank("backBody") < z_rank("backPants"));
        assert!(z_rank("backPants") < z_rank("backShoes"));
        assert!(z_rank("backShoes") < z_rank("backMailChest"));
        assert!(z_rank("backMailChest") < z_rank("backHead"));
    }

    #[test]
    fn body_face_flag_selects_the_head_view() {
        let front = WzNode::from_str("0", 0, None).into_lock();
        let back = WzNode::from_str("1", 0, None).into_lock();
        add(&back, WzNode::from_str("face", 0, Some(&back)).into_lock());

        assert_eq!(head_view(&front).expect("front view"), HeadView::Front);
        assert_eq!(head_view(&back).expect("back view"), HeadView::Back);
    }

    #[test]
    fn hair_frame_uses_the_action_before_the_head_view_fallback() {
        let hair = WzNode::from_str("hair", 0, None).into_lock();
        let stand_frame = add_branch(&add_branch(&hair, "stand1"), "0");
        let ladder_frame = add_branch(&add_branch(&hair, "ladder"), "0");
        let rope = add_branch(&hair, "rope");
        let rope_default_frame = add_branch(&rope, "0");
        let rope_frame = add_branch(&rope, "1");

        let selected = hair_frame(&hair, "rope", "1", HeadView::Back).expect("rope hair");
        assert!(Arc::ptr_eq(&selected, &rope_frame));

        let selected = hair_frame(&hair, "rope", "2", HeadView::Back).expect("rope fallback");
        assert!(Arc::ptr_eq(&selected, &rope_default_frame));

        let selected =
            hair_frame(&hair, "missing", "0", HeadView::Back).expect("back hair fallback");
        assert!(Arc::ptr_eq(&selected, &ladder_frame));

        let selected =
            hair_frame(&hair, "missing", "0", HeadView::Front).expect("front hair fallback");
        assert!(Arc::ptr_eq(&selected, &stand_frame));
    }

    #[test]
    fn clothing_uses_gendered_pajamas_for_unequipped_slots() {
        let root = WzNode::from_str("root", 0, None).into_lock();
        let male_top = WzNode::from_str("male-top", 0, None).into_lock();
        let male_bottom = WzNode::from_str("male-bottom", 0, None).into_lock();
        let female_top = WzNode::from_str("female-top", 0, None).into_lock();
        let female_bottom = WzNode::from_str("female-bottom", 0, None).into_lock();
        let equipped_top = WzNode::from_str("equipped-top", 0, None).into_lock();
        let content = CharacterContent {
            _base: Arc::clone(&root),
            root,
            bodies: HashMap::new(),
            heads: HashMap::new(),
            faces: HashMap::new(),
            hairs: HashMap::new(),
            equipment: HashMap::from([(crate::items::STARTER_TOP_ID, Arc::clone(&equipped_top))]),
            pajamas: PajamaSources {
                male_top: Arc::clone(&male_top),
                male_bottom: Arc::clone(&male_bottom),
                female_top: Arc::clone(&female_top),
                female_bottom: Arc::clone(&female_bottom),
            },
            options: CharacterCreationOptions::default(),
            fingerprint: "fake".to_owned(),
            sprites: RwLock::new(HashMap::new()),
            assets: RwLock::new(HashMap::new()),
        };
        let male = AppearanceKey {
            gender: CharacterGender::Male,
            skin_id: 0,
            face_id: 0,
            hair_id: 0,
        };

        let naked = character_clothing_sources(
            &content,
            &CharacterKey {
                appearance: male,
                equipment: Vec::new(),
            },
        )
        .expect("male pajamas");
        assert!(naked.iter().any(|source| Arc::ptr_eq(source, &male_top)));
        assert!(naked.iter().any(|source| Arc::ptr_eq(source, &male_bottom)));

        let top_equipped = character_clothing_sources(
            &content,
            &CharacterKey {
                appearance: male,
                equipment: vec![(EquipmentSlot::Top as i32, crate::items::STARTER_TOP_ID)],
            },
        )
        .expect("equipped top and pajama bottom");
        assert!(
            top_equipped
                .iter()
                .any(|source| Arc::ptr_eq(source, &equipped_top))
        );
        assert!(
            top_equipped
                .iter()
                .any(|source| Arc::ptr_eq(source, &male_bottom))
        );
        assert!(
            !top_equipped
                .iter()
                .any(|source| Arc::ptr_eq(source, &male_top))
        );

        let female = character_clothing_sources(
            &content,
            &CharacterKey {
                appearance: AppearanceKey {
                    gender: CharacterGender::Female,
                    ..male
                },
                equipment: Vec::new(),
            },
        )
        .expect("female pajamas");
        assert!(female.iter().any(|source| Arc::ptr_eq(source, &female_top)));
        assert!(
            female
                .iter()
                .any(|source| Arc::ptr_eq(source, &female_bottom))
        );
    }

    fn add(
        parent: &WzNodeArc,
        child: WzNodeArc,
    ) {
        parent.write().expect("parent lock").add(&child);
    }

    fn add_branch(
        parent: &WzNodeArc,
        name: &str,
    ) -> WzNodeArc {
        let child = WzNode::from_str(name, 0, Some(parent)).into_lock();
        add(parent, Arc::clone(&child));
        child
    }
}
