use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::path::PathBuf;

use serde::Serialize;

use super::policy::GlobalDrop;
use super::policy::GlobalItemDrop;
use super::policy::GlobalMesoDrop;
use super::policy::LevelExperienceFormula;
use super::policy::LootPolicy;
use super::policy::QuantityFormula;
use super::policy::apply_ratio;
use super::policy::classify;
use crate::Region;

const CHANCE_SCALE: u64 = 1_000_000;

#[derive(Clone, Debug, Default)]
pub(crate) struct WzFacts {
    pub associations: BTreeMap<u32, BTreeSet<u32>>,
    pub cards: BTreeMap<u32, u32>,
    pub mobs: BTreeMap<u32, MobFact>,
    pub items: BTreeMap<u32, ItemFact>,
    pub completion_quests: BTreeMap<u32, BTreeSet<u32>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct MobFact {
    pub mob_id: u32,
    pub name: Option<String>,
    pub level: u32,
    pub experience: u64,
    pub boss: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ItemFact {
    pub item_id: u32,
    pub kind: ItemKind,
    pub slot_max: Option<u32>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ItemKind {
    Equipment,
    Consume,
    Etc,
    MonsterBookCard,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Diagnostics {
    pub omissions: BTreeMap<String, u64>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct LootCatalog {
    pub policy_name: String,
    pub global_drops: Vec<CatalogGlobalDrop>,
    pub mobs: Vec<MobLoot>,
}

#[derive(Clone, Debug)]
pub(crate) enum CatalogGlobalDrop {
    Item(GlobalItemDrop),
    Mesos(GlobalMesoDrop),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct MobLoot {
    pub mob_id: u32,
    pub name: Option<String>,
    pub drops: Vec<MobDrop>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum MobDrop {
    Item(ItemDrop),
    Mesos(MesoDrop),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ItemDrop {
    pub item_id: u32,
    pub chance_per_million: u32,
    pub minimum_quantity: u32,
    pub maximum_quantity: u32,
    pub quest_id: Option<u32>,
    pub required_skill_id: Option<u32>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct MesoDrop {
    pub chance_per_million: u32,
    pub minimum_mesos: u32,
    pub maximum_mesos: u32,
}

#[derive(Debug, Serialize)]
pub struct GenerationReport {
    pub source_region: Region,
    pub requested_wz_version: Option<i16>,
    pub source_versions: BTreeMap<String, i16>,
    pub policy_name: String,
    pub counts: GenerationCounts,
    pub omissions: BTreeMap<String, u64>,
    pub warnings: Vec<String>,
    pub output_path: PathBuf,
}

#[derive(Debug, Default, Serialize)]
pub struct GenerationCounts {
    pub monster_book_associations: usize,
    pub monster_book_card_mappings: usize,
    pub source_mobs: usize,
    pub source_items: usize,
    pub completion_quest_items: usize,
    pub generated_mobs: usize,
    pub generated_reactors: usize,
    pub generated_item_rows: usize,
    pub generated_meso_rows: usize,
    pub generated_quest_rows: usize,
    pub generated_global_item_rows: usize,
    pub generated_global_meso_rows: usize,
}

impl Diagnostics {
    pub(crate) fn omit(
        &mut self,
        category: &str,
        warning: impl Into<String>,
    ) {
        *self.omissions.entry(category.to_owned()).or_default() += 1;
        self.warnings.push(warning.into());
    }

    pub(crate) fn warn(
        &mut self,
        warning: impl Into<String>,
    ) {
        self.warnings.push(warning.into());
    }
}

pub(crate) fn generate(
    facts: &WzFacts,
    policy: &LootPolicy,
    diagnostics: &mut Diagnostics,
) -> LootCatalog {
    let card_item_ids = facts.cards.values().copied().collect::<BTreeSet<_>>();
    let mut mobs = Vec::with_capacity(facts.mobs.len());
    for mob in facts.mobs.values() {
        let mut item_drops = BTreeMap::new();
        if let Some(item_ids) = facts.associations.get(&mob.mob_id) {
            for item_id in item_ids {
                if card_item_ids.contains(item_id) || *item_id / 10_000 == 238 {
                    continue;
                }
                let Some(item) = facts.items.get(item_id) else {
                    continue;
                };
                if *item_id / 10_000 == 403 {
                    let Some(quest_ids) = facts.completion_quests.get(item_id) else {
                        diagnostics.omit(
                            "unresolved_quest_item",
                            format!(
                                "mob {} item {} is a quest item without a positive completion \
                                 requirement",
                                mob.mob_id, item_id
                            ),
                        );
                        continue;
                    };
                    for quest_id in quest_ids {
                        let drop = item_drop(*item, mob, policy, Some(*quest_id));
                        item_drops
                            .insert((drop.item_id, drop.quest_id, drop.required_skill_id), drop);
                    }
                    continue;
                }
                let drop = item_drop(*item, mob, policy, None);
                item_drops.insert((drop.item_id, drop.quest_id, drop.required_skill_id), drop);
            }
        }
        if let Some(card_item_id) = facts.cards.get(&mob.mob_id)
            && let Some(card) = facts.items.get(card_item_id)
        {
            let drop = card_drop(*card, mob, policy);
            item_drops.insert((drop.item_id, None, None), drop);
        }
        for policy_drop in policy
            .mob_drops
            .iter()
            .filter(|drop| drop.mob_id == mob.mob_id)
        {
            assert!(
                facts.items.contains_key(&policy_drop.item_id),
                "validated quest mob drop item must exist"
            );
            let drop = ItemDrop {
                item_id: policy_drop.item_id,
                chance_per_million: policy_drop.chance_per_million,
                minimum_quantity: policy_drop.minimum_quantity,
                maximum_quantity: policy_drop.maximum_quantity,
                quest_id: policy_drop.quest_id,
                required_skill_id: policy_drop.required_skill_id,
            };
            item_drops.insert((drop.item_id, drop.quest_id, drop.required_skill_id), drop);
        }
        let mut drops = item_drops
            .into_values()
            .map(MobDrop::Item)
            .collect::<Vec<_>>();
        if facts.associations.contains_key(&mob.mob_id) || facts.cards.contains_key(&mob.mob_id) {
            drops.push(MobDrop::Mesos(meso_drop(mob, policy)));
        }
        mobs.push(MobLoot {
            mob_id: mob.mob_id,
            name: mob.name.clone(),
            drops,
        });
    }

    let mut globals = policy
        .global_drops
        .iter()
        .filter_map(|drop| match drop {
            GlobalDrop::Item(item) if facts.items.contains_key(&item.item_id) => {
                Some(CatalogGlobalDrop::Item(item.clone()))
            }
            GlobalDrop::Item(item) => {
                diagnostics.omit(
                    "unavailable_global_item",
                    format!(
                        "global item {} has no valid supported local WZ source",
                        item.item_id
                    ),
                );
                None
            }
            GlobalDrop::Mesos(mesos) => Some(CatalogGlobalDrop::Mesos(mesos.clone())),
        })
        .collect::<Vec<_>>();
    globals.sort_by_key(global_sort_key);

    LootCatalog {
        policy_name: policy.policy_name.clone(),
        global_drops: globals,
        mobs,
    }
}

pub(crate) fn report_counts(
    facts: &WzFacts,
    catalog: &LootCatalog,
) -> GenerationCounts {
    let mut counts = GenerationCounts {
        monster_book_associations: facts.associations.values().map(BTreeSet::len).sum(),
        monster_book_card_mappings: facts.cards.len(),
        source_mobs: facts.mobs.len(),
        source_items: facts.items.len(),
        completion_quest_items: facts.completion_quests.len(),
        generated_mobs: catalog.mobs.len(),
        ..GenerationCounts::default()
    };
    for drop in &catalog.global_drops {
        match drop {
            CatalogGlobalDrop::Item(_) => counts.generated_global_item_rows += 1,
            CatalogGlobalDrop::Mesos(_) => counts.generated_global_meso_rows += 1,
        }
    }
    for mob in &catalog.mobs {
        for drop in &mob.drops {
            match drop {
                MobDrop::Item(item) => {
                    counts.generated_item_rows += 1;
                    if item.quest_id.is_some() {
                        counts.generated_quest_rows += 1;
                    }
                }
                MobDrop::Mesos(_) => counts.generated_meso_rows += 1,
            }
        }
    }
    counts
}

fn item_drop(
    item: ItemFact,
    mob: &MobFact,
    policy: &LootPolicy,
    quest_id: Option<u32>,
) -> ItemDrop {
    let (rate_class, quantity_class) = classify(item.item_id);
    let mut chance = u64::from(policy.rate(rate_class));
    if mob.boss {
        chance = apply_ratio(chance, policy.boss_multipliers.item, CHANCE_SCALE);
    }
    let (minimum_quantity, maximum_quantity) =
        quantity_range(mob.level, policy.quantity(quantity_class), item.slot_max);
    ItemDrop {
        item_id: item.item_id,
        chance_per_million: chance as u32,
        minimum_quantity,
        maximum_quantity,
        quest_id,
        required_skill_id: None,
    }
}

fn card_drop(
    item: ItemFact,
    mob: &MobFact,
    policy: &LootPolicy,
) -> ItemDrop {
    let mut chance = formula_value(policy.card_chance, mob.level, mob.experience);
    if mob.boss {
        chance = apply_ratio(chance, policy.boss_multipliers.card, CHANCE_SCALE);
    }
    ItemDrop {
        item_id: item.item_id,
        chance_per_million: chance as u32,
        minimum_quantity: 1,
        maximum_quantity: 1,
        quest_id: None,
        required_skill_id: None,
    }
}

fn meso_drop(
    mob: &MobFact,
    policy: &LootPolicy,
) -> MesoDrop {
    let mut chance = formula_value(policy.meso_chance, mob.level, mob.experience);
    let mut expected = formula_value(policy.meso_expected_value, mob.level, mob.experience);
    if mob.boss {
        chance = apply_ratio(chance, policy.boss_multipliers.meso_chance, CHANCE_SCALE);
        expected = apply_ratio(
            expected,
            policy.boss_multipliers.meso_value,
            policy.meso_expected_value.maximum,
        );
    }
    let spread = u64::from(policy.meso_range.spread_per_thousand);
    let minimum = expected
        .saturating_mul(1_000 - spread)
        .checked_div(1_000)
        .unwrap_or_default()
        .max(u64::from(policy.meso_range.minimum));
    let maximum = expected
        .saturating_mul(1_000 + spread)
        .checked_div(1_000)
        .unwrap_or(expected)
        .max(minimum);
    MesoDrop {
        chance_per_million: chance as u32,
        minimum_mesos: minimum.min(u64::from(u32::MAX)) as u32,
        maximum_mesos: maximum.min(u64::from(u32::MAX)) as u32,
    }
}

fn formula_value(
    formula: LevelExperienceFormula,
    level: u32,
    experience: u64,
) -> u64 {
    formula
        .base
        .saturating_add(formula.per_level.saturating_mul(u64::from(level)))
        .saturating_add(
            formula
                .per_experience_sqrt
                .saturating_mul(integer_sqrt(experience)),
        )
        .min(formula.maximum)
}

fn integer_sqrt(value: u64) -> u64 {
    if value < 2 {
        return value;
    }
    let mut result = 1_u64 << ((64 - value.leading_zeros()).div_ceil(2));
    loop {
        let next = (result + value / result) / 2;
        if next >= result {
            return result;
        }
        result = next;
    }
}

fn quantity_range(
    level: u32,
    formula: QuantityFormula,
    slot_max: Option<u32>,
) -> (u32, u32) {
    let minimum = formula
        .minimum_base
        .saturating_add(level / formula.minimum_levels_per_extra)
        .min(formula.maximum);
    let maximum = formula
        .maximum_base
        .saturating_add(level / formula.maximum_levels_per_extra)
        .max(minimum)
        .min(formula.maximum);
    let stack_limit = slot_max.filter(|value| *value > 0).unwrap_or(u32::MAX);
    let maximum = maximum.min(stack_limit).max(1);
    (minimum.min(maximum).max(1), maximum)
}

fn global_sort_key(drop: &CatalogGlobalDrop) -> (u8, u32, u32, u32) {
    match drop {
        CatalogGlobalDrop::Item(item) => (
            0,
            item.item_id,
            item.quest_id.unwrap_or_default(),
            item.chance_per_million,
        ),
        CatalogGlobalDrop::Mesos(mesos) => (
            1,
            mesos.minimum_mesos,
            mesos.maximum_mesos,
            mesos.chance_per_million,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integer_square_root_is_exact_and_monotonic() {
        let values = (0..=10_000).map(integer_sqrt).collect::<Vec<_>>();
        assert!(values.windows(2).all(|window| window[0] <= window[1]));
        assert_eq!(integer_sqrt(15), 3);
        assert_eq!(integer_sqrt(16), 4);
        assert_eq!(integer_sqrt(u64::MAX), u64::from(u32::MAX));
    }

    #[test]
    fn level_quantities_are_inclusive_and_clamped_to_slot_max() {
        let formula = QuantityFormula {
            minimum_base: 1,
            minimum_levels_per_extra: 20,
            maximum_base: 2,
            maximum_levels_per_extra: 10,
            maximum: 20,
        };
        assert_eq!(quantity_range(40, formula, None), (3, 6));
        assert_eq!(quantity_range(40, formula, Some(4)), (3, 4));
        assert_eq!(quantity_range(100, formula, Some(1)), (1, 1));
    }

    #[test]
    fn formulas_are_monotonic_in_level_and_experience() {
        let formula = LevelExperienceFormula {
            base: 100,
            per_level: 3,
            per_experience_sqrt: 5,
            maximum: 1_000_000,
        };
        assert!(formula_value(formula, 11, 100) > formula_value(formula, 10, 100));
        assert!(formula_value(formula, 10, 121) > formula_value(formula, 10, 100));
    }

    #[test]
    fn generation_adds_one_mapped_card_and_one_row_per_completion_quest() {
        let policy: LootPolicy =
            toml::from_str(include_str!("../../../../config/loot-policy.toml"))
                .expect("tracked policy");
        let facts = WzFacts {
            associations: BTreeMap::from([(
                100,
                BTreeSet::from([2_380_000, 4_000_000, 4_030_000]),
            )]),
            cards: BTreeMap::from([(100, 2_380_000)]),
            mobs: BTreeMap::from([(
                100,
                MobFact {
                    mob_id: 100,
                    name: None,
                    level: 10,
                    experience: 100,
                    boss: false,
                },
            )]),
            items: [
                (2_380_000, ItemKind::MonsterBookCard),
                (4_000_000, ItemKind::Etc),
                (4_030_000, ItemKind::Etc),
            ]
            .into_iter()
            .map(|(item_id, kind)| {
                (
                    item_id,
                    ItemFact {
                        item_id,
                        kind,
                        slot_max: Some(100),
                    },
                )
            })
            .collect(),
            completion_quests: BTreeMap::from([(4_030_000, BTreeSet::from([10, 20]))]),
        };
        let mut diagnostics = Diagnostics::default();

        let catalog = generate(&facts, &policy, &mut diagnostics);
        let item_drops = catalog.mobs[0]
            .drops
            .iter()
            .filter_map(|drop| match drop {
                MobDrop::Item(item) => Some(*item),
                MobDrop::Mesos(_) => None,
            })
            .collect::<Vec<_>>();

        assert_eq!(
            item_drops
                .iter()
                .filter(|drop| drop.item_id == 2_380_000)
                .count(),
            1
        );
        assert_eq!(item_drops[0].chance_per_million, 8_500);
        assert_eq!(
            item_drops
                .iter()
                .filter_map(|drop| (drop.item_id == 4_030_000).then_some(drop.quest_id))
                .flatten()
                .collect::<Vec<_>>(),
            vec![10, 20]
        );
        assert!(matches!(
            catalog.mobs[0].drops.last(),
            Some(MobDrop::Mesos(_))
        ));
        assert!(diagnostics.omissions.is_empty());
    }

    #[test]
    fn explicit_quest_drop_does_not_add_mesos_to_a_policy_only_mob() {
        let policy: LootPolicy =
            toml::from_str(include_str!("../../../../config/loot-policy.toml"))
                .expect("tracked policy");
        let facts = WzFacts {
            mobs: BTreeMap::from([(
                9_300_018,
                MobFact {
                    mob_id: 9_300_018,
                    name: Some("Tutorial Jr. Sentinel".to_owned()),
                    level: 1,
                    experience: 1,
                    boss: false,
                },
            )]),
            items: BTreeMap::from([(
                4_031_802,
                ItemFact {
                    item_id: 4_031_802,
                    kind: ItemKind::Etc,
                    slot_max: Some(100),
                },
            )]),
            completion_quests: BTreeMap::from([(4_031_802, BTreeSet::from([1_035]))]),
            ..WzFacts::default()
        };
        let mut diagnostics = Diagnostics::default();

        let catalog = generate(&facts, &policy, &mut diagnostics);

        assert_eq!(
            catalog.mobs[0].drops,
            vec![MobDrop::Item(ItemDrop {
                item_id: 4_031_802,
                chance_per_million: 1_000_000,
                minimum_quantity: 1,
                maximum_quantity: 1,
                quest_id: Some(1_035),
                required_skill_id: None,
            })]
        );
        let counts = report_counts(&facts, &catalog);
        assert_eq!(counts.generated_item_rows, 1);
        assert_eq!(counts.generated_quest_rows, 1);
        assert_eq!(counts.generated_meso_rows, 0);
    }

    #[test]
    fn explicit_quest_drop_overrides_an_inferred_row_for_the_same_quest() {
        let policy: LootPolicy =
            toml::from_str(include_str!("../../../../config/loot-policy.toml"))
                .expect("tracked policy");
        let facts = WzFacts {
            associations: BTreeMap::from([(9_300_018, BTreeSet::from([4_031_802]))]),
            mobs: BTreeMap::from([(
                9_300_018,
                MobFact {
                    mob_id: 9_300_018,
                    name: None,
                    level: 1,
                    experience: 1,
                    boss: false,
                },
            )]),
            items: BTreeMap::from([(
                4_031_802,
                ItemFact {
                    item_id: 4_031_802,
                    kind: ItemKind::Etc,
                    slot_max: Some(100),
                },
            )]),
            completion_quests: BTreeMap::from([(4_031_802, BTreeSet::from([1_035]))]),
            ..WzFacts::default()
        };
        let mut diagnostics = Diagnostics::default();

        let catalog = generate(&facts, &policy, &mut diagnostics);
        let item_drops = catalog.mobs[0]
            .drops
            .iter()
            .filter_map(|drop| match drop {
                MobDrop::Item(item) => Some(*item),
                MobDrop::Mesos(_) => None,
            })
            .collect::<Vec<_>>();

        assert_eq!(
            item_drops,
            vec![ItemDrop {
                item_id: 4_031_802,
                chance_per_million: 1_000_000,
                minimum_quantity: 1,
                maximum_quantity: 1,
                quest_id: Some(1_035),
                required_skill_id: None,
            }]
        );
    }

    #[test]
    fn policy_drop_preserves_the_required_killing_skill() {
        let policy: LootPolicy =
            toml::from_str(include_str!("../../../../config/loot-policy.toml"))
                .expect("tracked policy");
        let facts = WzFacts {
            mobs: BTreeMap::from([(
                9_001_005,
                MobFact {
                    mob_id: 9_001_005,
                    name: Some("OctoPirate".to_owned()),
                    level: 1,
                    experience: 1,
                    boss: false,
                },
            )]),
            items: [4_031_856, 4_031_857]
                .into_iter()
                .map(|item_id| {
                    (
                        item_id,
                        ItemFact {
                            item_id,
                            kind: ItemKind::Etc,
                            slot_max: Some(100),
                        },
                    )
                })
                .collect(),
            ..WzFacts::default()
        };
        let mut diagnostics = Diagnostics::default();

        let catalog = generate(&facts, &policy, &mut diagnostics);

        assert_eq!(
            catalog.mobs[0].drops,
            vec![
                MobDrop::Item(ItemDrop {
                    item_id: 4_031_856,
                    chance_per_million: 1_000_000,
                    minimum_quantity: 1,
                    maximum_quantity: 1,
                    quest_id: None,
                    required_skill_id: Some(5_001_001),
                }),
                MobDrop::Item(ItemDrop {
                    item_id: 4_031_857,
                    chance_per_million: 1_000_000,
                    minimum_quantity: 1,
                    maximum_quantity: 1,
                    quest_id: None,
                    required_skill_id: Some(5_001_003),
                }),
            ]
        );
    }
}
