use oozems_proto::v1::GameGui;
use oozems_proto::v1::PlayerState;
use oozems_proto::v1::QuestStatus;
use oozems_proto::v1::QuestTrackerEntry;
use oozems_proto::v1::QuestTrackerObjective;
use oozems_proto::v1::QuestTrackerProgressKind;
use oozems_proto::v1::active_buff;

use crate::game::buffs::TrackedBuffs;

pub(crate) fn active_entries(
    gui: &GameGui,
    player: &PlayerState,
    active_buffs: &TrackedBuffs,
) -> Vec<QuestTrackerEntry> {
    gui.quest_tracker
        .iter()
        .filter_map(|source| {
            let player_quest = player.quests.iter().find(|quest| {
                quest.quest_id == source.quest_id
                    && QuestStatus::try_from(quest.status) == Ok(QuestStatus::Started)
            })?;
            let mut entry = source.clone();
            for objective in &mut entry.objectives {
                update_objective(objective, player_quest, player, active_buffs);
            }
            if !entry.objectives.is_empty() {
                entry.ready = entry.objectives.iter().all(|objective| objective.complete);
            }
            Some(entry)
        })
        .collect()
}

fn update_objective(
    objective: &mut QuestTrackerObjective,
    player_quest: &oozems_proto::v1::PlayerQuest,
    player: &PlayerState,
    active_buffs: &TrackedBuffs,
) {
    let kind = QuestTrackerProgressKind::try_from(objective.progress_kind)
        .unwrap_or(QuestTrackerProgressKind::Snapshot);
    let target_id = objective.target_ids.first().copied().unwrap_or_default();
    match kind {
        QuestTrackerProgressKind::Snapshot => {}
        QuestTrackerProgressKind::Mob => {
            objective.current = player_quest
                .mob_progress
                .iter()
                .find(|progress| progress.mob_id == target_id)
                .map_or(0, |progress| u64::from(progress.count))
                .min(objective.required);
            objective.complete = objective.current >= objective.required;
        }
        QuestTrackerProgressKind::Item => {
            let current = item_quantity(player, target_id);
            objective.current = current.unwrap_or_default();
            objective.complete = match (current, objective.required) {
                (Some(0), 0) => true,
                (Some(current), required) if required > 0 => current >= required,
                _ => false,
            };
        }
        QuestTrackerProgressKind::Level => {
            objective.current = u64::from(player.level);
            objective.complete = objective.current >= objective.required;
        }
        QuestTrackerProgressKind::Mesos => {
            objective.current = player.mesos;
            objective.complete = objective.current >= objective.required;
        }
        QuestTrackerProgressKind::EquipmentAll => {
            objective.complete = objective
                .target_ids
                .iter()
                .all(|item_id| is_equipped(player, *item_id));
            objective.current = u64::from(objective.complete);
        }
        QuestTrackerProgressKind::EquipmentAny => {
            objective.complete = objective
                .target_ids
                .iter()
                .any(|item_id| is_equipped(player, *item_id));
            objective.current = u64::from(objective.complete);
        }
        QuestTrackerProgressKind::Quest => {
            let current = player
                .quests
                .iter()
                .find(|quest| quest.quest_id == target_id)
                .and_then(|quest| QuestStatus::try_from(quest.status).ok())
                .unwrap_or(QuestStatus::Unspecified);
            objective.complete = current as i32 == objective.target_quest_status;
            objective.current = u64::from(objective.complete);
        }
        QuestTrackerProgressKind::EffectItem => {
            let active = active_buffs.buffs.iter().any(|buff| {
                matches!(buff.source, Some(active_buff::Source::ItemId(item_id)) if item_id == target_id)
            });
            objective.current = u64::from(active);
            objective.complete = active == (objective.required != 0);
        }
        QuestTrackerProgressKind::Morph => {
            objective.current = u64::from(active_buffs.morph_id.unwrap_or_default());
            objective.complete = objective.current == objective.required;
        }
        QuestTrackerProgressKind::MonsterBookCardMinimum => {
            objective.current = monster_book_card_count(player, target_id);
            objective.complete = objective.current >= objective.required;
        }
        QuestTrackerProgressKind::MonsterBookCardMaximum => {
            objective.current = monster_book_card_count(player, target_id);
            objective.complete = objective.current <= objective.required;
        }
        QuestTrackerProgressKind::MonsterBookUniqueMinimum => {
            objective.current = player.monster_book_cards.len() as u64;
            objective.complete = objective.current >= objective.required;
        }
        QuestTrackerProgressKind::MonsterBookUniqueMaximum => {
            objective.current = player.monster_book_cards.len() as u64;
            objective.complete = objective.current <= objective.required;
        }
    }
}

pub(crate) fn needs_refresh(
    gui: &GameGui,
    player: &PlayerState,
) -> bool {
    let mut projected = gui
        .quest_tracker
        .iter()
        .map(|quest| quest.quest_id)
        .collect::<Vec<_>>();
    let mut active = player
        .quests
        .iter()
        .filter(|quest| QuestStatus::try_from(quest.status) == Ok(QuestStatus::Started))
        .map(|quest| quest.quest_id)
        .collect::<Vec<_>>();
    projected.sort_unstable();
    active.sort_unstable();
    projected != active
}

fn item_quantity(
    player: &PlayerState,
    item_id: u32,
) -> Option<u64> {
    let Some(inventory) = &player.inventory else {
        return None;
    };
    Some(
        inventory
            .stacks
            .iter()
            .filter(|stack| stack.item_id == item_id)
            .fold(0_u64, |total, stack| {
                total.saturating_add(u64::from(stack.quantity))
            })
            .saturating_add(
                inventory
                    .equipment
                    .iter()
                    .filter(|item| item.item_id == item_id)
                    .count() as u64,
            ),
    )
}

fn is_equipped(
    player: &PlayerState,
    item_id: u32,
) -> bool {
    player.inventory.as_ref().is_some_and(|inventory| {
        inventory
            .equipment
            .iter()
            .any(|item| item.item_id == item_id)
    })
}

fn monster_book_card_count(
    player: &PlayerState,
    item_id: u32,
) -> u64 {
    player
        .monster_book_cards
        .iter()
        .find(|card| card.card_item_id == item_id)
        .map_or(0, |card| u64::from(card.count))
}

#[cfg(test)]
mod tests {
    use oozems_proto::v1::EquippedItem;
    use oozems_proto::v1::GameGui;
    use oozems_proto::v1::InventoryItemStack;
    use oozems_proto::v1::InventoryState;
    use oozems_proto::v1::MonsterBookCard;
    use oozems_proto::v1::PlayerQuest;
    use oozems_proto::v1::PlayerState;
    use oozems_proto::v1::QuestMobProgress;
    use oozems_proto::v1::QuestStatus;
    use oozems_proto::v1::QuestTrackerEntry;
    use oozems_proto::v1::QuestTrackerObjective;
    use oozems_proto::v1::QuestTrackerProgressKind;

    use super::active_entries;
    use super::needs_refresh;
    use crate::game::buffs::TrackedBuffs;

    #[test]
    fn live_player_progress_updates_mob_and_item_objectives() {
        let gui = GameGui {
            quest_tracker: vec![QuestTrackerEntry {
                quest_id: 10,
                objectives: vec![
                    QuestTrackerObjective {
                        progress_kind: QuestTrackerProgressKind::Mob as i32,
                        target_ids: vec![100],
                        required: 5,
                        ..QuestTrackerObjective::default()
                    },
                    QuestTrackerObjective {
                        progress_kind: QuestTrackerProgressKind::Item as i32,
                        target_ids: vec![200],
                        required: 3,
                        ..QuestTrackerObjective::default()
                    },
                ],
                ..QuestTrackerEntry::default()
            }],
            ..GameGui::default()
        };
        let player = PlayerState {
            quests: vec![PlayerQuest {
                quest_id: 10,
                status: QuestStatus::Started as i32,
                mob_progress: vec![QuestMobProgress {
                    mob_id: 100,
                    count: 7,
                }],
                ..PlayerQuest::default()
            }],
            inventory: Some(InventoryState {
                stacks: vec![InventoryItemStack {
                    item_id: 200,
                    quantity: 2,
                    ..InventoryItemStack::default()
                }],
                equipment: vec![EquippedItem {
                    item_id: 200,
                    ..EquippedItem::default()
                }],
                ..InventoryState::default()
            }),
            ..PlayerState::default()
        };

        let entries = active_entries(&gui, &player, &TrackedBuffs::default());

        assert_eq!(entries[0].objectives[0].current, 5);
        assert_eq!(entries[0].objectives[1].current, 3);
        assert!(entries[0].ready);
    }

    #[test]
    fn live_player_state_updates_typed_objectives() {
        let objective = |kind, required, target_ids| QuestTrackerObjective {
            progress_kind: kind as i32,
            required,
            target_ids,
            ..QuestTrackerObjective::default()
        };
        let gui = GameGui {
            quest_tracker: vec![QuestTrackerEntry {
                quest_id: 10,
                objectives: vec![
                    objective(QuestTrackerProgressKind::Level, 12, Vec::new()),
                    objective(QuestTrackerProgressKind::Mesos, 500, Vec::new()),
                    objective(QuestTrackerProgressKind::EquipmentAny, 1, vec![200, 201]),
                    QuestTrackerObjective {
                        target_quest_status: QuestStatus::Completed as i32,
                        ..objective(QuestTrackerProgressKind::Quest, 1, vec![20])
                    },
                    objective(
                        QuestTrackerProgressKind::MonsterBookCardMaximum,
                        4,
                        vec![300],
                    ),
                    objective(
                        QuestTrackerProgressKind::MonsterBookUniqueMinimum,
                        1,
                        Vec::new(),
                    ),
                    objective(QuestTrackerProgressKind::Morph, 7, Vec::new()),
                ],
                ..QuestTrackerEntry::default()
            }],
            ..GameGui::default()
        };
        let player = PlayerState {
            level: 12,
            mesos: 500,
            quests: vec![
                PlayerQuest {
                    quest_id: 10,
                    status: QuestStatus::Started as i32,
                    ..PlayerQuest::default()
                },
                PlayerQuest {
                    quest_id: 20,
                    status: QuestStatus::Completed as i32,
                    ..PlayerQuest::default()
                },
            ],
            inventory: Some(InventoryState {
                equipment: vec![EquippedItem {
                    item_id: 201,
                    ..EquippedItem::default()
                }],
                ..InventoryState::default()
            }),
            monster_book_cards: vec![MonsterBookCard {
                card_item_id: 300,
                count: 4,
            }],
            ..PlayerState::default()
        };
        let mut active_buffs = TrackedBuffs::default();
        active_buffs.morph_id = Some(7);

        let entries = active_entries(&gui, &player, &active_buffs);

        assert!(
            entries[0]
                .objectives
                .iter()
                .all(|objective| objective.complete)
        );
        assert!(entries[0].ready);
    }

    #[test]
    fn active_quest_set_changes_require_a_projection_refresh() {
        let gui = GameGui {
            quest_tracker: vec![QuestTrackerEntry {
                quest_id: 10,
                ..QuestTrackerEntry::default()
            }],
            ..GameGui::default()
        };
        let player = PlayerState {
            quests: vec![PlayerQuest {
                quest_id: 11,
                status: QuestStatus::Started as i32,
                ..PlayerQuest::default()
            }],
            ..PlayerState::default()
        };

        assert!(needs_refresh(&gui, &player));
        assert!(!needs_refresh(
            &gui,
            &PlayerState {
                quests: vec![PlayerQuest {
                    quest_id: 10,
                    status: QuestStatus::Started as i32,
                    ..PlayerQuest::default()
                }],
                ..PlayerState::default()
            }
        ));
    }
}
