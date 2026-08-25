use crate::quests::test_support::*;

#[test]
fn monster_book_requirements_gate_start_and_report_typed_completion_objectives() {
    let definitions = item_definitions();
    let requirements = QuestMonsterBookRequirements {
        cards: vec![QuestMonsterBookCardRequirement {
            card_item_id: CARD_A,
            minimum_count: Some(2),
            maximum_count: Some(3),
        }],
        minimum_unique_cards: Some(2),
        maximum_unique_cards: Some(2),
    };
    let mut quest = quest(100);
    quest.start.monster_book = requirements.clone();
    quest.completion.monster_book = requirements;

    let mut player = player(Vec::new(), 1);
    player.monster_book_cards = vec![
        MonsterBookCard {
            card_item_id: CARD_A,
            count: 1,
        },
        MonsterBookCard {
            card_item_id: CARD_B,
            count: 1,
        },
    ];
    assert!(!is_available(
        &player,
        &quest,
        &definitions,
        scripts(),
        1_000
    ));
    player.monster_book_cards[0].count = 2;
    assert!(is_available(
        &player,
        &quest,
        &definitions,
        scripts(),
        1_000
    ));
    player.monster_book_cards[0].count = 4;
    assert!(!is_available(
        &player,
        &quest,
        &definitions,
        scripts(),
        1_000
    ));

    player.monster_book_cards[0].count = 1;
    player
        .quests
        .push(player_quest(quest.id, QuestStatus::Started));
    let incomplete = completion_readiness(&player, &quest, &definitions, scripts());
    assert!(!incomplete.ready);
    assert_eq!(incomplete.objectives.len(), 4);
    assert_eq!(
        incomplete
            .objectives
            .iter()
            .map(|objective| objective.kind)
            .collect::<Vec<_>>(),
        vec![
            super::QuestObjectiveKind::MonsterBookCardMinimum,
            super::QuestObjectiveKind::MonsterBookCardMaximum,
            super::QuestObjectiveKind::MonsterBookUniqueMinimum,
            super::QuestObjectiveKind::MonsterBookUniqueMaximum,
        ]
    );
    assert!(!incomplete.objectives[0].complete);
    assert!(
        incomplete.objectives[1..]
            .iter()
            .all(|objective| objective.complete)
    );

    player.monster_book_cards[0].count = 2;
    assert!(completion_readiness(&player, &quest, &definitions, scripts()).ready);
}

#[test]
fn marker_skill_is_acquired_for_quest_checks() {
    let definitions = item_definitions();
    let mut grant = quest(100);
    grant.start_actions.skill_changes = vec![skill_change(9_999, 1, 0, Vec::new())];
    let accepted = select_choice(
        player(Vec::new(), 1),
        &grant,
        1,
        ACCEPT_CHOICE_ID,
        curve(),
        &definitions,
        scripts(),
        100,
    )
    .expect("marker grant");
    let mut gated = quest(200);
    gated.start.skills.push(QuestSkillRequirement {
        skill_id: 9_999,
        acquired: true,
    });

    assert!(is_available(
        &accepted.player,
        &gated,
        &definitions,
        scripts(),
        100,
    ));
}

#[test]
fn incomplete_completion_rejects_every_reward() {
    let definitions = item_definitions();
    let mut quest = quest(100);
    quest.completion.items.push(item_requirement(ITEM_A, 2));
    quest.completion_actions = reward_actions();
    quest.completion_actions.npc_animation_action = Some("quest".to_owned());
    let mut player = player(vec![(ITEM_A, 1)], 4);
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
        Err(QuestRuleError::ObjectivesIncomplete { quest_id: 100 })
    ));
    assert_eq!(unchanged.mesos, 0);
    assert_eq!(unchanged.stats.expect("stats").fame, 0);
    assert_eq!(unchanged.quests[0].status, QuestStatus::Started as i32);
}

#[test]
fn manual_completion_rejects_a_quest_at_its_deadline() {
    let definitions = item_definitions();
    let mut quest = quest(100);
    quest.info.time_limit_ms = Some(100);
    quest.completion_actions.money = 100;
    let mut player = player(Vec::new(), 1);
    player.quests.push(PlayerQuest {
        accepted_at_unix_ms: 100,
        ..player_quest(quest.id, QuestStatus::Started)
    });

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
        Err(QuestRuleError::Expired { quest_id: 100 })
    ));
}

#[test]
fn next_quest_becomes_available_through_its_prerequisite() {
    let definitions = item_definitions();
    let mut current = quest(100);
    current.completion_actions.next_quest_id = Some(200);
    let mut next = quest(200);
    next.start.quests.push(QuestStateRequirement {
        quest_id: 100,
        state: RequiredQuestState::Completed,
    });
    let mut player = player(Vec::new(), 1);
    player.quests.push(player_quest(100, QuestStatus::Started));
    assert!(!is_available(&player, &next, &definitions, scripts(), 200));

    let completed = select_choice(
        player,
        &current,
        2,
        COMPLETE_CHOICE_ID,
        curve(),
        &definitions,
        scripts(),
        200,
    )
    .expect("complete current quest");

    assert_eq!(completed.next_quest_id, Some(200));
    assert!(is_available(
        &completed.player,
        &next,
        &definitions,
        scripts(),
        200
    ));
    assert_eq!(progress(&completed.player, 200), QuestProgress::NotStarted);
}

#[test]
fn scripted_transitions_expose_names_without_succeeding() {
    let definitions = item_definitions();
    let mut start_scripted = quest(100);
    start_scripted.start.script = Some("start_quest".to_owned());
    let initial_player = player(Vec::new(), 1);
    assert!(!is_available(
        &initial_player,
        &start_scripted,
        &definitions,
        scripts(),
        100
    ));
    assert!(matches!(
        select_choice(
            initial_player,
            &start_scripted,
            1,
            ACCEPT_CHOICE_ID,
            curve(),
            &definitions,
            scripts(),
            100,
        ),
        Err(QuestRuleError::ScriptRequired {
            quest_id: 100,
            script,
            ..
        }) if script == "start_quest"
    ));

    let mut completion_scripted = quest(200);
    completion_scripted.completion.script = Some("end_quest".to_owned());
    let mut player = player(Vec::new(), 1);
    player.quests.push(player_quest(200, QuestStatus::Started));
    assert!(matches!(
        select_choice(
            player,
            &completion_scripted,
            2,
            COMPLETE_CHOICE_ID,
            curve(),
            &definitions,
            scripts(),
            200,
        ),
        Err(QuestRuleError::ScriptRequired {
            quest_id: 200,
            script,
            ..
        }) if script == "end_quest"
    ));
}
