use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::RwLock;

use oozems_proto::v1::AssetDescriptor;
use oozems_proto::v1::EquipmentSlot;
use oozems_proto::v1::ItemCategory;
use oozems_proto::v1::ItemDefinition;
use sha2::Digest;
use sha2::Sha256;
use thiserror::Error;
use wz_reader::WzNodeArc;
use wz_reader::WzNodeCast;
use wz_reader::property::WzValue;

use super::WzAsset;
use super::character;
use super::character::CharacterContent;
use super::wz;
use super::wz::WzContentError;

mod consume;
mod monster_book;

pub(crate) use consume::ConsumeEffectDefinition;
pub(crate) use monster_book::MonsterBookCardDefinition;

const ITEM_ARCHIVE: &str = "Item.wz";
const CHARACTER_ARCHIVE: &str = "Character.wz";
const STRING_ARCHIVE: &str = "String.wz";
const DEFAULT_STACK_MAX: u32 = 100;
const INSTALL_STACK_MAX: u32 = 1;

const ORDINARY_SOURCES: [OrdinarySource; 5] = [
    OrdinarySource {
        directory: "Consume",
        category: ItemCategory::Consume,
        direct_images: false,
    },
    OrdinarySource {
        directory: "Etc",
        category: ItemCategory::Etc,
        direct_images: false,
    },
    OrdinarySource {
        directory: "Install",
        category: ItemCategory::Install,
        direct_images: false,
    },
    OrdinarySource {
        directory: "Cash",
        category: ItemCategory::Cash,
        direct_images: false,
    },
    OrdinarySource {
        directory: "Pet",
        category: ItemCategory::Pet,
        direct_images: true,
    },
];

const INVENTORY_EQUIPMENT_DIRECTORIES: [&str; 14] = [
    "Accessory",
    "Cap",
    "Cape",
    "Coat",
    "Dragon",
    "Glove",
    "Longcoat",
    "Pants",
    "PetEquip",
    "Ring",
    "Shield",
    "Shoes",
    "TamingMob",
    "Weapon",
];

const STRING_IMAGES: [&str; 6] = [
    "Consume.img",
    "Etc.img",
    "Ins.img",
    "Cash.img",
    "Pet.img",
    "Eqp.img",
];

pub struct ItemContent {
    _bases: Vec<WzNodeArc>,
    sources: BTreeMap<u32, ItemSource>,
    texts: HashMap<u32, ItemText>,
    fingerprints: SourceFingerprints,
    eager_definitions: Vec<ItemDefinition>,
    consume_effects: BTreeMap<u32, ConsumeEffectDefinition>,
    monster_book_cards: BTreeMap<u32, MonsterBookCardDefinition>,
    materialization: Mutex<()>,
    assets: RwLock<HashMap<String, Arc<WzAsset>>>,
}

#[derive(Debug, Error)]
pub enum ItemContentError {
    #[error(transparent)]
    Wz(#[from] WzContentError),
    #[error("item WZ data is invalid: {message}")]
    Invalid { message: String },
    #[error("internal item content lock was poisoned while accessing {context}")]
    Lock { context: &'static str },
}

#[derive(Clone, Copy)]
struct OrdinarySource {
    directory: &'static str,
    category: ItemCategory,
    direct_images: bool,
}

#[derive(Default)]
struct ItemText {
    name: String,
    description: String,
}

struct OpenArchive {
    base: WzNodeArc,
    root: WzNodeArc,
    fingerprint: String,
}

#[derive(Clone, Copy)]
enum SourceArchive {
    Item,
    Character,
}

struct ItemSource {
    archive: SourceArchive,
    category: ItemCategory,
    image: WzNodeArc,
    inner_path: Option<String>,
    source_path: String,
    definition: OnceLock<ItemDefinition>,
}

#[derive(Clone, Copy)]
struct ItemSourceData<'a> {
    category: ItemCategory,
    image: &'a WzNodeArc,
    inner_path: Option<&'a str>,
    source_path: &'a str,
}

#[derive(Default)]
struct SourceFingerprints {
    item: Option<String>,
    character: Option<String>,
}

impl ItemContent {
    pub fn load(
        directory: &Path,
        characters: Option<&CharacterContent>,
    ) -> Result<Option<Self>, ItemContentError> {
        let texts = load_item_texts(&directory.join(STRING_ARCHIVE))?;
        let mut bases = Vec::new();
        let mut sources = BTreeMap::new();
        let mut fingerprints = SourceFingerprints::default();

        if let Some(archive) = open_optional_archive(&directory.join(ITEM_ARCHIVE))? {
            load_ordinary_sources(&mut sources, &archive.root)?;
            fingerprints.item = Some(archive.fingerprint);
            bases.push(archive.base);
        } else {
            tracing::warn!(
                path = %directory.join(ITEM_ARCHIVE).display(),
                "Item.wz is absent; ordinary item metadata is unavailable"
            );
        }

        if let Some(characters) = characters {
            let (root, fingerprint) = characters.item_source();
            load_equipment_sources(&mut sources, root)?;
            fingerprints.character = Some(fingerprint.to_owned());
        }

        if sources.is_empty() {
            return Ok(None);
        }
        let (consume_effects, monster_book_cards) = if fingerprints.item.is_some() {
            let source_data = item_source_data(&sources);
            (
                consume::load(&source_data)?,
                monster_book::load(&source_data)?,
            )
        } else {
            (BTreeMap::new(), BTreeMap::new())
        };

        tracing::info!(indexed_items = sources.len(), "WZ item source index ready");
        Ok(Some(Self {
            _bases: bases,
            sources,
            texts,
            fingerprints,
            eager_definitions: Vec::new(),
            consume_effects,
            monster_book_cards,
            materialization: Mutex::new(()),
            assets: RwLock::new(HashMap::new()),
        }))
    }

    pub fn source_ids(&self) -> BTreeSet<u32> {
        self.sources.keys().copied().collect()
    }

    pub fn consume_effect_ids(&self) -> BTreeSet<u32> {
        self.consume_effects.keys().copied().collect()
    }

    pub fn consume_effect_definitions(&self) -> Vec<ConsumeEffectDefinition> {
        self.consume_effects.values().copied().collect()
    }

    pub fn monster_book_card_ids(&self) -> BTreeSet<u32> {
        self.monster_book_cards.keys().copied().collect()
    }

    pub fn monster_book_card(
        &self,
        item_id: u32,
    ) -> Option<MonsterBookCardDefinition> {
        self.monster_book_cards.get(&item_id).copied()
    }

    pub fn equipment_source_ids(&self) -> BTreeSet<u32> {
        self.sources
            .iter()
            .filter_map(|(item_id, source)| {
                (source.category == ItemCategory::Equipment).then_some(*item_id)
            })
            .collect()
    }

    pub fn materialize_eager(
        &mut self,
        item_ids: &BTreeSet<u32>,
    ) -> Result<(), ItemContentError> {
        let mut requested = item_ids.clone();
        requested.extend(character::supported_equipment_ids());
        requested.retain(|item_id| self.sources.contains_key(item_id));
        self.materialize_additional_eager(&requested)?;
        tracing::info!(
            indexed_items = self.sources.len(),
            materialized_items = self.eager_definitions.len(),
            "WZ item catalog ready"
        );
        Ok(())
    }

    pub fn materialize_additional_eager(
        &mut self,
        item_ids: &BTreeSet<u32>,
    ) -> Result<(), ItemContentError> {
        if let Some(item_id) = item_ids
            .iter()
            .find(|item_id| !self.sources.contains_key(item_id))
        {
            return invalid(format!("item {item_id} is not in the item source index"));
        }
        let eager_ids = self
            .eager_definitions
            .iter()
            .map(|definition| definition.item_id)
            .collect::<BTreeSet<_>>();
        let requested = item_ids
            .difference(&eager_ids)
            .copied()
            .collect::<BTreeSet<_>>();
        let mut ordinary_groups = BTreeMap::<String, Vec<u32>>::new();
        for item_id in &requested {
            let source = self
                .sources
                .get(item_id)
                .expect("additional eager item IDs were checked against the source index");
            if source.inner_path.is_some() {
                let image_path = source
                    .source_path
                    .rsplit_once('/')
                    .map(|(image_path, _)| image_path)
                    .unwrap_or(source.source_path.as_str());
                ordinary_groups
                    .entry(image_path.to_owned())
                    .or_default()
                    .push(*item_id);
            }
        }
        for item_ids in ordinary_groups.values() {
            self.materialize_ordinary_group(item_ids)?;
        }
        for item_id in requested {
            let definition = self
                .definition(item_id)?
                .expect("additional eager item IDs were checked against the source index")
                .clone();
            self.eager_definitions.push(definition);
        }
        self.eager_definitions
            .sort_unstable_by_key(|definition| definition.item_id);
        Ok(())
    }

    fn materialize_ordinary_group(
        &self,
        item_ids: &[u32],
    ) -> Result<(), ItemContentError> {
        let Some(first) = item_ids
            .first()
            .and_then(|item_id| self.sources.get(item_id))
        else {
            return Ok(());
        };
        wz::parse(&first.image, first.source_path.clone())?;
        let result = item_ids.iter().try_for_each(|item_id| {
            let source = self
                .sources
                .get(item_id)
                .expect("ordinary materialization IDs come from the source index");
            if source.definition.get().is_some() {
                return Ok(());
            }
            let inner_path = source
                .inner_path
                .as_deref()
                .expect("ordinary materialization sources have an inner path");
            let node = required_child(&source.image, inner_path, &source.source_path)?;
            let definition = build_definition_from_source(self, *item_id, source, &node)?;
            let _ = source.definition.set(definition);
            Ok(())
        });
        first
            .image
            .write()
            .map_err(|_| ItemContentError::Lock {
                context: "ordinary item materialization image",
            })?
            .unparse();
        result
    }

    pub fn definition(
        &self,
        item_id: u32,
    ) -> Result<Option<&ItemDefinition>, ItemContentError> {
        let Some(source) = self.sources.get(&item_id) else {
            return Ok(None);
        };
        if source.definition.get().is_none() {
            let _guard = self
                .materialization
                .lock()
                .map_err(|_| ItemContentError::Lock {
                    context: "item materialization",
                })?;
            if source.definition.get().is_none() {
                let definition = build_definition(self, item_id, source)?;
                let _ = source.definition.set(definition);
            }
        }
        Ok(source.definition.get())
    }

    pub fn eager_definition_slice(&self) -> &[ItemDefinition] {
        &self.eager_definitions
    }

    pub fn gui_projection(
        &self,
        item_ids: &BTreeSet<u32>,
    ) -> Result<(Vec<ItemDefinition>, Vec<AssetDescriptor>), ItemContentError> {
        let mut definitions = self.eager_definitions.clone();
        let eager_ids = definitions
            .iter()
            .map(|definition| definition.item_id)
            .collect::<BTreeSet<_>>();
        for item_id in item_ids.difference(&eager_ids) {
            let definition =
                self.definition(*item_id)?
                    .ok_or_else(|| ItemContentError::Invalid {
                        message: format!("inventory item {item_id} is absent from the item index"),
                    })?;
            definitions.push(definition.clone());
        }
        definitions.sort_by_key(|definition| definition.item_id);
        let assets = definitions
            .iter()
            .map(|definition| descriptor_from_asset_id(&definition.icon_asset_id))
            .collect();
        Ok((definitions, assets))
    }

    pub fn get_asset(
        &self,
        asset_id: &str,
    ) -> Option<Arc<WzAsset>> {
        self.assets.read().ok()?.get(asset_id).cloned()
    }
}

fn open_optional_archive(path: &Path) -> Result<Option<OpenArchive>, ItemContentError> {
    if !archive_exists(path)? {
        return Ok(None);
    }
    let root = wz::open_archive(path)?;
    let base = wz::wrap_archive_root(&root)?;
    wz::parse(&root, format!("{} root", path.display()))?;
    Ok(Some(OpenArchive {
        base,
        root,
        fingerprint: wz::archive_fingerprint(path)?,
    }))
}

fn item_source_data(sources: &BTreeMap<u32, ItemSource>) -> BTreeMap<u32, ItemSourceData<'_>> {
    sources
        .iter()
        .map(|(item_id, source)| {
            (
                *item_id,
                ItemSourceData {
                    category: source.category,
                    image: &source.image,
                    inner_path: source.inner_path.as_deref(),
                    source_path: &source.source_path,
                },
            )
        })
        .collect()
}

fn archive_exists(path: &Path) -> Result<bool, WzContentError> {
    path.try_exists()
        .map_err(|source| WzContentError::Metadata {
            path: path.to_owned(),
            source,
        })
}

fn load_ordinary_sources(
    sources: &mut BTreeMap<u32, ItemSource>,
    root: &WzNodeArc,
) -> Result<(), ItemContentError> {
    for source in ORDINARY_SOURCES {
        let directory = required_child(root, source.directory, ITEM_ARCHIVE)?;
        wz::parse(&directory, format!("{ITEM_ARCHIVE}/{}", source.directory))?;
        for image in wz::sorted_children(&directory)? {
            let image_name = wz::node_name(&image)?;
            if source.direct_images {
                let Some(item_id) = image_item_id(&image_name) else {
                    continue;
                };
                add_source(
                    sources,
                    item_id,
                    source.category,
                    SourceArchive::Item,
                    Arc::clone(&image),
                    None,
                    format!("{ITEM_ARCHIVE}/{}/{image_name}", source.directory),
                )?;
                continue;
            }

            let image_path = format!("{ITEM_ARCHIVE}/{}/{image_name}", source.directory);
            wz::parse(&image, image_path.clone())?;
            let mut item_ids = Vec::new();
            for item in wz::sorted_children(&image)? {
                let item_name = wz::node_name(&item)?;
                if let Some(item_id) = numeric_item_id(&item_name) {
                    item_ids.push((item_id, item_name));
                }
            }
            image
                .write()
                .map_err(|_| ItemContentError::Lock {
                    context: "ordinary item source image",
                })?
                .unparse();
            for (item_id, item_name) in item_ids {
                add_source(
                    sources,
                    item_id,
                    source.category,
                    SourceArchive::Item,
                    Arc::clone(&image),
                    Some(item_name.clone()),
                    format!(
                        "{ITEM_ARCHIVE}/{}/{image_name}/{item_name}",
                        source.directory
                    ),
                )?;
            }
        }
    }
    Ok(())
}

fn load_equipment_sources(
    sources: &mut BTreeMap<u32, ItemSource>,
    root: &WzNodeArc,
) -> Result<(), ItemContentError> {
    for directory_name in INVENTORY_EQUIPMENT_DIRECTORIES {
        let directory = required_child(root, directory_name, CHARACTER_ARCHIVE)?;
        wz::parse(&directory, format!("{CHARACTER_ARCHIVE}/{directory_name}"))?;
        for image in wz::sorted_children(&directory)? {
            let image_name = wz::node_name(&image)?;
            let Some(item_id) = image_item_id(&image_name) else {
                continue;
            };
            add_source(
                sources,
                item_id,
                ItemCategory::Equipment,
                SourceArchive::Character,
                Arc::clone(&image),
                None,
                format!("{CHARACTER_ARCHIVE}/{directory_name}/{image_name}"),
            )?;
        }
    }
    Ok(())
}

fn add_source(
    sources: &mut BTreeMap<u32, ItemSource>,
    item_id: u32,
    category: ItemCategory,
    archive: SourceArchive,
    image: WzNodeArc,
    inner_path: Option<String>,
    source_path: String,
) -> Result<(), ItemContentError> {
    if sources.contains_key(&item_id) {
        return invalid(format!("item {item_id} appears more than once"));
    }
    sources.insert(
        item_id,
        ItemSource {
            archive,
            category,
            image,
            inner_path,
            source_path,
            definition: OnceLock::new(),
        },
    );
    Ok(())
}

fn build_definition(
    content: &ItemContent,
    item_id: u32,
    indexed: &ItemSource,
) -> Result<ItemDefinition, ItemContentError> {
    if let Some(inner_path) = &indexed.inner_path {
        wz::parse(&indexed.image, indexed.source_path.clone())?;
        let result = required_child(&indexed.image, inner_path, &indexed.source_path)
            .and_then(|source| build_definition_from_source(content, item_id, indexed, &source));
        indexed
            .image
            .write()
            .map_err(|_| ItemContentError::Lock {
                context: "ordinary item materialization image",
            })?
            .unparse();
        return result;
    }
    wz::parse(&indexed.image, indexed.source_path.clone())?;
    build_definition_from_source(content, item_id, indexed, &indexed.image)
}

fn build_definition_from_source(
    content: &ItemContent,
    item_id: u32,
    indexed: &ItemSource,
    source: &WzNodeArc,
) -> Result<ItemDefinition, ItemContentError> {
    let source_path = indexed.source_path.as_str();
    let info = required_child(source, "info", source_path)?;
    let icon = required_child(&info, "icon", source_path)?;
    let (icon_width, icon_height) = png_dimensions(&icon, item_id)?;
    let price = numeric_property(&info, "price", item_id)?;
    let not_sale = boolean_property(&info, "notSale", item_id)?.unwrap_or(false);
    let sale_price = normalize_sale_price(price, not_sale)
        .map_err(|message| ItemContentError::Invalid { message })?;
    let stack_max = normalize_stack_max(
        indexed.category,
        numeric_property(&info, "slotMax", item_id)?,
    )
    .map_err(|message| ItemContentError::Invalid { message })?;
    let text = content.texts.get(&item_id);
    let name = text
        .map(|text| text.name.as_str())
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| item_id.to_string());
    let description = text
        .map(|text| text.description.clone())
        .unwrap_or_default();
    let slot = (indexed.category == ItemCategory::Equipment)
        .then(|| character::supported_equipment_slot(item_id))
        .flatten();
    let appearance_supported = slot.is_some();
    let fingerprint = match indexed.archive {
        SourceArchive::Item => content.fingerprints.item.as_deref(),
        SourceArchive::Character => content.fingerprints.character.as_deref(),
    }
    .ok_or_else(|| ItemContentError::Invalid {
        message: format!("item {item_id} source archive has no fingerprint"),
    })?;
    let (descriptor, asset) = item_asset(fingerprint, source_path, &icon);
    content
        .assets
        .write()
        .map_err(|_| ItemContentError::Lock {
            context: "item asset registry",
        })?
        .entry(descriptor.id.clone())
        .or_insert(asset);

    Ok(ItemDefinition {
        item_id,
        name,
        slot: slot.unwrap_or(EquipmentSlot::Unspecified) as i32,
        icon_asset_id: descriptor.id,
        icon_width,
        icon_height,
        sale_price,
        category: indexed.category as i32,
        stack_max,
        description,
        appearance_supported,
    })
}

fn item_asset(
    fingerprint: &str,
    source_path: &str,
    node: &WzNodeArc,
) -> (AssetDescriptor, Arc<WzAsset>) {
    let version = hex::encode(Sha256::digest(
        format!("item\0{fingerprint}\0{source_path}").as_bytes(),
    ));
    let id = format!("wz-{version}");
    let descriptor = AssetDescriptor {
        id: id.clone(),
        url: format!("/wz-assets/{version}.png"),
    };
    (descriptor, Arc::new(WzAsset::new(id, Arc::clone(node))))
}

fn descriptor_from_asset_id(asset_id: &str) -> AssetDescriptor {
    let version = asset_id
        .strip_prefix("wz-")
        .expect("item asset IDs use the WZ prefix");
    AssetDescriptor {
        id: asset_id.to_owned(),
        url: format!("/wz-assets/{version}.png"),
    }
}

fn load_item_texts(path: &Path) -> Result<HashMap<u32, ItemText>, ItemContentError> {
    if !archive_exists(path)? {
        tracing::warn!(path = %path.display(), "String.wz is absent; using numeric item names");
        return Ok(HashMap::new());
    }

    let root = wz::open_archive(path)?;
    let _base = wz::wrap_archive_root(&root)?;
    wz::parse(&root, format!("{} root", path.display()))?;
    let mut texts = HashMap::new();
    for image_name in STRING_IMAGES {
        let Some(image) = wz::child(&root, image_name)? else {
            tracing::warn!(path = %path.display(), image = image_name, "String.wz item text image is absent");
            continue;
        };
        wz::parse(&image, format!("{} {image_name}", path.display()))?;
        collect_item_texts(&image, &mut texts)?;
        image
            .write()
            .map_err(|_| ItemContentError::Lock {
                context: "item text image",
            })?
            .unparse();
    }
    Ok(texts)
}

fn collect_item_texts(
    node: &WzNodeArc,
    texts: &mut HashMap<u32, ItemText>,
) -> Result<(), ItemContentError> {
    let node_name = wz::node_name(node)?;
    if let Some(item_id) = numeric_item_id(&node_name)
        && let Some(name) = wz::string_value(node, "name")?
    {
        let description = wz::string_value(node, "desc")?
            .map(normalize_text)
            .unwrap_or_default();
        texts.entry(item_id).or_insert_with(|| ItemText {
            name: normalize_text(name),
            description,
        });
        return Ok(());
    }

    for child in wz::sorted_children(node)? {
        collect_item_texts(&child, texts)?;
    }
    Ok(())
}

fn numeric_property(
    node: &WzNodeArc,
    name: &str,
    item_id: u32,
) -> Result<Option<i64>, ItemContentError> {
    let Some(value) = wz::child(node, name)? else {
        return Ok(None);
    };
    let read = value.read().map_err(|_| ItemContentError::Lock {
        context: "item numeric metadata",
    })?;
    if let Some(value) = read.try_as_int() {
        return Ok(Some(i64::from(*value)));
    }
    if let Some(value) = read.try_as_short() {
        return Ok(Some(i64::from(*value)));
    }
    if let Some(value) = read.try_as_long() {
        return Ok(Some(*value));
    }
    let text = read
        .try_as_string()
        .and_then(|value| value.get_string().ok())
        .or_else(|| match read.try_as_value() {
            Some(WzValue::ParsedString(value)) => Some(value.clone()),
            _ => None,
        });
    match text {
        Some(text) => text
            .trim()
            .parse::<i64>()
            .map(Some)
            .map_err(|_| ItemContentError::Invalid {
                message: format!("item {item_id} property {name} is not an integer: {text:?}"),
            }),
        None => invalid(format!(
            "item {item_id} property {name} has an unsupported value type"
        )),
    }
}

fn boolean_property(
    node: &WzNodeArc,
    name: &str,
    item_id: u32,
) -> Result<Option<bool>, ItemContentError> {
    numeric_property(node, name, item_id).map(|value| value.map(|value| value != 0))
}

fn normalize_sale_price(
    price: Option<i64>,
    not_sale: bool,
) -> Result<u64, String> {
    if not_sale {
        return Ok(0);
    }
    match price {
        Some(price) => {
            u64::try_from(price).map_err(|_| format!("item sale price cannot be negative: {price}"))
        }
        None => Ok(0),
    }
}

fn normalize_stack_max(
    category: ItemCategory,
    stack_max: Option<i64>,
) -> Result<u32, String> {
    if matches!(category, ItemCategory::Equipment | ItemCategory::Pet) {
        return Ok(1);
    }
    let fallback = if category == ItemCategory::Install {
        INSTALL_STACK_MAX
    } else {
        DEFAULT_STACK_MAX
    };
    match stack_max {
        Some(0) | None => Ok(fallback),
        Some(value) => u32::try_from(value)
            .map_err(|_| format!("item stack maximum is outside the supported range: {value}")),
    }
}

fn image_item_id(name: &str) -> Option<u32> {
    numeric_item_id(name.strip_suffix(".img")?)
}

fn numeric_item_id(name: &str) -> Option<u32> {
    name.parse().ok()
}

fn normalize_text(value: String) -> String {
    value.replace("\\n", "\n")
}

fn png_dimensions(
    node: &WzNodeArc,
    item_id: u32,
) -> Result<(f32, f32), ItemContentError> {
    let read = node.read().map_err(|_| ItemContentError::Lock {
        context: "item icon dimensions",
    })?;
    let png = read.try_as_png().ok_or_else(|| ItemContentError::Invalid {
        message: format!("item {item_id} icon is not a PNG sprite"),
    })?;
    if png.width == 0 || png.height == 0 {
        return invalid(format!("item {item_id} icon is empty"));
    }
    Ok((png.width as f32, png.height as f32))
}

fn required_child(
    node: &WzNodeArc,
    name: &str,
    context: &str,
) -> Result<WzNodeArc, ItemContentError> {
    wz::child(node, name)?.ok_or_else(|| ItemContentError::Invalid {
        message: format!("{context} has no {name} node"),
    })
}

fn invalid<T>(message: impl Into<String>) -> Result<T, ItemContentError> {
    Err(ItemContentError::Invalid {
        message: message.into(),
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::collections::BTreeSet;
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::sync::OnceLock;
    use std::sync::RwLock;

    use oozems_proto::v1::ItemCategory;
    use wz_reader::WzNode;
    use wz_reader::WzObjectType;
    use wz_reader::property::WzPng;
    use wz_reader::property::WzSubProperty;

    use super::ItemContent;
    use super::ItemSource;
    use super::ItemText;
    use super::SourceArchive;
    use super::SourceFingerprints;
    use super::normalize_sale_price;
    use super::normalize_stack_max;

    #[test]
    fn stack_max_uses_category_defaults_and_explicit_limits() {
        assert_eq!(
            normalize_stack_max(ItemCategory::Equipment, Some(200)),
            Ok(1)
        );
        assert_eq!(normalize_stack_max(ItemCategory::Pet, None), Ok(1));
        assert_eq!(normalize_stack_max(ItemCategory::Install, None), Ok(1));
        assert_eq!(normalize_stack_max(ItemCategory::Consume, None), Ok(100));
        assert_eq!(normalize_stack_max(ItemCategory::Etc, Some(200)), Ok(200));
        assert!(normalize_stack_max(ItemCategory::Cash, Some(-1)).is_err());
    }

    #[test]
    fn sale_price_honors_not_sale_and_rejects_negative_prices() {
        assert_eq!(normalize_sale_price(Some(500), false), Ok(500));
        assert_eq!(normalize_sale_price(None, false), Ok(0));
        assert_eq!(normalize_sale_price(Some(500), true), Ok(0));
        assert!(normalize_sale_price(Some(-1), false).is_err());
    }

    #[test]
    fn lazy_item_materialization_projects_metadata_and_registered_assets() {
        let item_id = 4_000_001;
        let mut content = synthetic_item_content(item_id);
        let requested = BTreeSet::from([item_id]);

        let (definitions, descriptors) = content
            .gui_projection(&requested)
            .expect("lazy GUI projection");

        assert!(content.eager_definition_slice().is_empty());
        assert_eq!(definitions.len(), 1);
        assert_eq!(definitions[0].item_id, item_id);
        assert_eq!(definitions[0].name, "Synthetic item");
        assert_eq!(definitions[0].description, "Synthetic description");
        assert_eq!(definitions[0].category, ItemCategory::Etc as i32);
        assert_eq!(definitions[0].stack_max, 200);
        assert_eq!(definitions[0].sale_price, 123);
        assert_eq!(
            (definitions[0].icon_width, definitions[0].icon_height),
            (16.0, 18.0)
        );
        assert_eq!(descriptors.len(), 1);
        assert_eq!(descriptors[0].id, definitions[0].icon_asset_id);
        assert!(content.get_asset(&descriptors[0].id).is_some());

        content
            .materialize_additional_eager(&requested)
            .expect("eager materialization");
        assert_eq!(content.eager_definition_slice(), definitions);

        let error = content
            .gui_projection(&BTreeSet::from([u32::MAX]))
            .expect_err("unknown projected item must fail");
        assert!(error.to_string().contains("absent from the item index"));
    }

    fn synthetic_item_content(item_id: u32) -> ItemContent {
        let item = WzNode::from_str(
            &item_id.to_string(),
            WzObjectType::Property(WzSubProperty::Property),
            None,
        )
        .into_lock();
        let info = WzNode::from_str(
            "info",
            WzObjectType::Property(WzSubProperty::Property),
            Some(&item),
        )
        .into_lock();
        item.write()
            .expect("item lock")
            .children
            .insert("info".into(), info.clone());

        let mut png = WzPng::default();
        png.width = 16;
        png.height = 18;
        let icon = WzNode::from_str("icon", png, Some(&info)).into_lock();
        info.write()
            .expect("info lock")
            .children
            .insert("icon".into(), icon);
        for (name, value) in [("price", 123), ("slotMax", 200)] {
            let child = WzNode::from_str(name, value, Some(&info)).into_lock();
            info.write()
                .expect("info lock")
                .children
                .insert(name.into(), child);
        }

        ItemContent {
            _bases: vec![item.clone()],
            sources: BTreeMap::from([(
                item_id,
                ItemSource {
                    archive: SourceArchive::Item,
                    category: ItemCategory::Etc,
                    image: item,
                    inner_path: None,
                    source_path: format!("synthetic/{item_id}"),
                    definition: OnceLock::new(),
                },
            )]),
            texts: HashMap::from([(
                item_id,
                ItemText {
                    name: "Synthetic item".to_owned(),
                    description: "Synthetic description".to_owned(),
                },
            )]),
            fingerprints: SourceFingerprints {
                item: Some("synthetic-item-fingerprint".to_owned()),
                character: None,
            },
            eager_definitions: Vec::new(),
            consume_effects: BTreeMap::new(),
            monster_book_cards: BTreeMap::new(),
            materialization: Mutex::new(()),
            assets: RwLock::new(HashMap::new()),
        }
    }
}
