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
const STARTER_COAT_ID: u32 = 1_040_002;
const STARTER_PANTS_ID: u32 = 1_060_002;
const STARTER_SHOES_ID: u32 = 1_072_000;
const MAXIMUM_STYLE_CHOICES: usize = 12;

pub struct CharacterContent {
    _base: WzNodeArc,
    bodies: HashMap<u32, WzNodeArc>,
    heads: HashMap<u32, WzNodeArc>,
    faces: HashMap<u32, WzNodeArc>,
    hairs: HashMap<u32, WzNodeArc>,
    starter_clothes: Vec<WzNodeArc>,
    options: CharacterCreationOptions,
    fingerprint: String,
    sprites: RwLock<HashMap<AppearanceKey, CharacterSpriteSet>>,
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
        let starter_clothes = [
            required_style(&coats, STARTER_COAT_ID, "starter coat")?,
            required_style(&pants, STARTER_PANTS_ID, "starter pants")?,
            required_style(&shoes, STARTER_SHOES_ID, "starter shoes")?,
        ]
        .into_iter()
        .collect();
        let options = build_creation_options(&bodies, &heads, &faces, &hairs);
        validate_options(&options)?;

        tracing::info!(
            path = %path.display(),
            skins = options.skins.len(),
            faces = options.faces.len(),
            hairs = options.hairs.len(),
            "WZ character source ready"
        );

        Ok(Some(Self {
            _base: base,
            bodies,
            heads,
            faces,
            hairs,
            starter_clothes,
            options,
            fingerprint: archive_fingerprint(&path)?,
            sprites: RwLock::new(HashMap::new()),
            assets: RwLock::new(HashMap::new()),
        }))
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
    ) -> Result<Option<CharacterSpriteSet>, CharacterContentError> {
        let Some(key) = AppearanceKey::parse(appearance).filter(|key| self.supports_key(*key))
        else {
            return Ok(None);
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

        let sprites = build_sprite_set(self, key)?;
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
            content_hash: version,
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
    appearance: AppearanceKey,
) -> Result<CharacterSpriteSet, CharacterContentError> {
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
    for node in &source.starter_clothes {
        parse(node, node_path(node)?)?;
    }

    let stand = child(&body, "stand1")?.ok_or_else(|| CharacterContentError::Invalid {
        message: format!("body {:08} does not contain stand1", appearance.skin_id),
    })?;
    let mut frames = Vec::new();
    let mut assets = Vec::new();
    let mut asset_ids = HashSet::new();
    for frame in sorted_children(&stand)? {
        let frame_name = node_name(&frame)?;
        if frame_name.parse::<u32>().is_err() {
            continue;
        }
        frames.push(build_frame(
            source,
            &frame,
            &frame_name,
            &head,
            &face,
            &hair,
            &mut assets,
            &mut asset_ids,
        )?);
    }
    if frames.is_empty() {
        return Err(CharacterContentError::Invalid {
            message: format!("body {:08} has no stand1 frames", appearance.skin_id),
        });
    }

    Ok(CharacterSpriteSet {
        idle_frames: frames,
        assets,
    })
}

#[allow(clippy::too_many_arguments)]
fn build_frame(
    source: &CharacterContent,
    body_frame: &WzNodeArc,
    frame_name: &str,
    head: &WzNodeArc,
    face: &WzNodeArc,
    hair: &WzNodeArc,
    assets: &mut Vec<AssetDescriptor>,
    asset_ids: &mut HashSet<String>,
) -> Result<CharacterFrame, CharacterContentError> {
    let mut bones = HashMap::new();
    let mut layers = Vec::new();

    let body_layer = child(body_frame, "body")?.ok_or_else(|| CharacterContentError::Invalid {
        message: format!("stand1 frame {frame_name} has no body layer"),
    })?;
    layers.push(place_layer(&body_layer, None, &mut bones)?);
    add_direct_layers(
        body_frame,
        &["navel"],
        Some("body"),
        &mut bones,
        &mut layers,
    )?;

    let head_frame = child(head, "front")?.ok_or_else(|| CharacterContentError::Invalid {
        message: "the selected head has no front frame".to_owned(),
    })?;
    add_direct_layers(&head_frame, &["neck"], None, &mut bones, &mut layers)?;

    let face_frame = child(face, "default")?.ok_or_else(|| CharacterContentError::Invalid {
        message: "the selected face has no default frame".to_owned(),
    })?;
    add_direct_layers(&face_frame, &["brow"], None, &mut bones, &mut layers)?;

    let hair_frame = child(hair, "default")?.ok_or_else(|| CharacterContentError::Invalid {
        message: "the selected hair has no default frame".to_owned(),
    })?;
    add_direct_layers(&hair_frame, &["brow"], None, &mut bones, &mut layers)?;

    for clothing in &source.starter_clothes {
        if let Some(frame) = animation_frame(clothing, "stand1", frame_name)? {
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
        "hair" | "backHair" | "backHairBelowCap" => 10,
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
    use std::path::Path;

    use oozems_proto::v1::CharacterAppearance;
    use oozems_proto::v1::CharacterGender;
    use wz_reader::property::Vector2D;

    use super::CharacterContent;
    use super::attachment_translation;
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
    fn local_character_archive_builds_and_decodes_a_sprite_set() {
        let directory = Path::new("../../data");
        if !directory.join("Character.wz").exists() {
            return;
        }
        let content = CharacterContent::open_optional(directory)
            .expect("open Character.wz")
            .expect("Character.wz is present");
        let options = content.creation_options();
        let face = options
            .faces
            .iter()
            .find(|option| option.gender == CharacterGender::Male as i32)
            .expect("male face");
        let hair = options
            .hairs
            .iter()
            .find(|option| option.gender == CharacterGender::Male as i32)
            .expect("male hair");
        let appearance = CharacterAppearance {
            gender: CharacterGender::Male as i32,
            skin_id: options.skins[0].id,
            face_id: face.id,
            hair_id: hair.id,
        };

        let sprites = content
            .get_sprites(&appearance)
            .expect("build sprites")
            .expect("supported appearance");

        assert!(!sprites.idle_frames.is_empty());
        assert!(
            sprites
                .idle_frames
                .iter()
                .all(|frame| !frame.layers.is_empty())
        );
        assert!(!sprites.assets.is_empty());
        let png = content
            .get_asset(&sprites.assets[0].id)
            .expect("registered asset")
            .png_bytes()
            .expect("decode PNG");
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    }
}
