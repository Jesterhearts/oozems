use oozems_proto::v1::AbilityStat;
use oozems_proto::v1::PlayerState;
use thiserror::Error;

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
) -> Result<PlayerState, AbilityRuleError> {
    let stats = player
        .stats
        .as_mut()
        .ok_or(AbilityRuleError::MissingStats)?;
    if stats.ability_points == 0 {
        return Err(AbilityRuleError::NoAbilityPoints);
    }

    let value = match stat {
        AbilityStat::Strength => &mut stats.strength,
        AbilityStat::Dexterity => &mut stats.dexterity,
        AbilityStat::Intelligence => &mut stats.intelligence,
        AbilityStat::Luck => &mut stats.luck,
        AbilityStat::Unspecified => {
            unreachable!("unspecified ability stats are rejected at the API boundary")
        }
    };
    *value = value.checked_add(1).ok_or(AbilityRuleError::MaximumStat)?;
    stats.ability_points -= 1;
    Ok(player)
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
        let player = allocate_ability_point(player_with_points(2), AbilityStat::Intelligence)
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
            allocate_ability_point(player_with_points(0), AbilityStat::Strength),
            Err(AbilityRuleError::NoAbilityPoints)
        );
    }

    #[test]
    fn allocation_rejects_overflow_without_spending_a_point() {
        let mut player = player_with_points(1);
        player.stats.as_mut().expect("stats").luck = u32::MAX;

        assert_eq!(
            allocate_ability_point(player, AbilityStat::Luck),
            Err(AbilityRuleError::MaximumStat)
        );
    }
}
