use oozems_proto::v1::PlayerQuest;
use oozems_proto::v1::PlayerState;
use oozems_proto::v1::QuestStatus;
use thiserror::Error;

use crate::content::QuestDefinition;
use crate::experience::ExperienceCurve;
use crate::experience::ExperienceRuleError;

pub const ACCEPT_CHOICE_ID: u32 = 1;
pub const DECLINE_CHOICE_ID: u32 = 2;
const ANSWER_CHOICE_OFFSET: u32 = 100;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuestProgress {
    NotStarted,
    Started,
    Completed,
}

#[derive(Clone, Debug)]
pub struct QuestSelection {
    pub player: PlayerState,
    pub pages: Vec<String>,
    pub changed: bool,
}

#[derive(Debug, Error)]
pub enum QuestRuleError {
    #[error("quest {quest_id} is not available")]
    Unavailable { quest_id: u32 },
    #[error("quest {quest_id} is not active")]
    NotActive { quest_id: u32 },
    #[error("NPC {npc_id} cannot perform that quest action")]
    WrongNpc { npc_id: u32 },
    #[error("quest choice {choice_id} is invalid")]
    InvalidChoice { choice_id: u32 },
    #[error(transparent)]
    Experience(#[from] ExperienceRuleError),
}

pub fn progress(
    player: &PlayerState,
    quest_id: u32,
) -> QuestProgress {
    player
        .quests
        .iter()
        .find(|quest| quest.quest_id == quest_id)
        .and_then(|quest| QuestStatus::try_from(quest.status).ok())
        .map_or(QuestProgress::NotStarted, |status| match status {
            QuestStatus::Started => QuestProgress::Started,
            QuestStatus::Completed => QuestProgress::Completed,
            QuestStatus::Unspecified => QuestProgress::NotStarted,
        })
}

pub fn is_available(
    player: &PlayerState,
    quest: &QuestDefinition,
) -> bool {
    if progress(player, quest.id) != QuestProgress::NotStarted
        || player.level < quest.minimum_level.unwrap_or(1)
    {
        return false;
    }
    let job_id = player.stats.as_ref().map_or(0, |stats| stats.job_id);
    quest.allowed_jobs.is_empty() || quest.allowed_jobs.contains(&job_id)
}

pub fn answer_choice_id(choice_id: u32) -> u32 {
    ANSWER_CHOICE_OFFSET.saturating_add(choice_id)
}

pub fn select_choice(
    player: PlayerState,
    quest: &QuestDefinition,
    npc_id: u32,
    choice_id: u32,
    curve: &ExperienceCurve,
) -> Result<QuestSelection, QuestRuleError> {
    match progress(&player, quest.id) {
        QuestProgress::NotStarted => select_offer_choice(player, quest, npc_id, choice_id),
        QuestProgress::Started => select_answer(player, quest, npc_id, choice_id, curve),
        QuestProgress::Completed => Err(QuestRuleError::Unavailable { quest_id: quest.id }),
    }
}

fn select_offer_choice(
    mut player: PlayerState,
    quest: &QuestDefinition,
    npc_id: u32,
    choice_id: u32,
) -> Result<QuestSelection, QuestRuleError> {
    if npc_id != quest.start_npc_id {
        return Err(QuestRuleError::WrongNpc { npc_id });
    }
    if choice_id == DECLINE_CHOICE_ID {
        return Ok(QuestSelection {
            player,
            pages: quest.declined_pages.clone(),
            changed: false,
        });
    }
    if choice_id != ACCEPT_CHOICE_ID {
        return Err(QuestRuleError::InvalidChoice { choice_id });
    }
    if !is_available(&player, quest) {
        return Err(QuestRuleError::Unavailable { quest_id: quest.id });
    }
    player.quests.push(PlayerQuest {
        quest_id: quest.id,
        status: QuestStatus::Started as i32,
    });
    player.quests.sort_by_key(|entry| entry.quest_id);
    Ok(QuestSelection {
        player,
        pages: quest.accepted_pages.clone(),
        changed: true,
    })
}

fn select_answer(
    mut player: PlayerState,
    quest: &QuestDefinition,
    npc_id: u32,
    choice_id: u32,
    curve: &ExperienceCurve,
) -> Result<QuestSelection, QuestRuleError> {
    if npc_id != quest.completion_npc_id {
        return Err(QuestRuleError::WrongNpc { npc_id });
    }
    let answer_id = choice_id
        .checked_sub(ANSWER_CHOICE_OFFSET)
        .ok_or(QuestRuleError::InvalidChoice { choice_id })?;
    let question = quest
        .question
        .as_ref()
        .ok_or(QuestRuleError::InvalidChoice { choice_id })?;
    if !question.choices.iter().any(|choice| choice.id == answer_id) {
        return Err(QuestRuleError::InvalidChoice { choice_id });
    }
    if answer_id != question.correct_choice_id {
        return Ok(QuestSelection {
            player,
            pages: question
                .failure_pages
                .get(&answer_id)
                .cloned()
                .unwrap_or_default(),
            changed: false,
        });
    }

    player = crate::experience::grant_experience(player, quest.reward_experience, curve)?;
    let entry = player
        .quests
        .iter_mut()
        .find(|entry| entry.quest_id == quest.id)
        .ok_or(QuestRuleError::NotActive { quest_id: quest.id })?;
    entry.status = QuestStatus::Completed as i32;
    Ok(QuestSelection {
        player,
        pages: question.success_pages.clone(),
        changed: true,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use oozems_proto::v1::CharacterStats;
    use oozems_proto::v1::PlayerState;
    use oozems_proto::v1::QuestStatus;

    use super::ACCEPT_CHOICE_ID;
    use super::QuestProgress;
    use super::answer_choice_id;
    use super::progress;
    use super::select_choice;
    use crate::content::QuestChoice;
    use crate::content::QuestDefinition;
    use crate::content::QuestQuestion;
    use crate::experience::ExperienceCurves;

    #[test]
    fn accepting_and_answering_a_quest_applies_its_reward_once() {
        let curves = ExperienceCurves::load(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/xp-curves.toml"),
        )
        .expect("XP curves");
        let quest = quest();
        let player = PlayerState {
            id: "player".to_owned(),
            level: 1,
            stats: Some(CharacterStats {
                job_id: 0,
                experience_required: 15,
                ..CharacterStats::default()
            }),
            ..PlayerState::default()
        };

        let accepted = select_choice(
            player,
            &quest,
            quest.start_npc_id,
            ACCEPT_CHOICE_ID,
            curves.default_curve(),
        )
        .expect("accept quest");
        assert_eq!(progress(&accepted.player, quest.id), QuestProgress::Started);

        let completed = select_choice(
            accepted.player,
            &quest,
            quest.completion_npc_id,
            answer_choice_id(0),
            curves.default_curve(),
        )
        .expect("answer quest");
        assert_eq!(
            progress(&completed.player, quest.id),
            QuestProgress::Completed
        );
        assert_eq!(completed.player.stats.expect("stats").experience, 2);
        assert_eq!(
            completed.player.quests[0].status,
            QuestStatus::Completed as i32
        );
    }

    fn quest() -> QuestDefinition {
        QuestDefinition {
            id: 1_009,
            name: "Rain's Maple Quiz 1".to_owned(),
            start_npc_id: 12_101,
            completion_npc_id: 12_101,
            allowed_jobs: vec![0],
            minimum_level: None,
            offer_pages: vec!["Take the quiz?".to_owned()],
            accepted_pages: vec!["Let's begin.".to_owned()],
            declined_pages: vec!["Come back later.".to_owned()],
            question: Some(QuestQuestion {
                prompt: "Which key opens inventory?".to_owned(),
                choices: vec![QuestChoice {
                    id: 0,
                    label: "I".to_owned(),
                }],
                correct_choice_id: 0,
                success_pages: vec!["Correct.".to_owned()],
                failure_pages: HashMap::new(),
            }),
            reward_experience: 2,
            next_quest_id: Some(1_010),
        }
    }
}
