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

const ITEM_CATEGORY: u32 = 238;
const IMAGE_PATH: &str = "Item.wz/Consume/0238.img";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MonsterBookCardDefinition {
    pub item_id: u32,
    pub source_mob_id: u32,
    pub max_count: u32,
}

pub(super) fn load(
    sources: &BTreeMap<u32, ItemSourceData<'_>>
) -> Result<BTreeMap<u32, MonsterBookCardDefinition>, ItemContentError> {
    let card_sources = sources
        .iter()
        .filter(|(item_id, _)| **item_id / 10_000 == ITEM_CATEGORY)
        .collect::<Vec<_>>();
    let Some((_, first)) = card_sources.first() else {
        return invalid(format!("{IMAGE_PATH} has no indexed Monster Book cards"));
    };
    let image = Arc::clone(first.image);
    wz::parse(&image, IMAGE_PATH.to_owned())?;
    let result = card_sources
        .into_iter()
        .map(|(item_id, source)| {
            validate_source(*item_id, source)?;
            let inner_path = source
                .inner_path
                .expect("validated Monster Book source has an inner path");
            let item = required_child(source.image, inner_path, source.source_path)?;
            read_card(*item_id, source.category, source.source_path, &item)
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

fn validate_source(
    item_id: u32,
    source: &ItemSourceData<'_>,
) -> Result<(), ItemContentError> {
    let expected_inner_path = format!("{item_id:08}");
    let expected_path = format!("{IMAGE_PATH}/{expected_inner_path}");
    if source.category != ItemCategory::Consume
        || source.inner_path != Some(expected_inner_path.as_str())
        || source.source_path != expected_path
    {
        return invalid(format!(
            "Monster Book card {item_id} is not an exact Consume/0238.img item source"
        ));
    }
    Ok(())
}

fn read_card(
    item_id: u32,
    category: ItemCategory,
    source_path: &str,
    item: &WzNodeArc,
) -> Result<MonsterBookCardDefinition, ItemContentError> {
    if item_id / 10_000 != ITEM_CATEGORY
        || category != ItemCategory::Consume
        || source_path != format!("{IMAGE_PATH}/{item_id:08}")
    {
        return invalid(format!(
            "Monster Book card {item_id} has an invalid item category or source"
        ));
    }
    let info = required_child(item, "info", source_path)?;
    let spec = required_child(item, "spec", source_path)?;
    if strict_integer(item_id, &info, "monsterBook")? != 1 {
        return invalid(format!(
            "Monster Book card {item_id} property \"monsterBook\" must equal 1"
        ));
    }
    if strict_integer(item_id, &spec, "consumeOnPickup")? != 1 {
        return invalid(format!(
            "Monster Book card {item_id} property \"consumeOnPickup\" must equal 1"
        ));
    }
    let source_mob_id = u32::try_from(strict_integer(item_id, &info, "mob")?)
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

fn strict_integer(
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

#[cfg(test)]
mod tests {
    use oozems_proto::v1::ItemCategory;
    use wz_reader::WzNode;
    use wz_reader::WzObjectType;
    use wz_reader::property::WzString;
    use wz_reader::property::WzSubProperty;
    use wz_reader::property::WzValue;

    use super::read_card;

    #[test]
    fn fields_require_exact_values_types_and_source() {
        let item = monster_book_item([
            ("monsterBook", WzValue::Int(1)),
            ("mob", WzValue::Int(100_100)),
            ("consumeOnPickup", WzValue::Int(1)),
        ]);
        let definition = read_card(
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
            read_card(
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
            read_card(
                2_380_000,
                ItemCategory::Consume,
                "Item.wz/Consume/0238.img/02380000",
                &invalid_mob,
            )
            .is_err()
        );
        assert!(
            read_card(
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
}
