use std::ops::Deref;

use oozems_proto::v1::ActiveBuff;
use oozems_proto::v1::ActiveBuffState;
use oozems_proto::v1::active_buff;

use crate::movement::PlayerInput;

#[derive(Clone)]
pub(crate) struct TrackedBuff {
    buff: ActiveBuff,
    key: BuffKey,
    lifetime: BuffLifetime,
    attacks_disabled: bool,
}

#[derive(Clone, Copy)]
pub(crate) struct TrackedCombo {
    count: u32,
    expires_at_local_ms: f64,
}

impl TrackedCombo {
    pub fn count(self) -> u32 {
        self.count
    }

    pub fn remaining_ms(
        self,
        now_local_ms: f64,
    ) -> u64 {
        (self.expires_at_local_ms - now_local_ms).max(0.0) as u64
    }
}

#[derive(Clone, Copy)]
enum BuffLifetime {
    Timed { expires_at_local_ms: f64 },
    Permanent,
}

impl TrackedBuff {
    pub fn key(&self) -> BuffKey {
        self.key
    }

    pub fn remaining_ms(
        &self,
        now_local_ms: f64,
    ) -> Option<u64> {
        match self.lifetime {
            BuffLifetime::Timed {
                expires_at_local_ms,
            } => Some((expires_at_local_ms - now_local_ms).max(0.0) as u64),
            BuffLifetime::Permanent => None,
        }
    }

    pub fn is_permanent(&self) -> bool {
        matches!(self.lifetime, BuffLifetime::Permanent)
    }
}

impl Deref for TrackedBuff {
    type Target = ActiveBuff;

    fn deref(&self) -> &Self::Target {
        &self.buff
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum BuffKey {
    Skill(u32),
    Item(u32),
}

#[derive(Default)]
pub(crate) struct TrackedBuffs {
    pub buffs: Vec<TrackedBuff>,
    pub combo: Option<TrackedCombo>,
    pub weapon_attack: i32,
    pub magic_attack: i32,
    pub weapon_defense: i32,
    pub magic_defense: i32,
    pub accuracy: i32,
    pub avoidability: i32,
    pub speed: i32,
    pub jump: i32,
    pub morph_id: Option<u32>,
    pub attacks_disabled: bool,
    revision: u64,
    observed_at_unix_ms: u64,
    installed: bool,
}

impl TrackedBuffs {
    pub fn has_periodic_hp_recovery(&self) -> bool {
        self.buffs
            .iter()
            .any(|buff| buff.hp_recovery_per_five_seconds > 0)
    }
}

pub(super) struct ValidatedState(ActiveBuffState);

pub(super) fn validate_state(state: ActiveBuffState) -> Result<ValidatedState, String> {
    for buff in &state.buffs {
        source_key(buff)?;
    }
    if (state.combo_count > 0) != (state.combo_expires_at_unix_ms > 0) {
        return Err("active combo must contain both a count and an expiration".to_owned());
    }
    Ok(ValidatedState(state))
}

pub(super) fn from_state(
    state: ActiveBuffState,
    now_local_ms: f64,
    response_transit_ms: f64,
) -> Result<TrackedBuffs, String> {
    let mut tracked = TrackedBuffs::default();
    install(
        &mut tracked,
        validate_state(state)?,
        now_local_ms,
        response_transit_ms,
    );
    Ok(tracked)
}

pub(super) fn install(
    current: &mut TrackedBuffs,
    received: ValidatedState,
    now_local_ms: f64,
    response_transit_ms: f64,
) {
    let received = received.0;
    let received_version = (received.observed_at_unix_ms, received.revision);
    let current_version = (current.observed_at_unix_ms, current.revision);
    if current.installed && received_version <= current_version {
        return;
    }
    let observed_at_unix_ms = received.observed_at_unix_ms;
    let response_transit_ms = response_transit_ms.max(0.0).ceil() as u64;
    let projected_morph_id = (received.morph_id > 0).then_some(received.morph_id);
    let attacks_disabled = received.attacks_disabled;
    let combo = (received.combo_count > 0)
        .then(|| {
            let remaining_ms = effect_remaining_ms(
                received.combo_expires_at_unix_ms,
                observed_at_unix_ms,
                response_transit_ms,
            );
            (remaining_ms > 0).then_some(TrackedCombo {
                count: received.combo_count,
                expires_at_local_ms: now_local_ms + remaining_ms as f64,
            })
        })
        .flatten();
    let mut buffs = received
        .buffs
        .into_iter()
        .filter_map(|buff| {
            let key = source_key(&buff).expect("validated buff source");
            let lifetime = if buff.permanent {
                BuffLifetime::Permanent
            } else {
                let remaining_ms = effect_remaining_ms(
                    buff.expires_at_unix_ms,
                    observed_at_unix_ms,
                    response_transit_ms,
                );
                (remaining_ms > 0).then_some(BuffLifetime::Timed {
                    expires_at_local_ms: now_local_ms + remaining_ms as f64,
                })?
            };
            Some(TrackedBuff {
                attacks_disabled: attacks_disabled
                    && projected_morph_id.is_some_and(|morph_id| buff.morph_id == morph_id),
                buff,
                key,
                lifetime,
            })
        })
        .collect::<Vec<_>>();
    buffs.sort_by_key(TrackedBuff::key);
    current.buffs = buffs;
    current.combo = combo;
    current.revision = received.revision;
    current.observed_at_unix_ms = observed_at_unix_ms;
    current.installed = true;
    project(current);
}

fn effect_remaining_ms(
    expires_at_unix_ms: u64,
    observed_at_unix_ms: u64,
    response_transit_ms: u64,
) -> u64 {
    expires_at_unix_ms
        .saturating_sub(observed_at_unix_ms)
        .saturating_sub(response_transit_ms)
}

pub(super) fn apply(
    active: &mut TrackedBuffs,
    input: &mut PlayerInput,
    now_local_ms: f64,
) {
    active.buffs.retain(|buff| match buff.lifetime {
        BuffLifetime::Timed {
            expires_at_local_ms,
        } => expires_at_local_ms > now_local_ms,
        BuffLifetime::Permanent => true,
    });
    if active
        .combo
        .is_some_and(|combo| combo.expires_at_local_ms <= now_local_ms)
    {
        active.combo = None;
    }
    project(active);
    input.speed_bonus = active.speed;
    input.jump_bonus = active.jump;
}

pub(super) fn item_source_ids(active: &TrackedBuffs) -> impl Iterator<Item = u32> + '_ {
    active.buffs.iter().filter_map(|buff| match buff.source {
        Some(active_buff::Source::ItemId(item_id)) => Some(item_id),
        Some(active_buff::Source::SkillId(_)) | None => None,
    })
}

fn source_key(buff: &ActiveBuff) -> Result<BuffKey, String> {
    match buff.source {
        Some(active_buff::Source::SkillId(skill_id)) if skill_id > 0 => {
            Ok(BuffKey::Skill(skill_id))
        }
        Some(active_buff::Source::ItemId(item_id)) if item_id > 0 => Ok(BuffKey::Item(item_id)),
        Some(active_buff::Source::SkillId(_)) => {
            Err("active buff has an invalid skill source ID".to_owned())
        }
        Some(active_buff::Source::ItemId(_)) => {
            Err("active buff has an invalid item source ID".to_owned())
        }
        None => Err("active buff does not contain a source".to_owned()),
    }
}

fn project(active: &mut TrackedBuffs) {
    let strongest = |select: fn(&ActiveBuff) -> i32| {
        active
            .buffs
            .iter()
            .map(|buff| select(buff))
            .filter(|value| *value != 0)
            .max()
            .unwrap_or_default()
    };
    active.weapon_attack = strongest(|buff| buff.weapon_attack);
    active.magic_attack = strongest(|buff| buff.magic_attack);
    active.weapon_defense = strongest(|buff| buff.weapon_defense);
    active.magic_defense = strongest(|buff| buff.magic_defense);
    active.accuracy = strongest(|buff| buff.accuracy);
    active.avoidability = strongest(|buff| buff.avoidability);
    active.speed = strongest(|buff| buff.speed_bonus);
    active.jump = strongest(|buff| buff.jump_bonus);
    active.morph_id = active
        .buffs
        .iter()
        .find_map(|buff| (buff.morph_id > 0).then_some(buff.morph_id));
    active.attacks_disabled = active.buffs.iter().any(|buff| buff.attacks_disabled);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buff(
        skill_id: u32,
        speed_bonus: i32,
        jump_bonus: i32,
        expires: u64,
    ) -> ActiveBuff {
        ActiveBuff {
            source: Some(active_buff::Source::SkillId(skill_id)),
            speed_bonus,
            jump_bonus,
            expires_at_unix_ms: expires,
            ..ActiveBuff::default()
        }
    }

    fn tracked(
        buff: ActiveBuff,
        deadline: f64,
    ) -> TrackedBuff {
        let key = source_key(&buff).expect("tracked test buff source");
        TrackedBuff {
            buff,
            key,
            lifetime: BuffLifetime::Timed {
                expires_at_local_ms: deadline,
            },
            attacks_disabled: false,
        }
    }

    #[test]
    fn install_converts_server_relative_duration_to_a_local_deadline() {
        let mut active = TrackedBuffs::default();
        install(
            &mut active,
            validate_state(ActiveBuffState {
                buffs: vec![buff(2, 1, 1, 1_300), buff(1, 1, 1, 900)],
                revision: 1,
                observed_at_unix_ms: 1_000,
                ..ActiveBuffState::default()
            })
            .expect("valid buff state"),
            40.0,
            0.0,
        );

        assert_eq!(active.buffs.len(), 1);
        assert_eq!(active.buffs[0].key(), BuffKey::Skill(2));
        assert_eq!(active.buffs[0].remaining_ms(40.0), Some(300));
        assert_eq!(active.buffs[0].remaining_ms(140.0), Some(200));
    }

    #[test]
    fn item_effects_are_retained_for_buff_icon_rendering() {
        let active = from_state(
            ActiveBuffState {
                buffs: vec![ActiveBuff {
                    source: Some(active_buff::Source::ItemId(2_022_253)),
                    jump_bonus: 3,
                    expires_at_unix_ms: 181_000,
                    ..ActiveBuff::default()
                }],
                revision: 1,
                observed_at_unix_ms: 1_000,
                jump: 3,
                ..ActiveBuffState::default()
            },
            40.0,
            0.0,
        )
        .expect("item buff state");

        assert_eq!(active.buffs.len(), 1);
        assert_eq!(active.buffs[0].key(), BuffKey::Item(2_022_253));
        assert_eq!(active.buffs[0].jump_bonus, 3);
        assert_eq!(active.buffs[0].remaining_ms(40.0), Some(180_000));
        assert_eq!(item_source_ids(&active).collect::<Vec<_>>(), [2_022_253]);
    }

    #[test]
    fn permanent_buffs_have_no_deadline_and_survive_local_pruning() {
        let mut active = from_state(
            ActiveBuffState {
                buffs: vec![ActiveBuff {
                    source: Some(active_buff::Source::SkillId(1_002)),
                    speed_bonus: 20,
                    permanent: true,
                    ..ActiveBuff::default()
                }],
                revision: 1,
                observed_at_unix_ms: 1_000,
                ..ActiveBuffState::default()
            },
            40.0,
            0.0,
        )
        .expect("permanent buff state");
        let mut input = PlayerInput::default();

        apply(&mut active, &mut input, f64::MAX);

        assert_eq!(active.buffs.len(), 1);
        assert!(active.buffs[0].is_permanent());
        assert_eq!(active.buffs[0].remaining_ms(f64::MAX), None);
        assert_eq!(input.speed_bonus, 20);
    }

    #[test]
    fn buffs_without_a_source_are_rejected() {
        let error = from_state(
            ActiveBuffState {
                buffs: vec![ActiveBuff {
                    speed_bonus: 20,
                    expires_at_unix_ms: 1_300,
                    ..ActiveBuff::default()
                }],
                revision: 1,
                observed_at_unix_ms: 1_000,
                ..ActiveBuffState::default()
            },
            40.0,
            0.0,
        )
        .err()
        .expect("missing source must fail");

        assert_eq!(error, "active buff does not contain a source");
    }

    #[test]
    fn equal_or_stale_snapshots_do_not_extend_local_deadlines() {
        let state = ActiveBuffState {
            buffs: vec![buff(2, 1, 1, 1_300)],
            revision: 2,
            observed_at_unix_ms: 1_000,
            ..ActiveBuffState::default()
        };
        let mut active = from_state(state.clone(), 40.0, 0.0).expect("valid buff state");

        install(
            &mut active,
            validate_state(state).expect("valid buff state"),
            140.0,
            0.0,
        );

        assert_eq!(active.buffs[0].remaining_ms(140.0), Some(200));
    }

    #[test]
    fn combo_uses_the_authoritative_count_and_local_expiration() {
        let mut active = from_state(
            ActiveBuffState {
                combo_count: 12,
                combo_expires_at_unix_ms: 4_000,
                revision: 1,
                observed_at_unix_ms: 1_000,
                ..ActiveBuffState::default()
            },
            100.0,
            250.0,
        )
        .expect("combo state");
        let combo = active.combo.expect("tracked combo");
        assert_eq!(combo.count(), 12);
        assert_eq!(combo.remaining_ms(100.0), 2_750);

        let mut input = PlayerInput::default();
        apply(&mut active, &mut input, 2_849.0);
        assert!(active.combo.is_some());
        apply(&mut active, &mut input, 2_850.0);
        assert!(active.combo.is_none());
    }

    #[test]
    fn newer_zero_combo_clears_and_stale_snapshots_cannot_restore_it() {
        let combo = ActiveBuffState {
            combo_count: 12,
            combo_expires_at_unix_ms: 4_000,
            revision: 1,
            observed_at_unix_ms: 1_000,
            ..ActiveBuffState::default()
        };
        let mut active = from_state(combo.clone(), 100.0, 0.0).expect("combo state");
        install(
            &mut active,
            validate_state(ActiveBuffState {
                revision: 2,
                observed_at_unix_ms: 1_500,
                ..ActiveBuffState::default()
            })
            .expect("cleared combo state"),
            200.0,
            0.0,
        );
        assert!(active.combo.is_none());

        install(
            &mut active,
            validate_state(combo).expect("stale combo state"),
            300.0,
            0.0,
        );
        assert!(active.combo.is_none());
    }

    #[test]
    fn combo_requires_a_count_and_expiration_together() {
        let error = validate_state(ActiveBuffState {
            combo_count: 1,
            ..ActiveBuffState::default()
        })
        .err()
        .expect("inconsistent combo must fail");
        assert_eq!(
            error,
            "active combo must contain both a count and an expiration"
        );
    }

    #[test]
    fn apply_projects_the_strongest_active_movement_modifiers() {
        let mut active = TrackedBuffs {
            buffs: vec![
                tracked(buff(1, 20, 5, 200), 200.0),
                tracked(buff(2, 10, 7, 300), 300.0),
            ],
            ..TrackedBuffs::default()
        };
        let mut input = PlayerInput::default();

        apply(&mut active, &mut input, 200.0);

        assert_eq!(active.buffs.len(), 1);
        assert_eq!(input.speed_bonus, 10);
        assert_eq!(input.jump_bonus, 7);
    }

    #[test]
    fn all_negative_holders_project_the_least_negative_modifier() {
        let mut active = TrackedBuffs {
            buffs: vec![
                tracked(buff(1, -10, -2, 300), 300.0),
                tracked(buff(2, -5, -8, 300), 300.0),
            ],
            ..TrackedBuffs::default()
        };
        let mut input = PlayerInput::default();

        apply(&mut active, &mut input, 100.0);

        assert_eq!(input.speed_bonus, -5);
        assert_eq!(input.jump_bonus, -2);
    }

    #[test]
    fn authoritative_attack_disable_expires_with_its_local_morph_holder() {
        let mut active = from_state(
            ActiveBuffState {
                buffs: vec![ActiveBuff {
                    source: Some(active_buff::Source::SkillId(1)),
                    morph_id: 4,
                    expires_at_unix_ms: 1_100,
                    ..ActiveBuff::default()
                }],
                revision: 1,
                observed_at_unix_ms: 1_000,
                morph_id: 4,
                attacks_disabled: true,
                ..ActiveBuffState::default()
            },
            20.0,
            0.0,
        )
        .expect("valid buff state");
        let mut input = PlayerInput::default();

        assert_eq!(active.morph_id, Some(4));
        assert!(active.attacks_disabled);
        apply(&mut active, &mut input, 120.0);
        assert_eq!(active.morph_id, None);
        assert!(!active.attacks_disabled);
    }

    #[test]
    fn response_transit_is_removed_before_installing_a_local_deadline() {
        let state = ActiveBuffState {
            buffs: vec![buff(1, 10, 5, 1_100)],
            revision: 1,
            observed_at_unix_ms: 1_000,
            ..ActiveBuffState::default()
        };

        let active = from_state(state.clone(), 500.0, 40.0).expect("valid buff state");
        assert_eq!(active.buffs[0].remaining_ms(500.0), Some(60));

        let expired = from_state(state, 500.0, 100.0).expect("valid buff state");
        assert!(expired.buffs.is_empty());
    }

    #[test]
    fn zero_source_ids_are_rejected() {
        let error = validate_state(ActiveBuffState {
            buffs: vec![buff(0, 1, 1, 1_300)],
            ..ActiveBuffState::default()
        })
        .err()
        .expect("zero source ID must fail");

        assert_eq!(error, "active buff has an invalid skill source ID");
    }
}
