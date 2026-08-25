use std::collections::HashMap;
use std::sync::Mutex;

use oozems_proto::v1::PlayerState;
use thiserror::Error;

use crate::skill_formula::FormulaCatalog;
use crate::skill_formula::FormulaEvaluationError;
use crate::skill_formula::evaluate_profile_property;

pub const RECOVERY_INTERVAL_MS: u64 = 10_000;

const IMPROVED_MP_RECOVERY_SKILL_ID: u32 = 2_000_000;

#[derive(Default)]
pub struct RecoveryTimers {
    deadlines: Mutex<HashMap<String, u64>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecoveryReservation {
    Ready(RecoveryToken),
    Waiting { remaining_ms: u64 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecoveryToken {
    player_id: String,
    previous_deadline_ms: u64,
    deadline_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecoveryActivityRollback {
    player_id: String,
    previous_deadline_ms: Option<u64>,
    deadline_ms: u64,
}

pub struct PreparedRecovery {
    pub player: PlayerState,
    pub hp_restored: u32,
    pub mp_restored: u32,
}

#[derive(Debug, Error)]
pub enum RecoveryError {
    #[error("the player does not have character stats")]
    MissingStats,
    #[error("no recovery formula is selected for {identifier:?}")]
    MissingProfile { identifier: &'static str },
    #[error("configured recovery formula failed")]
    Formula(#[from] FormulaEvaluationError),
    #[error("recovery formula {profile}.{property} returned invalid amount {value}")]
    InvalidAmount {
        profile: String,
        property: &'static str,
        value: f64,
    },
    #[error("the recovery timer store is unavailable")]
    TimerStore,
    #[error("the recovery reservation changed before it could be restored")]
    ReservationChanged,
}

pub fn prepare_recovery(
    mut player: PlayerState,
    formulas: &FormulaCatalog,
) -> Result<PreparedRecovery, RecoveryError> {
    let stats = player.stats.as_ref().ok_or(RecoveryError::MissingStats)?;
    let identifier = recovery_identifier(stats.job_id);
    let profile = formulas
        .recovery_profile(identifier)
        .or_else(|| formulas.recovery_profile("base"))
        .ok_or(RecoveryError::MissingProfile { identifier })?;
    let skill_level = player
        .learned_skills
        .iter()
        .find(|skill| skill.skill_id == IMPROVED_MP_RECOVERY_SKILL_ID)
        .map_or(0, |skill| skill.level);
    let variables = [
        ("CharacterLevel", f64::from(player.level)),
        ("SkillLevel", f64::from(skill_level)),
    ];
    let hp = evaluate_recovery_amount(profile, "hp", &variables)?;
    let mp = evaluate_recovery_amount(profile, "mp", &variables)?;

    let stats = player.stats.as_mut().ok_or(RecoveryError::MissingStats)?;
    let hp_before = stats.hp;
    stats.hp = stats.hp.saturating_add(hp).min(stats.max_hp);
    let mp_before = stats.mp;
    stats.mp = stats.mp.saturating_add(mp).min(stats.max_mp);
    Ok(PreparedRecovery {
        hp_restored: stats.hp - hp_before,
        mp_restored: stats.mp - mp_before,
        player,
    })
}

pub fn reserve_recovery(
    timers: &RecoveryTimers,
    player_id: &str,
    now_ms: u64,
) -> Result<RecoveryReservation, RecoveryError> {
    let mut deadlines = timers
        .deadlines
        .lock()
        .map_err(|_| RecoveryError::TimerStore)?;
    let Some(deadline_ms) = deadlines.get(player_id).copied() else {
        deadlines.insert(
            player_id.to_owned(),
            now_ms.saturating_add(RECOVERY_INTERVAL_MS),
        );
        return Ok(RecoveryReservation::Waiting {
            remaining_ms: RECOVERY_INTERVAL_MS,
        });
    };
    if deadline_ms > now_ms {
        return Ok(RecoveryReservation::Waiting {
            remaining_ms: deadline_ms - now_ms,
        });
    }
    let next_deadline_ms = now_ms.saturating_add(RECOVERY_INTERVAL_MS);
    deadlines.insert(player_id.to_owned(), next_deadline_ms);
    Ok(RecoveryReservation::Ready(RecoveryToken {
        player_id: player_id.to_owned(),
        previous_deadline_ms: deadline_ms,
        deadline_ms: next_deadline_ms,
    }))
}

pub fn delay_recovery_after_activity(
    timers: &RecoveryTimers,
    player_id: &str,
    now_ms: u64,
) -> Result<RecoveryActivityRollback, RecoveryError> {
    let deadline_ms = now_ms.saturating_add(RECOVERY_INTERVAL_MS);
    let previous_deadline_ms = timers
        .deadlines
        .lock()
        .map_err(|_| RecoveryError::TimerStore)?
        .insert(player_id.to_owned(), deadline_ms);
    Ok(RecoveryActivityRollback {
        player_id: player_id.to_owned(),
        previous_deadline_ms,
        deadline_ms,
    })
}

pub fn release_recovery(
    timers: &RecoveryTimers,
    reservation: &RecoveryToken,
) -> Result<(), RecoveryError> {
    let mut deadlines = timers
        .deadlines
        .lock()
        .map_err(|_| RecoveryError::TimerStore)?;
    if deadlines.get(&reservation.player_id) == Some(&reservation.deadline_ms) {
        deadlines.insert(
            reservation.player_id.clone(),
            reservation.previous_deadline_ms,
        );
    }
    Ok(())
}

pub fn restore_recovery_activity(
    timers: &RecoveryTimers,
    rollback: &RecoveryActivityRollback,
) -> Result<(), RecoveryError> {
    let mut deadlines = timers
        .deadlines
        .lock()
        .map_err(|_| RecoveryError::TimerStore)?;
    if deadlines.get(&rollback.player_id) != Some(&rollback.deadline_ms) {
        return Err(RecoveryError::ReservationChanged);
    }
    match rollback.previous_deadline_ms {
        Some(deadline_ms) => {
            deadlines.insert(rollback.player_id.clone(), deadline_ms);
        }
        None => {
            deadlines.remove(&rollback.player_id);
        }
    }
    Ok(())
}

fn recovery_identifier(job_id: u32) -> &'static str {
    if (200..300).contains(&job_id) {
        "mage"
    } else {
        "base"
    }
}

fn evaluate_recovery_amount(
    profile: &crate::skill_formula::FormulaProfile,
    property: &'static str,
    variables: &[(&str, f64)],
) -> Result<u32, RecoveryError> {
    let value = evaluate_profile_property(profile, property, variables)?;
    if value < 0.0 || value > f64::from(u32::MAX) {
        return Err(RecoveryError::InvalidAmount {
            profile: profile.name().to_owned(),
            property,
            value,
        });
    }
    Ok(value.trunc() as u32)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use oozems_proto::v1::CharacterStats;
    use oozems_proto::v1::LearnedSkill;
    use oozems_proto::v1::PlayerState;

    use super::IMPROVED_MP_RECOVERY_SKILL_ID;
    use super::RECOVERY_INTERVAL_MS;
    use super::RecoveryReservation;
    use super::RecoveryTimers;
    use super::delay_recovery_after_activity;
    use super::prepare_recovery;
    use super::reserve_recovery;
    use crate::skill_formula::FormulaCatalog;

    #[test]
    fn base_profile_restores_configured_hp_and_mp() {
        let recovered =
            prepare_recovery(player(0, 10, 20, 1, 2), &formulas()).expect("base recovery");

        assert_eq!(recovered.hp_restored, 10);
        assert_eq!(recovered.mp_restored, 3);
        let stats = recovered.player.stats.expect("stats");
        assert_eq!((stats.hp, stats.mp), (11, 5));
    }

    #[test]
    fn mage_profile_receives_character_and_skill_levels() {
        let mut player = player(200, 20, 100, 90, 20);
        player.learned_skills.push(LearnedSkill {
            skill_id: IMPROVED_MP_RECOVERY_SKILL_ID,
            level: 5,
            master_level: 0,
        });

        let recovered = prepare_recovery(player, &formulas()).expect("mage recovery");

        assert_eq!(recovered.hp_restored, 10);
        assert_eq!(recovered.mp_restored, 10);
    }

    #[test]
    fn timers_enforce_the_interval_and_activity_restarts_it() {
        let timers = RecoveryTimers::default();

        assert_eq!(
            reserve_recovery(&timers, "player", 1_000).expect("first tick"),
            RecoveryReservation::Waiting {
                remaining_ms: RECOVERY_INTERVAL_MS
            }
        );
        assert_eq!(
            reserve_recovery(&timers, "player", 2_000).expect("early tick"),
            RecoveryReservation::Waiting {
                remaining_ms: 9_000
            }
        );
        assert_eq!(
            reserve_recovery(&timers, "player", 11_000).expect("eligible tick"),
            RecoveryReservation::Ready(super::RecoveryToken {
                player_id: "player".to_owned(),
                previous_deadline_ms: 11_000,
                deadline_ms: 11_000 + RECOVERY_INTERVAL_MS,
            })
        );
        delay_recovery_after_activity(&timers, "player", 18_000).expect("activity");
        assert_eq!(
            reserve_recovery(&timers, "player", 21_000).expect("delayed tick"),
            RecoveryReservation::Waiting {
                remaining_ms: 7_000
            }
        );
    }

    fn player(
        job_id: u32,
        level: u32,
        maximum: u32,
        hp: u32,
        mp: u32,
    ) -> PlayerState {
        PlayerState {
            id: "player".to_owned(),
            level,
            stats: Some(CharacterStats {
                job_id,
                hp,
                max_hp: maximum,
                mp,
                max_mp: maximum,
                ..CharacterStats::default()
            }),
            ..PlayerState::default()
        }
    }

    fn formulas() -> FormulaCatalog {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/skill-formulas.toml");
        FormulaCatalog::load(&path).expect("default formulas")
    }
}
