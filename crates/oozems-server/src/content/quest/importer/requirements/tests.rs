use crate::content::quest::importer::test_support::*;

#[test]
fn monster_book_checks_parse_strict_card_and_unique_bounds() {
    let node = property("1");
    add_integer(&node, "mbmin", 1);
    add_integer(&node, "mbmax", 2);
    let cards = property("mbcard");
    let first = property("0");
    add_integer(&first, "id", 2_380_000);
    add_integer(&first, "min", 1);
    add_integer(&first, "max", 4);
    add_child(&cards, &first);
    add_child(&node, &cards);

    let requirements =
        super::read_monster_book_requirements(100, &node, &BTreeSet::from([2_380_000]))
            .expect("Monster Book requirements");
    assert_eq!(requirements.minimum_unique_cards, Some(1));
    assert_eq!(requirements.maximum_unique_cards, Some(2));
    assert_eq!(requirements.cards.len(), 1);
    assert_eq!(requirements.cards[0].card_item_id, 2_380_000);
    assert_eq!(requirements.cards[0].minimum_count, Some(1));
    assert_eq!(requirements.cards[0].maximum_count, Some(4));

    let malformed = property("1");
    let cards = property("mbcard");
    let entry = property("0");
    add_integer(&entry, "id", 2_380_000);
    add_string(&entry, "min", "1");
    add_child(&cards, &entry);
    add_child(&malformed, &cards);
    assert!(
        super::read_monster_book_requirements(100, &malformed, &BTreeSet::from([2_380_000]),)
            .is_err()
    );

    let invalid_bounds = property("1");
    add_integer(&invalid_bounds, "mbmin", 2);
    add_integer(&invalid_bounds, "mbmax", 1);
    assert!(super::read_monster_book_requirements(100, &invalid_bounds, &BTreeSet::new()).is_err());
}

#[test]
fn current_state_checks_are_typed_and_calendar_bounds_are_validated() {
    let start = property("0");
    add_integer(&start, "npc", 1);
    add_integer(&start, "pop", i32::MAX);
    add_string(&start, "worldmin", "0002");
    add_integer(&start, "worldmax", 4);

    let start = read_start_requirements(
        100,
        &start,
        &QuestInfo::default(),
        &BTreeSet::new(),
        &BTreeSet::new(),
    )
    .expect("typed start checks");
    assert_eq!(start.minimum_fame, Some(i32::MAX));
    assert_eq!(start.minimum_world_id, Some(2));
    assert_eq!(start.maximum_world_id, Some(4));

    let completion = property("1");
    add_long(&completion, "endmeso", i64::MAX);
    add_integer(&completion, "questComplete", 7);
    add_string(&completion, "start", "19700102010203");
    add_string(&completion, "end", "19700102010204");
    let completion = read_completion_requirements(
        100,
        &completion,
        &QuestInfo::default(),
        &BTreeSet::new(),
        &BTreeSet::new(),
    )
    .expect("typed completion checks");
    assert_eq!(completion.minimum_mesos, Some(i64::MAX as u64));
    assert_eq!(completion.minimum_completed_quest_count, Some(7));
    assert_eq!(
        completion
            .available_from
            .as_ref()
            .map(|calendar| calendar.unix_ms),
        Some(90_123_000)
    );
    assert_eq!(
        completion
            .available_until
            .as_ref()
            .map(|calendar| calendar.unix_ms),
        Some(90_124_000)
    );
}

#[test]
fn malformed_current_state_checks_fail_closed() {
    for (name, value) in [
        ("worldmin", "+1"),
        ("worldmin", " 1"),
        ("worldmax", "1x"),
        ("worldmax", "4294967296"),
    ] {
        let start = property("0");
        add_integer(&start, "npc", 1);
        add_string(&start, name, value);
        assert!(
            read_start_requirements(
                100,
                &start,
                &QuestInfo::default(),
                &BTreeSet::new(),
                &BTreeSet::new(),
            )
            .is_err(),
            "{name}={value:?} must fail",
        );
    }

    let negative_fame = property("0");
    add_integer(&negative_fame, "npc", 1);
    add_integer(&negative_fame, "pop", -1);
    assert!(
        read_start_requirements(
            100,
            &negative_fame,
            &QuestInfo::default(),
            &BTreeSet::new(),
            &BTreeSet::new(),
        )
        .is_err()
    );

    let reversed_worlds = property("0");
    add_integer(&reversed_worlds, "npc", 1);
    add_integer(&reversed_worlds, "worldmin", 4);
    add_integer(&reversed_worlds, "worldmax", 3);
    assert!(
        read_start_requirements(
            100,
            &reversed_worlds,
            &QuestInfo::default(),
            &BTreeSet::new(),
            &BTreeSet::new(),
        )
        .is_err()
    );

    let negative_mesos = property("1");
    add_long(&negative_mesos, "endmeso", -1);
    assert!(
        read_completion_requirements(
            100,
            &negative_mesos,
            &QuestInfo::default(),
            &BTreeSet::new(),
            &BTreeSet::new(),
        )
        .is_err()
    );

    let reversed_dates = property("1");
    add_string(&reversed_dates, "start", "19700103");
    add_string(&reversed_dates, "end", "19700102");
    assert!(
        read_completion_requirements(
            100,
            &reversed_dates,
            &QuestInfo::default(),
            &BTreeSet::new(),
            &BTreeSet::new(),
        )
        .is_err()
    );
}

#[test]
fn equipped_item_checks_are_typed_in_both_phases() {
    let equipment_ids = BTreeSet::from([1_002_800, 1_032_058, 1_052_166, 1_052_167]);
    let start = property("0");
    add_integer(&start, "npc", 1);
    let all = property("equipAllNeed");
    add_integer(&all, "0", 1_002_800);
    add_integer(&all, "1", 1_032_058);
    add_child(&start, &all);
    let any = property("equipSelectNeed");
    add_integer(&any, "0", 1_052_167);
    add_integer(&any, "1", 1_052_166);
    add_child(&start, &any);

    let start = read_start_requirements(
        100,
        &start,
        &QuestInfo::default(),
        &BTreeSet::new(),
        &equipment_ids,
    )
    .expect("typed start equipment checks");
    assert_eq!(start.equipped_items.all_of, vec![1_002_800, 1_032_058]);
    assert_eq!(start.equipped_items.any_of, vec![1_052_167, 1_052_166]);

    let completion = property("1");
    let all = property("equipAllNeed");
    add_integer(&all, "0", 1_032_058);
    add_child(&completion, &all);
    let any = property("equipSelectNeed");
    add_integer(&any, "0", 1_052_166);
    add_integer(&any, "1", 1_052_167);
    add_child(&completion, &any);

    let completion = read_completion_requirements(
        100,
        &completion,
        &QuestInfo::default(),
        &BTreeSet::new(),
        &equipment_ids,
    )
    .expect("typed completion equipment checks");
    assert_eq!(completion.equipped_items.all_of, vec![1_032_058]);
    assert_eq!(completion.equipped_items.any_of, vec![1_052_166, 1_052_167]);
}

#[test]
fn malformed_or_unknown_equipped_item_checks_fail_closed() {
    let equipment_ids = BTreeSet::from([1_002_800, 1_032_058]);
    for (name, entries) in [
        ("zero ID", vec![("0", 0)]),
        ("negative ID", vec![("0", -1)]),
        ("unknown ID", vec![("0", 1_002_801)]),
        ("ordinary item ID", vec![("0", 4_000_000)]),
        ("duplicate ID", vec![("0", 1_002_800), ("1", 1_002_800)]),
        ("noncanonical index", vec![("00", 1_002_800)]),
        ("index gap", vec![("1", 1_002_800)]),
    ] {
        let check = property("0");
        let all = property("equipAllNeed");
        for (index, item_id) in entries {
            add_integer(&all, index, item_id);
        }
        add_child(&check, &all);
        assert!(
            super::read_equipped_item_requirements(100, &check, &equipment_ids).is_err(),
            "{name} must fail",
        );
    }

    let empty = property("0");
    add_child(&empty, &property("equipSelectNeed"));
    assert!(super::read_equipped_item_requirements(100, &empty, &equipment_ids).is_err());

    let string_id = property("0");
    let any = property("equipSelectNeed");
    add_string(&any, "0", "1002800");
    add_child(&string_id, &any);
    assert!(super::read_equipped_item_requirements(100, &string_id, &equipment_ids).is_err());

    let scalar_list = property("0");
    add_integer(&scalar_list, "equipAllNeed", 1_002_800);
    assert!(super::read_equipped_item_requirements(100, &scalar_list, &equipment_ids).is_err());
}

#[test]
fn record_checks_import_redirects_or_alternatives_and_all_conditions() {
    let check = property("0");
    add_integer(&check, "infoNumber", 200);
    let infoex = property("infoex");
    for (index, condition, value) in [(0, None, "007"), (1, Some(1), "10"), (2, Some(2), "20")] {
        let entry = property(&index.to_string());
        if let Some(condition) = condition {
            add_integer(&entry, "cond", condition);
        }
        add_string(&entry, "value", value);
        add_child(&infoex, &entry);
    }
    add_child(&check, &infoex);

    let conditions = read_record_conditions(100, &check).expect("record predicates");

    assert_eq!(conditions.len(), 1);
    assert_eq!(conditions[0].quest_id, 200);
    assert_eq!(conditions[0].index, 0);
    assert_eq!(
        conditions[0].alternatives,
        vec![
            QuestRecordPredicate::Equal("007".to_owned()),
            QuestRecordPredicate::AtLeast(10),
            QuestRecordPredicate::AtMost(20),
        ]
    );
}

#[test]
fn direct_info_preserves_exact_strings_and_decimal_string_redirects() {
    let check = property("1");
    add_string(&check, "infoNumber", "00200");
    let info = property("info");
    add_string(&info, "0", "AbC");
    add_string(&info, "1", "007");
    add_child(&check, &info);

    let conditions = read_record_conditions(100, &check).expect("direct record predicates");

    assert_eq!(conditions[0].quest_id, 200);
    assert_eq!(
        conditions[0].alternatives,
        vec![
            QuestRecordPredicate::Equal("AbC".to_owned()),
            QuestRecordPredicate::Equal("007".to_owned()),
        ]
    );
}

#[test]
fn malformed_record_checks_are_rejected() {
    for mutate in 0..7 {
        let check = property("0");
        let infoex = property("infoex");
        let entry = property(if mutate == 0 { "1" } else { "0" });
        match mutate {
            1 => add_integer(&entry, "cond", 3),
            2 => add_integer(&entry, "cond", 1),
            3 => add_string(&entry, "value", ""),
            4 => add_string(&entry, "value", "1234567890123456"),
            5 => add_integer(&entry, "value", 1),
            6 => add_string(&check, "infoNumber", "+200"),
            _ => {}
        }
        if mutate != 3 && mutate != 4 && mutate != 5 {
            add_string(&entry, "value", if mutate == 2 { "1x" } else { "1" });
        }
        add_child(&infoex, &entry);
        add_child(&check, &infoex);

        assert!(
            read_record_conditions(100, &check).is_err(),
            "malformed record case {mutate} must fail"
        );
    }

    let redirect_only = property("0");
    add_integer(&redirect_only, "infoNumber", 200);
    assert!(read_record_conditions(100, &redirect_only).is_err());
}

#[test]
fn check_item_counts_default_to_one_and_nonpositive_counts_require_absence() {
    let check = property("1");
    let items = property("item");
    for (index, item_id, count) in [
        (0, 4_000_000, None),
        (1, 4_000_001, Some(0)),
        (2, 4_000_002, Some(-1)),
        (3, 4_000_003, Some(2)),
    ] {
        let entry = property(&index.to_string());
        add_integer(&entry, "id", item_id);
        if let Some(count) = count {
            add_integer(&entry, "count", count);
        }
        add_child(&items, &entry);
    }
    add_child(&check, &items);
    let item_ids = BTreeSet::from([4_000_000, 4_000_001, 4_000_002, 4_000_003]);

    let completion = read_completion_requirements(
        100,
        &check,
        &QuestInfo::default(),
        &item_ids,
        &BTreeSet::new(),
    )
    .expect("item count boundaries");

    assert_eq!(
        completion
            .items
            .iter()
            .map(|item| item.condition)
            .collect::<Vec<_>>(),
        vec![
            QuestItemCondition::AtLeast(NonZeroU32::MIN),
            QuestItemCondition::Absent,
            QuestItemCondition::Absent,
            QuestItemCondition::AtLeast(NonZeroU32::new(2).expect("positive")),
        ]
    );

    let unknown = property("1");
    let items = property("item");
    let entry = property("0");
    add_integer(&entry, "id", 4_000_099);
    add_integer(&entry, "count", -1);
    add_child(&items, &entry);
    add_child(&unknown, &items);
    assert!(matches!(
        read_completion_requirements(
            100,
            &unknown,
            &QuestInfo::default(),
            &BTreeSet::new(),
            &BTreeSet::new(),
        ),
        Err(QuestContentError::Unsupported { category, .. })
            if category == "unknown item reference"
    ));
}
