use std::collections::BTreeMap;
use std::sync::Arc;

use oozems_proto::v1::ItemCategory;
use wz_reader::WzNodeArc;
use wz_reader::WzNodeCast;
use wz_reader::property::WzValue;

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
    pub mp: u32,
    pub hp_percent: u32,
    pub mp_percent: u32,
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
    effect_node_from_item(item_id, &item)
}

fn effect_node_from_item(
    item_id: u32,
    item: &WzNodeArc,
) -> Result<WzNodeArc, ItemContentError> {
    wz::child(item, "specEx")?
        .or(wz::child(item, "spec")?)
        .ok_or_else(|| ItemContentError::Invalid {
            message: format!("consume effect item {item_id} has neither specEx nor spec"),
        })
}

pub(super) fn read_lazy_consume_effect(
    item_id: u32,
    item: &WzNodeArc,
) -> Result<Option<ConsumeEffectDefinition>, ItemContentError> {
    let Some(effect) = wz::child(item, "specEx")?.or(wz::child(item, "spec")?) else {
        return Ok(None);
    };
    let fields = wz::sorted_children(&effect)?;
    if fields.is_empty()
        || fields.iter().any(|field| {
            wz::node_name(field).is_ok_and(|name| {
                !matches!(
                    name.as_str(),
                    "pad"
                        | "mad"
                        | "pdd"
                        | "mdd"
                        | "acc"
                        | "eva"
                        | "speed"
                        | "jump"
                        | "hp"
                        | "mp"
                        | "hpR"
                        | "mpR"
                        | "time"
                )
            })
        })
    {
        return Ok(None);
    }

    let mut values = BTreeMap::new();
    for field in fields {
        let name = wz::node_name(&field)?;
        let Some(value) = lazy_effect_integer(&field)? else {
            return Ok(None);
        };
        values.insert(name, value);
    }
    if values.values().any(|value| *value < 0)
        || ["hpR", "mpR"]
            .into_iter()
            .filter_map(|name| values.get(name))
            .any(|value| *value > 100)
        || ["hp", "mp"]
            .into_iter()
            .filter_map(|name| values.get(name))
            .any(|value| u32::try_from(*value).is_err())
        || ["pad", "mad", "pdd", "mdd", "acc", "eva", "speed", "jump"]
            .into_iter()
            .filter_map(|name| values.get(name))
            .any(|value| !(-1_000..=1_000).contains(value))
    {
        return Ok(None);
    }
    let duration_ms = match values.remove("time") {
        Some(value) => match u64::try_from(value) {
            Ok(value) if value > 0 && value <= MAX_DURATION_MS => value,
            _ => return Ok(None),
        },
        None => 0,
    };
    let mut definition = ConsumeEffectDefinition {
        item_id,
        weapon_attack: take_modifier(&mut values, item_id, "pad")?,
        magic_attack: take_modifier(&mut values, item_id, "mad")?,
        weapon_defense: take_modifier(&mut values, item_id, "pdd")?,
        magic_defense: take_modifier(&mut values, item_id, "mdd")?,
        accuracy: take_modifier(&mut values, item_id, "acc")?,
        avoidability: take_modifier(&mut values, item_id, "eva")?,
        speed: take_modifier(&mut values, item_id, "speed")?,
        jump: take_modifier(&mut values, item_id, "jump")?,
        hp: take_nonnegative_u32(&mut values, item_id, "hp")?,
        mp: take_nonnegative_u32(&mut values, item_id, "mp")?,
        hp_percent: take_percentage(&mut values, item_id, "hpR")?,
        mp_percent: take_percentage(&mut values, item_id, "mpR")?,
        duration_ms,
        ..ConsumeEffectDefinition::default()
    };
    let restores_resource = definition.hp > 0
        || definition.mp > 0
        || definition.hp_percent > 0
        || definition.mp_percent > 0;
    let has_timed_modifier = definition.weapon_attack != 0
        || definition.magic_attack != 0
        || definition.weapon_defense != 0
        || definition.magic_defense != 0
        || definition.accuracy != 0
        || definition.avoidability != 0
        || definition.speed != 0
        || definition.jump != 0;
    if has_timed_modifier && definition.duration_ms == 0 {
        return Ok(None);
    }
    if !restores_resource && !has_timed_modifier {
        return Ok(None);
    }
    if !has_timed_modifier {
        definition.duration_ms = 0;
    }
    Ok(Some(definition))
}

fn lazy_effect_integer(node: &WzNodeArc) -> Result<Option<i64>, ItemContentError> {
    let read = node.read().map_err(|_| ItemContentError::Lock {
        context: "lazy consume effect property",
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
    Ok(text.and_then(|value| value.trim().parse().ok()))
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
        ..ConsumeEffectDefinition::default()
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

fn take_percentage(
    values: &mut BTreeMap<String, i64>,
    item_id: u32,
    name: &str,
) -> Result<u32, ItemContentError> {
    let value = take_nonnegative_u32(values, item_id, name)?;
    if value > 100 {
        return invalid(format!(
            "consume effect item {item_id} property {name:?} exceeds 100 percent"
        ));
    }
    Ok(value)
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
    use super::read_lazy_consume_effect;
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

    #[test]
    fn restoration_effects_accept_fixed_and_percentage_recovery() {
        let fixed = consume_effect_image([("hp", WzValue::Int(50)), ("mp", WzValue::Short(25))]);
        assert_eq!(
            read_lazy_consume_effect(2_000_000, &fixed).expect("fixed restoration"),
            Some(ConsumeEffectDefinition {
                item_id: 2_000_000,
                hp: 50,
                mp: 25,
                ..ConsumeEffectDefinition::default()
            })
        );

        let percentage =
            consume_effect_image([("hpR", WzValue::Int(50)), ("mpR", WzValue::Int(25))]);
        assert_eq!(
            read_lazy_consume_effect(2_000_004, &percentage).expect("percentage restoration"),
            Some(ConsumeEffectDefinition {
                item_id: 2_000_004,
                hp_percent: 50,
                mp_percent: 25,
                ..ConsumeEffectDefinition::default()
            })
        );

        let string_percentage = consume_effect_image([
            ("hpR", WzValue::ParsedString("50".to_owned())),
            ("mpR", WzValue::ParsedString("50".to_owned())),
        ]);
        assert_eq!(
            read_lazy_consume_effect(2_010_006, &string_percentage)
                .expect("string percentage restoration"),
            Some(ConsumeEffectDefinition {
                item_id: 2_010_006,
                hp_percent: 50,
                mp_percent: 50,
                ..ConsumeEffectDefinition::default()
            })
        );
    }

    #[test]
    fn timed_modifier_effects_are_supported_lazily() {
        let effect =
            consume_effect_image([("jump", WzValue::Int(3)), ("time", WzValue::Int(180_000))]);

        assert_eq!(
            read_lazy_consume_effect(2_022_253, &effect).expect("timed modifier"),
            Some(ConsumeEffectDefinition {
                item_id: 2_022_253,
                jump: 3,
                duration_ms: 180_000,
                ..ConsumeEffectDefinition::default()
            })
        );
    }

    #[test]
    fn restoration_effects_reject_partially_supported_items() {
        let effect = consume_effect_image([("hp", WzValue::Int(50)), ("poison", WzValue::Int(1))]);

        assert_eq!(
            read_lazy_consume_effect(2_000_006, &effect).expect("unsupported restoration"),
            None
        );

        let harmful =
            consume_effect_image([("hpR", WzValue::Int(-40)), ("mpR", WzValue::Int(-40))]);
        assert_eq!(
            read_lazy_consume_effect(2_022_228, &harmful).expect("harmful potion"),
            None
        );
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
