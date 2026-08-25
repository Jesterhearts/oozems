use crate::quests::test_support::*;

#[test]
fn lost_item_restoration_grants_only_the_partial_missing_quantity_once() {
    let definitions = item_definitions();
    let mut quest = quest(100);
    configure_lost_items(&mut quest, &[(ITEM_A, 3)]);
    quest.completion.items.clear();
    let expiration = Some(QuestItemExpiration::RelativeMilliseconds(1_000));
    quest.start_actions.fixed_items[0].expiration = expiration;
    quest
        .dialogue
        .completion
        .lost
        .as_mut()
        .expect("lost interaction")
        .items[0]
        .expiration = expiration;
    quest.completion_actions.money = 500;
    let mut player = player(vec![(ITEM_A, 1)], 2);
    player.quests.push(PlayerQuest {
        mob_progress: vec![QuestMobProgress {
            mob_id: MOB_A,
            count: 4,
        }],
        accepted_at_unix_ms: 123,
        ..player_quest(quest.id, QuestStatus::Started)
    });

    assert!(lost_item_restoration_needed(
        &player,
        &quest,
        &definitions,
        999
    ));
    let restored = select_choice(
        player,
        &quest,
        2,
        RESTORE_ITEMS_CHOICE_ID,
        curve(),
        &definitions,
        scripts(),
        999,
    )
    .expect("restore partial missing quantity");

    assert_eq!(item_count(&restored.player, ITEM_A), 3);
    assert_eq!(
        restored
            .player
            .inventory
            .as_ref()
            .expect("inventory")
            .stacks[1]
            .expires_at_unix_ms,
        1_999
    );
    assert_eq!(restored.pages, vec!["Replacement items"]);
    assert_eq!(restored.player.mesos, 0);
    let entry = &restored.player.quests[0];
    assert_eq!(entry.status, QuestStatus::Started as i32);
    assert_eq!(entry.accepted_at_unix_ms, 123);
    assert_eq!(entry.completed_at_unix_ms, 0);
    assert_eq!(entry.mob_progress[0].count, 4);

    let unchanged = restored.player.clone();
    assert!(matches!(
        select_choice(
            restored.player,
            &quest,
            2,
            RESTORE_ITEMS_CHOICE_ID,
            curve(),
            &definitions,
            scripts(),
            1_000,
        ),
        Err(QuestRuleError::NoMissingRestorableItems { quest_id: 100 })
    ));
    assert_eq!(item_count(&unchanged, ITEM_A), 3);
    assert_eq!(progress(&unchanged, quest.id), QuestProgress::Started);
}

#[test]
fn completed_restoration_requires_completion_and_every_downstream_guard() {
    const REQUIRED_QUEST: u32 = 200;
    const FORBIDDEN_QUEST: u32 = 300;
    const ABSENT_SKILL: u32 = 2_221_003;

    let definitions = item_definitions();
    let mut quest = quest(100);
    quest.dialogue.completion.lost = Some(QuestLostItemDialogue {
        prompt_pages: vec!["Did you lose the completed quest item?".to_owned()],
        success_pages: vec!["Completed quest replacement".to_owned()],
        items: vec![QuestRestorableItem {
            item_id: ITEM_A,
            target_count: 1,
            expiration: None,
            provenance: QuestRestorationProvenance::AuditedCompletionGrant,
            eligibility: QuestRestorationEligibility {
                owner_state: RequiredQuestState::Completed,
                required_quests: &[QuestStateRequirement {
                    quest_id: REQUIRED_QUEST,
                    state: RequiredQuestState::Started,
                }],
                forbidden_quests: &[QuestStateRequirement {
                    quest_id: FORBIDDEN_QUEST,
                    state: RequiredQuestState::Completed,
                }],
                absent_skill_ids: &[ABSENT_SKILL],
                absent_item_ids: &[ITEM_A, ITEM_B],
            },
        }],
    });
    let first_time = player(Vec::new(), 2);
    assert!(!lost_item_restoration_needed(
        &first_time,
        &quest,
        &definitions,
        200
    ));
    assert!(matches!(
        select_choice(
            first_time,
            &quest,
            1,
            RESTORE_ITEMS_CHOICE_ID,
            curve(),
            &definitions,
            scripts(),
            200,
        ),
        Err(QuestRuleError::InvalidChoice {
            choice_id: RESTORE_ITEMS_CHOICE_ID
        })
    ));

    let mut player = player(Vec::new(), 2);
    player
        .quests
        .push(player_quest(quest.id, QuestStatus::Started));
    player
        .quests
        .push(player_quest(REQUIRED_QUEST, QuestStatus::Started));
    assert!(!lost_item_restoration_needed(
        &player,
        &quest,
        &definitions,
        200
    ));
    assert!(matches!(
        select_choice(
            player.clone(),
            &quest,
            2,
            RESTORE_ITEMS_CHOICE_ID,
            curve(),
            &definitions,
            scripts(),
            200,
        ),
        Err(QuestRuleError::LostItemRestorationUnavailable { quest_id: 100 })
    ));
    player.quests[0] = PlayerQuest {
        completed_at_unix_ms: 123,
        ..player_quest(quest.id, QuestStatus::Completed)
    };
    assert!(lost_item_restoration_needed(
        &player,
        &quest,
        &definitions,
        200
    ));

    let mut blocked = player.clone();
    blocked.quests[1].status = QuestStatus::Completed as i32;
    assert!(!lost_item_restoration_needed(
        &blocked,
        &quest,
        &definitions,
        200
    ));
    blocked = player.clone();
    blocked
        .quests
        .push(player_quest(FORBIDDEN_QUEST, QuestStatus::Completed));
    assert!(!lost_item_restoration_needed(
        &blocked,
        &quest,
        &definitions,
        200
    ));
    blocked = player.clone();
    blocked.learned_skills.push(LearnedSkill {
        skill_id: ABSENT_SKILL,
        level: 1,
        master_level: 0,
    });
    assert!(!lost_item_restoration_needed(
        &blocked,
        &quest,
        &definitions,
        200
    ));
    blocked = player.clone();
    blocked
        .inventory
        .as_mut()
        .expect("inventory")
        .stacks
        .push(InventoryItemStack {
            item_id: ITEM_B,
            quantity: 1,
            expires_at_unix_ms: 0,
        });
    assert!(!lost_item_restoration_needed(
        &blocked,
        &quest,
        &definitions,
        200
    ));

    assert!(matches!(
        select_choice(
            player.clone(),
            &quest,
            999,
            RESTORE_ITEMS_CHOICE_ID,
            curve(),
            &definitions,
            scripts(),
            200,
        ),
        Err(QuestRuleError::WrongNpc { npc_id: 999 })
    ));
    let restored = select_choice(
        player,
        &quest,
        2,
        RESTORE_ITEMS_CHOICE_ID,
        curve(),
        &definitions,
        scripts(),
        200,
    )
    .expect("eligible completed restoration");
    assert_eq!(item_count(&restored.player, ITEM_A), 1);
    assert_eq!(
        progress(&restored.player, quest.id),
        QuestProgress::Completed
    );
    assert_eq!(restored.player.quests[0].completed_at_unix_ms, 123);
    assert!(matches!(
        select_choice(
            restored.player,
            &quest,
            2,
            RESTORE_ITEMS_CHOICE_ID,
            curve(),
            &definitions,
            scripts(),
            200,
        ),
        Err(QuestRuleError::NoMissingRestorableItems { quest_id: 100 })
    ));
}

#[test]
fn completed_restoration_capacity_failure_is_atomic() {
    let definitions = item_definitions();
    let mut quest = quest(100);
    quest.dialogue.completion.lost = Some(QuestLostItemDialogue {
        prompt_pages: vec!["Restore?".to_owned()],
        success_pages: vec!["Restored".to_owned()],
        items: vec![QuestRestorableItem {
            item_id: ITEM_A,
            target_count: 1,
            expiration: None,
            provenance: QuestRestorationProvenance::AuditedCompletionGrant,
            eligibility: QuestRestorationEligibility {
                owner_state: RequiredQuestState::Completed,
                required_quests: &[],
                forbidden_quests: &[],
                absent_skill_ids: &[],
                absent_item_ids: &[ITEM_A],
            },
        }],
    });
    let mut player = player(vec![(ITEM_B, 1)], 1);
    player
        .quests
        .push(player_quest(quest.id, QuestStatus::Completed));
    let unchanged = player.clone();

    assert!(matches!(
        select_choice(
            player.clone(),
            &quest,
            2,
            RESTORE_ITEMS_CHOICE_ID,
            curve(),
            &[],
            scripts(),
            200,
        ),
        Err(QuestRuleError::Item(ItemRuleError::UnknownItem { .. }))
    ));
    assert!(matches!(
        select_choice(
            player,
            &quest,
            2,
            RESTORE_ITEMS_CHOICE_ID,
            curve(),
            &definitions,
            scripts(),
            200,
        ),
        Err(QuestRuleError::Item(ItemRuleError::InventoryFull))
    ));
    assert_eq!(item_count(&unchanged, ITEM_A), 0);
    assert_eq!(item_count(&unchanged, ITEM_B), 1);
    assert_eq!(progress(&unchanged, quest.id), QuestProgress::Completed);
}

#[test]
fn repeat_start_changes_only_normal_quest_state_for_completed_restoration() {
    let definitions = item_definitions();
    let mut quest = quest(100);
    quest.start.repeat.interval_ms = Some(0);
    quest.dialogue.completion.lost = Some(QuestLostItemDialogue {
        prompt_pages: vec!["Restore?".to_owned()],
        success_pages: vec!["Restored".to_owned()],
        items: vec![QuestRestorableItem {
            item_id: ITEM_A,
            target_count: 1,
            expiration: None,
            provenance: QuestRestorationProvenance::AuditedCompletionGrant,
            eligibility: QuestRestorationEligibility {
                owner_state: RequiredQuestState::Completed,
                required_quests: &[],
                forbidden_quests: &[],
                absent_skill_ids: &[],
                absent_item_ids: &[ITEM_A],
            },
        }],
    });
    let mut player = player(Vec::new(), 1);
    player.quests.push(PlayerQuest {
        completed_at_unix_ms: 100,
        ..player_quest(quest.id, QuestStatus::Completed)
    });
    assert!(lost_item_restoration_needed(
        &player,
        &quest,
        &definitions,
        200
    ));

    let repeated = select_choice(
        player,
        &quest,
        1,
        ACCEPT_CHOICE_ID,
        curve(),
        &definitions,
        scripts(),
        200,
    )
    .expect("repeat quest start");

    assert_eq!(progress(&repeated.player, quest.id), QuestProgress::Started);
    assert_eq!(item_count(&repeated.player, ITEM_A), 0);
    assert!(!lost_item_restoration_needed(
        &repeated.player,
        &quest,
        &definitions,
        200
    ));
    assert!(matches!(
        select_choice(
            repeated.player,
            &quest,
            2,
            RESTORE_ITEMS_CHOICE_ID,
            curve(),
            &definitions,
            scripts(),
            200,
        ),
        Err(QuestRuleError::LostItemRestorationUnavailable { quest_id: 100 })
    ));
}

#[test]
fn quest_3310_reactor_device_restoration_is_active_only_while_started() {
    const DEVICE: u32 = 4_031_698;

    let definitions = vec![ItemDefinition {
        item_id: DEVICE,
        name: "Reactor device".to_owned(),
        stack_max: 1,
        ..ItemDefinition::default()
    }];
    let mut quest = quest(3_310);
    quest.dialogue.completion.lost = Some(QuestLostItemDialogue {
        prompt_pages: vec!["Did you lose the reactor device?".to_owned()],
        success_pages: vec!["Take another device.".to_owned()],
        items: vec![QuestRestorableItem {
            item_id: DEVICE,
            target_count: 1,
            expiration: None,
            provenance: QuestRestorationProvenance::AuditedReactorDevice,
            eligibility: QuestRestorationEligibility {
                owner_state: RequiredQuestState::Started,
                required_quests: &[],
                forbidden_quests: &[],
                absent_skill_ids: &[],
                absent_item_ids: &[DEVICE],
            },
        }],
    });
    let mut player = player(Vec::new(), 1);
    player
        .quests
        .push(player_quest(quest.id, QuestStatus::Started));

    let restored = select_choice(
        player,
        &quest,
        2,
        RESTORE_ITEMS_CHOICE_ID,
        curve(),
        &definitions,
        scripts(),
        200,
    )
    .expect("3310 active device restoration");
    assert_eq!(item_count(&restored.player, DEVICE), 1);
    assert_eq!(progress(&restored.player, quest.id), QuestProgress::Started);

    let mut completed = restored.player;
    completed
        .inventory
        .as_mut()
        .expect("inventory")
        .stacks
        .clear();
    completed.quests[0].status = QuestStatus::Completed as i32;
    assert!(!lost_item_restoration_needed(
        &completed,
        &quest,
        &definitions,
        200
    ));
    assert!(matches!(
        select_choice(
            completed,
            &quest,
            2,
            RESTORE_ITEMS_CHOICE_ID,
            curve(),
            &definitions,
            scripts(),
            200,
        ),
        Err(QuestRuleError::Unavailable { quest_id: 3_310 })
    ));
}

#[test]
fn lost_item_restoration_rejects_expired_absolute_grants() {
    let definitions = item_definitions();
    let mut quest = quest(100);
    configure_lost_items(&mut quest, &[(ITEM_A, 1)]);
    quest
        .dialogue
        .completion
        .lost
        .as_mut()
        .expect("lost interaction")
        .items[0]
        .expiration = Some(QuestItemExpiration::AbsoluteUnixMilliseconds(200));
    let mut player = player(Vec::new(), 1);
    player.revision = 7;
    player
        .quests
        .push(player_quest(quest.id, QuestStatus::Started));

    assert!(!lost_item_restoration_needed(
        &player,
        &quest,
        &definitions,
        200
    ));
    assert!(matches!(
        select_choice(
            player.clone(),
            &quest,
            2,
            RESTORE_ITEMS_CHOICE_ID,
            curve(),
            &definitions,
            scripts(),
            200,
        ),
        Err(QuestRuleError::NoMissingRestorableItems { quest_id: 100 })
    ));
    assert_eq!(player.revision, 7);
    assert_eq!(item_count(&player, ITEM_A), 0);
}

#[test]
fn lost_item_restoration_grants_only_live_items_from_a_mixed_branch() {
    let definitions = item_definitions();
    let mut quest = quest(100);
    configure_lost_items(&mut quest, &[(ITEM_A, 1), (ITEM_B, 1)]);
    let items = &mut quest
        .dialogue
        .completion
        .lost
        .as_mut()
        .expect("lost interaction")
        .items;
    items[0].expiration = Some(QuestItemExpiration::AbsoluteUnixMilliseconds(200));
    items[1].expiration = Some(QuestItemExpiration::AbsoluteUnixMilliseconds(201));
    let mut player = player(Vec::new(), 2);
    player
        .quests
        .push(player_quest(quest.id, QuestStatus::Started));

    assert!(lost_item_restoration_needed(
        &player,
        &quest,
        &definitions,
        200
    ));
    let restored = select_choice(
        player,
        &quest,
        2,
        RESTORE_ITEMS_CHOICE_ID,
        curve(),
        &definitions,
        scripts(),
        200,
    )
    .expect("restore the live grant");

    assert!(restored.changed);
    assert_eq!(item_count(&restored.player, ITEM_A), 0);
    assert_eq!(item_count(&restored.player, ITEM_B), 1);
    assert_eq!(
        restored
            .player
            .inventory
            .as_ref()
            .expect("inventory")
            .stacks[0]
            .expires_at_unix_ms,
        201
    );
    assert!(!lost_item_restoration_needed(
        &restored.player,
        &quest,
        &definitions,
        200
    ));
    assert!(matches!(
        select_choice(
            restored.player,
            &quest,
            2,
            RESTORE_ITEMS_CHOICE_ID,
            curve(),
            &definitions,
            scripts(),
            200,
        ),
        Err(QuestRuleError::NoMissingRestorableItems { quest_id: 100 })
    ));
}

#[test]
fn lost_item_restoration_restores_multiple_items_and_counts_equipment() {
    let definitions = item_definitions();
    let mut quest = quest(100);
    configure_lost_items(&mut quest, &[(ITEM_A, 3), (ITEM_B, 1)]);
    quest.completion.mobs.push(QuestMobObjective {
        mob_id: MOB_A,
        count: 1,
    });
    let mut player = player(vec![(ITEM_A, 1)], 3);
    player
        .inventory
        .as_mut()
        .expect("inventory")
        .equipment
        .push(EquippedItem {
            item_id: ITEM_A,
            ..EquippedItem::default()
        });
    player
        .quests
        .push(player_quest(quest.id, QuestStatus::Started));

    let restored = select_choice(
        player,
        &quest,
        2,
        RESTORE_ITEMS_CHOICE_ID,
        curve(),
        &definitions,
        scripts(),
        200,
    )
    .expect("restore every missing quest item");

    assert_eq!(item_count(&restored.player, ITEM_A), 2);
    assert_eq!(item_count(&restored.player, ITEM_B), 1);
    assert_eq!(progress(&restored.player, quest.id), QuestProgress::Started);
    assert!(!completion_readiness(&restored.player, &quest, &definitions, scripts()).ready);
}

#[test]
fn lost_item_restoration_capacity_failure_is_atomic() {
    let definitions = item_definitions();
    let mut quest = quest(100);
    configure_lost_items(&mut quest, &[(ITEM_A, 5), (ITEM_B, 1)]);
    let mut player = player(vec![(ITEM_A, 1)], 1);
    player
        .quests
        .push(player_quest(quest.id, QuestStatus::Started));
    let unchanged = player.clone();

    assert!(matches!(
        select_choice(
            player,
            &quest,
            2,
            RESTORE_ITEMS_CHOICE_ID,
            curve(),
            &definitions,
            scripts(),
            200,
        ),
        Err(QuestRuleError::Item(ItemRuleError::InventoryFull))
    ));
    assert_eq!(item_count(&unchanged, ITEM_A), 1);
    assert_eq!(item_count(&unchanged, ITEM_B), 0);
    assert_eq!(progress(&unchanged, quest.id), QuestProgress::Started);
}

#[test]
fn forged_lost_item_choices_reject_wrong_npc_inactive_and_unmapped_quests() {
    let definitions = item_definitions();
    let mut quest = quest(100);
    configure_lost_items(&mut quest, &[(ITEM_A, 1)]);
    let mut player = player(Vec::new(), 1);
    player
        .quests
        .push(player_quest(quest.id, QuestStatus::Started));

    assert!(matches!(
        select_choice(
            player.clone(),
            &quest,
            999,
            RESTORE_ITEMS_CHOICE_ID,
            curve(),
            &definitions,
            scripts(),
            200,
        ),
        Err(QuestRuleError::WrongNpc { npc_id: 999 })
    ));

    let mut inactive = player.clone();
    inactive.quests[0].status = QuestStatus::Completed as i32;
    assert!(
        select_choice(
            inactive,
            &quest,
            2,
            RESTORE_ITEMS_CHOICE_ID,
            curve(),
            &definitions,
            scripts(),
            200,
        )
        .is_err()
    );

    let mut ordinary = quest.clone();
    ordinary.dialogue.completion.lost = None;
    assert!(matches!(
        select_choice(
            player.clone(),
            &ordinary,
            2,
            RESTORE_ITEMS_CHOICE_ID,
            curve(),
            &definitions,
            scripts(),
            200,
        ),
        Err(QuestRuleError::LostItemRestorationUnavailable { quest_id: 100 })
    ));
    assert_eq!(item_count(&player, ITEM_A), 0);
    assert_eq!(progress(&player, quest.id), QuestProgress::Started);

    let mut satisfied = player.clone();
    satisfied
        .inventory
        .as_mut()
        .expect("inventory")
        .stacks
        .push(InventoryItemStack {
            item_id: ITEM_A,
            quantity: 2,
            expires_at_unix_ms: 0,
        });
    quest
        .dialogue
        .completion
        .lost
        .as_mut()
        .expect("lost interaction")
        .items[0]
        .target_count = 2;
    assert!(matches!(
        select_choice(
            satisfied,
            &quest,
            2,
            RESTORE_ITEMS_CHOICE_ID,
            curve(),
            &definitions,
            scripts(),
            200,
        ),
        Err(QuestRuleError::NoMissingRestorableItems { quest_id: 100 })
    ));
}
