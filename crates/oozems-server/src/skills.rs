use std::collections::HashMap;
use std::sync::Mutex;

use oozems_proto::v1::KeyBinding;
use oozems_proto::v1::LearnedSkill;
use oozems_proto::v1::PlayerState;
use oozems_proto::v1::SkillBook;
use oozems_proto::v1::SkillDefinition;
use oozems_proto::v1::SkillUseResult;
use oozems_proto::v1::SkillValue;
use oozems_proto::v1::skill_value;
use thiserror::Error;

use crate::content::AuthoritativeSkillDefinition;
use crate::content::SkillBookContext;
use crate::effects::ProjectedEffects;
use crate::jobs::SkillAttackType;
use crate::jobs::skill_attack_type;
use crate::skill_formula::FormulaCatalog;
use crate::skill_formula::evaluate_damage_profile;
use crate::skill_formula::evaluate_profile_property;

const MAX_DAMAGE: u32 = 99_999;
const BEGINNER_RECOVERY_SKILL_ID: u32 = 1_001;

#[derive(Default)]
pub struct SkillCooldowns {
    deadlines: Mutex<HashMap<(String, u32), u64>>,
}

pub struct PreparedSkillUse {
    pub player: PlayerState,
    pub result: SkillUseResult,
    pub cooldown_ms: u64,
    pub attack_type: SkillAttackType,
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
            || skill_definition(&context.book, skill_id).is_err()
        {
            return Err(SkillRuleError::NotLearned { skill_id });
        }
    }
    Ok(())
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
    let mut hp_recovery = numeric_stat(
        skill_id,
        "hp",
        level_stats.hp.as_ref(),
        common_stats.and_then(|stats| stats.hp.as_ref()),
    )?;
    let mp_recovery = numeric_stat(
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
    let duration_seconds = numeric_stat(
        skill_id,
        "time",
        level_stats.duration.as_ref(),
        common_stats.and_then(|stats| stats.duration.as_ref()),
    )?;
    if skill_id == BEGINNER_RECOVERY_SKILL_ID && hp_recovery == 0 {
        let recovery_per_tick = numeric_stat(
            skill_id,
            "x",
            level_stats.x.as_ref(),
            common_stats.and_then(|stats| stats.x.as_ref()),
        )?;
        hp_recovery = beginner_recovery_amount(recovery_per_tick, duration_seconds);
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
        },
        cooldown_ms: u64::from(cooldown_seconds).saturating_mul(1_000),
        attack_type,
    })
}

fn beginner_recovery_amount(
    recovery_per_tick: u32,
    duration_seconds: u32,
) -> u32 {
    recovery_per_tick.saturating_mul((duration_seconds / 5).max(1))
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
        ("Strength", f64::from(stats.strength)),
        ("Dexterity", f64::from(stats.dexterity)),
        ("Intelligence", f64::from(stats.intelligence)),
        ("Luck", f64::from(stats.luck)),
        ("SkillDamage", f64::from(skill_damage)),
        ("SkillLevel", f64::from(skill_level)),
        ("WeaponAttack", outgoing_attack),
        ("SpellAttack", outgoing_attack),
        ("Magic", f64::from(stats.intelligence)),
        ("Mastery", 0.1),
    ];
    if (500..600).contains(&stats.job_id) {
        variables.push(("JobMultiplier", if stats.job_id == 500 { 3.0 } else { 4.2 }));
    }
    let range = evaluate_damage_profile(profile, &variables).map_err(formula_error)?;
    let modifier = f64::from(skill_damage) / 100.0;
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
    cooldowns: &SkillCooldowns,
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
        player_id: player_id.to_owned(),
        skill_id,
        deadline_ms,
    }))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillCooldownReservation {
    player_id: String,
    skill_id: u32,
    deadline_ms: u64,
}

pub fn release_skill_cooldown(
    cooldowns: &SkillCooldowns,
    reservation: &SkillCooldownReservation,
) -> Result<(), SkillRuleError> {
    let mut deadlines = cooldowns
        .deadlines
        .lock()
        .map_err(|_| SkillRuleError::CooldownStore)?;
    let key = (reservation.player_id.clone(), reservation.skill_id);
    if deadlines.get(&key) == Some(&reservation.deadline_ms) {
        deadlines.remove(&key);
    }
    Ok(())
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

fn numeric_stat(
    skill_id: u32,
    property: &'static str,
    level_value: Option<&SkillValue>,
    common_value: Option<&SkillValue>,
) -> Result<u32, SkillRuleError> {
    optional_numeric_stat(skill_id, property, level_value, common_value)
        .map(Option::unwrap_or_default)
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
    use oozems_proto::v1::CharacterStats;
    use oozems_proto::v1::KeyAction;
    use oozems_proto::v1::KeyBinding;
    use oozems_proto::v1::PlayerSkill;
    use oozems_proto::v1::PlayerState;
    use oozems_proto::v1::SkillBook;
    use oozems_proto::v1::SkillDefinition;
    use oozems_proto::v1::SkillLevelDefinition;
    use oozems_proto::v1::SkillStats;
    use oozems_proto::v1::SkillValue;
    use oozems_proto::v1::skill_value;

    use super::SkillCooldowns;
    use super::SkillRuleError;
    use super::allocate_skill_point;
    use super::beginner_recovery_amount;
    use super::personalize_skill_book;
    use super::prepare_skill_use;
    use super::projected_skill_attack_bonus;
    use super::release_skill_cooldown;
    use super::reserve_skill_cooldown;
    use super::validate_bound_skills;
    use crate::effects::EffectModifiers;
    use crate::effects::ProjectedEffects;
    use crate::jobs::SkillAttackType;

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
        let cooldowns = SkillCooldowns::default();

        reserve_skill_cooldown(&cooldowns, "player", 1_000, 1_000, 5_000)
            .expect("first reservation");
        assert_eq!(
            reserve_skill_cooldown(&cooldowns, "player", 1_000, 2_000, 5_000),
            Err(SkillRuleError::Cooldown {
                skill_id: 1_000,
                remaining_ms: 4_000,
            })
        );
        reserve_skill_cooldown(&cooldowns, "player", 1_000, 6_000, 5_000)
            .expect("expired reservation");
    }

    #[test]
    fn a_failed_skill_transaction_can_release_its_reservation() {
        let cooldowns = SkillCooldowns::default();
        let reservation = reserve_skill_cooldown(&cooldowns, "player", 1_000, 1_000, 5_000)
            .expect("reserve cooldown")
            .expect("nonzero cooldown reservation");

        release_skill_cooldown(&cooldowns, &reservation).expect("release cooldown");

        reserve_skill_cooldown(&cooldowns, "player", 1_000, 1_001, 5_000)
            .expect("retry after downstream failure");
    }

    #[test]
    fn a_stale_skill_reservation_cannot_release_a_new_deadline() {
        let cooldowns = SkillCooldowns::default();
        let stale = reserve_skill_cooldown(&cooldowns, "player", 1_000, 1_000, 5_000)
            .expect("first reservation")
            .expect("nonzero cooldown reservation");
        reserve_skill_cooldown(&cooldowns, "player", 1_000, 6_000, 5_000)
            .expect("replacement reservation");

        release_skill_cooldown(&cooldowns, &stale).expect("release stale reservation");

        assert_eq!(
            reserve_skill_cooldown(&cooldowns, "player", 1_000, 6_001, 5_000),
            Err(SkillRuleError::Cooldown {
                skill_id: 1_000,
                remaining_ms: 4_999,
            })
        );
    }

    #[test]
    fn beginner_recovery_converts_five_second_ticks_to_the_wz_total() {
        assert_eq!(beginner_recovery_amount(4, 30), 24);
        assert_eq!(beginner_recovery_amount(8, 30), 48);
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
