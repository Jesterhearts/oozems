use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use oozems_proto::v1::KeyBinding;
use oozems_proto::v1::LearnedSkill;
use oozems_proto::v1::PlayerState;
use oozems_proto::v1::SkillActivation;
use oozems_proto::v1::SkillBook;
use oozems_proto::v1::SkillDefinition;
use oozems_proto::v1::SkillStats;
use oozems_proto::v1::SkillUseResult;
use oozems_proto::v1::SkillValue;
use oozems_proto::v1::skill_value;
use thiserror::Error;

use crate::content::AuthoritativeSkillDefinition;
use crate::content::SkillBookContext;
use crate::effects::EffectModifiers;
use crate::effects::ProjectedEffects;
use crate::jobs::SkillAttackType;
use crate::jobs::WeaponFamily;
use crate::jobs::skill_attack_type;
use crate::skill_formula::FormulaCatalog;
use crate::skill_formula::evaluate_damage_profile;
use crate::skill_formula::evaluate_profile_property;

const MAX_DAMAGE: u32 = 99_999;

#[derive(Default)]
pub struct SkillCooldowns {
    deadlines: Mutex<HashMap<(String, u32), u64>>,
}

pub struct PreparedSkillUse {
    pub player: PlayerState,
    pub result: SkillUseResult,
    pub cooldown_ms: u64,
    pub attack_type: SkillAttackType,
    pub consumes_combo: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LearnedSkillModifiers {
    pub combat: EffectModifiers,
    pub strength: i32,
    pub weapons: [WeaponSkillModifiers; WeaponFamily::COUNT],
    pub max_hp_per_level: u32,
    pub max_hp_per_ability_point: u32,
    pub max_mp_per_level: u32,
    pub max_mp_per_ability_point: u32,
    pub throwing_star_capacity: u32,
    pub bullet_capacity: u32,
    pub combo_ability: ComboAbilityModifiers,
    pub combo_critical: ComboCriticalModifiers,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ComboAbilityModifiers {
    pub maximum_tiers: u32,
    pub weapon_attack_per_tier: u32,
    pub defense_per_tier: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ComboCriticalModifiers {
    pub maximum_tiers: u32,
    pub critical_chance_per_tier: u32,
    pub critical_damage_per_tier: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WeaponSkillModifiers {
    pub weapon_attack: i32,
    pub accuracy: i32,
    pub mastery: u32,
}

impl LearnedSkillModifiers {
    pub fn weapon(
        self,
        family: Option<WeaponFamily>,
    ) -> WeaponSkillModifiers {
        family.map_or_else(WeaponSkillModifiers::default, |family| {
            self.weapons[family.index()]
        })
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SkillRuleError {
    #[error("the player does not have character stats")]
    MissingStats,
    #[error("skill book job {book_job_id} does not match player job {player_job_id}")]
    JobMismatch {
        book_job_id: u32,
        player_job_id: u32,
    },
    #[error("skill {skill_id} is not part of job {job_id}")]
    UnknownSkill { skill_id: u32, job_id: u32 },
    #[error("skill {skill_id} has an invalid maximum level")]
    InvalidMaximumLevel { skill_id: u32 },
    #[error("skill {skill_id} is recorded more than once")]
    DuplicateLearnedSkill { skill_id: u32 },
    #[error("skill {skill_id} has invalid learned level {level}; its maximum is {maximum}")]
    InvalidLearnedLevel {
        skill_id: u32,
        level: u32,
        maximum: u32,
    },
    #[error("skill {skill_id} has invalid master level {master_level}; its maximum is {maximum}")]
    InvalidMasterLevel {
        skill_id: u32,
        master_level: u32,
        maximum: u32,
    },
    #[error("there are no skill points available")]
    NoSkillPoints,
    #[error("skill {skill_id} is already at its maximum level")]
    MaximumLevel { skill_id: u32 },
    #[error("skill {skill_id} requires skill {required_skill_id} at level {required_level}")]
    RequirementNotMet {
        skill_id: u32,
        required_skill_id: u32,
        required_level: u32,
    },
    #[error("skill {skill_id} has not been learned")]
    NotLearned { skill_id: u32 },
    #[error("skill {skill_id} is not an activatable skill")]
    NotActive { skill_id: u32 },
    #[error("skill {skill_id} does not define learned level {level}")]
    MissingLevel { skill_id: u32, level: u32 },
    #[error("skill {skill_id} property {property:?} is not a supported numeric value")]
    InvalidProperty {
        skill_id: u32,
        property: &'static str,
    },
    #[error("skill {skill_id} requires {required} HP, but only {available} HP can be spent")]
    InsufficientHealth {
        skill_id: u32,
        required: u32,
        available: u32,
    },
    #[error("skill {skill_id} requires {required} MP, but only {available} MP is available")]
    InsufficientMana {
        skill_id: u32,
        required: u32,
        available: u32,
    },
    #[error("skill {skill_id} is on cooldown for another {remaining_ms} ms")]
    Cooldown { skill_id: u32, remaining_ms: u64 },
    #[error("skill {skill_id} requires {required} combo, but only {available} is available")]
    InsufficientCombo {
        skill_id: u32,
        required: u32,
        available: u32,
    },
    #[error("the skill cooldown store is unavailable")]
    CooldownStore,
    #[error("a configured formula failed: {message}")]
    Formula { message: String },
}

pub fn personalize_skill_book(
    mut context: SkillBookContext,
    player: &PlayerState,
) -> Result<SkillBook, SkillRuleError> {
    validate_job(&context.book, player)?;
    let levels = validate_learned_skills(
        context.book.job_id,
        &context.authoritative_skills,
        &player.learned_skills,
    )?;
    for skill in &mut context.book.skills {
        let Some(definition) = skill.definition.as_ref() else {
            continue;
        };
        let (level, master_level) = levels
            .get(&definition.skill_id)
            .copied()
            .unwrap_or_default();
        skill.level = level;
        skill.master_level = master_level;
    }
    context.book.available_points = player.skill_points;
    Ok(context.book)
}

pub fn validate_bound_skills(
    bindings: &[KeyBinding],
    player: &PlayerState,
    context: &SkillBookContext,
) -> Result<(), SkillRuleError> {
    validate_job(&context.book, player)?;
    let learned_skills = validate_learned_skills(
        context.book.job_id,
        &context.authoritative_skills,
        &player.learned_skills,
    )?;
    for skill_id in bindings
        .iter()
        .map(|binding| binding.skill_id)
        .filter(|skill_id| *skill_id != 0)
    {
        if learned_skills
            .get(&skill_id)
            .is_none_or(|(level, _)| *level == 0)
            || skill_definition(&context.book, skill_id)
                .map_or(true, |definition| !is_active_skill(definition))
        {
            return Err(SkillRuleError::NotLearned { skill_id });
        }
    }
    Ok(())
}

pub fn prune_non_active_skill_bindings(
    bindings: &mut Vec<KeyBinding>,
    context: &SkillBookContext,
) -> bool {
    let previous = bindings.len();
    bindings.retain(|binding| {
        binding.skill_id == 0
            || skill_definition(&context.book, binding.skill_id)
                .map(is_active_skill)
                .unwrap_or(true)
    });
    bindings.len() != previous
}

pub fn learned_skill_modifiers(
    context: &SkillBookContext,
    player: &PlayerState,
) -> Result<LearnedSkillModifiers, SkillRuleError> {
    validate_job(&context.book, player)?;
    let levels = validate_learned_skills(
        context.book.job_id,
        &context.authoritative_skills,
        &player.learned_skills,
    )?;
    let definitions = context
        .authoritative_skills
        .iter()
        .map(|skill| (skill.definition.skill_id, &skill.definition))
        .collect::<HashMap<_, _>>();
    let mut modifiers = LearnedSkillModifiers::default();
    for (skill_id, (level, _)) in levels {
        if level == 0 {
            continue;
        }
        let definition = definitions[&skill_id];
        let activation = skill_activation(definition);
        if matches!(
            activation,
            SkillActivation::Active | SkillActivation::Unspecified
        ) {
            continue;
        }
        let level_stats = definition
            .levels
            .iter()
            .find(|entry| entry.level == level)
            .and_then(|entry| entry.stats.as_ref())
            .ok_or(SkillRuleError::MissingLevel { skill_id, level })?;
        match activation {
            SkillActivation::Passive => add_passive_stats(
                &mut modifiers,
                skill_id,
                level_stats,
                definition.common_stats.as_ref(),
            )?,
            SkillActivation::Reactive => add_reactive_stats(
                &mut modifiers,
                skill_id,
                level_stats,
                definition.common_stats.as_ref(),
            )?,
            SkillActivation::Active | SkillActivation::Unspecified => unreachable!(),
        }
    }
    Ok(modifiers)
}

fn add_reactive_stats(
    modifiers: &mut LearnedSkillModifiers,
    skill_id: u32,
    level: &SkillStats,
    common: Option<&SkillStats>,
) -> Result<(), SkillRuleError> {
    let number = |property, level, common| numeric_stat(skill_id, property, level, common);
    match skill_id {
        21_000_000 => {
            modifiers.combo_ability.maximum_tiers = number(
                "combo_stat_increment",
                level.combo_stat_increment.as_ref(),
                common.and_then(|stats| stats.combo_stat_increment.as_ref()),
            )?;
            modifiers.combo_ability.weapon_attack_per_tier = number(
                "weapon_attack_per_combo_threshold",
                level.weapon_attack_per_combo_threshold.as_ref(),
                common.and_then(|stats| stats.weapon_attack_per_combo_threshold.as_ref()),
            )?;
            modifiers.combo_ability.defense_per_tier = number(
                "defense_per_combo_threshold",
                level.defense_per_combo_threshold.as_ref(),
                common.and_then(|stats| stats.defense_per_combo_threshold.as_ref()),
            )?;
        }
        21_110_000 => {
            modifiers.combo_critical.maximum_tiers = number(
                "combo_stat_increment",
                level.combo_stat_increment.as_ref(),
                common.and_then(|stats| stats.combo_stat_increment.as_ref()),
            )?;
            modifiers.combo_critical.critical_chance_per_tier = number(
                "critical_chance",
                level.critical_chance.as_ref(),
                common.and_then(|stats| stats.critical_chance.as_ref()),
            )?;
            modifiers.combo_critical.critical_damage_per_tier = number(
                "critical_damage",
                level.critical_damage.as_ref(),
                common.and_then(|stats| stats.critical_damage.as_ref()),
            )?;
        }
        _ => {}
    }
    Ok(())
}

fn add_passive_stats(
    modifiers: &mut LearnedSkillModifiers,
    skill_id: u32,
    level: &SkillStats,
    common: Option<&SkillStats>,
) -> Result<(), SkillRuleError> {
    let weapon_family = passive_weapon_family(skill_id);
    macro_rules! add_signed {
        ($field:ident, $property:literal, $destination:expr) => {
            *$destination = (*$destination).saturating_add(signed_numeric_stat(
                skill_id,
                $property,
                level.$field.as_ref(),
                common.and_then(|stats| stats.$field.as_ref()),
            )?);
        };
    }
    macro_rules! add_unsigned {
        ($field:ident, $property:literal, $destination:expr) => {
            *$destination = (*$destination).saturating_add(numeric_stat(
                skill_id,
                $property,
                level.$field.as_ref(),
                common.and_then(|stats| stats.$field.as_ref()),
            )?);
        };
    }

    if let Some(family) = weapon_family {
        add_signed!(
            weapon_attack,
            "weapon_attack",
            &mut modifiers.weapons[family.index()].weapon_attack
        );
    } else {
        add_signed!(
            weapon_attack,
            "weapon_attack",
            &mut modifiers.combat.weapon_attack
        );
    }
    add_signed!(
        magic_attack,
        "magic_attack",
        &mut modifiers.combat.magic_attack
    );
    if let Some(family) = weapon_family {
        add_signed!(
            accuracy,
            "accuracy",
            &mut modifiers.weapons[family.index()].accuracy
        );
    } else {
        add_signed!(accuracy, "accuracy", &mut modifiers.combat.accuracy);
    }
    add_signed!(
        avoidability,
        "avoidability",
        &mut modifiers.combat.avoidability
    );
    add_signed!(
        weapon_defense,
        "weapon_defense",
        &mut modifiers.combat.weapon_defense
    );
    add_signed!(
        magic_defense,
        "magic_defense",
        &mut modifiers.combat.magic_defense
    );
    add_signed!(speed, "speed", &mut modifiers.combat.speed);
    add_signed!(jump, "jump", &mut modifiers.combat.jump);
    add_signed!(strength, "strength", &mut modifiers.strength);
    add_unsigned!(
        max_hp_per_level,
        "max_hp_per_level",
        &mut modifiers.max_hp_per_level
    );
    add_unsigned!(
        max_hp_per_ability_point,
        "max_hp_per_ability_point",
        &mut modifiers.max_hp_per_ability_point
    );
    add_unsigned!(
        max_mp_per_level,
        "max_mp_per_level",
        &mut modifiers.max_mp_per_level
    );
    add_unsigned!(
        max_mp_per_ability_point,
        "max_mp_per_ability_point",
        &mut modifiers.max_mp_per_ability_point
    );
    add_unsigned!(
        throwing_star_capacity,
        "throwing_star_capacity",
        &mut modifiers.throwing_star_capacity
    );
    add_unsigned!(
        bullet_capacity,
        "bullet_capacity",
        &mut modifiers.bullet_capacity
    );
    let mastery = numeric_stat(
        skill_id,
        "mastery",
        level.mastery.as_ref(),
        common.and_then(|stats| stats.mastery.as_ref()),
    )?;
    if let Some(family) = weapon_family {
        modifiers.weapons[family.index()].mastery =
            modifiers.weapons[family.index()].mastery.max(mastery);
    }
    Ok(())
}

pub fn weapon_mastery(
    modifiers: LearnedSkillModifiers,
    family: Option<WeaponFamily>,
) -> f64 {
    mastery_ratio(modifiers.weapon(family).mastery)
}

fn passive_weapon_family(skill_id: u32) -> Option<WeaponFamily> {
    match skill_id {
        1_100_000 | 1_200_000 | 11_100_000 => Some(WeaponFamily::Sword),
        1_100_001 => Some(WeaponFamily::Axe),
        1_200_001 => Some(WeaponFamily::BluntWeapon),
        1_300_000 => Some(WeaponFamily::Spear),
        1_300_001 | 21_100_000 | 21_120_001 => Some(WeaponFamily::Polearm),
        3_100_000 | 3_120_005 | 13_100_000 | 13_110_003 => Some(WeaponFamily::Bow),
        3_200_000 | 3_220_004 => Some(WeaponFamily::Crossbow),
        4_100_000 | 14_100_000 => Some(WeaponFamily::Claw),
        4_200_000 => Some(WeaponFamily::Dagger),
        5_100_001 | 15_100_001 => Some(WeaponFamily::Knuckle),
        5_200_000 => Some(WeaponFamily::Gun),
        _ => None,
    }
}

fn mastery_ratio(mastery: u32) -> f64 {
    if mastery == 0 {
        0.1
    } else {
        (0.1 + f64::from(mastery) * 0.05).min(1.0)
    }
}

pub fn allocate_skill_point(
    mut player: PlayerState,
    context: &SkillBookContext,
    skill_id: u32,
) -> Result<PlayerState, SkillRuleError> {
    validate_job(&context.book, &player)?;
    let levels = validate_learned_skills(
        context.book.job_id,
        &context.authoritative_skills,
        &player.learned_skills,
    )?;
    let definition = skill_definition(&context.book, skill_id)?;
    if definition.max_level == 0 {
        return Err(SkillRuleError::InvalidMaximumLevel { skill_id });
    }
    if player.skill_points == 0 {
        return Err(SkillRuleError::NoSkillPoints);
    }
    let (current_level, master_level) = levels.get(&skill_id).copied().unwrap_or_default();
    let maximum_level = if master_level > 0 {
        definition.max_level.min(master_level)
    } else {
        definition.max_level
    };
    if current_level >= maximum_level {
        return Err(SkillRuleError::MaximumLevel { skill_id });
    }
    for requirement in &definition.requirements {
        let learned_level = levels
            .get(&requirement.skill_id)
            .map(|(level, _)| *level)
            .unwrap_or_default();
        if learned_level < requirement.level {
            return Err(SkillRuleError::RequirementNotMet {
                skill_id,
                required_skill_id: requirement.skill_id,
                required_level: requirement.level,
            });
        }
    }

    if let Some(skill) = player
        .learned_skills
        .iter_mut()
        .find(|skill| skill.skill_id == skill_id)
    {
        skill.level += 1;
    } else {
        player.learned_skills.push(LearnedSkill {
            skill_id,
            level: 1,
            master_level: 0,
        });
        player
            .learned_skills
            .sort_by_key(|learned| learned.skill_id);
    }
    player.skill_points -= 1;
    Ok(player)
}

pub fn prepare_skill_use(
    mut player: PlayerState,
    context: &SkillBookContext,
    skill_id: u32,
    formulas: &FormulaCatalog,
    effects: ProjectedEffects,
) -> Result<PreparedSkillUse, SkillRuleError> {
    validate_job(&context.book, &player)?;
    let levels = validate_learned_skills(
        context.book.job_id,
        &context.authoritative_skills,
        &player.learned_skills,
    )?;
    let level = levels
        .get(&skill_id)
        .map(|(level, _)| *level)
        .filter(|level| *level > 0)
        .ok_or(SkillRuleError::NotLearned { skill_id })?;
    let definition = skill_definition(&context.book, skill_id)?;
    if !is_active_skill(definition) {
        return Err(SkillRuleError::NotActive { skill_id });
    }
    let required_combo = combo_requirement(skill_id);
    if effects.combo_count < required_combo {
        return Err(SkillRuleError::InsufficientCombo {
            skill_id,
            required: required_combo,
            available: effects.combo_count,
        });
    }
    let level_stats = definition
        .levels
        .iter()
        .find(|entry| entry.level == level)
        .and_then(|entry| entry.stats.as_ref())
        .ok_or(SkillRuleError::MissingLevel { skill_id, level })?;
    let common_stats = definition.common_stats.as_ref();

    let hp_spent = numeric_stat(
        skill_id,
        "hpCon",
        level_stats.hp_cost.as_ref(),
        common_stats.and_then(|stats| stats.hp_cost.as_ref()),
    )?;
    let mp_spent = numeric_stat(
        skill_id,
        "mpCon",
        level_stats.mp_cost.as_ref(),
        common_stats.and_then(|stats| stats.mp_cost.as_ref()),
    )?;
    let hp_recovery = numeric_stat(
        skill_id,
        "hp",
        level_stats.hp.as_ref(),
        common_stats.and_then(|stats| stats.hp.as_ref()),
    )?;
    let mut mp_recovery = numeric_stat(
        skill_id,
        "mp",
        level_stats.mp.as_ref(),
        common_stats.and_then(|stats| stats.mp.as_ref()),
    )?;
    let fixed_damage = optional_numeric_stat(
        skill_id,
        "fixdamage",
        level_stats.fixed_damage.as_ref(),
        common_stats.and_then(|stats| stats.fixed_damage.as_ref()),
    )?;
    let skill_damage = optional_numeric_stat(
        skill_id,
        "damage",
        level_stats.damage.as_ref(),
        common_stats.and_then(|stats| stats.damage.as_ref()),
    )?;
    let speed_bonus = signed_numeric_stat(
        skill_id,
        "speed",
        level_stats.speed.as_ref(),
        common_stats.and_then(|stats| stats.speed.as_ref()),
    )?;
    let jump_bonus = signed_numeric_stat(
        skill_id,
        "jump",
        level_stats.jump.as_ref(),
        common_stats.and_then(|stats| stats.jump.as_ref()),
    )?;
    let weapon_attack_bonus = signed_numeric_stat(
        skill_id,
        "pad",
        level_stats.weapon_attack.as_ref(),
        common_stats.and_then(|stats| stats.weapon_attack.as_ref()),
    )?;
    let magic_attack_bonus = signed_numeric_stat(
        skill_id,
        "mad",
        level_stats.magic_attack.as_ref(),
        common_stats.and_then(|stats| stats.magic_attack.as_ref()),
    )?;
    let weapon_defense_bonus = signed_numeric_stat(
        skill_id,
        "pdd",
        level_stats.weapon_defense.as_ref(),
        common_stats.and_then(|stats| stats.weapon_defense.as_ref()),
    )?;
    let magic_defense_bonus = signed_numeric_stat(
        skill_id,
        "mdd",
        level_stats.magic_defense.as_ref(),
        common_stats.and_then(|stats| stats.magic_defense.as_ref()),
    )?;
    let accuracy_bonus = signed_numeric_stat(
        skill_id,
        "acc",
        level_stats.accuracy.as_ref(),
        common_stats.and_then(|stats| stats.accuracy.as_ref()),
    )?;
    let avoidability_bonus = signed_numeric_stat(
        skill_id,
        "eva",
        level_stats.avoidability.as_ref(),
        common_stats.and_then(|stats| stats.avoidability.as_ref()),
    )?;
    let strength_bonus = signed_numeric_stat(
        skill_id,
        "str",
        level_stats.strength.as_ref(),
        common_stats.and_then(|stats| stats.strength.as_ref()),
    )?;
    let critical_chance_bonus = numeric_stat(
        skill_id,
        "critical_chance",
        level_stats.critical_chance.as_ref(),
        common_stats.and_then(|stats| stats.critical_chance.as_ref()),
    )?;
    let critical_damage_bonus = numeric_stat(
        skill_id,
        "critical_damage",
        level_stats.critical_damage.as_ref(),
        common_stats.and_then(|stats| stats.critical_damage.as_ref()),
    )?;
    let outgoing_damage_percent = numeric_stat(
        skill_id,
        "outgoing_damage_percent",
        level_stats.outgoing_damage_percent.as_ref(),
        common_stats.and_then(|stats| stats.outgoing_damage_percent.as_ref()),
    )?;
    let enemy_speed_penalty = signed_numeric_stat(
        skill_id,
        "enemy_speed_penalty",
        level_stats.enemy_speed_penalty.as_ref(),
        common_stats.and_then(|stats| stats.enemy_speed_penalty.as_ref()),
    )?;
    let enemy_slow_duration = numeric_stat(
        skill_id,
        "enemy_slow_duration",
        level_stats.enemy_slow_duration.as_ref(),
        common_stats.and_then(|stats| stats.enemy_slow_duration.as_ref()),
    )?;
    let enemy_slow_chance = numeric_stat(
        skill_id,
        "success_probability",
        level_stats.success_probability.as_ref(),
        common_stats.and_then(|stats| stats.success_probability.as_ref()),
    )?;
    let duration = skill_duration(
        skill_id,
        "time",
        level_stats.duration.as_ref(),
        common_stats.and_then(|stats| stats.duration.as_ref()),
    )?;
    let duration_seconds = match duration {
        SkillDuration::Timed(seconds) => seconds,
        SkillDuration::None | SkillDuration::Permanent => 0,
    };
    let periodic_hp_recovery = numeric_stat(
        skill_id,
        "hpRecoveryPerFiveSeconds",
        level_stats.hp_recovery_per_five_seconds.as_ref(),
        common_stats.and_then(|stats| stats.hp_recovery_per_five_seconds.as_ref()),
    )?;
    if periodic_hp_recovery > 0 && !matches!(duration, SkillDuration::Timed(seconds) if seconds > 0)
    {
        return Err(SkillRuleError::InvalidProperty {
            skill_id,
            property: "time",
        });
    }
    let cooldown_seconds = numeric_stat(
        skill_id,
        "cooltime",
        level_stats.cooldown.as_ref(),
        common_stats.and_then(|stats| stats.cooldown.as_ref()),
    )?;

    let formula_damage = if fixed_damage.is_none() {
        calculate_formula_damage(&player, formulas, skill_id, level, skill_damage, effects)?
    } else {
        None
    };
    let stats = player.stats.as_mut().ok_or(SkillRuleError::MissingStats)?;
    let hp_conversion_percent = numeric_stat(
        skill_id,
        "max_hp_consumption_percent",
        level_stats.max_hp_consumption_percent.as_ref(),
        common_stats.and_then(|stats| stats.max_hp_consumption_percent.as_ref()),
    )?;
    let hp_to_mp_percent = numeric_stat(
        skill_id,
        "hp_to_mp_conversion_percent",
        level_stats.hp_to_mp_conversion_percent.as_ref(),
        common_stats.and_then(|stats| stats.hp_to_mp_conversion_percent.as_ref()),
    )?;
    let converted_hp = percent_of(stats.max_hp, hp_conversion_percent);
    let hp_spent = hp_spent.saturating_add(converted_hp);
    mp_recovery = mp_recovery.saturating_add(percent_of(converted_hp, hp_to_mp_percent));
    let spendable_hp = stats.hp.saturating_sub(1);
    if hp_spent > spendable_hp {
        return Err(SkillRuleError::InsufficientHealth {
            skill_id,
            required: hp_spent,
            available: spendable_hp,
        });
    }
    if mp_spent > stats.mp {
        return Err(SkillRuleError::InsufficientMana {
            skill_id,
            required: mp_spent,
            available: stats.mp,
        });
    }
    stats.hp -= hp_spent;
    stats.mp -= mp_spent;
    let hp_before_recovery = stats.hp;
    stats.hp = stats.hp.saturating_add(hp_recovery).min(stats.max_hp);
    let hp_restored = stats.hp - hp_before_recovery;
    let mp_before_recovery = stats.mp;
    stats.mp = stats.mp.saturating_add(mp_recovery).min(stats.max_mp);
    let mp_restored = stats.mp - mp_before_recovery;
    let damage = fixed_damage
        .map(|damage| {
            let damage = damage.clamp(1, MAX_DAMAGE);
            (damage, damage)
        })
        .or(formula_damage);

    let attack_type = skill_attack_type(stats.job_id);
    Ok(PreparedSkillUse {
        player,
        result: SkillUseResult {
            skill_id,
            skill_level: level,
            hp_spent,
            mp_spent,
            hp_restored,
            has_damage: damage.is_some(),
            minimum_damage: damage.map_or(0, |range| range.0),
            maximum_damage: damage.map_or(0, |range| range.1),
            speed_bonus,
            jump_bonus,
            duration_ms: u64::from(duration_seconds).saturating_mul(1_000),
            mp_restored,
            fixed_damage: fixed_damage.is_some(),
            weapon_attack_bonus,
            magic_attack_bonus,
            weapon_defense_bonus,
            magic_defense_bonus,
            accuracy_bonus,
            avoidability_bonus,
            permanent_buff: duration == SkillDuration::Permanent,
            critical_chance_bonus,
            critical_damage_bonus,
            strength_bonus,
            hp_recovery_per_five_seconds: periodic_hp_recovery,
            outgoing_damage_percent,
            enemy_speed_penalty,
            enemy_slow_duration_ms: u64::from(enemy_slow_duration).saturating_mul(1_000),
            enemy_slow_chance,
        },
        cooldown_ms: u64::from(cooldown_seconds).saturating_mul(1_000),
        attack_type,
        consumes_combo: required_combo > 0,
    })
}

fn combo_requirement(skill_id: u32) -> u32 {
    match skill_id {
        21_100_004 | 21_100_005 => 30,
        21_110_004 => 100,
        21_120_006 | 21_120_007 => 200,
        _ => 0,
    }
}

fn percent_of(
    amount: u32,
    percent: u32,
) -> u32 {
    u64::from(amount)
        .saturating_mul(u64::from(percent))
        .checked_div(100)
        .unwrap_or_default()
        .min(u64::from(u32::MAX)) as u32
}

fn calculate_formula_damage(
    player: &PlayerState,
    formulas: &FormulaCatalog,
    skill_id: u32,
    skill_level: u32,
    skill_damage: Option<u32>,
    effects: ProjectedEffects,
) -> Result<Option<(u32, u32)>, SkillRuleError> {
    let Some(skill_damage) = skill_damage.filter(|damage| *damage > 0) else {
        return Ok(None);
    };
    let stats = player.stats.as_ref().ok_or(SkillRuleError::MissingStats)?;
    let attack_type = skill_attack_type(stats.job_id);
    let profile = formulas
        .skill_profile(skill_id)
        .or_else(|| match attack_type {
            SkillAttackType::Magical => formulas.weapon_profile("wand"),
            SkillAttackType::Physical if (500..600).contains(&stats.job_id) => {
                formulas.weapon_profile("bare_hands")
            }
            SkillAttackType::Physical => None,
        });
    let Some(profile) = profile else {
        return Ok(None);
    };
    let bare_hands = formulas
        .weapon_profile("bare_hands")
        .expect("bare-hands weapon profile is validated during startup");
    let base_attack = evaluate_profile_property(
        bare_hands,
        "attack",
        &[("CharacterLevel", f64::from(player.level))],
    )
    .map_err(formula_error)?;
    let attack_bonus = projected_skill_attack_bonus(attack_type, effects);
    let outgoing_attack = base_attack + f64::from(attack_bonus);
    let mut variables = vec![
        ("CharacterLevel", f64::from(player.level)),
        ("PlayerLevel", f64::from(player.level)),
        (
            "Strength",
            f64::from(stats.strength) + f64::from(effects.modifiers.strength),
        ),
        ("Dexterity", f64::from(stats.dexterity)),
        ("Intelligence", f64::from(stats.intelligence)),
        ("Luck", f64::from(stats.luck)),
        ("SkillDamage", f64::from(skill_damage)),
        ("SkillLevel", f64::from(skill_level)),
        ("WeaponAttack", outgoing_attack),
        ("SpellAttack", outgoing_attack),
        ("Magic", f64::from(stats.intelligence)),
        ("Mastery", mastery_ratio(effects.modifiers.mastery)),
    ];
    if (500..600).contains(&stats.job_id) {
        variables.push(("JobMultiplier", if stats.job_id == 500 { 3.0 } else { 4.2 }));
    }
    let range = evaluate_damage_profile(profile, &variables).map_err(formula_error)?;
    let modifier = f64::from(skill_damage) / 100.0
        * (1.0 + f64::from(effects.modifiers.outgoing_damage_percent) / 100.0);
    let minimum = final_damage(range.minimum * modifier);
    let maximum = final_damage(range.maximum * modifier);
    if minimum > maximum {
        return Err(SkillRuleError::Formula {
            message: format!(
                "formula profile {:?} produced minimum {minimum}, above maximum {maximum}",
                profile.name()
            ),
        });
    }
    Ok(Some((minimum, maximum)))
}

fn projected_skill_attack_bonus(
    attack_type: SkillAttackType,
    effects: ProjectedEffects,
) -> i32 {
    match attack_type {
        SkillAttackType::Physical => effects.modifiers.weapon_attack,
        SkillAttackType::Magical => effects.modifiers.magic_attack,
    }
}

fn final_damage(value: f64) -> u32 {
    value.trunc().clamp(1.0, f64::from(MAX_DAMAGE)) as u32
}

fn formula_error(error: impl ToString) -> SkillRuleError {
    SkillRuleError::Formula {
        message: error.to_string(),
    }
}

pub fn reserve_skill_cooldown(
    cooldowns: &Arc<SkillCooldowns>,
    player_id: &str,
    skill_id: u32,
    now_ms: u64,
    cooldown_ms: u64,
) -> Result<Option<SkillCooldownReservation>, SkillRuleError> {
    if cooldown_ms == 0 {
        return Ok(None);
    }
    let mut deadlines = cooldowns
        .deadlines
        .lock()
        .map_err(|_| SkillRuleError::CooldownStore)?;
    deadlines.retain(|_, deadline| *deadline > now_ms);
    let key = (player_id.to_owned(), skill_id);
    if let Some(deadline) = deadlines.get(&key) {
        return Err(SkillRuleError::Cooldown {
            skill_id,
            remaining_ms: deadline.saturating_sub(now_ms),
        });
    }
    let deadline_ms = now_ms.saturating_add(cooldown_ms);
    deadlines.insert(key, deadline_ms);
    Ok(Some(SkillCooldownReservation {
        cooldowns: cooldowns.clone(),
        player_id: player_id.to_owned(),
        skill_id,
        deadline_ms,
        pending: true,
    }))
}

pub struct SkillCooldownReservation {
    cooldowns: Arc<SkillCooldowns>,
    player_id: String,
    skill_id: u32,
    deadline_ms: u64,
    pending: bool,
}

impl std::fmt::Debug for SkillCooldownReservation {
    fn fmt(
        &self,
        formatter: &mut std::fmt::Formatter<'_>,
    ) -> std::fmt::Result {
        formatter
            .debug_struct("SkillCooldownReservation")
            .field("player_id", &self.player_id)
            .field("skill_id", &self.skill_id)
            .field("deadline_ms", &self.deadline_ms)
            .field("pending", &self.pending)
            .finish()
    }
}

impl PartialEq for SkillCooldownReservation {
    fn eq(
        &self,
        other: &Self,
    ) -> bool {
        Arc::ptr_eq(&self.cooldowns, &other.cooldowns)
            && self.player_id == other.player_id
            && self.skill_id == other.skill_id
            && self.deadline_ms == other.deadline_ms
            && self.pending == other.pending
    }
}

impl Eq for SkillCooldownReservation {}

pub fn release_skill_cooldown(
    reservation: &mut SkillCooldownReservation
) -> Result<(), SkillRuleError> {
    let mut deadlines = reservation
        .cooldowns
        .deadlines
        .lock()
        .map_err(|_| SkillRuleError::CooldownStore)?;
    let key = (reservation.player_id.clone(), reservation.skill_id);
    if deadlines.get(&key) == Some(&reservation.deadline_ms) {
        deadlines.remove(&key);
    }
    reservation.pending = false;
    Ok(())
}

pub fn commit_skill_cooldown(reservation: &mut SkillCooldownReservation) {
    reservation.pending = false;
}

impl Drop for SkillCooldownReservation {
    fn drop(&mut self) {
        if !self.pending {
            return;
        }
        let mut deadlines = self
            .cooldowns
            .deadlines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let key = (self.player_id.clone(), self.skill_id);
        if deadlines.get(&key) == Some(&self.deadline_ms) {
            deadlines.remove(&key);
        }
    }
}

fn validate_job(
    book: &SkillBook,
    player: &PlayerState,
) -> Result<(), SkillRuleError> {
    let job_id = player
        .stats
        .as_ref()
        .ok_or(SkillRuleError::MissingStats)?
        .job_id;
    if book.job_id != job_id {
        return Err(SkillRuleError::JobMismatch {
            book_job_id: book.job_id,
            player_job_id: job_id,
        });
    }
    Ok(())
}

fn validate_learned_skills(
    job_id: u32,
    authoritative: &[AuthoritativeSkillDefinition],
    learned: &[LearnedSkill],
) -> Result<HashMap<u32, (u32, u32)>, SkillRuleError> {
    let definitions = authoritative
        .iter()
        .map(|skill| (skill.definition.skill_id, skill))
        .collect::<HashMap<_, _>>();
    let mut levels = HashMap::new();
    for skill in learned {
        if levels
            .insert(skill.skill_id, (skill.level, skill.master_level))
            .is_some()
        {
            return Err(SkillRuleError::DuplicateLearnedSkill {
                skill_id: skill.skill_id,
            });
        }
        let authoritative =
            definitions
                .get(&skill.skill_id)
                .ok_or(SkillRuleError::UnknownSkill {
                    skill_id: skill.skill_id,
                    job_id,
                })?;
        let definition = &authoritative.definition;
        // Some WZ quests use an invisible zero-maximum skill as an acquisition
        // marker. Its authored level 1 persists for quest checks, but exclusion
        // from the display book prevents allocation, binding, and use.
        let marker = authoritative.invisible
            && definition.max_level == 0
            && skill.level == 1
            && skill.master_level == 0;
        if !marker
            && (skill.level > definition.max_level || (skill.level == 0 && skill.master_level == 0))
        {
            return Err(SkillRuleError::InvalidLearnedLevel {
                skill_id: skill.skill_id,
                level: skill.level,
                maximum: definition.max_level,
            });
        }
        if skill.master_level > definition.max_level {
            return Err(SkillRuleError::InvalidMasterLevel {
                skill_id: skill.skill_id,
                master_level: skill.master_level,
                maximum: definition.max_level,
            });
        }
    }
    Ok(levels)
}

fn skill_definition(
    book: &SkillBook,
    skill_id: u32,
) -> Result<&SkillDefinition, SkillRuleError> {
    book.skills
        .iter()
        .filter_map(|skill| skill.definition.as_ref())
        .find(|definition| definition.skill_id == skill_id)
        .ok_or(SkillRuleError::UnknownSkill {
            skill_id,
            job_id: book.job_id,
        })
}

fn skill_activation(definition: &SkillDefinition) -> SkillActivation {
    SkillActivation::try_from(definition.activation)
        .ok()
        .filter(|activation| *activation != SkillActivation::Unspecified)
        .unwrap_or(SkillActivation::Active)
}

fn is_active_skill(definition: &SkillDefinition) -> bool {
    skill_activation(definition) == SkillActivation::Active
}

fn numeric_stat(
    skill_id: u32,
    property: &'static str,
    level_value: Option<&SkillValue>,
    common_value: Option<&SkillValue>,
) -> Result<u32, SkillRuleError> {
    optional_numeric_stat(skill_id, property, level_value, common_value)
        .map(Option::unwrap_or_default)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SkillDuration {
    None,
    Timed(u32),
    Permanent,
}

fn skill_duration(
    skill_id: u32,
    property: &'static str,
    level_value: Option<&SkillValue>,
    common_value: Option<&SkillValue>,
) -> Result<SkillDuration, SkillRuleError> {
    let Some(value) = level_value.or(common_value) else {
        return Ok(SkillDuration::None);
    };
    let number =
        skill_value_number(value).ok_or(SkillRuleError::InvalidProperty { skill_id, property })?;
    if number == -1.0 {
        return Ok(SkillDuration::Permanent);
    }
    if !number.is_finite() || number < 0.0 || number > f64::from(u32::MAX) {
        return Err(SkillRuleError::InvalidProperty { skill_id, property });
    }
    Ok(SkillDuration::Timed(number.trunc() as u32))
}

fn optional_numeric_stat(
    skill_id: u32,
    property: &'static str,
    level_value: Option<&SkillValue>,
    common_value: Option<&SkillValue>,
) -> Result<Option<u32>, SkillRuleError> {
    let Some(value) = level_value.or(common_value) else {
        return Ok(None);
    };
    let number =
        skill_value_number(value).ok_or(SkillRuleError::InvalidProperty { skill_id, property })?;
    if !number.is_finite() || number < 0.0 || number > f64::from(u32::MAX) {
        return Err(SkillRuleError::InvalidProperty { skill_id, property });
    }
    Ok(Some(number.trunc() as u32))
}

fn signed_numeric_stat(
    skill_id: u32,
    property: &'static str,
    level_value: Option<&SkillValue>,
    common_value: Option<&SkillValue>,
) -> Result<i32, SkillRuleError> {
    let Some(value) = level_value.or(common_value) else {
        return Ok(0);
    };
    let number =
        skill_value_number(value).ok_or(SkillRuleError::InvalidProperty { skill_id, property })?;
    if !number.is_finite() || number < f64::from(i32::MIN) || number > f64::from(i32::MAX) {
        return Err(SkillRuleError::InvalidProperty { skill_id, property });
    }
    Ok(number.trunc() as i32)
}

fn skill_value_number(value: &SkillValue) -> Option<f64> {
    match value.value.as_ref()? {
        skill_value::Value::Integer(value) => Some(*value as f64),
        skill_value::Value::Decimal(value) => Some(*value),
        skill_value::Value::Text(value) => value.trim().parse().ok(),
        skill_value::Value::Vector(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use oozems_proto::v1::CharacterStats;
    use oozems_proto::v1::KeyAction;
    use oozems_proto::v1::KeyBinding;
    use oozems_proto::v1::PlayerSkill;
    use oozems_proto::v1::PlayerState;
    use oozems_proto::v1::SkillActivation as ProtoSkillActivation;
    use oozems_proto::v1::SkillBook;
    use oozems_proto::v1::SkillDefinition;
    use oozems_proto::v1::SkillLevelDefinition;
    use oozems_proto::v1::SkillStats;
    use oozems_proto::v1::SkillValue;
    use oozems_proto::v1::skill_value;

    use super::SkillCooldowns;
    use super::SkillDuration;
    use super::SkillRuleError;
    use super::allocate_skill_point;
    use super::combo_requirement;
    use super::commit_skill_cooldown;
    use super::learned_skill_modifiers;
    use super::personalize_skill_book;
    use super::prepare_skill_use;
    use super::projected_skill_attack_bonus;
    use super::prune_non_active_skill_bindings;
    use super::reserve_skill_cooldown;
    use super::skill_duration;
    use super::validate_bound_skills;
    use crate::effects::EffectModifiers;
    use crate::effects::ProjectedEffects;
    use crate::jobs::SkillAttackType;

    #[test]
    fn negative_one_duration_is_an_explicit_permanent_lifetime() {
        let value = SkillValue {
            value: Some(skill_value::Value::Integer(-1)),
        };

        assert_eq!(
            skill_duration(1_002, "time", Some(&value), None).expect("duration"),
            SkillDuration::Permanent
        );
    }

    #[test]
    fn other_negative_durations_are_rejected() {
        let value = SkillValue {
            value: Some(skill_value::Value::Integer(-2)),
        };

        assert_eq!(
            skill_duration(1_002, "time", Some(&value), None),
            Err(SkillRuleError::InvalidProperty {
                skill_id: 1_002,
                property: "time",
            })
        );
    }

    #[test]
    fn combo_consumers_require_and_reset_their_authored_thresholds() {
        assert_eq!(combo_requirement(21_100_004), 30);
        assert_eq!(combo_requirement(21_100_005), 30);
        assert_eq!(combo_requirement(21_110_004), 100);
        assert_eq!(combo_requirement(21_120_006), 200);
        assert_eq!(combo_requirement(21_120_007), 200);

        let skill_id = 21_100_004;
        let book = SkillBook {
            job_id: 0,
            skills: vec![PlayerSkill {
                definition: Some(SkillDefinition {
                    skill_id,
                    job_id: 0,
                    max_level: 1,
                    levels: vec![SkillLevelDefinition {
                        level: 1,
                        stats: Some(SkillStats {
                            fixed_damage: Some(integer(10)),
                            ..SkillStats::default()
                        }),
                        ..SkillLevelDefinition::default()
                    }],
                    ..SkillDefinition::default()
                }),
                level: 0,
                master_level: 0,
            }],
            ..SkillBook::default()
        };
        let mut player = player(0, 5);
        player.learned_skills.push(oozems_proto::v1::LearnedSkill {
            skill_id,
            level: 1,
            master_level: 0,
        });
        let effects = ProjectedEffects {
            combo_count: 29,
            ..ProjectedEffects::default()
        };
        assert_eq!(
            prepare_skill_use(
                player.clone(),
                &context(book.clone()),
                skill_id,
                &formulas(),
                effects,
            )
            .err()
            .expect("insufficient combo"),
            SkillRuleError::InsufficientCombo {
                skill_id,
                required: 30,
                available: 29,
            }
        );

        let prepared = prepare_skill_use(
            player,
            &context(book),
            skill_id,
            &formulas(),
            ProjectedEffects {
                combo_count: 30,
                ..ProjectedEffects::default()
            },
        )
        .expect("sufficient combo");
        assert!(prepared.consumes_combo);
    }

    #[test]
    fn allocation_consumes_a_point_and_updates_the_personalized_book() {
        let player = player(3, 5);
        let context = context(book());

        let player = allocate_skill_point(player, &context, 1_000).expect("allocate point");
        let personal = personalize_skill_book(context, &player).expect("personal book");

        assert_eq!(player.skill_points, 2);
        assert_eq!(player.learned_skills[0].level, 1);
        assert_eq!(personal.available_points, 2);
        assert_eq!(personal.skills[0].level, 1);
        assert_eq!(personal.skills[0].master_level, 0);
    }

    #[test]
    fn master_only_unlock_sets_the_player_allocation_cap() {
        let definition = SkillDefinition {
            skill_id: 2_321_003,
            job_id: 232,
            max_level: 30,
            ..SkillDefinition::default()
        };
        let context = crate::content::SkillBookContext {
            book: SkillBook {
                job_id: 232,
                skills: vec![PlayerSkill {
                    definition: Some(definition.clone()),
                    level: 0,
                    master_level: 0,
                }],
                ..SkillBook::default()
            },
            authoritative_skills: vec![crate::content::AuthoritativeSkillDefinition {
                definition,
                invisible: true,
            }],
        };
        let mut player = player(11, 5);
        player.stats.as_mut().expect("stats").job_id = 232;
        player.learned_skills.push(oozems_proto::v1::LearnedSkill {
            skill_id: 2_321_003,
            level: 0,
            master_level: 10,
        });

        for _ in 0..10 {
            player = allocate_skill_point(player, &context, 2_321_003).expect("mastered point");
        }
        assert_eq!(player.skill_points, 1);
        assert_eq!(player.learned_skills[0].level, 10);
        assert_eq!(player.learned_skills[0].master_level, 10);
        assert_eq!(
            allocate_skill_point(player, &context, 2_321_003),
            Err(SkillRuleError::MaximumLevel {
                skill_id: 2_321_003
            })
        );
    }

    #[test]
    fn zero_maximum_invisible_marker_validates_but_stays_non_usable() {
        let definition = SkillDefinition {
            skill_id: 9_999,
            job_id: 0,
            max_level: 0,
            ..SkillDefinition::default()
        };
        let context = crate::content::SkillBookContext {
            book: SkillBook {
                job_id: 0,
                ..SkillBook::default()
            },
            authoritative_skills: vec![crate::content::AuthoritativeSkillDefinition {
                definition,
                invisible: true,
            }],
        };
        let mut player = player(3, 5);
        player.learned_skills.push(oozems_proto::v1::LearnedSkill {
            skill_id: 9_999,
            level: 1,
            master_level: 0,
        });

        let personal = personalize_skill_book(context.clone(), &player).expect("marker record");
        assert!(personal.skills.is_empty());
        assert_eq!(
            learned_skill_modifiers(&context, &player).expect("marker modifiers"),
            super::LearnedSkillModifiers::default()
        );
        assert_eq!(
            allocate_skill_point(player.clone(), &context, 9_999),
            Err(SkillRuleError::UnknownSkill {
                skill_id: 9_999,
                job_id: 0
            })
        );
        let binding = [KeyBinding {
            code: "KeyA".to_owned(),
            action: KeyAction::Unspecified as i32,
            skill_id: 9_999,
        }];
        assert_eq!(
            validate_bound_skills(&binding, &player, &context),
            Err(SkillRuleError::NotLearned { skill_id: 9_999 })
        );
    }

    #[test]
    fn legacy_passive_bindings_are_pruned_before_strict_validation() {
        let skill_id = 1_000;
        let definition = SkillDefinition {
            skill_id,
            job_id: 0,
            max_level: 1,
            activation: ProtoSkillActivation::Passive as i32,
            levels: vec![SkillLevelDefinition {
                level: 1,
                stats: Some(SkillStats::default()),
                ..SkillLevelDefinition::default()
            }],
            ..SkillDefinition::default()
        };
        let context = crate::content::SkillBookContext {
            book: SkillBook {
                job_id: 0,
                skills: vec![PlayerSkill {
                    definition: Some(definition.clone()),
                    level: 0,
                    master_level: 0,
                }],
                ..SkillBook::default()
            },
            authoritative_skills: vec![crate::content::AuthoritativeSkillDefinition {
                definition,
                invisible: false,
            }],
        };
        let mut player = player(3, 5);
        player.learned_skills.push(oozems_proto::v1::LearnedSkill {
            skill_id,
            level: 1,
            master_level: 0,
        });
        player.key_bindings = vec![KeyBinding {
            code: "KeyA".to_owned(),
            action: KeyAction::Unspecified as i32,
            skill_id,
        }];

        assert!(prune_non_active_skill_bindings(
            &mut player.key_bindings,
            &context
        ));
        assert!(player.key_bindings.is_empty());
        validate_bound_skills(&player.key_bindings, &player, &context).expect("migrated bindings");
    }

    #[test]
    fn use_spends_mp_and_reports_fixed_damage() {
        let mut player = player(0, 5);
        player.learned_skills.push(oozems_proto::v1::LearnedSkill {
            skill_id: 1_000,
            level: 1,
            master_level: 0,
        });

        let formulas = formulas();
        let prepared = prepare_skill_use(
            player,
            &context(book()),
            1_000,
            &formulas,
            ProjectedEffects::default(),
        )
        .expect("use skill");

        let stats = prepared.player.stats.expect("stats");
        assert_eq!(stats.mp, 2);
        assert_eq!(prepared.result.mp_spent, 3);
        assert_eq!(prepared.result.minimum_damage, 10);
        assert_eq!(prepared.result.maximum_damage, 10);
        assert!(prepared.result.has_damage);
    }

    #[test]
    fn typed_periodic_hp_recovery_creates_a_timed_effect_without_healing_immediately() {
        let mut player = player(0, 5);
        player.stats.as_mut().expect("stats").hp = 1;
        player.learned_skills.push(oozems_proto::v1::LearnedSkill {
            skill_id: 1_000,
            level: 1,
            master_level: 0,
        });
        let mut book = book();
        let stats = book.skills[0]
            .definition
            .as_mut()
            .expect("skill definition")
            .levels[0]
            .stats
            .as_mut()
            .expect("level stats");
        stats.hp_recovery_per_five_seconds = Some(integer(4));
        stats.duration = Some(integer(30));

        let prepared = prepare_skill_use(
            player,
            &context(book),
            1_000,
            &formulas(),
            ProjectedEffects::default(),
        )
        .expect("use periodic recovery skill");

        assert_eq!(prepared.result.hp_restored, 0);
        assert_eq!(prepared.result.hp_recovery_per_five_seconds, 4);
        assert_eq!(prepared.player.stats.expect("stats").hp, 1);
    }

    #[test]
    fn permanent_wz_duration_reaches_the_prepared_skill_result() {
        let mut player = player(0, 5);
        player.learned_skills.push(oozems_proto::v1::LearnedSkill {
            skill_id: 1_000,
            level: 1,
            master_level: 0,
        });
        let mut book = book();
        book.skills[0]
            .definition
            .as_mut()
            .expect("skill definition")
            .levels[0]
            .stats
            .as_mut()
            .expect("level stats")
            .duration = Some(integer(-1));

        let prepared = prepare_skill_use(
            player,
            &context(book),
            1_000,
            &formulas(),
            ProjectedEffects::default(),
        )
        .expect("use permanent skill");

        assert!(prepared.result.permanent_buff);
        assert_eq!(prepared.result.duration_ms, 0);
    }

    #[test]
    fn configured_profile_controls_damage_for_a_mapped_skill() {
        let mut player = player(0, 5);
        player.level = 10;
        player.stats = Some(CharacterStats {
            job_id: 400,
            hp: 50,
            max_hp: 50,
            mp: 5,
            max_mp: 5,
            luck: 20,
            ..CharacterStats::default()
        });
        player.learned_skills.push(oozems_proto::v1::LearnedSkill {
            skill_id: 4_001_334,
            level: 1,
            master_level: 0,
        });
        let book = SkillBook {
            job_id: 400,
            skills: vec![PlayerSkill {
                definition: Some(SkillDefinition {
                    skill_id: 4_001_334,
                    job_id: 400,
                    name: "Double Stab".to_owned(),
                    max_level: 20,
                    levels: vec![SkillLevelDefinition {
                        level: 1,
                        stats: Some(SkillStats {
                            damage: Some(integer(100)),
                            ..SkillStats::default()
                        }),
                        ..SkillLevelDefinition::default()
                    }],
                    ..SkillDefinition::default()
                }),
                level: 1,
                master_level: 0,
            }],
            ..SkillBook::default()
        };

        let prepared = prepare_skill_use(
            player,
            &context(book),
            4_001_334,
            &mapped_formulas(),
            ProjectedEffects::default(),
        )
        .expect("use mapped skill");

        assert_eq!(prepared.result.minimum_damage, 8);
        assert_eq!(prepared.result.maximum_damage, 17);
    }

    #[test]
    fn projected_attack_effects_follow_the_typed_skill_damage_path() {
        let effects = ProjectedEffects {
            modifiers: EffectModifiers {
                weapon_attack: 7,
                magic_attack: 11,
                ..EffectModifiers::default()
            },
            ..ProjectedEffects::default()
        };

        assert_eq!(
            projected_skill_attack_bonus(SkillAttackType::Physical, effects),
            7
        );
        assert_eq!(
            projected_skill_attack_bonus(SkillAttackType::Magical, effects),
            11
        );

        let only_magic = ProjectedEffects {
            modifiers: EffectModifiers {
                magic_attack: 11,
                ..EffectModifiers::default()
            },
            ..ProjectedEffects::default()
        };
        assert_eq!(
            projected_skill_attack_bonus(SkillAttackType::Physical, only_magic),
            0
        );
        let only_weapon = ProjectedEffects {
            modifiers: EffectModifiers {
                weapon_attack: 7,
                ..EffectModifiers::default()
            },
            ..ProjectedEffects::default()
        };
        assert_eq!(
            projected_skill_attack_bonus(SkillAttackType::Magical, only_weapon),
            0
        );
    }

    #[test]
    fn insufficient_resources_do_not_change_the_player() {
        let mut player = player(0, 2);
        player.learned_skills.push(oozems_proto::v1::LearnedSkill {
            skill_id: 1_000,
            level: 1,
            master_level: 0,
        });

        assert_eq!(
            prepare_skill_use(
                player,
                &context(book()),
                1_000,
                &formulas(),
                ProjectedEffects::default(),
            )
            .err()
            .expect("insufficient MP"),
            SkillRuleError::InsufficientMana {
                skill_id: 1_000,
                required: 3,
                available: 2,
            }
        );
    }

    #[test]
    fn cooldown_reservations_are_atomic_and_expire() {
        let cooldowns = Arc::new(SkillCooldowns::default());

        let mut first = reserve_skill_cooldown(&cooldowns, "player", 1_000, 1_000, 5_000)
            .expect("first reservation")
            .expect("nonzero cooldown reservation");
        commit_skill_cooldown(&mut first);
        assert_eq!(
            reserve_skill_cooldown(&cooldowns, "player", 1_000, 2_000, 5_000),
            Err(SkillRuleError::Cooldown {
                skill_id: 1_000,
                remaining_ms: 4_000,
            })
        );
        let mut replacement = reserve_skill_cooldown(&cooldowns, "player", 1_000, 6_000, 5_000)
            .expect("expired reservation")
            .expect("nonzero replacement reservation");
        commit_skill_cooldown(&mut replacement);
    }

    #[test]
    fn dropping_a_pending_skill_reservation_releases_it() {
        let cooldowns = Arc::new(SkillCooldowns::default());
        let reservation = reserve_skill_cooldown(&cooldowns, "player", 1_000, 1_000, 5_000)
            .expect("reserve cooldown")
            .expect("nonzero cooldown reservation");

        drop(reservation);

        reserve_skill_cooldown(&cooldowns, "player", 1_000, 1_001, 5_000)
            .expect("retry after downstream failure");
    }

    #[test]
    fn a_stale_skill_reservation_cannot_release_a_new_deadline() {
        let cooldowns = Arc::new(SkillCooldowns::default());
        let stale = reserve_skill_cooldown(&cooldowns, "player", 1_000, 1_000, 5_000)
            .expect("first reservation")
            .expect("nonzero cooldown reservation");
        let mut replacement = reserve_skill_cooldown(&cooldowns, "player", 1_000, 6_000, 5_000)
            .expect("replacement reservation")
            .expect("nonzero replacement reservation");
        commit_skill_cooldown(&mut replacement);

        drop(stale);

        assert_eq!(
            reserve_skill_cooldown(&cooldowns, "player", 1_000, 6_001, 5_000),
            Err(SkillRuleError::Cooldown {
                skill_id: 1_000,
                remaining_ms: 4_999,
            })
        );
    }

    #[test]
    fn only_learned_skills_can_be_bound() {
        let binding = [KeyBinding {
            code: "KeyA".to_owned(),
            action: KeyAction::Unspecified as i32,
            skill_id: 1_000,
        }];
        let player = player(0, 5);
        let context = context(book());
        assert_eq!(
            validate_bound_skills(&binding, &player, &context),
            Err(SkillRuleError::NotLearned { skill_id: 1_000 })
        );
        let mut player = player;
        player.learned_skills.push(oozems_proto::v1::LearnedSkill {
            skill_id: 1_000,
            level: 1,
            master_level: 0,
        });
        validate_bound_skills(&binding, &player, &context).expect("learned skill binding");
    }

    fn player(
        skill_points: u32,
        mp: u32,
    ) -> PlayerState {
        PlayerState {
            id: "player".to_owned(),
            stats: Some(CharacterStats {
                job_id: 0,
                hp: 50,
                max_hp: 50,
                mp,
                max_mp: 5,
                ..CharacterStats::default()
            }),
            skill_points,
            ..PlayerState::default()
        }
    }

    fn book() -> SkillBook {
        SkillBook {
            job_id: 0,
            name: "Beginner's Basics".to_owned(),
            skills: vec![PlayerSkill {
                definition: Some(SkillDefinition {
                    skill_id: 1_000,
                    job_id: 0,
                    name: "Three Snails".to_owned(),
                    max_level: 3,
                    levels: vec![SkillLevelDefinition {
                        level: 1,
                        stats: Some(SkillStats {
                            mp_cost: Some(integer(3)),
                            fixed_damage: Some(integer(10)),
                            ..SkillStats::default()
                        }),
                        ..SkillLevelDefinition::default()
                    }],
                    ..SkillDefinition::default()
                }),
                level: 0,
                master_level: 0,
            }],
            ..SkillBook::default()
        }
    }

    fn context(book: SkillBook) -> crate::content::SkillBookContext {
        let authoritative_skills = book
            .skills
            .iter()
            .filter_map(|skill| skill.definition.clone())
            .map(|definition| crate::content::AuthoritativeSkillDefinition {
                definition,
                invisible: false,
            })
            .collect();
        crate::content::SkillBookContext {
            book,
            authoritative_skills,
        }
    }

    fn integer(value: i64) -> SkillValue {
        SkillValue {
            value: Some(skill_value::Value::Integer(value)),
        }
    }

    fn formulas() -> crate::skill_formula::FormulaCatalog {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../config/skill-formulas.toml");
        crate::skill_formula::FormulaCatalog::load(&path).expect("default formulas")
    }

    fn mapped_formulas() -> crate::skill_formula::FormulaCatalog {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("skill-formulas.toml");
        std::fs::write(
            &path,
            r#"
source_url = "https://example.test/formulas"

[weapon_profiles.unarmed]
attack = "min(floor((2 * CharacterLevel + 31) / 3), 31)"
minimum = "1"
maximum = "1"

[weapons.bare_hands]
profile = "unarmed"

[skill_profiles.lucky_seven]
minimum = "Luck * 2.5 * WeaponAttack / 100"
maximum = "Luck * 5.0 * WeaponAttack / 100"

[skills."4001334"]
profile = "lucky_seven"
"#,
        )
        .expect("write formula configuration");
        crate::skill_formula::FormulaCatalog::load(&path).expect("mapped formulas")
    }
}
