use std::collections::HashMap;
use std::sync::Mutex;

use oozems_proto::v1::ActiveBuff;
use oozems_proto::v1::ActiveBuffState;
use oozems_proto::v1::SkillUseResult;

use super::SkillRuleError;

#[derive(Default)]
pub struct SkillBuffs {
    state: Mutex<BuffStoreState>,
}

#[derive(Default)]
struct BuffStoreState {
    entries: HashMap<(String, u32), ActiveBuff>,
    revision: u64,
}

pub fn record_skill_buff(
    buffs: &SkillBuffs,
    player_id: &str,
    result: &SkillUseResult,
    now_ms: u64,
) -> Result<ActiveBuffState, SkillRuleError> {
    let mut state = buffs.state.lock().map_err(|_| SkillRuleError::BuffStore)?;
    if prune_expired(&mut state.entries, now_ms) {
        state.revision = state.revision.saturating_add(1);
    }
    if result.duration_ms > 0 {
        state.entries.insert(
            (player_id.to_owned(), result.skill_id),
            ActiveBuff {
                skill_id: result.skill_id,
                skill_level: result.skill_level,
                speed_bonus: result.speed_bonus,
                jump_bonus: result.jump_bonus,
                activated_at_unix_ms: now_ms,
                expires_at_unix_ms: now_ms.saturating_add(result.duration_ms),
            },
        );
        state.revision = state.revision.saturating_add(1);
    }
    Ok(player_buff_state(&state, player_id, now_ms))
}

pub fn active_skill_buffs(
    buffs: &SkillBuffs,
    player_id: &str,
    now_ms: u64,
) -> Result<ActiveBuffState, SkillRuleError> {
    let mut state = buffs.state.lock().map_err(|_| SkillRuleError::BuffStore)?;
    if prune_expired(&mut state.entries, now_ms) {
        state.revision = state.revision.saturating_add(1);
    }
    Ok(player_buff_state(&state, player_id, now_ms))
}

fn prune_expired(
    entries: &mut HashMap<(String, u32), ActiveBuff>,
    now_ms: u64,
) -> bool {
    let original_len = entries.len();
    entries.retain(|_, buff| buff.expires_at_unix_ms > now_ms);
    entries.len() != original_len
}

fn player_buff_state(
    state: &BuffStoreState,
    player_id: &str,
    observed_at_unix_ms: u64,
) -> ActiveBuffState {
    let mut buffs = state
        .entries
        .iter()
        .filter(|((owner, _), _)| owner == player_id)
        .map(|(_, buff)| *buff)
        .collect::<Vec<_>>();
    buffs.sort_by_key(|buff| buff.skill_id);
    ActiveBuffState {
        buffs,
        revision: state.revision,
        observed_at_unix_ms,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skill_result(
        skill_id: u32,
        duration_ms: u64,
    ) -> SkillUseResult {
        SkillUseResult {
            skill_id,
            skill_level: 2,
            speed_bonus: 10,
            jump_bonus: 5,
            duration_ms,
            ..SkillUseResult::default()
        }
    }

    #[test]
    fn records_replaces_and_expires_buffs() {
        let buffs = SkillBuffs::default();
        let first = record_skill_buff(&buffs, "alice", &skill_result(1001, 500), 100).unwrap();
        assert_eq!(first.buffs[0].expires_at_unix_ms, 600);

        let replacement =
            record_skill_buff(&buffs, "alice", &skill_result(1001, 900), 200).unwrap();
        assert_eq!(replacement.buffs.len(), 1);
        assert_eq!(replacement.buffs[0].activated_at_unix_ms, 200);
        assert_eq!(replacement.buffs[0].expires_at_unix_ms, 1100);
        assert!(replacement.revision > first.revision);
        assert!(
            active_skill_buffs(&buffs, "alice", 1100)
                .unwrap()
                .buffs
                .is_empty()
        );
    }

    #[test]
    fn returns_only_the_requested_players_buffs_in_skill_order() {
        let buffs = SkillBuffs::default();
        record_skill_buff(&buffs, "alice", &skill_result(2002, 500), 100).unwrap();
        record_skill_buff(&buffs, "bob", &skill_result(1001, 500), 100).unwrap();
        record_skill_buff(&buffs, "alice", &skill_result(1001, 500), 100).unwrap();

        let alice = active_skill_buffs(&buffs, "alice", 200).unwrap();
        assert_eq!(
            alice
                .buffs
                .iter()
                .map(|buff| buff.skill_id)
                .collect::<Vec<_>>(),
            vec![1001, 2002]
        );
    }

    #[test]
    fn ignores_skills_without_a_duration() {
        let buffs = SkillBuffs::default();
        let active = record_skill_buff(&buffs, "alice", &skill_result(1001, 0), 100).unwrap();
        assert!(active.buffs.is_empty());
    }
}
