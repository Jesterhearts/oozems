use oozems_proto::v1::AbilityStat;
use oozems_proto::v1::PlayerState;
use thiserror::Error;

const MAX_PRIMARY_STAT: u32 = 999;
const MAX_RESOURCE: u32 = 30_000;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum AbilityRuleError {
    #[error("the player does not contain character stats")]
    MissingStats,
    #[error("the player has no ability points to allocate")]
    NoAbilityPoints,
    #[error("the selected ability stat is already at its maximum value")]
    MaximumStat,
}

pub fn allocate_ability_point(
    mut player: PlayerState,
    stat: AbilityStat,
    learned: crate::skills::LearnedSkillModifiers,
) -> Result<PlayerState, AbilityRuleError> {
    let resource_gain = ability_resource_gain(&player, stat, learned)?;
    let stats = player
        .stats
        .as_mut()
        .ok_or(AbilityRuleError::MissingStats)?;
    if stats.ability_points == 0 {
        return Err(AbilityRuleError::NoAbilityPoints);
    }

    let value = match stat {
        AbilityStat::Strength => Some(&mut stats.strength),
        AbilityStat::Dexterity => Some(&mut stats.dexterity),
        AbilityStat::Intelligence => Some(&mut stats.intelligence),
        AbilityStat::Luck => Some(&mut stats.luck),
        AbilityStat::MaxHp => {
            let gain = resource_gain.expect("HP allocation has a resource gain");
            let next = stats.max_hp.saturating_add(gain).min(MAX_RESOURCE);
            if next == stats.max_hp {
                return Err(AbilityRuleError::MaximumStat);
            }
            stats.max_hp = next;
            None
        }
        AbilityStat::MaxMp => {
            let gain = resource_gain.expect("MP allocation has a resource gain");
            let next = stats.max_mp.saturating_add(gain).min(MAX_RESOURCE);
            if next == stats.max_mp {
                return Err(AbilityRuleError::MaximumStat);
            }
            stats.max_mp = next;
            None
        }
        AbilityStat::Unspecified => {
            unreachable!("unspecified ability stats are rejected at the API boundary")
        }
    };
    if let Some(value) = value {
        if *value >= MAX_PRIMARY_STAT {
            return Err(AbilityRuleError::MaximumStat);
        }
        *value += 1;
    }
    stats.ability_points -= 1;
    Ok(player)
}

fn ability_resource_gain(
    player: &PlayerState,
    stat: AbilityStat,
    learned: crate::skills::LearnedSkillModifiers,
) -> Result<Option<u32>, AbilityRuleError> {
    use crate::jobs::GrowthFamily;

    if !matches!(stat, AbilityStat::MaxHp | AbilityStat::MaxMp) {
        return Ok(None);
    }
    let stats = player
        .stats
        .as_ref()
        .ok_or(AbilityRuleError::MissingStats)?;
    let family = crate::jobs::growth_family(stats.job_id);
    let sequence = stats
        .ability_points
        .wrapping_add(stats.max_hp)
        .wrapping_add(stats.max_mp);
    let gain = match stat {
        AbilityStat::MaxHp => {
            let (minimum, maximum) = match family {
                GrowthFamily::Beginner => (8, 12),
                GrowthFamily::Warrior => (20, 24),
                GrowthFamily::Magician => (6, 10),
                GrowthFamily::Bowman | GrowthFamily::Thief => (16, 20),
                GrowthFamily::Pirate => (18, 22),
                GrowthFamily::Aran => (36, 40),
            };
            crate::experience::stable_growth_roll(
                &player.id,
                sequence,
                b"ability-hp",
                minimum,
                maximum,
            )
            .saturating_add(learned.max_hp_per_ability_point)
        }
        AbilityStat::MaxMp => {
            let (minimum, maximum) = match family {
                GrowthFamily::Beginner => (6, 8),
                GrowthFamily::Warrior => (2, 4),
                GrowthFamily::Magician => (18, 20),
                GrowthFamily::Bowman | GrowthFamily::Thief => (10, 12),
                GrowthFamily::Pirate => (14, 16),
                GrowthFamily::Aran => (2, 4),
            };
            crate::experience::stable_growth_roll(
                &player.id,
                sequence,
                b"ability-mp",
                minimum,
                maximum,
            )
            .saturating_add(stats.intelligence.saturating_mul(3) / 40)
            .saturating_add(learned.max_mp_per_ability_point)
        }
        _ => unreachable!("non-resource stats returned above"),
    };
    Ok(Some(gain))
}

#[cfg(test)]
mod tests {
    use oozems_proto::v1::CharacterStats;

    use super::*;

    fn player_with_points(points: u32) -> PlayerState {
        PlayerState {
            stats: Some(CharacterStats {
                ability_points: points,
                strength: 4,
                dexterity: 4,
                intelligence: 4,
                luck: 4,
                ..CharacterStats::default()
            }),
            ..PlayerState::default()
        }
    }

    #[test]
    fn allocation_increases_only_the_selected_stat() {
        let player = allocate_ability_point(
            player_with_points(2),
            AbilityStat::Intelligence,
            crate::skills::LearnedSkillModifiers::default(),
        )
        .expect("allocate intelligence");
        let stats = player.stats.expect("stats");

        assert_eq!(stats.ability_points, 1);
        assert_eq!(stats.strength, 4);
        assert_eq!(stats.dexterity, 4);
        assert_eq!(stats.intelligence, 5);
        assert_eq!(stats.luck, 4);
    }

    #[test]
    fn allocation_requires_an_available_point() {
        assert_eq!(
            allocate_ability_point(
                player_with_points(0),
                AbilityStat::Strength,
                crate::skills::LearnedSkillModifiers::default(),
            ),
            Err(AbilityRuleError::NoAbilityPoints)
        );
    }

    #[test]
    fn allocation_rejects_overflow_without_spending_a_point() {
        let mut player = player_with_points(1);
        player.stats.as_mut().expect("stats").luck = u32::MAX;

        assert_eq!(
            allocate_ability_point(
                player,
                AbilityStat::Luck,
                crate::skills::LearnedSkillModifiers::default(),
            ),
            Err(AbilityRuleError::MaximumStat)
        );
    }

    #[test]
    fn resource_allocation_includes_base_growth_without_restoring_current_resource() {
        let mut player = player_with_points(2);
        player.id = "beginner-growth".to_owned();
        player.stats.as_mut().expect("stats").hp = 5;
        player.stats.as_mut().expect("stats").max_hp = 50;

        let player = allocate_ability_point(
            player,
            AbilityStat::MaxHp,
            crate::skills::LearnedSkillModifiers::default(),
        )
        .expect("allocate maximum HP");
        let stats = player.stats.expect("stats");

        assert!((58..=62).contains(&stats.max_hp));
        assert_eq!(stats.hp, 5);
        assert_eq!(stats.ability_points, 1);
    }

    #[test]
    fn capped_stats_reject_allocation_without_spending_a_point() {
        let mut primary = player_with_points(1);
        primary.stats.as_mut().expect("stats").strength = MAX_PRIMARY_STAT;
        assert_eq!(
            allocate_ability_point(
                primary,
                AbilityStat::Strength,
                crate::skills::LearnedSkillModifiers::default(),
            ),
            Err(AbilityRuleError::MaximumStat)
        );

        let mut resource = player_with_points(1);
        resource.stats.as_mut().expect("stats").max_mp = MAX_RESOURCE;
        assert_eq!(
            allocate_ability_point(
                resource,
                AbilityStat::MaxMp,
                crate::skills::LearnedSkillModifiers::default(),
            ),
            Err(AbilityRuleError::MaximumStat)
        );
    }
}
