use std::collections::HashMap;
use std::sync::Arc;
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

pub struct BasicAttackReservation {
    cooldowns: Arc<BasicAttackCooldowns>,
    player_id: String,
    deadline_ms: u64,
    pending: bool,
}

impl std::fmt::Debug for BasicAttackReservation {
    fn fmt(
        &self,
        formatter: &mut std::fmt::Formatter<'_>,
    ) -> std::fmt::Result {
        formatter
            .debug_struct("BasicAttackReservation")
            .field("player_id", &self.player_id)
            .field("deadline_ms", &self.deadline_ms)
            .field("pending", &self.pending)
            .finish()
    }
}

impl PartialEq for BasicAttackReservation {
    fn eq(
        &self,
        other: &Self,
    ) -> bool {
        Arc::ptr_eq(&self.cooldowns, &other.cooldowns)
            && self.player_id == other.player_id
            && self.deadline_ms == other.deadline_ms
            && self.pending == other.pending
    }
}

impl Eq for BasicAttackReservation {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DamageRange {
    pub minimum: u32,
    pub maximum: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AttackReach {
    pub horizontal: f32,
    pub top: f32,
    pub bottom: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VerticalBounds {
    pub top: f32,
    pub bottom: f32,
}

pub const DEFAULT_TARGET_VERTICAL_BOUNDS: VerticalBounds = VerticalBounds {
    top: -48.0,
    bottom: 0.0,
};
pub const POINT_VERTICAL_BOUNDS: VerticalBounds = VerticalBounds {
    top: 0.0,
    bottom: 0.0,
};

pub fn vertical_attack_intersects(
    attacker_y: f32,
    attack: AttackReach,
    target_y: f32,
    target: VerticalBounds,
) -> bool {
    attacker_y + attack.top <= target_y + target.bottom
        && attacker_y + attack.bottom >= target_y + target.top
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
    strength_bonus: i32,
    mastery: f64,
    weapon: Option<crate::jobs::WeaponType>,
    outgoing_damage_percent: u32,
) -> Result<DamageRange, AttackRuleError> {
    let stats = player.stats.as_ref().ok_or(AttackRuleError::MissingStats)?;
    let profile_name = weapon.map_or("bare_hands", |weapon| weapon.profile_name);
    let profile = formulas
        .weapon_profile(profile_name)
        .expect("weapon profiles are validated during startup");
    let weapon_attack = if weapon.is_none() {
        evaluate_profile_property(
            profile,
            "attack",
            &[("CharacterLevel", f64::from(player.level))],
        )
        .map_err(formula_error)?
            + f64::from(weapon_attack_bonus)
    } else {
        f64::from(weapon_attack_bonus)
    };
    let (primary_stat, secondary_stat) = weapon_formula_stats(
        profile,
        weapon.map(|weapon| weapon.family),
        stats.strength.saturating_add_signed(strength_bonus),
        stats.dexterity,
        stats.luck,
    )?;
    let variables = [
        ("CharacterLevel", f64::from(player.level)),
        ("PlayerLevel", f64::from(player.level)),
        (
            "Strength",
            f64::from(stats.strength) + f64::from(strength_bonus),
        ),
        ("Dexterity", f64::from(stats.dexterity)),
        ("Intelligence", f64::from(stats.intelligence)),
        ("Luck", f64::from(stats.luck)),
        ("WeaponAttack", weapon_attack),
        ("JobMultiplier", basic_attack_job_multiplier(stats.job_id)),
        ("Mastery", mastery),
        ("PrimaryStat", primary_stat),
        ("SecondaryStat", secondary_stat),
    ];
    let range = evaluate_damage_profile(profile, &variables).map_err(formula_error)?;
    let outgoing_multiplier = 1.0 + f64::from(outgoing_damage_percent) / 100.0;
    let minimum = final_damage(range.minimum * outgoing_multiplier);
    let maximum = final_damage(range.maximum * outgoing_multiplier);
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

fn weapon_formula_stats(
    profile: &crate::skill_formula::FormulaProfile,
    family: Option<crate::jobs::WeaponFamily>,
    strength: u32,
    dexterity: u32,
    luck: u32,
) -> Result<(f64, f64), AttackRuleError> {
    use crate::jobs::WeaponFamily;

    let Some(family) = family else {
        return Ok((0.0, 0.0));
    };
    let (primary, secondary) = match family {
        WeaponFamily::Sword
        | WeaponFamily::Axe
        | WeaponFamily::BluntWeapon
        | WeaponFamily::Wand
        | WeaponFamily::Staff => (f64::from(strength), f64::from(dexterity)),
        WeaponFamily::Spear | WeaponFamily::Polearm => (f64::from(strength), f64::from(dexterity)),
        WeaponFamily::Dagger => (
            f64::from(luck),
            f64::from(strength.saturating_add(dexterity)),
        ),
        WeaponFamily::Bow | WeaponFamily::Crossbow | WeaponFamily::Gun => {
            (f64::from(dexterity), f64::from(strength))
        }
        WeaponFamily::Knuckle => (f64::from(strength), f64::from(dexterity)),
        WeaponFamily::Claw => return Ok((0.0, 0.0)),
    };
    // Character content renders basic weapon attacks with swingO1.
    let modifier = evaluate_profile_property(profile, "primary_modifier", &[])
        .or_else(|_| evaluate_profile_property(profile, "swing_modifier", &[]))
        .map_err(formula_error)?;
    Ok((primary * modifier, secondary))
}

pub fn basic_attack_interval(
    configured: Duration,
    animation: Option<Duration>,
) -> Duration {
    animation.map_or(configured, |animation| configured.max(animation))
}

pub fn reserve_basic_attack(
    cooldowns: &Arc<BasicAttackCooldowns>,
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
        cooldowns: cooldowns.clone(),
        player_id: player_id.to_owned(),
        deadline_ms,
        pending: true,
    })
}

pub fn release_basic_attack(
    reservation: &mut BasicAttackReservation
) -> Result<(), AttackRuleError> {
    let mut deadlines = reservation
        .cooldowns
        .deadlines
        .lock()
        .map_err(|_| AttackRuleError::CooldownStore)?;
    if deadlines.get(&reservation.player_id) == Some(&reservation.deadline_ms) {
        deadlines.remove(&reservation.player_id);
    }
    reservation.pending = false;
    Ok(())
}

pub fn commit_basic_attack(reservation: &mut BasicAttackReservation) {
    reservation.pending = false;
}

impl Drop for BasicAttackReservation {
    fn drop(&mut self) {
        if !self.pending {
            return;
        }
        let mut deadlines = self
            .cooldowns
            .deadlines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if deadlines.get(&self.player_id) == Some(&self.deadline_ms) {
            deadlines.remove(&self.player_id);
        }
    }
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
    use std::sync::Arc;

    use oozems_proto::v1::CharacterStats;
    use oozems_proto::v1::PlayerState;

    use super::AttackReach;
    use super::AttackRuleError;
    use super::BasicAttackCooldowns;
    use super::DamageRange;
    use super::VerticalBounds;
    use super::basic_attack_interval;
    use super::calculate_basic_attack;
    use super::commit_basic_attack;
    use super::reserve_basic_attack;
    use super::vertical_attack_intersects;

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
            calculate_basic_attack(&player, &formulas(), 0, 0, 0.1, None, 0)
                .expect("basic attack damage"),
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

        let bare_hands = calculate_basic_attack(&player, &formulas, 0, 0, 0.1, None, 0)
            .expect("bare-hands damage");
        let weapon = calculate_basic_attack(
            &player,
            &formulas,
            17,
            0,
            0.1,
            crate::jobs::weapon_type(1_302_000),
            0,
        )
        .expect("weapon damage");

        assert!(weapon.minimum >= bare_hands.minimum);
        assert!(weapon.maximum > bare_hands.maximum);
    }

    #[test]
    fn every_supported_weapon_family_has_a_usable_basic_attack_profile() {
        let player = PlayerState {
            level: 30,
            stats: Some(CharacterStats {
                strength: 40,
                dexterity: 40,
                intelligence: 40,
                luck: 40,
                ..CharacterStats::default()
            }),
            ..PlayerState::default()
        };
        let formulas = formulas();

        for item_id in [
            1_302_000, 1_312_000, 1_322_000, 1_332_000, 1_372_000, 1_382_000, 1_402_000, 1_412_000,
            1_422_000, 1_432_000, 1_442_000, 1_452_000, 1_462_000, 1_472_000, 1_482_000, 1_492_000,
        ] {
            let weapon = crate::jobs::weapon_type(item_id).expect("supported weapon type");
            let damage = calculate_basic_attack(&player, &formulas, 20, 0, 0.1, Some(weapon), 0)
                .unwrap_or_else(|error| panic!("weapon {item_id} has no usable profile: {error}"));
            assert!(damage.maximum > 1, "weapon {item_id} has trivial damage");
        }
    }

    #[test]
    fn authored_attack_intersects_fake_target_body_at_inclusive_edges() {
        let attack = AttackReach {
            horizontal: 88.0,
            top: -62.0,
            bottom: -6.0,
        };
        let target = VerticalBounds {
            top: -48.0,
            bottom: 0.0,
        };

        assert!(vertical_attack_intersects(100.0, attack, 38.0, target));
        assert!(!vertical_attack_intersects(100.0, attack, 37.0, target));
        assert!(vertical_attack_intersects(100.0, attack, 142.0, target));
        assert!(!vertical_attack_intersects(100.0, attack, 143.0, target));
    }

    #[test]
    fn basic_attack_cooldown_is_atomic_and_expires() {
        let cooldowns = Arc::new(BasicAttackCooldowns::default());
        let interval = std::time::Duration::from_millis(600);

        let deadline =
            reserve_basic_attack(&cooldowns, "player", 1_000, interval).expect("first attack");
        assert_eq!(
            reserve_basic_attack(&cooldowns, "player", 1_100, interval),
            Err(AttackRuleError::Cooldown { remaining_ms: 500 })
        );
        let mut current_deadline =
            reserve_basic_attack(&cooldowns, "player", 1_600, interval).expect("expired cooldown");
        commit_basic_attack(&mut current_deadline);

        drop(deadline);
        assert_eq!(
            reserve_basic_attack(&cooldowns, "player", 1_601, interval),
            Err(AttackRuleError::Cooldown { remaining_ms: 599 })
        );
    }

    #[test]
    fn dropping_a_pending_basic_attack_releases_it() {
        let cooldowns = Arc::new(BasicAttackCooldowns::default());
        let interval = std::time::Duration::from_millis(600);

        let pending =
            reserve_basic_attack(&cooldowns, "player", 1_000, interval).expect("first attack");
        drop(pending);

        reserve_basic_attack(&cooldowns, "player", 1_001, interval)
            .expect("retry after cancellation");
    }

    #[test]
    fn animation_duration_sets_the_minimum_attack_interval() {
        let configured = std::time::Duration::from_millis(600);

        assert_eq!(
            basic_attack_interval(configured, Some(std::time::Duration::from_millis(800))),
            std::time::Duration::from_millis(800)
        );
        assert_eq!(
            basic_attack_interval(configured, Some(std::time::Duration::from_millis(400))),
            configured
        );
        assert_eq!(basic_attack_interval(configured, None), configured);
    }

    fn formulas() -> crate::skill_formula::FormulaCatalog {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../config/skill-formulas.toml");
        crate::skill_formula::FormulaCatalog::load(&path).expect("default formulas")
    }
}
