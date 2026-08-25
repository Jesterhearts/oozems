use crate::quests::test_support::*;

#[test]
fn presentation_npc_metadata_does_not_replace_the_required_start_npc() {
    let definitions = item_definitions();
    let mut quest = quest(100);
    quest.start_actions.presentation_npc_id = Some(9_201_142);

    assert!(matches!(
        select_choice(
            player(Vec::new(), 1),
            &quest,
            9_201_142,
            ACCEPT_CHOICE_ID,
            curve(),
            &definitions,
            scripts(),
            1_000,
        ),
        Err(QuestRuleError::WrongNpc { npc_id: 9_201_142 })
    ));
    let accepted = select_choice(
        player(Vec::new(), 1),
        &quest,
        1,
        ACCEPT_CHOICE_ID,
        curve(),
        &definitions,
        scripts(),
        1_000,
    )
    .expect("required Check NPC starts quest");
    assert_eq!(progress(&accepted.player, quest.id), QuestProgress::Started);
    assert_eq!(quest.start.npc_id, Some(1));
    assert_eq!(quest.start_actions.presentation_npc_id, Some(9_201_142));
}

#[test]
fn wrong_start_answer_does_not_apply_a_record_action() {
    let definitions = item_definitions();
    let mut quest = quest(100);
    configure_start_question(&mut quest);
    quest.start_actions.record_writes.push(QuestRecordWrite {
        quest_id: 100,
        index: 0,
        value: "started".to_owned(),
    });

    let pending = open_start_question(
        player(Vec::new(), 1),
        &quest,
        &definitions,
        scripts(),
        1_000,
    );
    let wrong = select_choice(
        pending,
        &quest,
        1,
        answer_choice_id(QuestQuestionPhase::Start, 0, 0).expect("first start answer"),
        curve(),
        &definitions,
        scripts(),
        1_000,
    )
    .expect("wrong answer");

    assert!(!wrong.changed);
    assert_eq!(crate::quest_records::get(&wrong.player, 100, 0), None);
}

#[test]
fn start_question_answers_control_atomic_acceptance_and_page_order() {
    let definitions = item_definitions();
    let mut quest = quest(100);
    configure_start_question(&mut quest);
    quest.start.script = Some("question_start".to_owned());
    quest.start_actions.money = 5;
    let scripts = script_catalog(
        r#"
                [[scripts]]
                name = "question_start"
                result_pages = ["Script result"]
            "#,
        &quest,
        &definitions,
    );
    let initial = open_start_question(player(Vec::new(), 1), &quest, &definitions, &scripts, 100);

    let wrong = select_choice(
        initial.clone(),
        &quest,
        1,
        answer_choice_id(QuestQuestionPhase::Start, 0, 0).expect("first start answer"),
        curve(),
        &definitions,
        &scripts,
        100,
    )
    .expect("authored wrong-answer result");
    assert!(!wrong.changed);
    assert_eq!(wrong.pages, vec!["Wrong answer"]);
    assert_eq!(wrong.player, initial);

    let correct = select_choice(
        initial,
        &quest,
        1,
        answer_choice_id(QuestQuestionPhase::Start, 0, 1).expect("second start answer"),
        curve(),
        &definitions,
        &scripts,
        100,
    )
    .expect("correct start answer");
    assert!(correct.changed);
    assert_eq!(progress(&correct.player, quest.id), QuestProgress::Started);
    assert_eq!(correct.player.mesos, 5);
    assert_eq!(
        correct.pages,
        vec!["Question continuation", "Accepted", "Script result"]
    );
}

#[test]
fn quest_10000_style_start_question_waits_for_an_explicit_decision() {
    let definitions = item_definitions();
    let mut quest = quest(10_000);
    configure_start_question(&mut quest);
    quest.dialogue.has_start_decision = true;
    let pending = open_start_question(player(Vec::new(), 1), &quest, &definitions, scripts(), 100);

    let passed = select_choice(
        pending,
        &quest,
        1,
        answer_choice_id(QuestQuestionPhase::Start, 0, 1).expect("correct start answer"),
        curve(),
        &definitions,
        scripts(),
        100,
    )
    .expect("pass start question");
    assert_eq!(passed.pages, vec!["Question continuation"]);
    assert_eq!(
        progress(&passed.player, quest.id),
        QuestProgress::NotStarted
    );
    assert_eq!(
        passed.player.quests[0].status,
        QuestStatus::Unspecified as i32
    );
    assert_eq!(passed.player.quests[0].dialogue_step, 2);
    assert_eq!(
        passed.next_interaction,
        Some(QuestNextInteraction::StartDecision)
    );

    let mut unavailable_quest = quest.clone();
    unavailable_quest.start.minimum_level = Some(2);
    let resumed = begin_start_question(
        passed.player.clone(),
        &unavailable_quest,
        1,
        &definitions,
        scripts(),
        environment(100),
    )
    .expect("resume pending decision");
    assert!(!resumed.changed);
    assert_eq!(resumed.pages, vec!["Question continuation"]);
    assert_eq!(
        resumed.next_interaction,
        Some(QuestNextInteraction::StartDecision)
    );

    let declined = select_choice(
        passed.player.clone(),
        &unavailable_quest,
        1,
        DECLINE_CHOICE_ID,
        curve(),
        &definitions,
        scripts(),
        100,
    )
    .expect("decline after passing question");
    assert!(declined.changed);
    assert_eq!(declined.pages, vec!["Declined"]);
    assert!(declined.player.quests.is_empty());

    let accepted = select_choice(
        passed.player,
        &quest,
        1,
        ACCEPT_CHOICE_ID,
        curve(),
        &definitions,
        scripts(),
        100,
    )
    .expect("accept after passing question");
    assert_eq!(progress(&accepted.player, quest.id), QuestProgress::Started);
    assert_eq!(accepted.pages, vec!["Accepted"]);
    assert_eq!(accepted.player.quests[0].accepted_at_unix_ms, 100);
}

#[test]
fn one_choice_start_question_without_a_decision_branch_starts_immediately() {
    let definitions = item_definitions();
    let mut quest = quest(100);
    quest.dialogue.start_question = Some(QuestQuestionSequence {
        leading_pages: Vec::new(),
        steps: vec![QuestQuestionStep {
            archive_index: 0,
            prompt: "Continue?".to_owned(),
            choices: vec![QuestChoice {
                id: 7,
                label: "Continue".to_owned(),
            }],
            correct_choice_id: 7,
            continuation_pages: Vec::new(),
            failure_pages: HashMap::new(),
        }],
        trailing_pages: vec!["Question continuation".to_owned()],
    });
    let pending = open_start_question(player(Vec::new(), 1), &quest, &definitions, scripts(), 100);

    let accepted = select_choice(
        pending,
        &quest,
        1,
        answer_choice_id(QuestQuestionPhase::Start, 0, 0).expect("only start answer"),
        curve(),
        &definitions,
        scripts(),
        100,
    )
    .expect("one-choice question starts immediately");

    assert_eq!(progress(&accepted.player, quest.id), QuestProgress::Started);
    assert_eq!(accepted.pages, vec!["Question continuation", "Accepted"]);
}

#[test]
fn replayed_answer_id_cannot_answer_the_next_identical_step() {
    let definitions = item_definitions();
    let mut quest = quest(100);
    let choices = vec![
        QuestChoice {
            id: 7,
            label: "Wrong".to_owned(),
        },
        QuestChoice {
            id: 42,
            label: "Right".to_owned(),
        },
    ];
    quest.dialogue.start_question = Some(QuestQuestionSequence {
        leading_pages: Vec::new(),
        steps: vec![
            QuestQuestionStep {
                archive_index: 1,
                prompt: "First".to_owned(),
                choices: choices.clone(),
                correct_choice_id: 42,
                continuation_pages: vec!["Next".to_owned()],
                failure_pages: HashMap::from([(7, vec!["Wrong".to_owned()])]),
            },
            QuestQuestionStep {
                archive_index: 3,
                prompt: "Second".to_owned(),
                choices,
                correct_choice_id: 42,
                continuation_pages: Vec::new(),
                failure_pages: HashMap::from([(7, vec!["Wrong".to_owned()])]),
            },
        ],
        trailing_pages: Vec::new(),
    });
    let pending = open_start_question(player(Vec::new(), 1), &quest, &definitions, scripts(), 100);
    let first_answer =
        answer_choice_id(QuestQuestionPhase::Start, 0, 1).expect("first-step answer");
    let second_answer =
        answer_choice_id(QuestQuestionPhase::Start, 1, 1).expect("second-step answer");
    assert_ne!(first_answer, second_answer);

    let second_step = select_choice(
        pending,
        &quest,
        1,
        first_answer,
        curve(),
        &definitions,
        scripts(),
        100,
    )
    .expect("advance to second question");
    assert_eq!(second_step.player.quests[0].dialogue_step, 2);
    assert!(matches!(
        select_choice(
            second_step.player.clone(),
            &quest,
            1,
            first_answer,
            curve(),
            &definitions,
            scripts(),
            100,
        ),
        Err(QuestRuleError::InvalidChoice { choice_id }) if choice_id == first_answer
    ));

    let accepted = select_choice(
        second_step.player,
        &quest,
        1,
        second_answer,
        curve(),
        &definitions,
        scripts(),
        100,
    )
    .expect("answer authoritative second step");
    assert_eq!(progress(&accepted.player, quest.id), QuestProgress::Started);
}

#[test]
fn answer_choice_encoding_is_checked_and_disjoint_from_other_choice_ranges() {
    assert!(answer_choice_id(QuestQuestionPhase::Start, 0, 0x1_0000).is_none());
    assert!(answer_choice_id(QuestQuestionPhase::Completion, usize::MAX, 0).is_none());
}

#[test]
fn forged_and_unavailable_start_answers_do_not_start_the_quest() {
    let definitions = item_definitions();
    let mut quest = quest(100);
    configure_start_question(&mut quest);
    let player = player(Vec::new(), 1);

    for choice_id in [
        ACCEPT_CHOICE_ID,
        DECLINE_CHOICE_ID,
        answer_choice_id(QuestQuestionPhase::Start, 0, 2).expect("third start answer"),
        RESTORE_ITEMS_CHOICE_ID,
        selectable_reward_choice_id(0).expect("selectable reward choice"),
    ] {
        assert!(matches!(
            select_choice(
                player.clone(),
                &quest,
                1,
                choice_id,
                curve(),
                &definitions,
                scripts(),
                100,
            ),
            Err(QuestRuleError::InvalidChoice { choice_id: rejected }) if rejected == choice_id
        ));
    }
    assert_eq!(progress(&player, quest.id), QuestProgress::NotStarted);

    let pending = open_start_question(player.clone(), &quest, &definitions, scripts(), 100);
    quest.start.minimum_level = Some(2);
    assert!(matches!(
        select_choice(
            pending.clone(),
            &quest,
            1,
            answer_choice_id(QuestQuestionPhase::Start, 0, 0).expect("first start answer"),
            curve(),
            &definitions,
            scripts(),
            100,
        ),
        Err(QuestRuleError::Unavailable { quest_id: 100 })
    ));
    assert!(matches!(
        select_choice(
            pending,
            &quest,
            1,
            answer_choice_id(QuestQuestionPhase::Start, 0, 1).expect("second start answer"),
            curve(),
            &definitions,
            scripts(),
            100,
        ),
        Err(QuestRuleError::Unavailable { quest_id: 100 })
    ));
    assert_eq!(progress(&player, quest.id), QuestProgress::NotStarted);

    let mut completed = player.clone();
    completed
        .quests
        .push(player_quest(quest.id, QuestStatus::Completed));
    assert!(matches!(
        select_choice(
            completed,
            &quest,
            1,
            answer_choice_id(QuestQuestionPhase::Start, 0, 2).expect("third start answer"),
            curve(),
            &definitions,
            scripts(),
            100,
        ),
        Err(QuestRuleError::Unavailable { quest_id: 100 })
    ));
}

#[test]
fn ordinary_accept_decline_and_question_automation_semantics_are_preserved() {
    let definitions = item_definitions();
    let mut quest = quest(100);
    quest.start_actions.npc_animation_action = Some("accept".to_owned());
    let initial = player(Vec::new(), 1);

    let declined = select_choice(
        initial.clone(),
        &quest,
        1,
        DECLINE_CHOICE_ID,
        curve(),
        &definitions,
        scripts(),
        100,
    )
    .expect("decline ordinary quest");
    assert!(!declined.changed);
    assert_eq!(declined.npc_animation_action, None);
    assert_eq!(declined.pages, vec!["Declined"]);
    assert_eq!(declined.player, initial);

    let accepted = select_choice(
        initial.clone(),
        &quest,
        1,
        ACCEPT_CHOICE_ID,
        curve(),
        &definitions,
        scripts(),
        100,
    )
    .expect("accept ordinary quest");
    assert_eq!(progress(&accepted.player, quest.id), QuestProgress::Started);
    assert_eq!(accepted.pages, vec!["Accepted"]);
    assert_eq!(accepted.npc_animation_action.as_deref(), Some("accept"));

    let mut automatic_question = quest;
    configure_start_question(&mut automatic_question);
    automatic_question.info.auto_accept = true;
    let advanced = advance_automatic_quests(
        initial,
        [&automatic_question],
        curve(),
        &definitions,
        scripts(),
        100,
    );
    assert!(!advanced.changed);
    assert_eq!(
        progress(&advanced.player, automatic_question.id),
        QuestProgress::NotStarted
    );
}

#[test]
fn correct_quiz_answer_is_required_for_completion() {
    let definitions = item_definitions();
    let mut quest = quest(1_009);
    quest.dialogue.question = Some(QuestQuestionSequence {
        leading_pages: Vec::new(),
        steps: vec![QuestQuestionStep {
            archive_index: 0,
            prompt: "Question".to_owned(),
            choices: vec![
                QuestChoice {
                    id: 7,
                    label: "Wrong".to_owned(),
                },
                QuestChoice {
                    id: 42,
                    label: "Right".to_owned(),
                },
            ],
            correct_choice_id: 42,
            continuation_pages: Vec::new(),
            failure_pages: HashMap::from([(7, vec!["Try again".to_owned()])]),
        }],
        trailing_pages: vec!["Correct".to_owned()],
    });
    quest.completion_actions.experience = 2;
    quest.completion_actions.npc_animation_action = Some("quiz".to_owned());
    let mut player = player(Vec::new(), 1);
    player
        .quests
        .push(player_quest(1_009, QuestStatus::Started));
    let player = open_completion_question(player, &quest, &definitions, scripts(), 200);

    let wrong = select_choice(
        player,
        &quest,
        2,
        answer_choice_id(QuestQuestionPhase::Completion, 0, 0).expect("first completion answer"),
        curve(),
        &definitions,
        scripts(),
        200,
    )
    .expect("wrong answer dialogue");
    assert!(!wrong.changed);
    assert_eq!(wrong.npc_animation_action, None);
    assert_eq!(progress(&wrong.player, 1_009), QuestProgress::Started);
    let correct = select_choice(
        wrong.player,
        &quest,
        2,
        answer_choice_id(QuestQuestionPhase::Completion, 0, 1).expect("second completion answer"),
        curve(),
        &definitions,
        scripts(),
        200,
    )
    .expect("correct answer");
    assert_eq!(progress(&correct.player, 1_009), QuestProgress::Completed);
    assert_eq!(correct.player.stats.expect("stats").experience, 2);
    assert_eq!(correct.npc_animation_action.as_deref(), Some("quiz"));
}

#[test]
fn quest_6034_no_closes_and_yes_continues_to_completion() {
    let definitions = item_definitions();
    let mut quest = quest(6_034);
    quest.dialogue.question = Some(QuestQuestionSequence {
        leading_pages: Vec::new(),
        steps: vec![QuestQuestionStep {
            archive_index: 0,
            prompt: "Would you like to pick up the crumbled piece of paper?".to_owned(),
            choices: vec![
                QuestChoice {
                    id: 0,
                    label: "Yes".to_owned(),
                },
                QuestChoice {
                    id: 1,
                    label: "No".to_owned(),
                },
            ],
            correct_choice_id: 0,
            continuation_pages: Vec::new(),
            failure_pages: HashMap::new(),
        }],
        trailing_pages: vec!["The writing on the paper is illegible.".to_owned()],
    });
    quest.completion_actions.next_quest_id = Some(6_035);
    let mut player = player(Vec::new(), 1);
    player
        .quests
        .push(player_quest(quest.id, QuestStatus::Started));
    let player = open_completion_question(player, &quest, &definitions, scripts(), 200);

    let no = select_choice(
        player,
        &quest,
        2,
        answer_choice_id(QuestQuestionPhase::Completion, 0, 1).expect("quest 6034 no answer"),
        curve(),
        &definitions,
        scripts(),
        200,
    )
    .expect("quest 6034 no closes cleanly");
    assert!(!no.changed);
    assert!(no.pages.is_empty());
    assert_eq!(no.next_interaction, None);
    assert_eq!(progress(&no.player, quest.id), QuestProgress::Started);
    assert_eq!(no.player.quests[0].dialogue_step, 1);

    let reopened = begin_completion_question(
        no.player,
        &quest,
        &[&quest],
        2,
        &definitions,
        scripts(),
        environment(201),
    )
    .expect("quest 6034 question reopens");
    assert!(!reopened.changed);
    assert_eq!(
        reopened.next_interaction,
        Some(QuestNextInteraction::Question {
            phase: QuestQuestionPhase::Completion,
            step_index: 0,
        })
    );
    assert_eq!(reopened.player.quests[0].dialogue_step, 1);

    let yes = select_choice(
        reopened.player,
        &quest,
        2,
        answer_choice_id(QuestQuestionPhase::Completion, 0, 0).expect("quest 6034 yes answer"),
        curve(),
        &definitions,
        scripts(),
        201,
    )
    .expect("quest 6034 yes completes");
    assert_eq!(progress(&yes.player, quest.id), QuestProgress::Completed);
    assert_eq!(
        yes.pages,
        vec!["The writing on the paper is illegible.", "Complete"]
    );
    assert_eq!(yes.next_quest_id, Some(6_035));
}

#[test]
fn selectable_quiz_emits_animation_only_when_reward_completes_quest() {
    let definitions = item_definitions();
    let mut quest = quest(100);
    quest.dialogue.question = Some(QuestQuestionSequence {
        leading_pages: Vec::new(),
        steps: vec![QuestQuestionStep {
            archive_index: 0,
            prompt: "Question".to_owned(),
            choices: vec![QuestChoice {
                id: 42,
                label: "Right".to_owned(),
            }],
            correct_choice_id: 42,
            continuation_pages: Vec::new(),
            failure_pages: HashMap::new(),
        }],
        trailing_pages: vec!["Choose a reward".to_owned()],
    });
    quest
        .completion_actions
        .selectable_items
        .push(selectable_reward(
            ITEM_B,
            1,
            QuestRewardEligibility::default(),
        ));
    quest.completion_actions.npc_animation_action = Some("quizReward".to_owned());
    let mut player = player(Vec::new(), 1);
    player
        .quests
        .push(player_quest(quest.id, QuestStatus::Started));
    let player = open_completion_question(player, &quest, &definitions, scripts(), 200);

    let passed = select_choice(
        player,
        &quest,
        2,
        answer_choice_id(QuestQuestionPhase::Completion, 0, 0).expect("completion answer"),
        curve(),
        &definitions,
        scripts(),
        200,
    )
    .expect("pass completion quiz");
    assert_eq!(passed.npc_animation_action, None);
    assert_eq!(progress(&passed.player, quest.id), QuestProgress::Started);
    assert!(passed.player.quests[0].completion_quiz_passed);

    let reward_choice = eligible_selectable_reward_choices(&passed.player, &quest)[0].0;
    let completed = select_choice(
        passed.player,
        &quest,
        2,
        reward_choice,
        curve(),
        &definitions,
        scripts(),
        201,
    )
    .expect("select completion reward");
    assert_eq!(
        completed.npc_animation_action.as_deref(),
        Some("quizReward")
    );
    assert_eq!(
        progress(&completed.player, quest.id),
        QuestProgress::Completed
    );
    assert!(matches!(
        select_choice(
            completed.player,
            &quest,
            2,
            reward_choice,
            curve(),
            &definitions,
            scripts(),
            202,
        ),
        Err(QuestRuleError::Unavailable { quest_id: 100 })
    ));
}
