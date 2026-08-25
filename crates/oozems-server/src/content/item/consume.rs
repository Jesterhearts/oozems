use std::collections::BTreeMap;
use std::sync::Arc;

use oozems_proto::v1::ItemCategory;
use wz_reader::WzNodeArc;
use wz_reader::WzNodeCast;

use super::ItemContentError;
use super::ItemSourceData;
use super::invalid;
use super::required_child;
use crate::content::wz;

const MAX_DURATION_MS: u64 = 24 * 60 * 60 * 1_000;
const SUPPORTED_ITEM_IDS: [u32; 9] = [
    2_022_070, 2_022_109, 2_022_152, 2_022_239, 2_022_631, 2_022_632, 2_022_633, 2_210_003,
    2_210_034,
];
const MAP_PROTECTION_ITEM_ID: u32 = 2_022_187;

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

pub(super) fn load(
    sources: &BTreeMap<u32, ItemSourceData<'_>>
) -> Result<BTreeMap<u32, ConsumeEffectDefinition>, ItemContentError> {
    let mut effects = BTreeMap::new();
    for item_id in SUPPORTED_ITEM_IDS {
        let source = sources
            .get(&item_id)
            .ok_or_else(|| ItemContentError::Invalid {
                message: format!("required consume effect item {item_id} is absent from Item.wz"),
            })?;
        effects.insert(item_id, read_consume_effect(item_id, source)?);
    }
    let map_protection =
        sources
            .get(&MAP_PROTECTION_ITEM_ID)
            .ok_or_else(|| ItemContentError::Invalid {
                message: format!(
                    "required map protection item {MAP_PROTECTION_ITEM_ID} is absent from Item.wz"
                ),
            })?;
    validate_map_protection_effect(map_protection)?;
    Ok(effects)
}

fn effect_node(
    item_id: u32,
    source: &ItemSourceData<'_>,
) -> Result<WzNodeArc, ItemContentError> {
    if source.category != ItemCategory::Consume {
        return invalid(format!(
            "consume effect source {item_id} has category {:?}, not consume",
            source.category
        ));
    }
    wz::parse(source.image, source.source_path.to_owned())?;
    let item = match source.inner_path {
        Some(path) => required_child(source.image, path, source.source_path)?,
        None => Arc::clone(source.image),
    };
    wz::child(&item, "specEx")?
        .or(wz::child(&item, "spec")?)
        .ok_or_else(|| ItemContentError::Invalid {
            message: format!("consume effect item {item_id} has neither specEx nor spec"),
        })
}

fn read_consume_effect(
    item_id: u32,
    source: &ItemSourceData<'_>,
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
    if duration_ms > MAX_DURATION_MS {
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

fn validate_map_protection_effect(source: &ItemSourceData<'_>) -> Result<(), ItemContentError> {
    let effect = effect_node(MAP_PROTECTION_ITEM_ID, source)?;
    let fields = wz::sorted_children(&effect)?;
    if fields.len() != 2 {
        return invalid(format!(
            "map protection item {MAP_PROTECTION_ITEM_ID} must define exactly thaw and time"
        ));
    }
    let thaw = wz::child(&effect, "thaw")?.ok_or_else(|| ItemContentError::Invalid {
        message: format!("map protection item {MAP_PROTECTION_ITEM_ID} has no thaw property"),
    })?;
    let time = wz::child(&effect, "time")?.ok_or_else(|| ItemContentError::Invalid {
        message: format!("map protection item {MAP_PROTECTION_ITEM_ID} has no time property"),
    })?;
    if strict_effect_integer(MAP_PROTECTION_ITEM_ID, "thaw", &thaw)? != -6
        || strict_effect_integer(MAP_PROTECTION_ITEM_ID, "time", &time)? != 1_800_000
    {
        return invalid(format!(
            "map protection item {MAP_PROTECTION_ITEM_ID} does not match audited thaw=-6, \
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

#[cfg(test)]
mod tests {
    use oozems_proto::v1::ItemCategory;
    use wz_reader::WzNode;
    use wz_reader::WzObjectType;
    use wz_reader::property::WzSubProperty;
    use wz_reader::property::WzValue;

    use super::ConsumeEffectDefinition;
    use super::ItemSourceData;
    use super::read_consume_effect;
    use super::validate_map_protection_effect;

    #[test]
    fn consume_effects_parse_synthetic_integer_properties() {
        let image = consume_effect_image([
            ("time", WzValue::Long(40_000)),
            ("pad", WzValue::Short(20)),
            ("speed", WzValue::Int(-5)),
            ("hp", WzValue::Int(50)),
            ("morph", WzValue::Int(4)),
        ]);
        let source = synthetic_source(&image);
        assert_eq!(
            read_consume_effect(2_000_000, &source).expect("synthetic consume effect"),
            ConsumeEffectDefinition {
                item_id: 2_000_000,
                weapon_attack: 20,
                speed: -5,
                hp: 50,
                morph_id: Some(4),
                duration_ms: 40_000,
                ..ConsumeEffectDefinition::default()
            }
        );

        let image = consume_effect_image([
            ("thaw", WzValue::Int(-6)),
            ("time", WzValue::Int(1_800_000)),
        ]);
        let source = synthetic_source(&image);
        validate_map_protection_effect(&source).expect("synthetic map protection effect");
    }

    fn consume_effect_image<const N: usize>(fields: [(&str, WzValue); N]) -> wz_reader::WzNodeArc {
        let item = WzNode::from_str(
            "2000000",
            WzObjectType::Property(WzSubProperty::Property),
            None,
        )
        .into_lock();
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
            let child = WzNode::from_str(name, WzObjectType::Value(value), Some(&spec)).into_lock();
            spec.write()
                .expect("effect lock")
                .children
                .insert(name.into(), child);
        }
        item
    }

    fn synthetic_source(image: &wz_reader::WzNodeArc) -> ItemSourceData<'_> {
        ItemSourceData {
            category: ItemCategory::Consume,
            image,
            inner_path: None,
            source_path: "synthetic consume effect",
        }
    }
}
