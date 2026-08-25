use crate::content::quest::importer::test_support::*;

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
