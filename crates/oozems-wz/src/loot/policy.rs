use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use serde::Deserialize;
use serde::Serialize;

const CHANCE_SCALE: u32 = 1_000_000;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LootPolicy {
    pub schema_version: u32,
    pub policy_name: String,
    pub item_class_ppm: ItemClassRates,
    pub boss_multipliers: BossMultipliers,
    pub card_chance: LevelExperienceFormula,
    pub meso_chance: LevelExperienceFormula,
    pub meso_expected_value: LevelExperienceFormula,
    pub meso_range: MesoRange,
    pub quantities: QuantityPolicies,
    #[serde(default)]
    pub mob_drops: Vec<PolicyMobDrop>,
    #[serde(default)]
    pub global_drops: Vec<GlobalDrop>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ItemClassRates {
    pub equipment: u32,
    pub recovery: u32,
    pub mobility: u32,
    pub scroll: u32,
    pub cure: u32,
    pub ammo: u32,
    pub rechargeable: u32,
    pub common_mob_material: u32,
    pub ore_crafting: u32,
    pub miscellaneous: u32,
    pub quest: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BossMultipliers {
    pub item: Ratio,
    pub card: Ratio,
    pub meso_chance: Ratio,
    pub meso_value: Ratio,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Ratio {
    pub numerator: u32,
    pub denominator: u32,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LevelExperienceFormula {
    pub base: u64,
    pub per_level: u64,
    pub per_experience_sqrt: u64,
    pub maximum: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MesoRange {
    pub spread_per_thousand: u32,
    pub minimum: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct QuantityPolicies {
    pub consumable: QuantityFormula,
    pub material: QuantityFormula,
    pub ammo: QuantityFormula,
    pub single: QuantityFormula,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct QuantityFormula {
    pub minimum_base: u32,
    pub minimum_levels_per_extra: u32,
    pub maximum_base: u32,
    pub maximum_levels_per_extra: u32,
    pub maximum: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub(crate) enum GlobalDrop {
    Item(GlobalItemDrop),
    Mesos(GlobalMesoDrop),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GlobalItemDrop {
    pub item_id: u32,
    pub chance_per_million: u32,
    pub minimum_quantity: Option<u32>,
    pub maximum_quantity: Option<u32>,
    pub quest_id: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GlobalMesoDrop {
    pub chance_per_million: u32,
    pub minimum_mesos: u32,
    pub maximum_mesos: u32,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PolicyMobDrop {
    pub mob_id: u32,
    pub item_id: u32,
    pub quest_id: Option<u32>,
    pub source_map_id: Option<u32>,
    pub required_skill_id: Option<u32>,
    pub chance_per_million: u32,
    pub minimum_quantity: u32,
    pub maximum_quantity: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RateClass {
    Equipment,
    Recovery,
    Mobility,
    Scroll,
    Cure,
    Ammo,
    Rechargeable,
    CommonMobMaterial,
    OreCrafting,
    Miscellaneous,
    Quest,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum QuantityClass {
    Consumable,
    Material,
    Ammo,
    Single,
}

pub(crate) fn load(path: &Path) -> Result<LootPolicy> {
    let source = fs::read_to_string(path)
        .with_context(|| format!("failed to read loot policy {}", path.display()))?;
    let policy: LootPolicy = toml::from_str(&source)
        .with_context(|| format!("failed to parse loot policy {}", path.display()))?;
    validate(&policy).with_context(|| format!("invalid loot policy {}", path.display()))?;
    Ok(policy)
}

pub(crate) fn classify(item_id: u32) -> (RateClass, QuantityClass) {
    match item_id / 10_000 {
        100..=199 => (RateClass::Equipment, QuantityClass::Single),
        200..=202 => (RateClass::Recovery, QuantityClass::Consumable),
        203 => (RateClass::Mobility, QuantityClass::Consumable),
        204 => (RateClass::Scroll, QuantityClass::Single),
        205 => (RateClass::Cure, QuantityClass::Consumable),
        206 => (RateClass::Ammo, QuantityClass::Ammo),
        207 | 233 => (RateClass::Rechargeable, QuantityClass::Single),
        400 => (RateClass::CommonMobMaterial, QuantityClass::Material),
        401 | 402 | 425 | 426 => (RateClass::OreCrafting, QuantityClass::Material),
        403 => (RateClass::Quest, QuantityClass::Material),
        _ if item_id / 1_000_000 == 2 => (RateClass::Miscellaneous, QuantityClass::Consumable),
        _ => (RateClass::Miscellaneous, QuantityClass::Material),
    }
}

impl LootPolicy {
    pub(crate) fn rate(
        &self,
        class: RateClass,
    ) -> u32 {
        match class {
            RateClass::Equipment => self.item_class_ppm.equipment,
            RateClass::Recovery => self.item_class_ppm.recovery,
            RateClass::Mobility => self.item_class_ppm.mobility,
            RateClass::Scroll => self.item_class_ppm.scroll,
            RateClass::Cure => self.item_class_ppm.cure,
            RateClass::Ammo => self.item_class_ppm.ammo,
            RateClass::Rechargeable => self.item_class_ppm.rechargeable,
            RateClass::CommonMobMaterial => self.item_class_ppm.common_mob_material,
            RateClass::OreCrafting => self.item_class_ppm.ore_crafting,
            RateClass::Miscellaneous => self.item_class_ppm.miscellaneous,
            RateClass::Quest => self.item_class_ppm.quest,
        }
    }

    pub(crate) fn quantity(
        &self,
        class: QuantityClass,
    ) -> QuantityFormula {
        match class {
            QuantityClass::Consumable => self.quantities.consumable,
            QuantityClass::Material => self.quantities.material,
            QuantityClass::Ammo => self.quantities.ammo,
            QuantityClass::Single => self.quantities.single,
        }
    }
}

pub(crate) fn apply_ratio(
    value: u64,
    ratio: Ratio,
    maximum: u64,
) -> u64 {
    value
        .saturating_mul(u64::from(ratio.numerator))
        .checked_div(u64::from(ratio.denominator))
        .unwrap_or(maximum)
        .min(maximum)
}

fn validate(policy: &LootPolicy) -> Result<()> {
    if policy.schema_version != 1 {
        bail!("schema_version must be 1, not {}", policy.schema_version);
    }
    if policy.policy_name.trim().is_empty() {
        bail!("policy_name must not be empty");
    }
    for (name, value) in [
        ("equipment", policy.item_class_ppm.equipment),
        ("recovery", policy.item_class_ppm.recovery),
        ("mobility", policy.item_class_ppm.mobility),
        ("scroll", policy.item_class_ppm.scroll),
        ("cure", policy.item_class_ppm.cure),
        ("ammo", policy.item_class_ppm.ammo),
        ("rechargeable", policy.item_class_ppm.rechargeable),
        (
            "common_mob_material",
            policy.item_class_ppm.common_mob_material,
        ),
        ("ore_crafting", policy.item_class_ppm.ore_crafting),
        ("miscellaneous", policy.item_class_ppm.miscellaneous),
        ("quest", policy.item_class_ppm.quest),
    ] {
        validate_chance(&format!("item_class_ppm.{name}"), value)?;
    }
    for (name, ratio) in [
        ("item", policy.boss_multipliers.item),
        ("card", policy.boss_multipliers.card),
        ("meso_chance", policy.boss_multipliers.meso_chance),
        ("meso_value", policy.boss_multipliers.meso_value),
    ] {
        if ratio.numerator == 0 || ratio.denominator == 0 {
            bail!("boss multiplier {name} must have a positive numerator and denominator");
        }
        if ratio.numerator < ratio.denominator {
            bail!("boss multiplier {name} must not reduce its non-boss value");
        }
    }
    validate_formula("card_chance", policy.card_chance, CHANCE_SCALE.into())?;
    validate_formula("meso_chance", policy.meso_chance, CHANCE_SCALE.into())?;
    validate_formula(
        "meso_expected_value",
        policy.meso_expected_value,
        u64::from(u32::MAX),
    )?;
    if policy.meso_range.spread_per_thousand > 1_000 {
        bail!("meso_range.spread_per_thousand must not exceed 1000");
    }
    if policy.meso_range.minimum == 0 {
        bail!("meso_range.minimum must be positive");
    }
    for (name, formula) in [
        ("consumable", policy.quantities.consumable),
        ("material", policy.quantities.material),
        ("ammo", policy.quantities.ammo),
        ("single", policy.quantities.single),
    ] {
        validate_quantity(name, formula)?;
    }
    validate_mob_drops(&policy.mob_drops)?;
    validate_globals(&policy.global_drops)
}

fn validate_chance(
    name: &str,
    chance: u32,
) -> Result<()> {
    if !(1..=CHANCE_SCALE).contains(&chance) {
        bail!("{name} must be between 1 and {CHANCE_SCALE}");
    }
    Ok(())
}

fn validate_formula(
    name: &str,
    formula: LevelExperienceFormula,
    limit: u64,
) -> Result<()> {
    if formula.maximum == 0 || formula.maximum > limit {
        bail!("{name}.maximum must be between 1 and {limit}");
    }
    if formula.base > formula.maximum {
        bail!("{name}.base must not exceed its maximum");
    }
    Ok(())
}

fn validate_quantity(
    name: &str,
    formula: QuantityFormula,
) -> Result<()> {
    if formula.minimum_base == 0
        || formula.maximum_base == 0
        || formula.minimum_levels_per_extra == 0
        || formula.maximum_levels_per_extra == 0
        || formula.maximum == 0
    {
        bail!("quantities.{name} values must be positive");
    }
    if formula.minimum_base > formula.maximum_base || formula.maximum_base > formula.maximum {
        bail!("quantities.{name} must satisfy minimum_base <= maximum_base <= maximum");
    }
    Ok(())
}

fn validate_globals(globals: &[GlobalDrop]) -> Result<()> {
    let mut seen = BTreeSet::new();
    for (index, drop) in globals.iter().enumerate() {
        let key = match drop {
            GlobalDrop::Item(item) => {
                if item.item_id == 0 {
                    bail!("global_drops[{index}].item_id must be positive");
                }
                validate_chance(
                    &format!("global_drops[{index}].chance_per_million"),
                    item.chance_per_million,
                )?;
                match (item.minimum_quantity, item.maximum_quantity) {
                    (None, None) => {}
                    (Some(minimum), Some(maximum)) if minimum > 0 && minimum <= maximum => {}
                    (Some(_), Some(_)) => {
                        bail!("global_drops[{index}] quantity range must be positive and inclusive")
                    }
                    _ => {
                        bail!("global_drops[{index}] must specify both quantity limits or neither")
                    }
                }
                if item.quest_id == Some(0) {
                    bail!("global_drops[{index}].quest_id must be positive");
                }
                (0, item.item_id, item.quest_id.unwrap_or_default())
            }
            GlobalDrop::Mesos(mesos) => {
                validate_chance(
                    &format!("global_drops[{index}].chance_per_million"),
                    mesos.chance_per_million,
                )?;
                if mesos.minimum_mesos == 0 || mesos.minimum_mesos > mesos.maximum_mesos {
                    bail!("global_drops[{index}] meso range must be positive and inclusive");
                }
                (1, mesos.minimum_mesos, mesos.maximum_mesos)
            }
        };
        if !seen.insert(key) {
            bail!("global_drops[{index}] duplicates an earlier global drop");
        }
    }
    Ok(())
}

fn validate_mob_drops(drops: &[PolicyMobDrop]) -> Result<()> {
    let mut seen = BTreeSet::new();
    for (index, drop) in drops.iter().enumerate() {
        if drop.mob_id == 0 || drop.item_id == 0 {
            bail!("mob_drops[{index}] mob and item IDs must be positive");
        }
        if drop.quest_id == Some(0)
            || drop.source_map_id == Some(0)
            || drop.required_skill_id == Some(0)
        {
            bail!("mob_drops[{index}] optional IDs must be positive");
        }
        if drop.quest_id.is_none() && drop.source_map_id.is_none() {
            bail!("mob_drops[{index}] must specify quest_id or source_map_id as WZ evidence");
        }
        validate_chance(
            &format!("mob_drops[{index}].chance_per_million"),
            drop.chance_per_million,
        )?;
        if drop.minimum_quantity == 0 || drop.minimum_quantity > drop.maximum_quantity {
            bail!("mob_drops[{index}] quantity range must be positive and inclusive");
        }
        if !seen.insert((
            drop.mob_id,
            drop.item_id,
            drop.quest_id,
            drop.required_skill_id,
        )) {
            bail!("mob_drops[{index}] duplicates an earlier mob drop");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_policy_fields_are_rejected() {
        let source = r#"
schema_version = 1
policy_name = "test"
unexpected = true
"#;

        assert!(toml::from_str::<LootPolicy>(source).is_err());
    }

    #[test]
    fn item_ids_select_explicit_rate_and_quantity_classes() {
        assert_eq!(
            classify(2_000_000),
            (RateClass::Recovery, QuantityClass::Consumable)
        );
        assert_eq!(classify(2_060_000), (RateClass::Ammo, QuantityClass::Ammo));
        assert_eq!(
            classify(4_030_000),
            (RateClass::Quest, QuantityClass::Material)
        );
    }

    #[test]
    fn tracked_policy_parses_and_validates() {
        let source = include_str!("../../../../config/loot-policy.toml");
        let policy: LootPolicy = toml::from_str(source).expect("tracked policy schema");

        validate(&policy).expect("valid tracked policy");
        assert!(policy.global_drops.is_empty());
        assert_eq!(
            policy
                .mob_drops
                .iter()
                .map(|drop| (
                    drop.mob_id,
                    drop.item_id,
                    drop.quest_id,
                    drop.source_map_id,
                    drop.required_skill_id,
                ))
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([
                (9_000_001, 4_031_013, None, Some(108_000_200), None),
                (9_000_002, 4_031_013, None, Some(108_000_200), None),
                (9_000_100, 4_031_013, None, Some(108_000_300), None),
                (9_000_101, 4_031_013, None, Some(108_000_300), None),
                (9_000_200, 4_031_013, None, Some(108_000_100), None),
                (9_000_201, 4_031_013, None, Some(108_000_100), None),
                (9_000_300, 4_031_013, None, Some(108_000_400), None),
                (9_000_301, 4_031_013, None, Some(108_000_400), None),
                (
                    9_001_005,
                    4_031_856,
                    None,
                    Some(108_000_500),
                    Some(5_001_001),
                ),
                (
                    9_001_005,
                    4_031_857,
                    None,
                    Some(108_000_500),
                    Some(5_001_003),
                ),
                (9_300_018, 4_031_802, Some(1_035), None, None),
            ])
        );
        assert!(policy.mob_drops.iter().all(|drop| {
            drop.chance_per_million == 1_000_000
                && drop.minimum_quantity == 1
                && drop.maximum_quantity == 1
        }));
    }
}
