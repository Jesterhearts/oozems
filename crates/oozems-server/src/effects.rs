use std::collections::BTreeMap;
use std::collections::HashMap;
use std::sync::Mutex;

use oozems_proto::v1::ActiveBuff;
use oozems_proto::v1::ActiveBuffState;
use oozems_proto::v1::MorphDefinition;
use oozems_proto::v1::PlayerState;
use oozems_proto::v1::SkillUseResult;
use oozems_proto::v1::active_buff;
use thiserror::Error;

use crate::content::ConsumeEffectDefinition;

#[derive(Default)]
pub struct ActiveEffects {
    players: Mutex<HashMap<String, PlayerEffects>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum EffectSource {
    Skill(u32),
    Item(u32),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EffectModifiers {
    pub weapon_attack: i32,
    pub magic_attack: i32,
    pub weapon_defense: i32,
    pub magic_defense: i32,
    pub accuracy: i32,
    pub avoidability: i32,
    pub speed: i32,
    pub jump: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActiveEffect {
    pub source: EffectSource,
    pub skill_level: u32,
    pub modifiers: EffectModifiers,
    pub morph_id: Option<u32>,
    pub attacks_disabled: bool,
    pub activated_at_unix_ms: u64,
    pub lifetime: EffectLifetime,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EffectLifetime {
    Timed { expires_at_unix_ms: u64 },
    Permanent,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PlayerEffects {
    holders: BTreeMap<EffectSource, ActiveEffect>,
    revision: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProjectedEffects {
    pub modifiers: EffectModifiers,
    pub morph_id: Option<u32>,
    pub attacks_disabled: bool,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum EffectStoreError {
    #[error("the active effect store is unavailable")]
    Store,
    #[error("the active effects changed during a player transaction")]
    Conflict,
}

impl PlayerEffects {
    #[cfg(test)]
    pub fn revision(&self) -> u64 {
        self.revision
    }

    #[cfg(test)]
    pub fn holders(&self) -> impl Iterator<Item = &ActiveEffect> {
        self.holders.values()
    }

    pub fn contains_item(
        &self,
        item_id: u32,
    ) -> bool {
        self.holders.contains_key(&EffectSource::Item(item_id))
    }

    pub fn projected(&self) -> ProjectedEffects {
        project_effects(self.holders.values())
    }

    pub fn attacks_disabled(&self) -> bool {
        self.projected().attacks_disabled
    }
}

pub fn snapshot(
    effects: &ActiveEffects,
    player_id: &str,
    now_unix_ms: u64,
) -> Result<PlayerEffects, EffectStoreError> {
    let mut players = effects
        .players
        .lock()
        .map_err(|_| EffectStoreError::Store)?;
    let player = players.entry(player_id.to_owned()).or_default();
    prune(player, now_unix_ms);
    Ok(player.clone())
}

#[cfg(test)]
pub fn commit(
    effects: &ActiveEffects,
    player_id: &str,
    staged: PlayerEffects,
) -> Result<(), EffectStoreError> {
    let mut players = effects
        .players
        .lock()
        .map_err(|_| EffectStoreError::Store)?;
    let current_revision = players.get(player_id).map_or(0, PlayerEffects::revision);
    if staged.revision >= current_revision {
        players.insert(player_id.to_owned(), staged);
    }
    Ok(())
}

pub fn commit_staged(
    effects: &ActiveEffects,
    player_id: &str,
    original: &PlayerEffects,
    staged: &PlayerEffects,
) -> Result<(), EffectStoreError> {
    let mut players = effects
        .players
        .lock()
        .map_err(|_| EffectStoreError::Store)?;
    let current = players.get(player_id).cloned().unwrap_or_default();
    if &current != original {
        return Err(EffectStoreError::Conflict);
    }
    players.insert(player_id.to_owned(), staged.clone());
    Ok(())
}

pub fn rollback_staged(
    effects: &ActiveEffects,
    player_id: &str,
    staged: &PlayerEffects,
    original: &PlayerEffects,
) -> Result<(), EffectStoreError> {
    let mut players = effects
        .players
        .lock()
        .map_err(|_| EffectStoreError::Store)?;
    if players.get(player_id) != Some(staged) {
        return Err(EffectStoreError::Conflict);
    }
    players.insert(player_id.to_owned(), original.clone());
    Ok(())
}

pub fn apply_effect(
    effects: &mut PlayerEffects,
    effect: ActiveEffect,
) {
    effects.holders.remove(&effect.source);
    if effect.morph_id.is_some() {
        effects
            .holders
            .retain(|_, holder| holder.morph_id.is_none());
    }
    effects.holders.insert(effect.source, effect);
    effects.revision = effects.revision.saturating_add(1);
}

pub fn apply_skill_effect(
    effects: &mut PlayerEffects,
    result: &SkillUseResult,
    now_unix_ms: u64,
) {
    effects
        .holders
        .remove(&EffectSource::Skill(result.skill_id));
    if result.duration_ms == 0 && !result.permanent_buff {
        return;
    }
    let lifetime = if result.permanent_buff {
        EffectLifetime::Permanent
    } else {
        EffectLifetime::Timed {
            expires_at_unix_ms: now_unix_ms.saturating_add(result.duration_ms),
        }
    };
    apply_effect(
        effects,
        ActiveEffect {
            source: EffectSource::Skill(result.skill_id),
            skill_level: result.skill_level,
            modifiers: EffectModifiers {
                weapon_attack: result.weapon_attack_bonus,
                magic_attack: result.magic_attack_bonus,
                weapon_defense: result.weapon_defense_bonus,
                magic_defense: result.magic_defense_bonus,
                accuracy: result.accuracy_bonus,
                avoidability: result.avoidability_bonus,
                speed: result.speed_bonus,
                jump: result.jump_bonus,
            },
            morph_id: None,
            attacks_disabled: false,
            activated_at_unix_ms: now_unix_ms,
            lifetime,
        },
    );
}

pub fn apply_consume_effect(
    mut player: PlayerState,
    effects: &mut PlayerEffects,
    definition: ConsumeEffectDefinition,
    now_unix_ms: u64,
) -> PlayerState {
    if let Some(stats) = player.stats.as_mut() {
        stats.hp = restore_resource(stats.hp, stats.max_hp, definition.hp, definition.hp_percent);
        stats.mp = restore_resource(stats.mp, stats.max_mp, definition.mp, definition.mp_percent);
    }
    if definition.duration_ms == 0 {
        return player;
    }
    apply_effect(
        effects,
        ActiveEffect {
            source: EffectSource::Item(definition.item_id),
            skill_level: 0,
            modifiers: EffectModifiers {
                weapon_attack: definition.weapon_attack,
                magic_attack: definition.magic_attack,
                weapon_defense: definition.weapon_defense,
                magic_defense: definition.magic_defense,
                accuracy: definition.accuracy,
                avoidability: definition.avoidability,
                speed: definition.speed,
                jump: definition.jump,
            },
            morph_id: definition.morph_id,
            attacks_disabled: definition.morph_id.is_some_and(|morph_id| morph_id < 100),
            activated_at_unix_ms: now_unix_ms,
            lifetime: EffectLifetime::Timed {
                expires_at_unix_ms: now_unix_ms.saturating_add(definition.duration_ms),
            },
        },
    );
    player
}

fn restore_resource(
    current: u32,
    maximum: u32,
    fixed: u32,
    percentage: u32,
) -> u32 {
    let percentage = u64::from(maximum)
        .saturating_mul(u64::from(percentage))
        .checked_div(100)
        .unwrap_or_default();
    u64::from(current)
        .saturating_add(u64::from(fixed))
        .saturating_add(percentage)
        .min(u64::from(maximum)) as u32
}

pub fn state(
    effects: &PlayerEffects,
    observed_at_unix_ms: u64,
) -> ActiveBuffState {
    let projection = effects.projected();
    ActiveBuffState {
        buffs: effects.holders.values().map(to_proto).collect(),
        revision: effects.revision,
        observed_at_unix_ms,
        weapon_attack: projection.modifiers.weapon_attack,
        magic_attack: projection.modifiers.magic_attack,
        weapon_defense: projection.modifiers.weapon_defense,
        magic_defense: projection.modifiers.magic_defense,
        accuracy: projection.modifiers.accuracy,
        avoidability: projection.modifiers.avoidability,
        speed: projection.modifiers.speed,
        jump: projection.modifiers.jump,
        morph_id: projection.morph_id.unwrap_or_default(),
        attacks_disabled: projection.attacks_disabled,
    }
}

pub fn cancel_damage_morphs(
    effects: &mut PlayerEffects,
    mut definition: impl FnMut(u32) -> Option<MorphDefinition>,
) -> bool {
    let previous = effects.holders.len();
    effects.holders.retain(|_, effect| {
        effect
            .morph_id
            .is_none_or(|morph_id| definition(morph_id).is_some_and(|morph| morph.no_cancel_damage))
    });
    let changed = effects.holders.len() != previous;
    if changed {
        effects.revision = effects.revision.saturating_add(1);
    }
    changed
}

pub fn prune(
    effects: &mut PlayerEffects,
    now_unix_ms: u64,
) -> bool {
    let previous = effects.holders.len();
    effects.holders.retain(|_, effect| match effect.lifetime {
        EffectLifetime::Timed { expires_at_unix_ms } => expires_at_unix_ms > now_unix_ms,
        EffectLifetime::Permanent => true,
    });
    let changed = previous != effects.holders.len();
    if changed {
        effects.revision = effects.revision.saturating_add(1);
    }
    changed
}

fn project_effects<'a>(holders: impl IntoIterator<Item = &'a ActiveEffect>) -> ProjectedEffects {
    let holders = holders.into_iter().collect::<Vec<_>>();
    let strongest = |select: fn(&EffectModifiers) -> i32| {
        holders
            .iter()
            .map(|holder| select(&holder.modifiers))
            .filter(|value| *value != 0)
            .max()
            .unwrap_or_default()
    };
    ProjectedEffects {
        modifiers: EffectModifiers {
            weapon_attack: strongest(|modifiers| modifiers.weapon_attack),
            magic_attack: strongest(|modifiers| modifiers.magic_attack),
            weapon_defense: strongest(|modifiers| modifiers.weapon_defense),
            magic_defense: strongest(|modifiers| modifiers.magic_defense),
            accuracy: strongest(|modifiers| modifiers.accuracy),
            avoidability: strongest(|modifiers| modifiers.avoidability),
            speed: strongest(|modifiers| modifiers.speed),
            jump: strongest(|modifiers| modifiers.jump),
        },
        morph_id: holders.iter().find_map(|holder| holder.morph_id),
        attacks_disabled: holders.iter().any(|holder| holder.attacks_disabled),
    }
}

fn to_proto(effect: &ActiveEffect) -> ActiveBuff {
    let source = match effect.source {
        EffectSource::Skill(skill_id) => Some(active_buff::Source::SkillId(skill_id)),
        EffectSource::Item(item_id) => Some(active_buff::Source::ItemId(item_id)),
    };
    let (expires_at_unix_ms, permanent) = match effect.lifetime {
        EffectLifetime::Timed { expires_at_unix_ms } => (expires_at_unix_ms, false),
        EffectLifetime::Permanent => (0, true),
    };
    ActiveBuff {
        skill_level: effect.skill_level,
        speed_bonus: effect.modifiers.speed,
        jump_bonus: effect.modifiers.jump,
        activated_at_unix_ms: effect.activated_at_unix_ms,
        expires_at_unix_ms,
        source,
        weapon_attack: effect.modifiers.weapon_attack,
        magic_attack: effect.modifiers.magic_attack,
        weapon_defense: effect.modifiers.weapon_defense,
        magic_defense: effect.modifiers.magic_defense,
        accuracy: effect.modifiers.accuracy,
        avoidability: effect.modifiers.avoidability,
        morph_id: effect.morph_id.unwrap_or_default(),
        permanent,
    }
}

#[cfg(test)]
mod tests {
    use oozems_proto::v1::CharacterStats;

    use super::*;

    fn effect(
        source: EffectSource,
        speed: i32,
        morph_id: Option<u32>,
        activated: u64,
        expires: u64,
    ) -> ActiveEffect {
        ActiveEffect {
            source,
            skill_level: 1,
            modifiers: EffectModifiers {
                speed,
                ..EffectModifiers::default()
            },
            morph_id,
            attacks_disabled: morph_id.is_some_and(|morph_id| morph_id < 100),
            activated_at_unix_ms: activated,
            lifetime: EffectLifetime::Timed {
                expires_at_unix_ms: expires,
            },
        }
    }

    #[test]
    fn same_source_replaces_while_losing_holders_remain_present() {
        let mut effects = PlayerEffects::default();
        apply_effect(
            &mut effects,
            effect(EffectSource::Skill(1), 20, None, 10, 100),
        );
        apply_effect(
            &mut effects,
            effect(EffectSource::Item(2), 10, None, 20, 200),
        );
        apply_effect(
            &mut effects,
            effect(EffectSource::Skill(1), 15, None, 30, 300),
        );

        assert_eq!(effects.holders().count(), 2);
        assert!(effects.contains_item(2));
        assert_eq!(effects.projected().modifiers.speed, 15);
        assert_eq!(
            effects.holders[&EffectSource::Skill(1)].activated_at_unix_ms,
            30
        );
    }

    #[test]
    fn all_negative_modifiers_project_the_least_negative_value() {
        let mut effects = PlayerEffects::default();
        apply_effect(
            &mut effects,
            effect(EffectSource::Item(1), -10, None, 0, 100),
        );
        apply_effect(
            &mut effects,
            effect(EffectSource::Skill(2), -5, None, 0, 100),
        );

        assert_eq!(effects.projected().modifiers.speed, -5);
    }

    #[test]
    fn morph_is_singleton_and_disables_attacks_below_one_hundred() {
        let mut effects = PlayerEffects::default();
        apply_effect(
            &mut effects,
            effect(EffectSource::Item(1), 10, Some(4), 0, 100),
        );
        apply_effect(
            &mut effects,
            effect(EffectSource::Item(2), 20, Some(140), 1, 100),
        );

        assert_eq!(effects.holders().count(), 1);
        assert_eq!(effects.projected().morph_id, Some(140));
        assert!(!effects.attacks_disabled());
        apply_effect(
            &mut effects,
            effect(EffectSource::Item(3), 0, Some(40), 2, 100),
        );
        assert!(effects.attacks_disabled());
    }

    #[test]
    fn expiry_equality_prunes_and_advances_snapshot_revision() {
        let store = ActiveEffects::default();
        let mut staged = snapshot(&store, "player", 10).expect("initial snapshot");
        apply_effect(&mut staged, effect(EffectSource::Item(1), 10, None, 10, 20));
        commit(&store, "player", staged).expect("commit");
        let before = snapshot(&store, "player", 19).expect("active snapshot");
        let expired = snapshot(&store, "player", 20).expect("expired snapshot");

        assert_eq!(before.holders().count(), 1);
        assert_eq!(expired.holders().count(), 0);
        assert!(expired.revision() > before.revision());
    }

    #[test]
    fn permanent_skill_effect_survives_pruning_and_is_explicit_in_protocol() {
        let mut effects = PlayerEffects::default();
        apply_skill_effect(
            &mut effects,
            &SkillUseResult {
                skill_id: 1_002,
                skill_level: 3,
                speed_bonus: 20,
                permanent_buff: true,
                ..SkillUseResult::default()
            },
            1_000,
        );

        assert!(!prune(&mut effects, u64::MAX));
        assert_eq!(effects.projected().modifiers.speed, 20);
        let state = state(&effects, u64::MAX);
        assert_eq!(state.buffs.len(), 1);
        assert!(state.buffs[0].permanent);
        assert_eq!(state.buffs[0].expires_at_unix_ms, 0);
    }

    #[test]
    fn consume_hp_is_immediate_capped_and_not_repeated_by_projection() {
        let player = PlayerState {
            stats: Some(CharacterStats {
                hp: 80,
                max_hp: 100,
                ..CharacterStats::default()
            }),
            ..PlayerState::default()
        };
        let definition = ConsumeEffectDefinition {
            item_id: 2_210_003,
            hp: 50,
            morph_id: Some(4),
            duration_ms: 60_000,
            ..ConsumeEffectDefinition::default()
        };
        let mut effects = PlayerEffects::default();
        let player = apply_consume_effect(player, &mut effects, definition, 1_000);

        assert_eq!(player.stats.expect("stats").hp, 100);
        assert_eq!(effects.projected().morph_id, Some(4));
    }

    #[test]
    fn restoration_consumables_restore_hp_and_mp_without_creating_a_buff() {
        let player = PlayerState {
            stats: Some(CharacterStats {
                hp: 20,
                max_hp: 200,
                mp: 10,
                max_mp: 100,
                ..CharacterStats::default()
            }),
            ..PlayerState::default()
        };
        let definition = ConsumeEffectDefinition {
            item_id: 2_000_004,
            hp: 10,
            mp: 5,
            hp_percent: 50,
            mp_percent: 25,
            ..ConsumeEffectDefinition::default()
        };
        let mut effects = PlayerEffects::default();

        let player = apply_consume_effect(player, &mut effects, definition, 1_000);
        let stats = player.stats.expect("stats");

        assert_eq!(stats.hp, 130);
        assert_eq!(stats.mp, 40);
        assert_eq!(effects.holders().count(), 0);
        assert_eq!(effects.revision(), 0);
    }

    #[test]
    fn timed_item_modifier_creates_an_expiring_item_buff() {
        let definition = ConsumeEffectDefinition {
            item_id: 2_022_253,
            jump: 3,
            duration_ms: 180_000,
            ..ConsumeEffectDefinition::default()
        };
        let mut effects = PlayerEffects::default();

        apply_consume_effect(PlayerState::default(), &mut effects, definition, 1_000);
        let state = state(&effects, 1_000);

        assert_eq!(state.jump, 3);
        assert_eq!(state.buffs.len(), 1);
        assert_eq!(state.buffs[0].jump_bonus, 3);
        assert_eq!(state.buffs[0].expires_at_unix_ms, 181_000);
        assert!(matches!(
            state.buffs[0].source,
            Some(active_buff::Source::ItemId(2_022_253))
        ));
    }

    #[test]
    fn protocol_snapshot_contains_holders_projection_revision_and_observation() {
        let mut effects = PlayerEffects::default();
        apply_effect(
            &mut effects,
            effect(EffectSource::Item(2_022_631), -5, None, 10, 100),
        );
        let state = state(&effects, 20);

        assert_eq!(state.revision, 1);
        assert_eq!(state.observed_at_unix_ms, 20);
        assert_eq!(state.speed, -5);
        assert_eq!(state.buffs.len(), 1);
        assert!(!state.attacks_disabled);
        assert!(matches!(
            state.buffs[0].source,
            Some(active_buff::Source::ItemId(2_022_631))
        ));

        let skill = to_proto(&effect(EffectSource::Skill(1_001_000), 10, None, 20, 100));
        assert!(matches!(
            skill.source,
            Some(active_buff::Source::SkillId(1_001_000))
        ));
    }

    #[test]
    fn damage_cancels_only_morphs_without_no_cancel_damage() {
        let mut cancellable = PlayerEffects::default();
        apply_effect(
            &mut cancellable,
            effect(EffectSource::Item(1), 10, Some(4), 10, 100),
        );
        let before_revision = cancellable.revision();

        assert!(cancel_damage_morphs(&mut cancellable, |morph_id| {
            Some(MorphDefinition {
                morph_id,
                no_cancel_damage: false,
                ..MorphDefinition::default()
            })
        }));
        assert_eq!(cancellable.holders().count(), 0);
        assert_eq!(cancellable.revision(), before_revision + 1);

        let mut preserved = PlayerEffects::default();
        apply_effect(
            &mut preserved,
            effect(EffectSource::Item(2), 10, Some(40), 10, 100),
        );
        let before_revision = preserved.revision();
        assert!(!cancel_damage_morphs(&mut preserved, |morph_id| {
            Some(MorphDefinition {
                morph_id,
                no_cancel_damage: true,
                ..MorphDefinition::default()
            })
        }));
        assert_eq!(preserved.holders().count(), 1);
        assert_eq!(preserved.revision(), before_revision);
    }
}
