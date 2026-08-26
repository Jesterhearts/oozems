use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use oozems_proto::v1::PlayerState;
use thiserror::Error;

use crate::skill_formula::FormulaCatalog;
use crate::skill_formula::evaluate_damage_profile;
use crate::skill_formula::evaluate_profile_property;

const MAX_DAMAGE: u32 = 99_999;

#[derive(Default)]
pub struct BasicAttackCooldowns {
    deadlines: Mutex<HashMap<String, u64>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BasicAttackReservation {
    player_id: String,
    deadline_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DamageRange {
    pub minimum: u32,
    pub maximum: u32,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum AttackRuleError {
    #[error("the player does not have character stats")]
    MissingStats,
    #[error("basic attack is on cooldown for another {remaining_ms} ms")]
    Cooldown { remaining_ms: u64 },
    #[error("the basic attack cooldown store is unavailable")]
    CooldownStore,
    #[error("a configured formula failed: {message}")]
    Formula { message: String },
}

pub fn calculate_basic_attack(
    player: &PlayerState,
    formulas: &FormulaCatalog,
    weapon_attack_bonus: i32,
) -> Result<DamageRange, AttackRuleError> {
    let stats = player.stats.as_ref().ok_or(AttackRuleError::MissingStats)?;
    let profile = formulas
        .weapon_profile("bare_hands")
        .expect("bare-hands weapon profile is validated during startup");
    let weapon_attack = evaluate_profile_property(
        profile,
        "attack",
        &[("CharacterLevel", f64::from(player.level))],
    )
    .map_err(formula_error)?
        + f64::from(weapon_attack_bonus);
    let variables = [
        ("CharacterLevel", f64::from(player.level)),
        ("PlayerLevel", f64::from(player.level)),
        ("Strength", f64::from(stats.strength)),
        ("Dexterity", f64::from(stats.dexterity)),
        ("Intelligence", f64::from(stats.intelligence)),
        ("Luck", f64::from(stats.luck)),
        ("WeaponAttack", weapon_attack),
        ("JobMultiplier", basic_attack_job_multiplier(stats.job_id)),
    ];
    let range = evaluate_damage_profile(profile, &variables).map_err(formula_error)?;
    let minimum = final_damage(range.minimum);
    let maximum = final_damage(range.maximum);
    if minimum > maximum {
        return Err(AttackRuleError::Formula {
            message: format!(
                "formula profile {:?} produced minimum {minimum}, above maximum {maximum}",
                profile.name()
            ),
        });
    }
    Ok(DamageRange { minimum, maximum })
}

pub fn reserve_basic_attack(
    cooldowns: &BasicAttackCooldowns,
    player_id: &str,
    now_ms: u64,
    interval: Duration,
) -> Result<BasicAttackReservation, AttackRuleError> {
    let mut deadlines = cooldowns
        .deadlines
        .lock()
        .map_err(|_| AttackRuleError::CooldownStore)?;
    deadlines.retain(|_, deadline| *deadline > now_ms);
    if let Some(deadline) = deadlines.get(player_id) {
        return Err(AttackRuleError::Cooldown {
            remaining_ms: deadline.saturating_sub(now_ms),
        });
    }
    let interval_ms = u64::try_from(interval.as_millis())
        .unwrap_or(u64::MAX)
        .max(1);
    let deadline_ms = now_ms.saturating_add(interval_ms);
    deadlines.insert(player_id.to_owned(), deadline_ms);
    Ok(BasicAttackReservation {
        player_id: player_id.to_owned(),
        deadline_ms,
    })
}

pub fn release_basic_attack(
    cooldowns: &BasicAttackCooldowns,
    reservation: &BasicAttackReservation,
) -> Result<(), AttackRuleError> {
    let mut deadlines = cooldowns
        .deadlines
        .lock()
        .map_err(|_| AttackRuleError::CooldownStore)?;
    if deadlines.get(&reservation.player_id) == Some(&reservation.deadline_ms) {
        deadlines.remove(&reservation.player_id);
    }
    Ok(())
}

fn basic_attack_job_multiplier(job_id: u32) -> f64 {
    if job_id == 500 {
        3.0
    } else if (500..600).contains(&job_id) {
        4.2
    } else {
        4.0
    }
}

fn final_damage(value: f64) -> u32 {
    value.trunc().clamp(1.0, f64::from(MAX_DAMAGE)) as u32
}

fn formula_error(error: impl ToString) -> AttackRuleError {
    AttackRuleError::Formula {
        message: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use oozems_proto::v1::CharacterStats;
    use oozems_proto::v1::PlayerState;

    use super::AttackRuleError;
    use super::BasicAttackCooldowns;
    use super::DamageRange;
    use super::calculate_basic_attack;
    use super::release_basic_attack;
    use super::reserve_basic_attack;

    #[test]
    fn starter_basic_attack_uses_the_bare_hands_profile() {
        let player = PlayerState {
            level: 1,
            stats: Some(CharacterStats {
                job_id: 0,
                strength: 12,
                dexterity: 5,
                intelligence: 4,
                luck: 4,
                ..CharacterStats::default()
            }),
            ..PlayerState::default()
        };

        assert_eq!(
            calculate_basic_attack(&player, &formulas(), 0).expect("basic attack damage"),
            DamageRange {
                minimum: 1,
                maximum: 5,
            }
        );
    }

    #[test]
    fn equipped_weapon_attack_increases_basic_attack_damage() {
        let player = PlayerState {
            level: 1,
            stats: Some(CharacterStats {
                job_id: 0,
                strength: 12,
                dexterity: 5,
                intelligence: 4,
                luck: 4,
                ..CharacterStats::default()
            }),
            ..PlayerState::default()
        };
        let formulas = formulas();

        let bare_hands = calculate_basic_attack(&player, &formulas, 0).expect("bare-hands damage");
        let weapon = calculate_basic_attack(&player, &formulas, 17).expect("weapon damage");

        assert!(weapon.minimum > bare_hands.minimum);
        assert!(weapon.maximum > bare_hands.maximum);
    }

    #[test]
    fn basic_attack_cooldown_is_atomic_and_expires() {
        let cooldowns = BasicAttackCooldowns::default();
        let interval = std::time::Duration::from_millis(600);

        let deadline =
            reserve_basic_attack(&cooldowns, "player", 1_000, interval).expect("first attack");
        assert_eq!(
            reserve_basic_attack(&cooldowns, "player", 1_100, interval),
            Err(AttackRuleError::Cooldown { remaining_ms: 500 })
        );
        let current_deadline =
            reserve_basic_attack(&cooldowns, "player", 1_600, interval).expect("expired cooldown");

        release_basic_attack(&cooldowns, &current_deadline).expect("release current cooldown");
        reserve_basic_attack(&cooldowns, "player", 1_601, interval).expect("released cooldown");
        release_basic_attack(&cooldowns, &deadline).expect("ignore stale release");
        assert_eq!(
            reserve_basic_attack(&cooldowns, "player", 1_602, interval),
            Err(AttackRuleError::Cooldown { remaining_ms: 599 })
        );
    }

    fn formulas() -> crate::skill_formula::FormulaCatalog {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../config/skill-formulas.toml");
        crate::skill_formula::FormulaCatalog::load(&path).expect("default formulas")
    }
}
