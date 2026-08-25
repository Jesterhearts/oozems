use crate::quests::test_support::*;

#[test]
fn reward_capacity_failure_is_atomic() {
    let definitions = item_definitions();
    let mut quest = quest(100);
    quest.completion_actions.fixed_items.push(QuestItemDelta {
        item_id: ITEM_B,
        count: 1,
        expiration: None,
    });
    quest.completion_actions.money = 100;
    quest.completion_actions.npc_animation_action = Some("quest".to_owned());
    let mut player = player(vec![(ITEM_A, 10)], 1);
    player.quests.push(player_quest(100, QuestStatus::Started));
    let unchanged = player.clone();

    assert!(matches!(
        select_choice(
            player,
            &quest,
            2,
            COMPLETE_CHOICE_ID,
            curve(),
            &definitions,
            scripts(),
            200,
        ),
        Err(QuestRuleError::Item(ItemRuleError::InventoryFull))
    ));
    assert_eq!(item_count(&unchanged, ITEM_A), 10);
    assert_eq!(unchanged.mesos, 0);
    assert_eq!(unchanged.quests[0].status, QuestStatus::Started as i32);
}

#[test]
fn deadline_separated_reward_stacks_are_preflighted_atomically() {
    let definitions = item_definitions();
    let mut quest = quest(100);
    quest.completion_actions.fixed_items = vec![
        QuestItemDelta {
            item_id: ITEM_A,
            count: 1,
            expiration: Some(QuestItemExpiration::AbsoluteUnixMilliseconds(10_000)),
        },
        QuestItemDelta {
            item_id: ITEM_A,
            count: 1,
            expiration: Some(QuestItemExpiration::AbsoluteUnixMilliseconds(20_000)),
        },
    ];
    quest.completion_actions.money = 100;
    let mut player = player(Vec::new(), 1);
    player
        .quests
        .push(player_quest(quest.id, QuestStatus::Started));
    let unchanged = player.clone();

    assert!(matches!(
        select_choice(
            player,
            &quest,
            2,
            COMPLETE_CHOICE_ID,
            curve(),
            &definitions,
            scripts(),
            200,
        ),
        Err(QuestRuleError::Item(ItemRuleError::InventoryFull))
    ));
    assert_eq!(item_count(&unchanged, ITEM_A), 0);
    assert_eq!(unchanged.mesos, 0);
    assert_eq!(progress(&unchanged, quest.id), QuestProgress::Started);
}

#[test]
fn objective_consumption_creates_reward_room() {
    let definitions = item_definitions();
    let mut quest = quest(100);
    quest.completion.items.push(item_requirement(ITEM_A, 1));
    quest.completion_actions.fixed_items = vec![
        QuestItemDelta {
            item_id: ITEM_A,
            count: -1,
            expiration: None,
        },
        QuestItemDelta {
            item_id: ITEM_B,
            count: 1,
            expiration: None,
        },
    ];
    let mut player = player(vec![(ITEM_A, 1)], 1);
    player.quests.push(player_quest(100, QuestStatus::Started));

    let completed = select_choice(
        player,
        &quest,
        2,
        COMPLETE_CHOICE_ID,
        curve(),
        &definitions,
        scripts(),
        200,
    )
    .expect("complete after removal");

    assert_eq!(item_count(&completed.player, ITEM_A), 0);
    assert_eq!(item_count(&completed.player, ITEM_B), 1);
    assert_eq!(progress(&completed.player, 100), QuestProgress::Completed);
}

#[test]
fn completion_grants_mesos_fame_and_experience_once() {
    let definitions = item_definitions();
    let mut quest = quest(100);
    quest.completion_actions = reward_actions();
    quest.completion_actions.npc_animation_action = Some("quest".to_owned());
    let mut player = player(Vec::new(), 1);
    player.quests.push(player_quest(100, QuestStatus::Started));

    let completed = select_choice(
        player,
        &quest,
        2,
        COMPLETE_CHOICE_ID,
        curve(),
        &definitions,
        scripts(),
        200,
    )
    .expect("complete quest");
    assert_eq!(completed.player.mesos, 100);
    let stats = completed.player.stats.as_ref().expect("stats");
    assert_eq!(stats.fame, 2);
    assert_eq!(stats.experience, 5);
    assert_eq!(completed.player.quests[0].completed_at_unix_ms, 200);
    assert_eq!(completed.npc_animation_action.as_deref(), Some("quest"));
    let unchanged = completed.player.clone();

    assert!(matches!(
        select_choice(
            completed.player,
            &quest,
            2,
            COMPLETE_CHOICE_ID,
            curve(),
            &definitions,
            scripts(),
            300,
        ),
        Err(QuestRuleError::Unavailable { quest_id: 100 })
    ));
    assert_eq!(unchanged.mesos, 100);
    assert_eq!(unchanged.stats.expect("stats").experience, 5);
}

#[test]
fn repeat_acceptance_resets_timestamps_and_progress() {
    let definitions = item_definitions();
    let mut quest = quest(100);
    quest.start.repeat = QuestRepeatMetadata {
        interval_ms: Some(100),
        ..QuestRepeatMetadata::default()
    };
    let mut player = player(Vec::new(), 1);
    player.quests.push(PlayerQuest {
        quest_id: 100,
        status: QuestStatus::Completed as i32,
        mob_progress: vec![QuestMobProgress {
            mob_id: MOB_A,
            count: 5,
        }],
        accepted_at_unix_ms: 10,
        completed_at_unix_ms: 100,
        dialogue_step: 0,
        completion_quiz_passed: false,
    });

    assert!(!is_available(&player, &quest, &definitions, scripts(), 199));
    assert!(is_available(&player, &quest, &definitions, scripts(), 200));
    let accepted = select_choice(
        player,
        &quest,
        1,
        ACCEPT_CHOICE_ID,
        curve(),
        &definitions,
        scripts(),
        200,
    )
    .expect("repeat quest");
    let entry = &accepted.player.quests[0];
    assert_eq!(entry.status, QuestStatus::Started as i32);
    assert_eq!(entry.accepted_at_unix_ms, 200);
    assert_eq!(entry.completed_at_unix_ms, 0);
    assert!(entry.mob_progress.is_empty());
}

#[test]
fn zero_interval_allows_immediate_repeat_acceptance() {
    let definitions = item_definitions();
    let mut quest = quest(100);
    quest.start.repeat.interval_ms = Some(0);
    let mut player = player(Vec::new(), 1);
    player.quests.push(PlayerQuest {
        quest_id: 100,
        status: QuestStatus::Completed as i32,
        completed_at_unix_ms: 100,
        ..PlayerQuest::default()
    });

    assert!(is_available(&player, &quest, &definitions, scripts(), 100));
}

#[test]
fn weighted_selection_is_deterministic_and_grants_one_alternative() {
    let rewards = vec![
        QuestWeightedItem {
            item_id: ITEM_B,
            count: 1,
            expiration: None,
            weight: 1,
            eligibility: Default::default(),
        },
        QuestWeightedItem {
            item_id: ITEM_C,
            count: 1,
            expiration: None,
            weight: 3,
            eligibility: Default::default(),
        },
    ];
    let selected = select_weighted_item("player", 100, 123, &rewards).expect("selection");
    assert_eq!(
        select_weighted_item("player", 100, 123, &rewards),
        Some(selected)
    );
    let definitions = item_definitions();
    let mut quest = quest(100);
    quest.completion_actions.weighted_items = rewards;
    let mut player = player(Vec::new(), 2);
    player.quests.push(PlayerQuest {
        accepted_at_unix_ms: 123,
        ..player_quest(100, QuestStatus::Started)
    });

    let completed = select_choice(
        player,
        &quest,
        2,
        COMPLETE_CHOICE_ID,
        curve(),
        &definitions,
        scripts(),
        200,
    )
    .expect("weighted reward");

    assert_eq!(
        item_count(&completed.player, ITEM_B) + item_count(&completed.player, ITEM_C),
        1
    );
}

#[test]
fn weighted_and_selectable_rewards_preserve_expiration() {
    let definitions = item_definitions();
    let mut weighted = quest(100);
    weighted.completion_actions.weighted_items = vec![QuestWeightedItem {
        item_id: ITEM_A,
        count: 1,
        expiration: Some(QuestItemExpiration::RelativeMilliseconds(1_000)),
        weight: 1,
        eligibility: QuestRewardEligibility::default(),
    }];
    let mut weighted_player = player(Vec::new(), 1);
    weighted_player
        .quests
        .push(player_quest(weighted.id, QuestStatus::Started));
    let weighted = select_choice(
        weighted_player,
        &weighted,
        2,
        COMPLETE_CHOICE_ID,
        curve(),
        &definitions,
        scripts(),
        200,
    )
    .expect("weighted expiring reward");
    assert_eq!(
        weighted.player.inventory.expect("inventory").stacks[0].expires_at_unix_ms,
        1_200
    );

    let mut selectable = quest(200);
    selectable
        .completion_actions
        .selectable_items
        .push(QuestSelectableItemReward {
            item_id: ITEM_A,
            count: 1,
            expiration: Some(QuestItemExpiration::AbsoluteUnixMilliseconds(5_000)),
            eligibility: QuestRewardEligibility::default(),
        });
    let mut player = player(Vec::new(), 1);
    player
        .quests
        .push(player_quest(selectable.id, QuestStatus::Started));
    let choice = eligible_selectable_reward_choices(&player, &selectable)[0].0;
    let selected = select_choice(
        player,
        &selectable,
        2,
        choice,
        curve(),
        &definitions,
        scripts(),
        200,
    )
    .expect("selectable expiring reward");
    assert_eq!(
        selected.player.inventory.expect("inventory").stacks[0].expires_at_unix_ms,
        5_000
    );
}

#[test]
fn item_rewards_filter_by_job_family_and_gender_before_selection() {
    let definitions = item_definitions();
    let warrior_mask = 1 << 1;
    let magician_mask = 1 << 2;
    let mut quest = quest(100);
    quest.completion_actions.conditional_items = vec![
        QuestConditionalItemReward {
            item_id: ITEM_B,
            count: 1,
            expiration: None,
            eligibility: QuestRewardEligibility {
                job_mask: Some(warrior_mask),
                gender: Some(QuestRewardGender::Male),
            },
        },
        QuestConditionalItemReward {
            item_id: ITEM_C,
            count: 1,
            expiration: None,
            eligibility: QuestRewardEligibility {
                job_mask: Some(warrior_mask),
                gender: Some(QuestRewardGender::Female),
            },
        },
    ];
    quest.completion_actions.weighted_items = vec![
        QuestWeightedItem {
            item_id: ITEM_A,
            count: 1,
            expiration: None,
            weight: 1,
            eligibility: QuestRewardEligibility {
                job_mask: Some(warrior_mask),
                gender: None,
            },
        },
        QuestWeightedItem {
            item_id: ITEM_C,
            count: 1,
            expiration: None,
            weight: u32::MAX,
            eligibility: QuestRewardEligibility {
                job_mask: Some(magician_mask),
                gender: None,
            },
        },
    ];
    let mut player = player(Vec::new(), 2);
    player.stats.as_mut().expect("stats").job_id = 112;
    player.appearance = Some(CharacterAppearance {
        gender: CharacterGender::Male as i32,
        ..CharacterAppearance::default()
    });
    player.quests.push(player_quest(100, QuestStatus::Started));

    let completed = select_choice(
        player,
        &quest,
        2,
        COMPLETE_CHOICE_ID,
        curve(),
        &definitions,
        scripts(),
        200,
    )
    .expect("filtered quest reward");

    assert_eq!(item_count(&completed.player, ITEM_A), 1);
    assert_eq!(item_count(&completed.player, ITEM_B), 1);
    assert_eq!(item_count(&completed.player, ITEM_C), 0);
}

#[test]
fn selectable_completion_grants_exactly_the_chosen_reward_with_other_actions() {
    let definitions = item_definitions();
    let mut quest = quest(100);
    quest.completion_actions.fixed_items = vec![
        QuestItemDelta {
            item_id: ITEM_A,
            count: -1,
            expiration: None,
        },
        QuestItemDelta {
            item_id: ITEM_A,
            count: 1,
            expiration: None,
        },
    ];
    quest.completion_actions.weighted_items = vec![QuestWeightedItem {
        item_id: ITEM_A,
        count: 1,
        expiration: None,
        weight: 1,
        eligibility: QuestRewardEligibility::default(),
    }];
    quest.completion_actions.selectable_items = vec![
        selectable_reward(ITEM_B, 1, QuestRewardEligibility::default()),
        selectable_reward(ITEM_C, 1, QuestRewardEligibility::default()),
    ];
    quest.completion_actions.money = 100;
    quest.completion_actions.experience = 5;
    quest.completion_actions.fame = 2;
    quest.completion_actions.npc_animation_action = Some("reward".to_owned());
    quest.completion.script = Some("selected_reward_script".to_owned());
    let scripts = script_catalog(
        r#"
                [[scripts]]
                name = "selected_reward_script"

                [[scripts.actions]]
                type = "item_delta"
                item_id = 4000000
                delta = 1

                [[scripts.actions]]
                type = "mesos"
                delta = 7
            "#,
        &quest,
        &definitions,
    );
    let mut player = player(vec![(ITEM_A, 1)], 3);
    player.quests.push(PlayerQuest {
        accepted_at_unix_ms: 123,
        ..player_quest(quest.id, QuestStatus::Started)
    });
    let choices = eligible_selectable_reward_choices(&player, &quest);
    assert_eq!(choices.len(), 2);
    assert_ne!(choices[0].0, ACCEPT_CHOICE_ID);
    assert_ne!(choices[0].0, COMPLETE_CHOICE_ID);
    assert_ne!(choices[0].0, RESTORE_ITEMS_CHOICE_ID);
    assert_ne!(
        Some(choices[0].0),
        answer_choice_id(QuestQuestionPhase::Completion, 0, 0)
    );

    let completed = select_choice(
        player,
        &quest,
        2,
        choices[1].0,
        curve(),
        &definitions,
        &scripts,
        200,
    )
    .expect("selected completion reward");

    assert_eq!(item_count(&completed.player, ITEM_A), 3);
    assert_eq!(item_count(&completed.player, ITEM_B), 0);
    assert_eq!(item_count(&completed.player, ITEM_C), 1);
    assert_eq!(completed.player.mesos, 107);
    let stats = completed.player.stats.as_ref().expect("stats");
    assert_eq!(stats.experience, 5);
    assert_eq!(stats.fame, 2);
    let entry = &completed.player.quests[0];
    assert_eq!(entry.status, QuestStatus::Completed as i32);
    assert_eq!(entry.completed_at_unix_ms, 200);
    assert_eq!(completed.npc_animation_action.as_deref(), Some("reward"));
}

#[test]
fn forged_ineligible_and_out_of_range_reward_choices_are_atomic() {
    let definitions = item_definitions();
    let mut quest = quest(100);
    quest.completion_actions.fixed_items.push(QuestItemDelta {
        item_id: ITEM_A,
        count: -1,
        expiration: None,
    });
    quest.completion_actions.selectable_items = vec![
        selectable_reward(
            ITEM_B,
            1,
            QuestRewardEligibility {
                job_mask: Some(1 << 1),
                gender: Some(QuestRewardGender::Female),
            },
        ),
        selectable_reward(ITEM_C, 1, QuestRewardEligibility::default()),
    ];
    let mut player = player(vec![(ITEM_A, 1)], 2);
    player.stats.as_mut().expect("stats").job_id = 112;
    player.appearance = Some(CharacterAppearance {
        gender: CharacterGender::Male as i32,
        ..CharacterAppearance::default()
    });
    player
        .quests
        .push(player_quest(quest.id, QuestStatus::Started));

    for choice_id in [
        COMPLETE_CHOICE_ID,
        selectable_reward_choice_id(0).expect("first selectable choice"),
        selectable_reward_choice_id(2).expect("out-of-range selectable choice"),
    ] {
        assert!(matches!(
            select_choice(
                player.clone(),
                &quest,
                2,
                choice_id,
                curve(),
                &definitions,
                scripts(),
                200,
            ),
            Err(QuestRuleError::InvalidChoice { choice_id: rejected }) if rejected == choice_id
        ));
    }
    assert_eq!(item_count(&player, ITEM_A), 1);
    assert_eq!(item_count(&player, ITEM_B), 0);
    assert_eq!(item_count(&player, ITEM_C), 0);
    assert_eq!(progress(&player, quest.id), QuestProgress::Started);
}

#[test]
fn selectable_reward_capacity_failure_is_atomic() {
    let definitions = item_definitions();
    let mut quest = quest(100);
    quest
        .completion_actions
        .selectable_items
        .push(selectable_reward(
            ITEM_B,
            1,
            QuestRewardEligibility::default(),
        ));
    quest.completion_actions.money = 100;
    quest.completion_actions.npc_animation_action = Some("reward".to_owned());
    let mut player = player(vec![(ITEM_A, 10)], 1);
    player
        .quests
        .push(player_quest(quest.id, QuestStatus::Started));
    let choice_id = eligible_selectable_reward_choices(&player, &quest)[0].0;

    assert!(matches!(
        select_choice(
            player.clone(),
            &quest,
            2,
            choice_id,
            curve(),
            &definitions,
            scripts(),
            200,
        ),
        Err(QuestRuleError::Item(ItemRuleError::InventoryFull))
    ));
    assert_eq!(item_count(&player, ITEM_A), 10);
    assert_eq!(item_count(&player, ITEM_B), 0);
    assert_eq!(player.mesos, 0);
    assert_eq!(progress(&player, quest.id), QuestProgress::Started);
}
