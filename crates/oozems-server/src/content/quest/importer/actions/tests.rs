use crate::content::quest::importer::test_support::*;

#[test]
fn skill_actions_import_master_only_defaults_and_exact_jobs() {
    let action = property("quest");
    let phase = property("1");
    let skills = property("skill");
    let entry = property("0");
    add_integer(&entry, "id", 2_321_003);
    add_integer(&entry, "masterLevel", 15);
    add_integer(&entry, "onlyMasterLevel", 1);
    let jobs = property("job");
    add_integer(&jobs, "0", 232);
    add_integer(&jobs, "1", 0);
    add_child(&entry, &jobs);
    add_child(&skills, &entry);
    add_child(&phase, &skills);
    add_child(&action, &phase);

    let (actions, retained) = read_action_phase_with_skills(
        100,
        &action,
        "1",
        &BTreeSet::new(),
        &BTreeSet::new(),
        &BTreeSet::from([2_321_003]),
        &BTreeSet::from([100]),
        None,
    )
    .expect("master-only skill action");

    assert!(retained.is_empty());
    assert_eq!(
        actions.skill_changes,
        vec![crate::content::QuestSkillChange {
            skill_id: 2_321_003,
            operation: crate::content::QuestSkillOperation::Grant {
                skill_level: 0,
                master_level: 15,
            },
            job_ids: vec![232, 0],
        }]
    );
}

#[test]
fn malformed_skill_actions_and_unknown_references_fail_closed() {
    for malformed in [
        "duplicate_job",
        "unknown_field",
        "bad_level",
        "bad_only_master",
        "missing_id",
        "zero_id",
        "missing_master",
        "duplicate_skill",
    ] {
        let action = property("quest");
        let phase = property("1");
        let skills = property("skill");
        let entry = property("0");
        if malformed != "missing_id" {
            add_integer(&entry, "id", if malformed == "zero_id" { 0 } else { 1_000 });
        }
        if malformed != "missing_master" {
            add_integer(&entry, "masterLevel", 1);
        }
        match malformed {
            "duplicate_job" => {
                let jobs = property("job");
                add_integer(&jobs, "0", 100);
                add_integer(&jobs, "1", 100);
                add_child(&entry, &jobs);
            }
            "unknown_field" => add_integer(&entry, "mystery", 1),
            "bad_level" => add_long(&entry, "skillLevel", i64::from(u32::MAX) + 1),
            "bad_only_master" => add_integer(&entry, "onlyMasterLevel", 2),
            "missing_id" | "zero_id" | "missing_master" => {}
            "duplicate_skill" => {
                let duplicate = property("1");
                add_integer(&duplicate, "id", 1_000);
                add_integer(&duplicate, "masterLevel", 1);
                add_child(&skills, &duplicate);
            }
            _ => unreachable!(),
        }
        add_child(&skills, &entry);
        add_child(&phase, &skills);
        add_child(&action, &phase);

        assert!(
            read_action_phase_with_skills(
                100,
                &action,
                "1",
                &BTreeSet::new(),
                &BTreeSet::new(),
                &BTreeSet::from([1_000]),
                &BTreeSet::from([100]),
                None,
            )
            .is_err(),
            "{malformed} must fail",
        );
    }

    let unknown = skill_action(9_999, 1);
    assert!(matches!(
        read_action_phase_with_skills(
            100,
            &unknown,
            "1",
            &BTreeSet::new(),
            &BTreeSet::new(),
            &BTreeSet::new(),
            &BTreeSet::from([100]),
            None,
        ),
        Err(QuestContentError::Unsupported { category, .. })
            if category == "unknown skill reference"
    ));
}

#[test]
fn acquire_minus_one_imports_an_exact_skill_removal() {
    let action = property("quest");
    let phase = property("0");
    let skills = property("skill");
    let entry = property("0");
    add_integer(&entry, "id", 1_007);
    add_integer(&entry, "acquire", -1);
    let jobs = property("job");
    add_integer(&jobs, "0", 0);
    add_integer(&jobs, "1", 100);
    add_child(&entry, &jobs);
    add_child(&skills, &entry);
    add_child(&phase, &skills);
    add_child(&action, &phase);

    let (actions, retained) = read_action_phase_with_skills(
        6_034,
        &action,
        "0",
        &BTreeSet::new(),
        &BTreeSet::new(),
        &BTreeSet::from([1_007]),
        &BTreeSet::from([6_034]),
        None,
    )
    .expect("exact skill removal");

    assert!(retained.is_empty());
    assert_eq!(
        actions.skill_changes,
        vec![crate::content::QuestSkillChange {
            skill_id: 1_007,
            operation: crate::content::QuestSkillOperation::Remove,
            job_ids: vec![0, 100],
        }]
    );
}

#[test]
fn malformed_skill_removals_fail_closed() {
    for malformed in [
        "negative_acquire",
        "positive_acquire",
        "skill_level",
        "master_level",
        "only_master_level",
        "duplicate_job",
    ] {
        let action = property("quest");
        let phase = property("0");
        let skills = property("skill");
        let entry = property("0");
        add_integer(&entry, "id", 1_007);
        add_integer(
            &entry,
            "acquire",
            match malformed {
                "negative_acquire" => -2,
                "positive_acquire" => 1,
                _ => -1,
            },
        );
        match malformed {
            "skill_level" => add_integer(&entry, "skillLevel", 1),
            "master_level" => add_integer(&entry, "masterLevel", 1),
            "only_master_level" => add_integer(&entry, "onlyMasterLevel", 1),
            "duplicate_job" => {
                let jobs = property("job");
                add_integer(&jobs, "0", 100);
                add_integer(&jobs, "1", 100);
                add_child(&entry, &jobs);
            }
            "negative_acquire" | "positive_acquire" => {}
            _ => unreachable!(),
        }
        add_child(&skills, &entry);
        add_child(&phase, &skills);
        add_child(&action, &phase);

        assert!(
            read_action_phase_with_skills(
                6_034,
                &action,
                "0",
                &BTreeSet::new(),
                &BTreeSet::new(),
                &BTreeSet::from([1_007]),
                &BTreeSet::from([6_034]),
                None,
            )
            .is_err(),
            "{malformed} removal must fail",
        );
    }
}

#[test]
fn start_action_info_is_an_exact_record_write_and_completion_info_is_rejected() {
    let action = property("quest");
    let start = property("0");
    add_string(&start, "info", "000007");
    add_child(&action, &start);

    let (actions, retained) =
        read_action_phase(100, &action, "0", &BTreeSet::new(), &BTreeSet::new())
            .expect("start record write");
    assert_eq!(
        actions.record_writes,
        vec![QuestRecordWrite {
            quest_id: 100,
            index: 0,
            value: "000007".to_owned(),
        }]
    );
    assert!(retained.is_empty());

    let completion_action = property("quest");
    let completion = property("1");
    add_string(&completion, "info", "done");
    add_child(&completion_action, &completion);
    assert!(matches!(
        read_action_phase(
            100,
            &completion_action,
            "1",
            &BTreeSet::new(),
            &BTreeSet::new()
        ),
        Err(QuestContentError::Unsupported { category, .. })
            if category == "quest progress action phase 1"
    ));
}

#[test]
fn recognized_inert_action_metadata_is_validated_and_retained() {
    let action = property("quest");
    let phase = property("0");
    add_string(&phase, "0", "Offer text");
    for branch_name in ["yes", "no"] {
        let branch = property(branch_name);
        add_string(&branch, "0", "Branch text");
        add_child(&phase, &branch);
    }
    add_integer(&phase, "ask", 1);
    let stop = property("stop");
    let answers = property("0");
    add_integer(&answers, "answer", 1);
    add_string(&answers, "0", "Response text");
    add_child(&stop, &answers);
    add_child(&phase, &stop);
    add_string(&phase, "start", "19700101");
    add_string(&phase, "end", "19700102");
    add_integer(&phase, "interval", 60);
    add_string(&phase, "message", "Legacy message");
    add_integer(&phase, "lvmin", 10);
    add_integer(&phase, "lvmax", 20);
    let jobs = property("job");
    add_integer(&jobs, "0", 100);
    add_integer(&jobs, "1", 200);
    add_child(&phase, &jobs);
    add_integer(&phase, "gender", 2);
    add_child(&action, &phase);

    let (actions, retained) =
        read_action_phase(100, &action, "0", &BTreeSet::new(), &BTreeSet::new())
            .expect("recognized inert action metadata");

    assert_eq!(actions, QuestActions::default());
    assert_eq!(
        retained,
        [
            "0", "ask", "end", "gender", "interval", "job", "lvmax", "lvmin", "message", "no",
            "start", "stop", "yes",
        ]
        .map(|name| format!("act/0/{name}"))
    );
}

#[test]
fn act_zero_npc_is_positive_typed_presentation_metadata_only() {
    let action = property("quest");
    let phase = property("0");
    add_integer(&phase, "npc", 9_201_142);
    add_child(&action, &phase);

    let (actions, retained) =
        read_action_phase(100, &action, "0", &BTreeSet::new(), &BTreeSet::new())
            .expect("presentation NPC metadata");

    assert_eq!(actions.presentation_npc_id, Some(9_201_142));
    assert_eq!(retained, vec!["act/0/npc"]);

    let zero = property("quest");
    let phase = property("0");
    add_integer(&phase, "npc", 0);
    add_child(&zero, &phase);
    assert!(matches!(
        read_action_phase(100, &zero, "0", &BTreeSet::new(), &BTreeSet::new()),
        Err(QuestContentError::Invalid { .. })
    ));
}

#[test]
fn npc_animation_actions_preserve_exact_nonempty_string_names() {
    for phase_name in ["0", "1"] {
        let action = property("quest");
        let phase = property(phase_name);
        add_string(&phase, "npcAct", "  exact Action  ");
        add_child(&action, &phase);
        let (actions, retained) =
            read_action_phase(100, &action, phase_name, &BTreeSet::new(), &BTreeSet::new())
                .expect("typed NPC animation action");
        assert_eq!(
            actions.npc_animation_action.as_deref(),
            Some("  exact Action  ")
        );
        assert!(retained.is_empty());
    }
}

#[test]
fn npc_animation_actions_reject_empty_and_non_string_values() {
    for add_invalid in [
        add_empty_npc_animation as fn(&WzNodeArc),
        add_integer_npc_animation,
    ] {
        let action = property("quest");
        let phase = property("1");
        add_invalid(&phase);
        add_child(&action, &phase);
        assert!(matches!(
            read_action_phase(100, &action, "1", &BTreeSet::new(), &BTreeSet::new()),
            Err(QuestContentError::Invalid { .. })
        ));
    }

    let action = property("quest");
    let phase = property("1");
    add_integer(&phase, "npc", 1);
    add_child(&action, &phase);
    assert!(matches!(
        read_action_phase(100, &action, "1", &BTreeSet::new(), &BTreeSet::new()),
        Err(QuestContentError::Unsupported { category, .. }) if category == "NPC action"
    ));
}

#[test]
fn automatic_npc_animation_transitions_are_rejected_without_a_spawn_target() {
    let start = super::QuestStartRequirements {
        npc_id: Some(1),
        ..super::QuestStartRequirements::default()
    };
    let completion = QuestCompletionRequirements {
        npc_id: Some(2),
        ..QuestCompletionRequirements::default()
    };
    let start_actions = QuestActions {
        npc_animation_action: Some("start".to_owned()),
        ..QuestActions::default()
    };
    let completion_actions = QuestActions {
        npc_animation_action: Some("complete".to_owned()),
        ..QuestActions::default()
    };

    assert!(matches!(
        super::validate_npc_animation_transitions(
            100,
            &start,
            &completion,
            &start_actions,
            &QuestActions::default(),
            &QuestInfo {
                auto_accept: true,
                ..QuestInfo::default()
            },
        ),
        Err(QuestContentError::Unsupported { category, .. })
            if category == "automatic NPC animation action"
    ));
    assert!(matches!(
        super::validate_npc_animation_transitions(
            100,
            &start,
            &completion,
            &QuestActions::default(),
            &completion_actions,
            &QuestInfo {
                auto_complete: true,
                ..QuestInfo::default()
            },
        ),
        Err(QuestContentError::Unsupported { category, .. })
            if category == "automatic NPC animation action"
    ));
}

#[test]
fn typed_start_questions_require_an_npc_but_allow_automatic_metadata() {
    let dialogue = QuestDialogue {
        start_question: Some(super::QuestQuestionSequence {
            leading_pages: Vec::new(),
            steps: Vec::new(),
            trailing_pages: Vec::new(),
        }),
        ..QuestDialogue::default()
    };
    assert!(matches!(
        super::validate_start_question_reachability(
            100,
            &super::QuestStartRequirements::default(),
            &dialogue,
        ),
        Err(QuestContentError::Unsupported { category, .. })
            if category == "unreachable start question"
    ));

    let start = super::QuestStartRequirements {
        npc_id: Some(20_000),
        normal_auto_start: true,
        ..super::QuestStartRequirements::default()
    };
    assert!(super::validate_start_question_reachability(100, &start, &dialogue).is_ok());
}

#[test]
fn quest_state_actions_parse_in_numeric_order() {
    let action = quest_state_action(&[(2, 400, 2), (0, 200, 0), (1, 300, 1)]);

    let (actions, retained) = read_action_phase_with_skills(
        100,
        &action,
        "0",
        &BTreeSet::new(),
        &BTreeSet::new(),
        &BTreeSet::new(),
        &BTreeSet::from([100, 200, 300, 400]),
        None,
    )
    .expect("typed quest state actions");

    assert!(retained.is_empty());
    assert_eq!(
        actions.quest_state_actions,
        vec![
            QuestStateAction {
                quest_id: 200,
                state: QuestStateActionState::NotStarted,
            },
            QuestStateAction {
                quest_id: 300,
                state: QuestStateActionState::Started,
            },
            QuestStateAction {
                quest_id: 400,
                state: QuestStateActionState::Completed,
            },
        ]
    );
}

#[test]
fn malformed_quest_state_actions_fail_closed() {
    let cases = [
        ("zero target", quest_state_action(&[(0, 0, 2)])),
        (
            "duplicate target",
            quest_state_action(&[(0, 200, 1), (1, 200, 2)]),
        ),
        ("self target", quest_state_action(&[(0, 100, 2)])),
        ("unknown target", quest_state_action(&[(0, 999, 2)])),
        ("unknown state", quest_state_action(&[(0, 200, 3)])),
        ("index gap", quest_state_action(&[(1, 200, 2)])),
    ];
    for (name, action) in cases {
        assert!(
            read_action_phase_with_skills(
                100,
                &action,
                "0",
                &BTreeSet::new(),
                &BTreeSet::new(),
                &BTreeSet::new(),
                &BTreeSet::from([100, 200]),
                None,
            )
            .is_err(),
            "{name} must fail",
        );
    }

    let unknown_field = quest_state_action(&[(0, 200, 2)]);
    let phase = unknown_field
        .read()
        .expect("action")
        .at("0")
        .expect("phase");
    let quests = phase
        .read()
        .expect("phase")
        .at("quest")
        .expect("quest actions");
    let entry = quests
        .read()
        .expect("quest actions")
        .at("0")
        .expect("entry");
    add_integer(&entry, "mystery", 1);
    assert!(
        read_action_phase_with_skills(
            100,
            &unknown_field,
            "0",
            &BTreeSet::new(),
            &BTreeSet::new(),
            &BTreeSet::new(),
            &BTreeSet::from([100, 200]),
            None,
        )
        .is_err()
    );
}

#[test]
fn inert_action_metadata_with_invalid_shapes_is_rejected() {
    let invalid_fields = [
        ("0", false),
        ("yes", true),
        ("stop", true),
        ("start", false),
        ("interval", true),
        ("message", false),
        ("lvmin", true),
        ("job", false),
        ("gender", true),
    ];
    for (name, string_value) in invalid_fields {
        let action = property("quest");
        let phase = property("0");
        if string_value {
            add_string(&phase, name, "wrong shape");
        } else {
            add_integer(&phase, name, 1);
        }
        add_child(&action, &phase);

        let error = read_action_phase(100, &action, "0", &BTreeSet::new(), &BTreeSet::new())
            .expect_err("invalid inert metadata shape must fail");
        assert!(
            matches!(error, QuestContentError::Invalid { .. }),
            "field {name} produced {error}"
        );
    }
}

#[test]
fn nested_inert_action_metadata_shapes_are_strict() {
    let action = property("quest");
    let phase = property("0");
    add_string(&phase, "0", "First");
    add_string(&phase, "2", "Gap");
    add_child(&action, &phase);
    assert_invalid_action_phase(&action);

    let action = property("quest");
    let phase = property("0");
    let yes = property("yes");
    add_integer(&yes, "0", 1);
    add_child(&phase, &yes);
    add_child(&action, &phase);
    assert_invalid_action_phase(&action);

    let action = property("quest");
    let phase = property("0");
    add_integer(&phase, "ask", 1);
    let stop = property("stop");
    let answers = property("0");
    add_integer(&answers, "answer", 1);
    add_integer(&answers, "mystery", 1);
    add_child(&stop, &answers);
    add_child(&phase, &stop);
    add_child(&action, &phase);
    assert_invalid_action_phase(&action);

    let action = property("quest");
    let phase = property("0");
    let jobs = property("job");
    add_string(&jobs, "0", "not a job ID");
    add_child(&phase, &jobs);
    add_child(&action, &phase);
    assert_invalid_action_phase(&action);
}

#[test]
fn unknown_action_fields_are_unsupported_and_malformed_info_is_invalid() {
    let action = property("quest");
    let phase = property("0");
    add_integer(&phase, "mystery", 1);
    add_child(&action, &phase);
    assert!(matches!(
        read_action_phase(100, &action, "0", &BTreeSet::new(), &BTreeSet::new()),
        Err(QuestContentError::Unsupported { category, .. })
            if category == "unknown action field"
    ));

    let action = property("quest");
    let phase = property("0");
    add_integer(&phase, "info", 1);
    add_child(&action, &phase);
    assert!(matches!(
        read_action_phase(100, &action, "0", &BTreeSet::new(), &BTreeSet::new()),
        Err(QuestContentError::Invalid { .. })
    ));
}
