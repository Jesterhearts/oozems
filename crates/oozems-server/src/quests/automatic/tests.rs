use crate::quests::test_support::*;

#[test]
fn automatic_transitions_enforce_equipped_item_checks() {
    let definitions = item_definitions();
    let mut automatic_start = quest(100);
    automatic_start.info.auto_start = true;
    automatic_start.start.equipped_items.all_of.push(ITEM_B);
    let bag_only = player(vec![(ITEM_B, 1)], 2);

    let blocked_start = advance_automatic_quests(
        bag_only.clone(),
        [&automatic_start],
        curve(),
        &definitions,
        scripts(),
        1_000,
    );
    assert!(blocked_start.started_quest_ids.is_empty());
    let mut equipped = bag_only;
    equipped
        .inventory
        .as_mut()
        .expect("inventory")
        .equipment
        .push(EquippedItem {
            slot: EquipmentSlot::Top as i32,
            item_id: ITEM_B,
            expires_at_unix_ms: 0,
        });
    let started = advance_automatic_quests(
        equipped,
        [&automatic_start],
        curve(),
        &definitions,
        scripts(),
        1_000,
    );
    assert_eq!(started.started_quest_ids, vec![automatic_start.id]);

    let mut automatic_completion = quest(200);
    automatic_completion.info.auto_pre_complete = true;
    automatic_completion
        .completion
        .equipped_items
        .all_of
        .push(ITEM_B);
    let mut active = player(vec![(ITEM_B, 1)], 2);
    active
        .quests
        .push(player_quest(automatic_completion.id, QuestStatus::Started));
    let blocked_completion = advance_automatic_quests(
        active.clone(),
        [&automatic_completion],
        curve(),
        &definitions,
        scripts(),
        1_000,
    );
    assert!(blocked_completion.completed_quest_ids.is_empty());
    active
        .inventory
        .as_mut()
        .expect("inventory")
        .equipment
        .push(EquippedItem {
            slot: EquipmentSlot::Top as i32,
            item_id: ITEM_B,
            expires_at_unix_ms: 0,
        });
    let completed = advance_automatic_quests(
        active,
        [&automatic_completion],
        curve(),
        &definitions,
        scripts(),
        1_000,
    );
    assert_eq!(completed.completed_quest_ids, vec![automatic_completion.id]);
}

#[test]
fn automatic_transitions_enforce_monster_book_checks_without_consuming_cards() {
    let definitions = item_definitions();
    let requirement = QuestMonsterBookCardRequirement {
        card_item_id: CARD_A,
        minimum_count: Some(1),
        maximum_count: None,
    };
    let mut automatic_start = quest(100);
    automatic_start.info.auto_start = true;
    automatic_start.start.monster_book.cards.push(requirement);
    let missing = player(Vec::new(), 1);

    let blocked_start = advance_automatic_quests(
        missing.clone(),
        [&automatic_start],
        curve(),
        &definitions,
        scripts(),
        1_000,
    );
    assert!(blocked_start.started_quest_ids.is_empty());
    let mut collected = missing;
    collected.monster_book_cards.push(MonsterBookCard {
        card_item_id: CARD_A,
        count: 1,
    });
    let started = advance_automatic_quests(
        collected,
        [&automatic_start],
        curve(),
        &definitions,
        scripts(),
        1_000,
    );
    assert_eq!(started.started_quest_ids, vec![automatic_start.id]);

    let mut automatic_completion = quest(200);
    automatic_completion.info.auto_pre_complete = true;
    automatic_completion
        .completion
        .monster_book
        .cards
        .push(requirement);
    let mut active = player(Vec::new(), 1);
    active
        .quests
        .push(player_quest(automatic_completion.id, QuestStatus::Started));
    let blocked_completion = advance_automatic_quests(
        active.clone(),
        [&automatic_completion],
        curve(),
        &definitions,
        scripts(),
        1_000,
    );
    assert!(blocked_completion.completed_quest_ids.is_empty());
    active.monster_book_cards.push(MonsterBookCard {
        card_item_id: CARD_A,
        count: 1,
    });
    let completed = advance_automatic_quests(
        active,
        [&automatic_completion],
        curve(),
        &definitions,
        scripts(),
        1_000,
    );
    assert_eq!(completed.completed_quest_ids, vec![automatic_completion.id]);
    assert_eq!(
        completed.player.monster_book_cards,
        vec![MonsterBookCard {
            card_item_id: CARD_A,
            count: 1,
        }]
    );
}

#[test]
fn every_automatic_start_flag_enforces_normal_requirements() {
    let definitions = item_definitions();
    let mut auto_start = quest(100);
    auto_start.info.auto_start = true;
    auto_start.start.minimum_level = Some(10);
    let mut auto_accept = quest(200);
    auto_accept.info.auto_accept = true;
    auto_accept.start.minimum_level = Some(10);
    let mut normal_auto_start = quest(300);
    normal_auto_start.start.normal_auto_start = true;
    normal_auto_start.start.minimum_level = Some(10);
    let quests = [&auto_start, &auto_accept, &normal_auto_start];

    let low_level = advance_automatic_quests(
        player(Vec::new(), 1),
        quests,
        curve(),
        &definitions,
        scripts(),
        1_000,
    );
    assert!(low_level.started_quest_ids.is_empty());
    assert_eq!(progress(&low_level.player, 100), QuestProgress::NotStarted);
    assert_eq!(progress(&low_level.player, 200), QuestProgress::NotStarted);
    assert_eq!(progress(&low_level.player, 300), QuestProgress::NotStarted);

    let mut eligible = player(Vec::new(), 1);
    eligible.level = 10;
    let eligible =
        advance_automatic_quests(eligible, quests, curve(), &definitions, scripts(), 1_000);
    assert_eq!(eligible.started_quest_ids, vec![100, 200, 300]);
}

#[test]
fn combined_automatic_start_flags_enforce_normal_requirements() {
    let definitions = item_definitions();
    let mut normal_auto_start = quest(100);
    normal_auto_start.info.auto_start = true;
    normal_auto_start.start.normal_auto_start = true;
    normal_auto_start.start.minimum_level = Some(10);
    let mut auto_accept = quest(200);
    auto_accept.info.auto_start = true;
    auto_accept.info.auto_accept = true;
    auto_accept.start.allowed_jobs = vec![100];
    let quests = [&normal_auto_start, &auto_accept];

    let ineligible = advance_automatic_quests(
        player(Vec::new(), 1),
        quests,
        curve(),
        &definitions,
        scripts(),
        1_000,
    );
    assert!(ineligible.started_quest_ids.is_empty());

    let mut eligible = player(Vec::new(), 1);
    eligible.level = 10;
    eligible.stats.as_mut().expect("stats").job_id = 100;
    let eligible =
        advance_automatic_quests(eligible, quests, curve(), &definitions, scripts(), 1_000);
    assert_eq!(eligible.started_quest_ids, vec![100, 200]);
}

#[test]
fn automatic_start_does_not_restart_a_completed_nonrepeatable_quest() {
    let definitions = item_definitions();
    let mut quest = quest(100);
    quest.info.auto_start = true;
    let mut player = player(Vec::new(), 1);
    player
        .quests
        .push(player_quest(quest.id, QuestStatus::Completed));

    let advanced =
        advance_automatic_quests(player, [&quest], curve(), &definitions, scripts(), 1_000);

    assert!(!advanced.changed);
    assert_eq!(
        progress(&advanced.player, quest.id),
        QuestProgress::Completed
    );
}

#[test]
fn timed_quests_expire_without_rewards() {
    let definitions = item_definitions();
    let mut quest = quest(100);
    quest.info.time_limit_ms = Some(100);
    quest.completion_actions.money = 500;
    let mut player = player(Vec::new(), 1);
    player.quests.push(PlayerQuest {
        accepted_at_unix_ms: 1_000,
        ..player_quest(quest.id, QuestStatus::Started)
    });
    crate::quest_records::set(&mut player, 100, 0, "owned".to_owned()).expect("owned record");
    crate::quest_records::set(&mut player, 999, 0, "redirected".to_owned())
        .expect("redirected record");

    let before_deadline = advance_automatic_quests(
        player.clone(),
        [&quest],
        curve(),
        &definitions,
        scripts(),
        1_099,
    );
    assert!(!before_deadline.changed);
    assert_eq!(
        crate::quest_records::get(&before_deadline.player, 100, 0),
        Some("owned")
    );
    let expired =
        advance_automatic_quests(player, [&quest], curve(), &definitions, scripts(), 1_100);

    assert!(expired.changed);
    assert_eq!(expired.expired_quest_ids, vec![100]);
    assert_eq!(progress(&expired.player, 100), QuestProgress::NotStarted);
    assert_eq!(expired.player.mesos, 0);
    assert_eq!(crate::quest_records::get(&expired.player, 100, 0), None);
    assert_eq!(
        crate::quest_records::get(&expired.player, 999, 0),
        Some("redirected")
    );
}

#[test]
fn expired_automatic_quest_is_not_reaccepted_in_the_same_pass() {
    let definitions = item_definitions();
    let mut quest = quest(100);
    quest.info.time_limit2_ms = Some(100);
    quest.info.auto_accept = true;
    let mut player = player(Vec::new(), 1);
    player.quests.push(PlayerQuest {
        accepted_at_unix_ms: 1_000,
        ..player_quest(quest.id, QuestStatus::Started)
    });

    let advanced =
        advance_automatic_quests(player, [&quest], curve(), &definitions, scripts(), 1_100);

    assert_eq!(advanced.expired_quest_ids, vec![100]);
    assert!(advanced.started_quest_ids.is_empty());
    assert_eq!(progress(&advanced.player, 100), QuestProgress::NotStarted);
}

#[test]
fn automatic_completion_uses_normal_readiness_and_rewards() {
    let definitions = item_definitions();
    let mut quest = quest(100);
    quest.info.auto_complete = true;
    quest.completion.items.push(item_requirement(ITEM_A, 1));
    quest.completion_actions.money = 100;
    let mut player = player(vec![(ITEM_A, 1)], 1);
    player.quests.push(PlayerQuest {
        accepted_at_unix_ms: 100,
        ..player_quest(quest.id, QuestStatus::Started)
    });

    let advanced =
        advance_automatic_quests(player, [&quest], curve(), &definitions, scripts(), 200);

    assert_eq!(advanced.completed_quest_ids, vec![100]);
    assert_eq!(progress(&advanced.player, 100), QuestProgress::Completed);
    assert_eq!(advanced.player.mesos, 100);
}

#[test]
fn automatic_precompletion_bypasses_ordinary_objectives() {
    let definitions = item_definitions();
    let mut quest = quest(100);
    quest.info.auto_pre_complete = true;
    quest.completion.required_level = Some(99);
    quest.completion.items.push(item_requirement(ITEM_A, 10));
    quest.completion_actions.money = 100;
    let mut player = player(Vec::new(), 1);
    player.quests.push(PlayerQuest {
        accepted_at_unix_ms: 100,
        ..player_quest(quest.id, QuestStatus::Started)
    });

    let advanced =
        advance_automatic_quests(player, [&quest], curve(), &definitions, scripts(), 200);

    assert_eq!(advanced.completed_quest_ids, vec![100]);
    assert_eq!(progress(&advanced.player, 100), QuestProgress::Completed);
    assert_eq!(advanced.player.mesos, 100);
}

#[test]
fn automatic_transitions_repeat_until_dependency_chains_are_stable() {
    let definitions = item_definitions();
    let mut dependent = quest(100);
    dependent.info.auto_accept = true;
    dependent.info.auto_complete = true;
    dependent.start.quests.push(QuestStateRequirement {
        quest_id: 200,
        state: RequiredQuestState::Completed,
    });
    let mut first = quest(200);
    first.info.auto_accept = true;
    first.info.auto_complete = true;
    first.completion_actions.next_quest_id = Some(100);

    let advanced = advance_automatic_quests(
        player(Vec::new(), 1),
        [&dependent, &first],
        curve(),
        &definitions,
        scripts(),
        1_000,
    );

    assert_eq!(advanced.started_quest_ids, vec![200, 100]);
    assert_eq!(advanced.completed_quest_ids, vec![200, 100]);
    assert_eq!(progress(&advanced.player, 100), QuestProgress::Completed);
    assert_eq!(progress(&advanced.player, 200), QuestProgress::Completed);
}

#[test]
fn record_writes_unlock_automatic_quests_on_the_next_fixed_point_pass() {
    let definitions = item_definitions();
    let mut dependent = quest(100);
    dependent.info.auto_accept = true;
    dependent.start.record_conditions.push(record_condition(
        300,
        vec![QuestRecordPredicate::Equal("ready".to_owned())],
    ));
    let mut producer = quest(200);
    producer.info.auto_accept = true;
    producer.start_actions.record_writes.push(QuestRecordWrite {
        quest_id: 300,
        index: 0,
        value: "ready".to_owned(),
    });

    let advanced = advance_automatic_quests(
        player(Vec::new(), 1),
        [&dependent, &producer],
        curve(),
        &definitions,
        scripts(),
        1_000,
    );

    assert_eq!(advanced.started_quest_ids, vec![200, 100]);
    assert_eq!(progress(&advanced.player, 100), QuestProgress::Started);
    assert_eq!(
        crate::quest_records::get(&advanced.player, 300, 0),
        Some("ready")
    );
}

#[test]
fn quest_state_actions_unlock_automatic_prerequisite_chains_in_one_advance() {
    let definitions = item_definitions();
    let mut dependent = quest(100);
    dependent.info.auto_accept = true;
    dependent.start.quests.push(QuestStateRequirement {
        quest_id: 300,
        state: RequiredQuestState::Completed,
    });
    let mut producer = quest(200);
    producer.info.auto_accept = true;
    producer
        .start_actions
        .quest_state_actions
        .push(QuestStateAction {
            quest_id: 300,
            state: QuestStateActionState::Completed,
        });

    let advanced = advance_automatic_quests(
        player(Vec::new(), 1),
        [&dependent, &producer],
        curve(),
        &definitions,
        scripts(),
        1_000,
    );

    assert_eq!(advanced.started_quest_ids, vec![200, 100]);
    assert_eq!(progress(&advanced.player, 100), QuestProgress::Started);
    assert_eq!(progress(&advanced.player, 300), QuestProgress::Completed);
}

#[test]
fn automatic_start_flags_enforce_records_while_auto_pre_complete_bypasses_them() {
    let definitions = item_definitions();
    let missing = record_condition(900, vec![QuestRecordPredicate::Equal("ready".to_owned())]);
    let mut auto_start = quest(100);
    auto_start.info.auto_start = true;
    auto_start.start.record_conditions.push(missing.clone());
    let mut auto_accept = quest(200);
    auto_accept.info.auto_accept = true;
    auto_accept.start.record_conditions.push(missing.clone());
    let mut auto_complete = quest(300);
    auto_complete.info.auto_complete = true;
    auto_complete
        .completion
        .record_conditions
        .push(missing.clone());
    let mut auto_pre_complete = quest(400);
    auto_pre_complete.info.auto_pre_complete = true;
    auto_pre_complete.completion.record_conditions.push(missing);
    let mut player = player(Vec::new(), 1);
    player.quests.push(player_quest(300, QuestStatus::Started));
    player.quests.push(player_quest(400, QuestStatus::Started));

    let advanced = advance_automatic_quests(
        player,
        [
            &auto_start,
            &auto_accept,
            &auto_complete,
            &auto_pre_complete,
        ],
        curve(),
        &definitions,
        scripts(),
        1_000,
    );

    assert!(advanced.started_quest_ids.is_empty());
    assert_eq!(advanced.completed_quest_ids, vec![400]);
    assert_eq!(progress(&advanced.player, 200), QuestProgress::NotStarted);
    assert_eq!(progress(&advanced.player, 300), QuestProgress::Started);
}

#[test]
fn blocked_automatic_action_is_atomic_and_does_not_stop_other_quests() {
    let definitions = item_definitions();
    let mut blocked = quest(100);
    blocked.info.auto_complete = true;
    blocked.completion_actions.fixed_items.push(QuestItemDelta {
        item_id: ITEM_B,
        count: 1,
        expiration: None,
    });
    blocked.completion_actions.skill_changes = vec![skill_change(1_003, 1, 1, Vec::new())];
    let mut startable = quest(200);
    startable.info.auto_accept = true;
    startable.start_actions.skill_changes = vec![skill_change(1_004, 1, 1, Vec::new())];
    let mut player = player(vec![(ITEM_A, 10)], 1);
    player.quests.push(PlayerQuest {
        accepted_at_unix_ms: 100,
        ..player_quest(blocked.id, QuestStatus::Started)
    });

    let advanced = advance_automatic_quests(
        player,
        [&blocked, &startable],
        curve(),
        &definitions,
        scripts(),
        200,
    );

    assert_eq!(
        progress(&advanced.player, blocked.id),
        QuestProgress::Started
    );
    assert_eq!(item_count(&advanced.player, ITEM_B), 0);
    assert_eq!(
        progress(&advanced.player, startable.id),
        QuestProgress::Started
    );
    assert_eq!(advanced.failures.len(), 1);
    assert_eq!(advanced.failures[0].quest_id, blocked.id);
    assert!(
        !advanced
            .player
            .learned_skills
            .iter()
            .any(|skill| skill.skill_id == 1_003)
    );
    assert!(
        advanced
            .player
            .learned_skills
            .iter()
            .any(|skill| skill.skill_id == 1_004)
    );
}

#[test]
fn automatic_completion_reports_selectable_reward_blocks_and_keeps_advancing() {
    let definitions = item_definitions();
    let mut no_eligible_reward = quest(100);
    no_eligible_reward.info.auto_complete = true;
    no_eligible_reward
        .completion_actions
        .selectable_items
        .push(selectable_reward(
            ITEM_B,
            1,
            QuestRewardEligibility {
                job_mask: None,
                gender: Some(QuestRewardGender::Female),
            },
        ));
    let mut selection_required = quest(200);
    selection_required.info.auto_pre_complete = true;
    selection_required
        .completion_actions
        .selectable_items
        .push(selectable_reward(
            ITEM_C,
            1,
            QuestRewardEligibility::default(),
        ));
    let mut unrelated = quest(300);
    unrelated.info.auto_accept = true;
    let mut player = player(Vec::new(), 2);
    player.appearance = Some(CharacterAppearance {
        gender: CharacterGender::Male as i32,
        ..CharacterAppearance::default()
    });
    player.quests = vec![
        player_quest(no_eligible_reward.id, QuestStatus::Started),
        player_quest(selection_required.id, QuestStatus::Started),
    ];

    let advanced = advance_automatic_quests(
        player,
        [&no_eligible_reward, &selection_required, &unrelated],
        curve(),
        &definitions,
        scripts(),
        200,
    );

    assert!(advanced.completed_quest_ids.is_empty());
    assert_eq!(advanced.started_quest_ids, vec![unrelated.id]);
    assert_eq!(
        progress(&advanced.player, no_eligible_reward.id),
        QuestProgress::Started
    );
    assert_eq!(
        progress(&advanced.player, selection_required.id),
        QuestProgress::Started
    );
    assert_eq!(
        progress(&advanced.player, unrelated.id),
        QuestProgress::Started
    );
    assert_eq!(advanced.failures.len(), 2);
    assert!(advanced.failures.iter().any(|failure| {
        failure.quest_id == no_eligible_reward.id
            && failure
                .message
                .contains("no selectable completion reward eligible")
    }));
    assert!(advanced.failures.iter().any(|failure| {
        failure.quest_id == selection_required.id
            && failure.message.contains("requires the player to select")
    }));
}

#[test]
fn automatic_quests_recheck_effect_requirements_to_a_fixed_point() {
    let definitions = item_definitions();
    let mut dependent = quest(100);
    dependent.info.auto_accept = true;
    dependent.start.effects.push(QuestEffectRequirement {
        item_id: EFFECT_ITEM,
        active: true,
    });
    let mut producer = quest(200);
    producer.info.auto_accept = true;
    producer.start_actions.buff_item_ids.push(EFFECT_ITEM);

    let advanced = advance_automatic_quests_with_effects(
        player(Vec::new(), 1),
        PlayerEffects::default(),
        [&dependent, &producer],
        curve(),
        &definitions,
        &[consume_effect(EFFECT_ITEM, None)],
        scripts(),
        environment(100),
    );

    assert!(advanced.failures.is_empty());
    assert_eq!(advanced.started_quest_ids, vec![200, 100]);
    assert_eq!(progress(&advanced.player, 100), QuestProgress::Started);
    assert_eq!(progress(&advanced.player, 200), QuestProgress::Started);
    assert!(advanced.effects.contains_item(EFFECT_ITEM));
}
