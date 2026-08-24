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

const ITEM_ARCHIVE: &str = "Item.wz";
const CHARACTER_ARCHIVE: &str = "Character.wz";
const STRING_ARCHIVE: &str = "String.wz";
const DEFAULT_STACK_MAX: u32 = 100;
const INSTALL_STACK_MAX: u32 = 1;
const MAX_CONSUME_EFFECT_DURATION_MS: u64 = 24 * 60 * 60 * 1_000;
const SUPPORTED_CONSUME_EFFECT_IDS: [u32; 9] = [
    2_022_070, 2_022_109, 2_022_152, 2_022_239, 2_022_631, 2_022_632, 2_022_633, 2_210_003,
    2_210_034,
];
const MAP_PROTECTION_EFFECT_ID: u32 = 2_022_187;
const MONSTER_BOOK_CATEGORY: u32 = 238;
const MONSTER_BOOK_IMAGE_PATH: &str = "Item.wz/Consume/0238.img";

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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ConsumeEffectDefinition {
    pub item_id: u32,
    pub weapon_attack: i32,
    pub magic_attack: i32,
    pub weapon_defense: i32,
    pub magic_defense: i32,
    pub accuracy: i32,
    pub avoidability: i32,
    pub speed: i32,
    pub jump: i32,
    pub hp: u32,
    pub morph_id: Option<u32>,
    pub duration_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MonsterBookCardDefinition {
    pub item_id: u32,
    pub source_mob_id: u32,
    pub max_count: u32,
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
        let consume_effects = if fingerprints.item.is_some() {
            load_consume_effects(&sources)?
        } else {
            BTreeMap::new()
        };
        let monster_book_cards = if fingerprints.item.is_some() {
            load_monster_book_cards(&sources)?
        } else {
            BTreeMap::new()
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

    #[cfg(test)]
    pub fn consume_effect(
        &self,
        item_id: u32,
    ) -> Option<&ConsumeEffectDefinition> {
        self.consume_effects.get(&item_id)
    }

    pub fn equipment_source_ids(&self) -> BTreeSet<u32> {
        self.sources
            .iter()
            .filter_map(|(item_id, source)| {
                (source.category == ItemCategory::Equipment).then_some(*item_id)
            })
            .collect()
    }

    #[cfg(test)]
    pub fn source_id_iter(&self) -> impl Iterator<Item = u32> + '_ {
        self.sources.keys().copied()
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

    pub fn definition_clones_for_gui(&self) -> Vec<ItemDefinition> {
        self.eager_definitions.clone()
    }

    pub fn descriptor_clones_for_gui(&self) -> Vec<AssetDescriptor> {
        self.eager_definitions
            .iter()
            .map(|definition| descriptor_from_asset_id(&definition.icon_asset_id))
            .collect()
    }

    pub fn get_asset(
        &self,
        asset_id: &str,
    ) -> Option<Arc<WzAsset>> {
        self.assets.read().ok()?.get(asset_id).cloned()
    }
}

fn load_monster_book_cards(
    sources: &BTreeMap<u32, ItemSource>
) -> Result<BTreeMap<u32, MonsterBookCardDefinition>, ItemContentError> {
    let card_sources = sources
        .iter()
        .filter(|(item_id, _)| **item_id / 10_000 == MONSTER_BOOK_CATEGORY)
        .collect::<Vec<_>>();
    let Some((_, first)) = card_sources.first() else {
        return invalid(format!(
            "{MONSTER_BOOK_IMAGE_PATH} has no indexed Monster Book cards"
        ));
    };
    let image = Arc::clone(&first.image);
    wz::parse(&image, MONSTER_BOOK_IMAGE_PATH.to_owned())?;
    let result = card_sources
        .into_iter()
        .map(|(item_id, source)| {
            validate_monster_book_source(*item_id, source)?;
            let inner_path = source
                .inner_path
                .as_deref()
                .expect("validated Monster Book source has an inner path");
            let item = required_child(&source.image, inner_path, &source.source_path)?;
            read_monster_book_card(*item_id, source.category, &source.source_path, &item)
                .map(|definition| (*item_id, definition))
        })
        .collect::<Result<BTreeMap<_, _>, _>>();
    image
        .write()
        .map_err(|_| ItemContentError::Lock {
            context: "Monster Book item source image",
        })?
        .unparse();
    result
}

fn validate_monster_book_source(
    item_id: u32,
    source: &ItemSource,
) -> Result<(), ItemContentError> {
    let expected_inner_path = format!("{item_id:08}");
    let expected_path = format!("{MONSTER_BOOK_IMAGE_PATH}/{expected_inner_path}");
    if source.category != ItemCategory::Consume
        || source.inner_path.as_deref() != Some(expected_inner_path.as_str())
        || source.source_path != expected_path
    {
        return invalid(format!(
            "Monster Book card {item_id} is not an exact Consume/0238.img item source"
        ));
    }
    Ok(())
}

fn read_monster_book_card(
    item_id: u32,
    category: ItemCategory,
    source_path: &str,
    item: &WzNodeArc,
) -> Result<MonsterBookCardDefinition, ItemContentError> {
    if item_id / 10_000 != MONSTER_BOOK_CATEGORY
        || category != ItemCategory::Consume
        || source_path != format!("{MONSTER_BOOK_IMAGE_PATH}/{item_id:08}")
    {
        return invalid(format!(
            "Monster Book card {item_id} has an invalid item category or source"
        ));
    }
    let info = required_child(item, "info", source_path)?;
    let spec = required_child(item, "spec", source_path)?;
    if strict_monster_book_integer(item_id, &info, "monsterBook")? != 1 {
        return invalid(format!(
            "Monster Book card {item_id} property \"monsterBook\" must equal 1"
        ));
    }
    if strict_monster_book_integer(item_id, &spec, "consumeOnPickup")? != 1 {
        return invalid(format!(
            "Monster Book card {item_id} property \"consumeOnPickup\" must equal 1"
        ));
    }
    let source_mob_id = u32::try_from(strict_monster_book_integer(item_id, &info, "mob")?)
        .ok()
        .filter(|mob_id| *mob_id > 0)
        .ok_or_else(|| ItemContentError::Invalid {
            message: format!("Monster Book card {item_id} property \"mob\" must be positive"),
        })?;
    Ok(MonsterBookCardDefinition {
        item_id,
        source_mob_id,
        max_count: crate::monster_book::MAX_CARD_COUNT,
    })
}

fn strict_monster_book_integer(
    item_id: u32,
    info: &WzNodeArc,
    name: &str,
) -> Result<i32, ItemContentError> {
    let value = required_child(info, name, &format!("Monster Book card {item_id} info"))?;
    let read = value.read().map_err(|_| ItemContentError::Lock {
        context: "Monster Book item property",
    })?;
    read.try_as_int()
        .copied()
        .ok_or_else(|| ItemContentError::Invalid {
            message: format!(
                "Monster Book card {item_id} property {name:?} is not an exact WZ int"
            ),
        })
}

fn load_consume_effects(
    sources: &BTreeMap<u32, ItemSource>
) -> Result<BTreeMap<u32, ConsumeEffectDefinition>, ItemContentError> {
    let mut effects = BTreeMap::new();
    for item_id in SUPPORTED_CONSUME_EFFECT_IDS {
        let source = sources
            .get(&item_id)
            .ok_or_else(|| ItemContentError::Invalid {
                message: format!("required consume effect item {item_id} is absent from Item.wz"),
            })?;
        effects.insert(item_id, read_consume_effect(item_id, source)?);
    }
    let map_protection =
        sources
            .get(&MAP_PROTECTION_EFFECT_ID)
            .ok_or_else(|| ItemContentError::Invalid {
                message: format!(
                    "required map protection item {MAP_PROTECTION_EFFECT_ID} is absent from \
                     Item.wz"
                ),
            })?;
    validate_map_protection_effect(map_protection)?;
    Ok(effects)
}

fn effect_node(
    item_id: u32,
    source: &ItemSource,
) -> Result<WzNodeArc, ItemContentError> {
    if source.category != ItemCategory::Consume {
        return invalid(format!(
            "consume effect source {item_id} has category {:?}, not consume",
            source.category
        ));
    }
    wz::parse(&source.image, source.source_path.clone())?;
    let item = match source.inner_path.as_deref() {
        Some(path) => required_child(&source.image, path, &source.source_path)?,
        None => Arc::clone(&source.image),
    };
    wz::child(&item, "specEx")?
        .or(wz::child(&item, "spec")?)
        .ok_or_else(|| ItemContentError::Invalid {
            message: format!("consume effect item {item_id} has neither specEx nor spec"),
        })
}

fn read_consume_effect(
    item_id: u32,
    source: &ItemSource,
) -> Result<ConsumeEffectDefinition, ItemContentError> {
    let effect = effect_node(item_id, source)?;
    let mut values = BTreeMap::new();
    for field in wz::sorted_children(&effect)? {
        let name = wz::node_name(&field)?;
        if ![
            "pad", "mad", "pdd", "mdd", "acc", "eva", "speed", "jump", "hp", "morph", "time",
        ]
        .contains(&name.as_str())
        {
            return invalid(format!(
                "consume effect item {item_id} has unsupported property {name:?}"
            ));
        }
        if values
            .insert(name.clone(), strict_effect_integer(item_id, &name, &field)?)
            .is_some()
        {
            return invalid(format!(
                "consume effect item {item_id} property {name:?} appears more than once"
            ));
        }
    }
    let duration = take_required_positive(&mut values, item_id, "time")?;
    let duration_ms = u64::try_from(duration).map_err(|_| ItemContentError::Invalid {
        message: format!("consume effect item {item_id} duration is outside the u64 range"),
    })?;
    if duration_ms > MAX_CONSUME_EFFECT_DURATION_MS {
        return invalid(format!(
            "consume effect item {item_id} duration {duration_ms} exceeds the supported maximum"
        ));
    }
    let hp = take_nonnegative_u32(&mut values, item_id, "hp")?;
    let morph_id = take_optional_positive_u32(&mut values, item_id, "morph")?;
    let definition = ConsumeEffectDefinition {
        item_id,
        weapon_attack: take_modifier(&mut values, item_id, "pad")?,
        magic_attack: take_modifier(&mut values, item_id, "mad")?,
        weapon_defense: take_modifier(&mut values, item_id, "pdd")?,
        magic_defense: take_modifier(&mut values, item_id, "mdd")?,
        accuracy: take_modifier(&mut values, item_id, "acc")?,
        avoidability: take_modifier(&mut values, item_id, "eva")?,
        speed: take_modifier(&mut values, item_id, "speed")?,
        jump: take_modifier(&mut values, item_id, "jump")?,
        hp,
        morph_id,
        duration_ms,
    };
    if !values.is_empty() {
        return invalid(format!(
            "consume effect item {item_id} contains unconsumed properties: {:?}",
            values.keys().collect::<Vec<_>>()
        ));
    }
    Ok(definition)
}

fn validate_map_protection_effect(source: &ItemSource) -> Result<(), ItemContentError> {
    let effect = effect_node(MAP_PROTECTION_EFFECT_ID, source)?;
    let fields = wz::sorted_children(&effect)?;
    if fields.len() != 2 {
        return invalid(format!(
            "map protection item {MAP_PROTECTION_EFFECT_ID} must define exactly thaw and time"
        ));
    }
    let thaw = wz::child(&effect, "thaw")?.ok_or_else(|| ItemContentError::Invalid {
        message: format!("map protection item {MAP_PROTECTION_EFFECT_ID} has no thaw property"),
    })?;
    let time = wz::child(&effect, "time")?.ok_or_else(|| ItemContentError::Invalid {
        message: format!("map protection item {MAP_PROTECTION_EFFECT_ID} has no time property"),
    })?;
    if strict_effect_integer(MAP_PROTECTION_EFFECT_ID, "thaw", &thaw)? != -6
        || strict_effect_integer(MAP_PROTECTION_EFFECT_ID, "time", &time)? != 1_800_000
    {
        return invalid(format!(
            "map protection item {MAP_PROTECTION_EFFECT_ID} does not match audited thaw=-6, \
             time=1800000"
        ));
    }
    Ok(())
}

fn strict_effect_integer(
    item_id: u32,
    name: &str,
    node: &WzNodeArc,
) -> Result<i64, ItemContentError> {
    let read = node.read().map_err(|_| ItemContentError::Lock {
        context: "consume effect property",
    })?;
    if let Some(value) = read.try_as_int() {
        Ok(i64::from(*value))
    } else if let Some(value) = read.try_as_short() {
        Ok(i64::from(*value))
    } else if let Some(value) = read.try_as_long() {
        Ok(*value)
    } else {
        invalid(format!(
            "consume effect item {item_id} property {name:?} is not an integer WZ value"
        ))
    }
}

fn take_required_positive(
    values: &mut BTreeMap<String, i64>,
    item_id: u32,
    name: &str,
) -> Result<i64, ItemContentError> {
    values
        .remove(name)
        .filter(|value| *value > 0)
        .ok_or_else(|| ItemContentError::Invalid {
            message: format!("consume effect item {item_id} property {name:?} must be positive"),
        })
}

fn take_modifier(
    values: &mut BTreeMap<String, i64>,
    item_id: u32,
    name: &str,
) -> Result<i32, ItemContentError> {
    let Some(value) = values.remove(name) else {
        return Ok(0);
    };
    let value = i32::try_from(value).map_err(|_| ItemContentError::Invalid {
        message: format!("consume effect item {item_id} property {name:?} is outside i32"),
    })?;
    if !(-1_000..=1_000).contains(&value) {
        return invalid(format!(
            "consume effect item {item_id} property {name:?} is outside -1000..=1000"
        ));
    }
    Ok(value)
}

fn take_nonnegative_u32(
    values: &mut BTreeMap<String, i64>,
    item_id: u32,
    name: &str,
) -> Result<u32, ItemContentError> {
    values
        .remove(name)
        .map(|value| {
            u32::try_from(value).map_err(|_| ItemContentError::Invalid {
                message: format!(
                    "consume effect item {item_id} property {name:?} must be a nonnegative u32"
                ),
            })
        })
        .transpose()
        .map(Option::unwrap_or_default)
}

fn take_optional_positive_u32(
    values: &mut BTreeMap<String, i64>,
    item_id: u32,
    name: &str,
) -> Result<Option<u32>, ItemContentError> {
    values
        .remove(name)
        .map(|value| {
            u32::try_from(value)
                .ok()
                .filter(|value| *value > 0)
                .ok_or_else(|| ItemContentError::Invalid {
                    message: format!(
                        "consume effect item {item_id} property {name:?} must be a positive u32"
                    ),
                })
        })
        .transpose()
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
    use std::collections::BTreeSet;
    use std::collections::HashSet;
    use std::path::Path;

    use oozems_proto::v1::EquipmentSlot;
    use oozems_proto::v1::ItemCategory;
    use wz_reader::WzNode;
    use wz_reader::WzObjectType;
    use wz_reader::property::WzString;
    use wz_reader::property::WzSubProperty;
    use wz_reader::property::WzValue;

    use super::ItemContent;
    use super::normalize_sale_price;
    use super::normalize_stack_max;
    use super::read_monster_book_card;
    use crate::content::character::CharacterContent;

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
    fn local_item_archives_load_quest_reward_and_current_equipment_when_present() {
        let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data");
        if !directory.join("Item.wz").exists()
            || !directory.join("Character.wz").exists()
            || !directory.join("String.wz").exists()
        {
            return;
        }

        let characters = CharacterContent::open_optional(&directory)
            .expect("sample Character.wz should be valid")
            .expect("sample Character.wz should be present");
        let mut content = ItemContent::load(&directory, Some(&characters))
            .expect("sample item archives should be valid")
            .expect("sample item archives should provide definitions");
        assert_eq!(content.sources.len(), 10_592);
        content
            .materialize_eager(&BTreeSet::from([
                1_332_005, 1_332_007, 4_000_000, 4_000_001,
            ]))
            .expect("selected item definitions should load");
        let definitions = content.eager_definition_slice();
        eprintln!(
            "indexed item sources: {}; focused materialized items: {}",
            content.sources.len(),
            definitions.len()
        );
        let quest_item = definition(definitions, 4_000_000);
        assert_eq!(quest_item.name, "Blue Snail Shell");
        assert_eq!(quest_item.stack_max, 200);
        assert_eq!(
            ItemCategory::try_from(quest_item.category),
            Ok(ItemCategory::Etc)
        );
        let quest_item = definition(definitions, 4_000_001);
        assert_eq!(quest_item.name, "Orange Mushroom Cap");

        let reward_weapon = definition(definitions, 1_332_005);
        assert_eq!(reward_weapon.name, "Razor");

        let reward_weapon = definition(definitions, 1_332_007);
        assert_eq!(reward_weapon.name, "Fruit Knife");
        assert_eq!(reward_weapon.stack_max, 1);
        assert_eq!(
            ItemCategory::try_from(reward_weapon.category),
            Ok(ItemCategory::Equipment)
        );
        assert!(!reward_weapon.appearance_supported);

        let current = [
            (
                crate::items::STARTER_TOP_ID,
                "White Undershirt",
                EquipmentSlot::Top,
            ),
            (
                crate::items::STARTER_BOTTOM_ID,
                "Blue Jean Shorts",
                EquipmentSlot::Bottom,
            ),
            (
                crate::items::STARTER_SHOES_ID,
                "Brown Jangoon Shoes",
                EquipmentSlot::Shoes,
            ),
            (
                crate::items::SPARE_TOP_ID,
                "Brown Hard Leather Top",
                EquipmentSlot::Top,
            ),
            (
                crate::items::SPARE_BOTTOM_ID,
                "Black Suit Pants",
                EquipmentSlot::Bottom,
            ),
            (
                crate::items::SPARE_SHOES_ID,
                "Red Rubber Boots",
                EquipmentSlot::Shoes,
            ),
        ];
        for (item_id, name, slot) in current {
            let item = definition(definitions, item_id);
            assert_eq!(item.name, name);
            assert_eq!(item.slot, slot as i32);
            assert_eq!(item.stack_max, 1);
            assert!(item.appearance_supported);
        }

        let appearance_ids = definitions
            .iter()
            .filter(|definition| definition.appearance_supported)
            .map(|definition| definition.item_id)
            .collect::<HashSet<_>>();
        assert_eq!(
            appearance_ids,
            current.into_iter().map(|(item_id, _, _)| item_id).collect()
        );
        assert!(definitions.iter().all(|definition| {
            !definition.icon_asset_id.is_empty()
                && content.get_asset(&definition.icon_asset_id).is_some()
        }));

        let source_only_id = content
            .sources
            .iter()
            .find_map(|(item_id, source)| source.definition.get().is_none().then_some(*item_id))
            .expect("the source index should contain a non-eager item");
        assert!(
            !definitions
                .iter()
                .any(|definition| definition.item_id == source_only_id)
        );
        let source_only = content
            .definition(source_only_id)
            .expect("source-only item lookup should succeed")
            .expect("source-only item should be indexed");
        assert_eq!(source_only.item_id, source_only_id);
        assert!(content.get_asset(&source_only.icon_asset_id).is_some());
        assert!(
            content
                .definition(u32::MAX)
                .expect("unsupported item lookup should not fail")
                .is_none()
        );
    }

    #[test]
    fn local_supported_consume_effects_match_the_audited_item_archive() {
        let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data");
        if !directory.join("Item.wz").exists() {
            return;
        }
        let content = ItemContent::load(&directory, None)
            .expect("sample item archive should be valid")
            .expect("sample item archive should be present");
        use super::ConsumeEffectDefinition as Effect;
        let modifier = |item_id, pad, mad, pdd, mdd, acc, eva, speed, jump, duration_ms| Effect {
            item_id,
            weapon_attack: pad,
            magic_attack: mad,
            weapon_defense: pdd,
            magic_defense: mdd,
            accuracy: acc,
            avoidability: eva,
            speed,
            jump,
            duration_ms,
            ..Effect::default()
        };
        assert_eq!(
            content.consume_effect_definitions(),
            vec![
                modifier(2_022_070, 20, 20, 100, 100, 50, 50, 10, 10, 3_600_000),
                modifier(2_022_109, 25, 35, 150, 150, 0, 0, 0, 0, 3_600_000),
                modifier(2_022_152, 10, 10, 30, 30, 20, 20, 3, 3, 1_200_000),
                modifier(2_022_239, 10, 10, 30, 30, 20, 20, 7, 5, 1_800_000),
                modifier(2_022_631, 0, 0, 0, 0, 0, 0, -5, 0, 40_000),
                modifier(2_022_632, 0, 0, 0, 0, 0, 0, -5, 0, 40_000),
                modifier(2_022_633, 0, 0, 0, 0, 0, 0, -5, 0, 40_000),
                Effect {
                    item_id: 2_210_003,
                    hp: 50,
                    morph_id: Some(4),
                    duration_ms: 3_600_000,
                    ..Effect::default()
                },
                Effect {
                    item_id: 2_210_034,
                    hp: 50,
                    morph_id: Some(40),
                    duration_ms: 1_800_000,
                    ..Effect::default()
                },
            ]
        );
        assert!(content.consume_effect(2_022_187).is_none());
    }

    #[test]
    fn local_monster_book_cards_match_the_audited_item_archive() {
        let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data");
        if !directory.join("Item.wz").exists() {
            return;
        }
        let content = ItemContent::load(&directory, None)
            .expect("sample item archive should be valid")
            .expect("sample item archive should be present");

        assert_eq!(content.monster_book_cards.len(), 343);
        assert_eq!(
            content.monster_book_card(2_380_000),
            Some(super::MonsterBookCardDefinition {
                item_id: 2_380_000,
                source_mob_id: 100_100,
                max_count: 5,
            })
        );
        assert!(content.monster_book_card(2_390_000).is_none());
    }

    #[test]
    fn monster_book_fields_require_exact_values_types_and_source() {
        let item = monster_book_item([
            ("monsterBook", WzValue::Int(1)),
            ("mob", WzValue::Int(100_100)),
            ("consumeOnPickup", WzValue::Int(1)),
        ]);
        let definition = read_monster_book_card(
            2_380_000,
            ItemCategory::Consume,
            "Item.wz/Consume/0238.img/02380000",
            &item,
        )
        .expect("exact card fields");
        assert_eq!(definition.source_mob_id, 100_100);

        let wrong_type = monster_book_item([
            (
                "monsterBook",
                WzValue::String(WzString::from_str("1", [0; 4])),
            ),
            ("mob", WzValue::Int(100_100)),
            ("consumeOnPickup", WzValue::Int(1)),
        ]);
        assert!(
            read_monster_book_card(
                2_380_000,
                ItemCategory::Consume,
                "Item.wz/Consume/0238.img/02380000",
                &wrong_type,
            )
            .is_err()
        );

        let invalid_mob = monster_book_item([
            ("monsterBook", WzValue::Int(1)),
            ("mob", WzValue::Int(0)),
            ("consumeOnPickup", WzValue::Int(1)),
        ]);
        assert!(
            read_monster_book_card(
                2_380_000,
                ItemCategory::Consume,
                "Item.wz/Consume/0238.img/02380000",
                &invalid_mob,
            )
            .is_err()
        );
        assert!(
            read_monster_book_card(
                2_380_000,
                ItemCategory::Etc,
                "Item.wz/Etc/0238.img/2380000",
                &item,
            )
            .is_err()
        );
    }

    fn monster_book_item<const N: usize>(fields: [(&str, WzValue); N]) -> wz_reader::WzNodeArc {
        let item = WzNode::from_str(
            "2380000",
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
        let spec = WzNode::from_str(
            "spec",
            WzObjectType::Property(WzSubProperty::Property),
            Some(&item),
        )
        .into_lock();
        item.write()
            .expect("item lock")
            .children
            .insert("spec".into(), spec.clone());
        for (name, value) in fields {
            let parent = if name == "consumeOnPickup" {
                &spec
            } else {
                &info
            };
            let child =
                WzNode::from_str(name, WzObjectType::Value(value), Some(parent)).into_lock();
            parent
                .write()
                .expect("property lock")
                .children
                .insert(name.into(), child);
        }
        item
    }

    fn definition(
        definitions: &[oozems_proto::v1::ItemDefinition],
        item_id: u32,
    ) -> &oozems_proto::v1::ItemDefinition {
        definitions
            .iter()
            .find(|definition| definition.item_id == item_id)
            .unwrap_or_else(|| panic!("item {item_id} definition"))
    }
}
