use crate::quests::test_support::*;

#[test]
fn completion_readiness_reports_equipped_item_objectives() {
    let definitions = item_definitions();
    let mut quest = quest(100);
    quest.completion.equipped_items = QuestEquippedItemRequirements {
        all_of: vec![ITEM_B],
        any_of: vec![ITEM_C],
    };
    let mut player = player(vec![(ITEM_C, 1)], 2);
    player.quests.push(player_quest(100, QuestStatus::Started));
    player
        .inventory
        .as_mut()
        .expect("inventory")
        .equipment
        .push(EquippedItem {
            slot: EquipmentSlot::Top as i32,
            item_id: ITEM_B,
            expires_at_unix_ms: 0,
        });

    let missing = completion_readiness(&player, &quest, &definitions, scripts());
    assert!(!missing.ready);
    assert_eq!(missing.objectives.len(), 2);
    assert!(
        missing
            .objectives
            .iter()
            .all(|objective| { objective.kind == super::QuestObjectiveKind::Equipment })
    );
    assert!(missing.objectives[0].complete);
    assert!(!missing.objectives[1].complete);
    assert!(missing.objectives[0].label.contains("Dagger A"));
    assert!(missing.objectives[1].label.contains("Dagger B"));

    player
        .inventory
        .as_mut()
        .expect("inventory")
        .equipment
        .push(EquippedItem {
            slot: EquipmentSlot::Bottom as i32,
            item_id: ITEM_C,
            expires_at_unix_ms: 0,
        });
    assert!(completion_readiness(&player, &quest, &definitions, scripts()).ready);

    player.inventory = None;
    let missing_inventory = completion_readiness(&player, &quest, &definitions, scripts());
    assert!(!missing_inventory.ready);
    assert!(
        missing_inventory
            .objectives
            .iter()
            .all(|objective| !objective.complete)
    );
}

#[test]
fn record_predicates_are_or_alternatives_and_missing_records_fail_closed() {
    let definitions = item_definitions();
    let mut quest = quest(100);
    quest.start.record_conditions.push(record_condition(
        200,
        vec![
            QuestRecordPredicate::Equal("007".to_owned()),
            QuestRecordPredicate::AtLeast(10),
            QuestRecordPredicate::AtMost(2),
        ],
    ));
    let mut player = player(Vec::new(), 1);

    assert!(!is_available(
        &player,
        &quest,
        &definitions,
        scripts(),
        1_000
    ));
    crate::quest_records::set(&mut player, 200, 0, "007".to_owned()).expect("exact record");
    assert!(is_available(
        &player,
        &quest,
        &definitions,
        scripts(),
        1_000
    ));
    let mut exact_quest = quest.clone();
    exact_quest.start.record_conditions[0].alternatives =
        vec![QuestRecordPredicate::Equal("007".to_owned())];
    assert!(is_available(
        &player,
        &exact_quest,
        &definitions,
        scripts(),
        1_000
    ));
    crate::quest_records::set(&mut player, 200, 0, "0070".to_owned())
        .expect("different leading zeros");
    assert!(!is_available(
        &player,
        &exact_quest,
        &definitions,
        scripts(),
        1_000
    ));
    assert!(is_available(
        &player,
        &quest,
        &definitions,
        scripts(),
        1_000
    ));
    crate::quest_records::set(&mut player, 200, 0, "AbC".to_owned()).expect("case record");
    assert!(!is_available(
        &player,
        &quest,
        &definitions,
        scripts(),
        1_000
    ));
    crate::quest_records::set(&mut player, 200, 0, "2".to_owned()).expect("upper-bound record");
    assert!(is_available(
        &player,
        &quest,
        &definitions,
        scripts(),
        1_000
    ));
}

#[test]
fn completion_record_progress_is_reported_and_completion_preserves_records() {
    let definitions = item_definitions();
    let mut quest = quest(100);
    quest.completion.record_conditions.push(record_condition(
        200,
        vec![QuestRecordPredicate::Equal("Done".to_owned())],
    ));
    let mut player = player(Vec::new(), 1);
    player
        .quests
        .push(player_quest(quest.id, QuestStatus::Started));

    let missing = completion_readiness(&player, &quest, &definitions, scripts());
    assert!(!missing.ready);
    assert_eq!(
        missing.objectives[0].kind,
        super::QuestObjectiveKind::Record
    );
    assert!(missing.objectives[0].label.contains("is missing"));
    crate::quest_records::set(&mut player, 200, 0, "done".to_owned()).expect("wrong-case record");
    assert!(!completion_readiness(&player, &quest, &definitions, scripts()).ready);
    crate::quest_records::set(&mut player, 200, 0, "Done".to_owned()).expect("ready record");
    assert!(completion_readiness(&player, &quest, &definitions, scripts()).ready);

    let completed = select_choice(
        player,
        &quest,
        2,
        COMPLETE_CHOICE_ID,
        curve(),
        &definitions,
        scripts(),
        2_000,
    )
    .expect("complete record quest");
    assert_eq!(
        crate::quest_records::get(&completed.player, 200, 0),
        Some("Done")
    );
}

#[test]
fn completion_mesos_is_inclusive_and_does_not_require_character_stats() {
    let definitions = item_definitions();
    let mut quest = quest(100);
    quest.completion.minimum_mesos = Some(100);
    let mut player = player(Vec::new(), 1);
    player.stats = None;
    player.quests.push(player_quest(100, QuestStatus::Started));

    player.mesos = 99;
    let below = completion_readiness(&player, &quest, &definitions, scripts());
    assert!(!below.ready);
    assert_eq!(below.objectives[0].kind, super::QuestObjectiveKind::Mesos);
    assert_eq!(below.objectives[0].current, 99);
    player.mesos = 100;
    assert!(completion_readiness(&player, &quest, &definitions, scripts()).ready);
    player.mesos = 101;
    assert!(completion_readiness(&player, &quest, &definitions, scripts()).ready);
}

#[test]
fn completed_quest_count_uses_only_known_eligible_completed_definitions() {
    let definitions = item_definitions();
    let mut gate = quest(100);
    gate.completion.minimum_completed_quest_count = Some(2);
    let known = quest(200);
    let second_known = quest(300);
    let excluded_low = quest(9_000);
    let excluded_high = quest(10_999);
    let mut excluded_area = quest(11_000);
    excluded_area.info.area = Some(51);
    let quest_definitions = [
        &gate,
        &known,
        &second_known,
        &excluded_low,
        &excluded_high,
        &excluded_area,
    ];
    let mut player = player(Vec::new(), 1);
    player.stats = None;
    player.quests = vec![
        player_quest(gate.id, QuestStatus::Started),
        player_quest(known.id, QuestStatus::Completed),
        PlayerQuest {
            quest_id: second_known.id,
            status: 99,
            ..PlayerQuest::default()
        },
        player_quest(excluded_low.id, QuestStatus::Completed),
        player_quest(excluded_high.id, QuestStatus::Completed),
        player_quest(excluded_area.id, QuestStatus::Completed),
        player_quest(777, QuestStatus::Completed),
    ];

    let incomplete = completion_readiness_in_environment(
        &player,
        &gate,
        &quest_definitions,
        &definitions,
        scripts(),
        environment(1_000),
    );
    assert!(!incomplete.ready);
    assert_eq!(
        incomplete.objectives[0],
        super::QuestObjectiveProgress {
            kind: super::QuestObjectiveKind::CompletedQuests,
            label: "Eligible completed quests".to_owned(),
            current: 1,
            required: 2,
            complete: false,
        }
    );

    player
        .quests
        .iter_mut()
        .find(|entry| entry.quest_id == second_known.id)
        .expect("second known quest")
        .status = QuestStatus::Completed as i32;
    assert!(
        completion_readiness_in_environment(
            &player,
            &gate,
            &quest_definitions,
            &definitions,
            scripts(),
            environment(1_000),
        )
        .ready
    );
}

#[test]
fn completion_window_is_inclusive_for_readiness_manual_and_automatic_completion() {
    let definitions = item_definitions();
    let initial_automatic_player = player(Vec::new(), 1);
    let mut automatic = quest(200);
    let mut quest = quest(100);
    quest.completion.available_from = Some(QuestCalendar {
        source: "start".to_owned(),
        unix_ms: 100,
    });
    quest.completion.available_until = Some(QuestCalendar {
        source: "end".to_owned(),
        unix_ms: 200,
    });
    let quest_definitions = [&quest];
    let mut player = player(Vec::new(), 1);
    player.stats = None;
    player
        .quests
        .push(player_quest(quest.id, QuestStatus::Started));

    for (now_unix_ms, expected) in [(99, false), (100, true), (200, true), (201, false)] {
        let readiness = completion_readiness_in_environment(
            &player,
            &quest,
            &quest_definitions,
            &definitions,
            scripts(),
            environment(now_unix_ms),
        );
        assert_eq!(readiness.ready, expected, "timestamp {now_unix_ms}");
        assert_eq!(
            readiness
                .objectives
                .iter()
                .filter(|objective| { objective.kind == super::QuestObjectiveKind::Availability })
                .count(),
            2
        );
    }

    assert!(matches!(
        select_choice_in_environment(
            player.clone(),
            &quest,
            &quest_definitions,
            2,
            COMPLETE_CHOICE_ID,
            curve(),
            &definitions,
            scripts(),
            environment(99),
        ),
        Err(QuestRuleError::ObjectivesIncomplete { quest_id: 100 })
    ));
    let completed = select_choice_in_environment(
        player,
        &quest,
        &quest_definitions,
        2,
        COMPLETE_CHOICE_ID,
        curve(),
        &definitions,
        scripts(),
        environment(100),
    )
    .expect("completion at inclusive start");
    assert_eq!(completed.player.quests[0].completed_at_unix_ms, 100);

    automatic.info.auto_accept = true;
    automatic.info.auto_complete = true;
    automatic.start.available_from = Some(QuestCalendar {
        source: "same".to_owned(),
        unix_ms: 500,
    });
    automatic.start.available_until = Some(QuestCalendar {
        source: "same".to_owned(),
        unix_ms: 500,
    });
    automatic.completion.available_from = Some(QuestCalendar {
        source: "same".to_owned(),
        unix_ms: 500,
    });
    automatic.completion.available_until = Some(QuestCalendar {
        source: "same".to_owned(),
        unix_ms: 500,
    });
    let advanced = advance_automatic_quests_in_environment(
        initial_automatic_player,
        [&automatic],
        curve(),
        &definitions,
        scripts(),
        environment(500),
    );
    assert_eq!(advanced.started_quest_ids, vec![automatic.id]);
    assert_eq!(advanced.completed_quest_ids, vec![automatic.id]);
    let entry = advanced
        .player
        .quests
        .iter()
        .find(|entry| entry.quest_id == automatic.id)
        .expect("automatic quest record");
    assert_eq!(entry.accepted_at_unix_ms, 500);
    assert_eq!(entry.completed_at_unix_ms, 500);
}

#[test]
fn item_objectives_track_current_authoritative_possession() {
    let definitions = item_definitions();
    let mut quest = quest(100);
    quest.completion.items.push(item_requirement(ITEM_A, 2));
    let mut player = player(vec![(ITEM_A, 1)], 4);
    player.quests.push(player_quest(100, QuestStatus::Started));

    let incomplete = completion_readiness(&player, &quest, &definitions, scripts());
    assert!(!incomplete.ready);
    assert_eq!(incomplete.objectives[0].current, 1);
    player.inventory.as_mut().expect("inventory").stacks[0].quantity = 2;
    assert!(completion_readiness(&player, &quest, &definitions, scripts()).ready);
    player.inventory.as_mut().expect("inventory").stacks[0].quantity = 1;
    assert!(!completion_readiness(&player, &quest, &definitions, scripts()).ready);
}

#[test]
fn item_absence_objectives_require_an_authoritative_empty_inventory() {
    let definitions = item_definitions();
    let mut quest = quest(100);
    quest.completion.items.push(QuestItemRequirement {
        item_id: ITEM_A,
        condition: QuestItemCondition::Absent,
    });
    let mut player = player(Vec::new(), 1);
    player.quests.push(player_quest(100, QuestStatus::Started));

    assert!(completion_readiness(&player, &quest, &definitions, scripts()).ready);
    player.inventory = None;
    assert!(!completion_readiness(&player, &quest, &definitions, scripts()).ready);
}

#[test]
fn completion_script_conditions_use_authored_incomplete_pages() {
    let definitions = item_definitions();
    let mut quest = quest(100);
    quest.completion.script = Some("completion_check".to_owned());
    let scripts = script_catalog(
        r#"
                [[scripts]]
                name = "completion_check"
                incomplete_pages = ["Return at level 2."]

                [[scripts.conditions]]
                type = "minimum_level"
                level = 2
            "#,
        &quest,
        &definitions,
    );
    let mut player = player(Vec::new(), 1);
    player.quests.push(player_quest(100, QuestStatus::Started));

    let readiness = completion_readiness(&player, &quest, &definitions, &scripts);
    assert!(!readiness.ready);
    assert_eq!(
        readiness.objectives[0].kind,
        super::QuestObjectiveKind::Script
    );
    let pages = incomplete_dialogue_pages(&player, &quest, &definitions, &scripts);
    assert_eq!(pages[0], "Ready");
    assert_eq!(pages[1], "Return at level 2.");
    assert!(pages[2].contains("Script conditions not met: completion_check"));

    player.level = 2;
    assert!(completion_readiness(&player, &quest, &definitions, &scripts).ready);
}

#[test]
fn mob_kills_cap_across_every_matching_active_quest() {
    let mut first = quest(100);
    first.completion.mobs.push(QuestMobObjective {
        mob_id: MOB_A,
        count: 2,
    });
    let mut second = quest(200);
    second.completion.mobs.push(QuestMobObjective {
        mob_id: MOB_A,
        count: 5,
    });
    let mut inactive = quest(300);
    inactive.completion.mobs.push(QuestMobObjective {
        mob_id: MOB_A,
        count: 10,
    });
    let mut player = player(Vec::new(), 1);
    player.quests = vec![
        player_quest(100, QuestStatus::Started),
        player_quest(200, QuestStatus::Started),
        player_quest(300, QuestStatus::Completed),
    ];
    let quests = vec![first, second, inactive];

    let first = record_mob_kills(
        player,
        &[(MOB_A, None), (MOB_A, None), (MOB_A, None)],
        &quests,
    );
    assert_eq!(first.changed_quest_ids, vec![100, 200]);
    assert_eq!(mob_count(&first.player, 100, MOB_A), 2);
    assert_eq!(mob_count(&first.player, 200, MOB_A), 3);
    assert_eq!(mob_count(&first.player, 300, MOB_A), 0);
    let capped = record_mob_kills(first.player, &[(MOB_A, None); 10], &quests);
    assert_eq!(capped.changed_quest_ids, vec![200]);
    assert_eq!(mob_count(&capped.player, 100, MOB_A), 2);
    assert_eq!(mob_count(&capped.player, 200, MOB_A), 5);
    let unchanged = capped.player.clone();
    let empty = record_mob_kills(capped.player, &[], &quests);
    assert!(empty.changed_quest_ids.is_empty());
    assert_eq!(empty.player, unchanged);
}

#[test]
fn selected_skill_mob_credit_requires_exact_authoritative_provenance() {
    const REQUIRED_SKILL: u32 = 1_001_004;
    let mut ordinary = quest(100);
    ordinary.completion.mobs.push(QuestMobObjective {
        mob_id: MOB_A,
        count: 5,
    });
    let mut selected = quest(200);
    selected.completion.mobs.push(QuestMobObjective {
        mob_id: MOB_A,
        count: 3,
    });
    selected.info.selected_skill = Some(QuestSelectedSkill {
        id: NonZeroU32::new(REQUIRED_SKILL).expect("positive skill"),
        name: Some("Power Strike".to_owned()),
    });
    let quests = vec![ordinary, selected];
    let mut player = player(Vec::new(), 1);
    player.quests = vec![
        player_quest(100, QuestStatus::Started),
        player_quest(200, QuestStatus::Started),
    ];

    let credited = record_mob_kills(
        player,
        &[
            (MOB_A, None),
            (MOB_A, Some(REQUIRED_SKILL + 1)),
            (MOB_A, Some(REQUIRED_SKILL)),
            (MOB_A, Some(REQUIRED_SKILL)),
        ],
        &quests,
    );

    assert_eq!(mob_count(&credited.player, 100, MOB_A), 4);
    assert_eq!(mob_count(&credited.player, 200, MOB_A), 2);
    assert_eq!(credited.changed_quest_ids, vec![100, 200]);
    let capped = record_mob_kills(
        credited.player,
        &[(MOB_A, Some(REQUIRED_SKILL)); 10],
        &quests,
    );
    assert_eq!(mob_count(&capped.player, 100, MOB_A), 5);
    assert_eq!(mob_count(&capped.player, 200, MOB_A), 3);
    let readiness = completion_readiness(&capped.player, &quests[1], &[], scripts());
    assert!(readiness.objectives[0].label.contains("Power Strike"));
    assert!(readiness.objectives[0].label.contains("1001004"));
}
