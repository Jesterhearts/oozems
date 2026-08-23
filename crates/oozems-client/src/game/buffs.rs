use oozems_proto::v1::ActiveBuff;
use oozems_proto::v1::ActiveBuffState;

use crate::movement::PlayerInput;

#[derive(Default)]
pub(crate) struct TrackedBuffs {
    pub buffs: Vec<ActiveBuff>,
    revision: u64,
    observed_at_unix_ms: u64,
}

pub(super) fn from_state(
    state: ActiveBuffState,
    now_ms: u64,
) -> TrackedBuffs {
    let mut tracked = TrackedBuffs::default();
    install(&mut tracked, state, now_ms);
    tracked
}

pub(super) fn install(
    current: &mut TrackedBuffs,
    mut received: ActiveBuffState,
    now_ms: u64,
) {
    let received_version = (received.observed_at_unix_ms, received.revision);
    let current_version = (current.observed_at_unix_ms, current.revision);
    if received_version < current_version {
        return;
    }
    received
        .buffs
        .retain(|buff| buff.expires_at_unix_ms > now_ms);
    received.buffs.sort_by_key(|buff| buff.skill_id);
    current.buffs = received.buffs;
    current.revision = received.revision;
    current.observed_at_unix_ms = received.observed_at_unix_ms;
}

pub(super) fn apply(
    active: &mut TrackedBuffs,
    input: &mut PlayerInput,
    now_ms: u64,
) {
    active.buffs.retain(|buff| buff.expires_at_unix_ms > now_ms);
    input.speed_bonus = active
        .buffs
        .iter()
        .map(|buff| buff.speed_bonus)
        .fold(0, i32::saturating_add);
    input.jump_bonus = active
        .buffs
        .iter()
        .map(|buff| buff.jump_bonus)
        .fold(0, i32::saturating_add);
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
            skill_id,
            speed_bonus,
            jump_bonus,
            expires_at_unix_ms: expires,
            ..ActiveBuff::default()
        }
    }

    #[test]
    fn install_discards_expired_buffs_and_sorts_the_rest() {
        let mut active = TrackedBuffs::default();

        install(
            &mut active,
            ActiveBuffState {
                buffs: vec![buff(2, 1, 1, 300), buff(1, 1, 1, 100)],
                revision: 1,
                observed_at_unix_ms: 90,
            },
            100,
        );

        assert_eq!(
            active
                .buffs
                .iter()
                .map(|buff| buff.skill_id)
                .collect::<Vec<_>>(),
            vec![2]
        );
    }

    #[test]
    fn install_rejects_a_stale_snapshot() {
        let mut active = from_state(
            ActiveBuffState {
                buffs: vec![buff(2, 1, 1, 300)],
                revision: 2,
                observed_at_unix_ms: 200,
            },
            100,
        );

        install(
            &mut active,
            ActiveBuffState {
                buffs: Vec::new(),
                revision: 1,
                observed_at_unix_ms: 100,
            },
            100,
        );

        assert_eq!(active.buffs.len(), 1);
    }

    #[test]
    fn apply_combines_active_movement_modifiers() {
        let mut active = TrackedBuffs {
            buffs: vec![buff(1, 20, 5, 200), buff(2, 10, 7, 300)],
            ..TrackedBuffs::default()
        };
        let mut input = PlayerInput::default();

        apply(&mut active, &mut input, 200);

        assert_eq!(active.buffs.len(), 1);
        assert_eq!(input.speed_bonus, 10);
        assert_eq!(input.jump_bonus, 7);
    }
}
