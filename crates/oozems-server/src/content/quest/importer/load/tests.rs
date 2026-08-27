use crate::content::quest::importer::test_support::*;

#[test]
fn pio_style_quest_inherits_its_start_npc_for_completion() {
    const QUEST_ID: u32 = 1_008;
    const PIO_ID: u32 = 10_000;
    const RUSTY_SCREW_ID: u32 = 4_031_161;
    const OLD_BOARD_ID: u32 = 4_031_162;

    let checks = property("checks");
    let check = property(&QUEST_ID.to_string());
    let start_check = property("0");
    add_integer(&start_check, "npc", PIO_ID as i32);
    add_child(&check, &start_check);
    let completion_check = property("1");
    let required_items = property("item");
    for (index, item_id) in [RUSTY_SCREW_ID, OLD_BOARD_ID].into_iter().enumerate() {
        let item = property(&index.to_string());
        add_integer(&item, "id", item_id as i32);
        add_integer(&item, "count", 1);
        add_child(&required_items, &item);
    }
    add_child(&completion_check, &required_items);
    add_child(&check, &completion_check);
    add_child(&checks, &check);

    let actions = property("actions");
    let action = property(&QUEST_ID.to_string());
    add_child(&action, &property("0"));
    let completion_action = property("1");
    let removed_items = property("item");
    for (index, item_id) in [RUSTY_SCREW_ID, OLD_BOARD_ID].into_iter().enumerate() {
        let item = property(&index.to_string());
        add_integer(&item, "id", item_id as i32);
        add_integer(&item, "count", -1);
        add_child(&removed_items, &item);
    }
    add_child(&completion_action, &removed_items);
    add_child(&action, &completion_action);
    add_child(&actions, &action);

    let info_root = property("info");
    let info = property(&QUEST_ID.to_string());
    add_string(&info, "name", "Pio's Collecting Recycled Goods");
    add_child(&info_root, &info);
    let item_ids = BTreeSet::from([RUSTY_SCREW_ID, OLD_BOARD_ID]);
    let quest = super::load_definition(
        QUEST_ID,
        &checks,
        &actions,
        &property("dialogue"),
        &info_root,
        &item_ids,
        &BTreeSet::new(),
        &BTreeSet::new(),
        &BTreeSet::new(),
        &BTreeSet::new(),
        &BTreeSet::new(),
        &BTreeMap::new(),
        &BTreeSet::from([QUEST_ID]),
    )
    .expect("Pio-style quest definition");

    assert_eq!(quest.start.npc_id, Some(PIO_ID));
    assert_eq!(quest.completion.npc_id, Some(PIO_ID));
    assert_eq!(quest.completion.items.len(), 2);
}

#[test]
fn ordinary_completion_without_an_npc_inherits_the_start_npc() {
    let completion = super::resolve_completion_npc(
        QuestCompletionRequirements::default(),
        &crate::content::QuestStartRequirements {
            npc_id: Some(10_000),
            ..crate::content::QuestStartRequirements::default()
        },
        &QuestInfo::default(),
        &QuestDialogue::default(),
    );

    assert_eq!(completion.npc_id, Some(10_000));
}

#[test]
fn automatic_completion_without_an_npc_stays_automatic() {
    for info in [
        QuestInfo {
            auto_complete: true,
            ..QuestInfo::default()
        },
        QuestInfo {
            auto_pre_complete: true,
            ..QuestInfo::default()
        },
    ] {
        let completion = super::resolve_completion_npc(
            QuestCompletionRequirements::default(),
            &crate::content::QuestStartRequirements {
                npc_id: Some(10_000),
                ..crate::content::QuestStartRequirements::default()
            },
            &info,
            &QuestDialogue::default(),
        );

        assert_eq!(completion.npc_id, None);
    }
}

#[test]
fn automatic_completion_with_a_question_inherits_the_start_npc() {
    let completion = super::resolve_completion_npc(
        QuestCompletionRequirements::default(),
        &crate::content::QuestStartRequirements {
            npc_id: Some(10_000),
            ..crate::content::QuestStartRequirements::default()
        },
        &QuestInfo {
            auto_complete: true,
            ..QuestInfo::default()
        },
        &QuestDialogue {
            question: Some(crate::content::QuestQuestionSequence {
                leading_pages: Vec::new(),
                steps: Vec::new(),
                trailing_pages: Vec::new(),
            }),
            ..QuestDialogue::default()
        },
    );

    assert_eq!(completion.npc_id, Some(10_000));
}

#[test]
fn explicit_completion_npc_takes_precedence() {
    let completion = super::resolve_completion_npc(
        QuestCompletionRequirements {
            npc_id: Some(20_000),
            ..QuestCompletionRequirements::default()
        },
        &crate::content::QuestStartRequirements {
            npc_id: Some(10_000),
            ..crate::content::QuestStartRequirements::default()
        },
        &QuestInfo::default(),
        &QuestDialogue::default(),
    );

    assert_eq!(completion.npc_id, Some(20_000));
}

#[test]
fn script_reference_scan_includes_quests_the_importer_may_reject() {
    let checks = property("checks");
    let quest = property("4490");
    let start = property("0");
    add_string(&start, "startscript", "q4490s");
    let completion = property("1");
    add_string(&completion, "endscript", "q4490e");
    add_integer(&completion, "userInteract", 1);
    add_child(&quest, &start);
    add_child(&quest, &completion);
    add_child(&checks, &quest);
    let metadata = property("metadata");
    let metadata_start = property("0");
    add_string(&metadata_start, "startscript", "not_a_quest");
    add_child(&metadata, &metadata_start);
    add_child(&checks, &metadata);

    let names = super::script_reference_names(&checks).expect("script references");

    assert_eq!(
        names,
        BTreeSet::from(["q4490e".to_owned(), "q4490s".to_owned()])
    );
}

#[test]
fn malformed_item_reference_scan_returns_a_quest_error() {
    let checks = property("checks");
    let actions = property("actions");
    let quest = property("100");
    let phase = property("0");
    let items = property("item");
    let entry = property("0");
    add_string(&entry, "count", "not-an-integer");
    add_integer(&entry, "id", 4_000_000);
    add_child(&items, &entry);
    add_child(&phase, &items);
    add_child(&quest, &phase);
    add_child(&actions, &quest);

    let error = super::item_reference_ids(100, &checks, &actions)
        .expect_err("malformed reference metadata must fail closed for its quest");

    assert!(matches!(
        error,
        QuestContentError::Invalid { quest_id: 100, message }
            if message.contains("count")
    ));
}
