use crate::content::quest::importer::test_support::*;

#[test]
fn item_expiration_civil_times_use_the_gms_pacific_timezone() {
    assert_eq!(
        item_expiration_unix_ms("2024011500"),
        Ok(1_705_305_600_000),
        "midnight PST is 08:00 UTC"
    );
    assert_eq!(
        item_expiration_unix_ms("2024071500"),
        Ok(1_721_026_800_000),
        "midnight PDT is 07:00 UTC"
    );
    assert!(item_expiration_unix_ms("202401150000").is_err());
    assert!(item_expiration_unix_ms("2024023000").is_err());
}

#[test]
fn item_period_is_a_relative_minute_lifetime() {
    let phase = action_phase(None);
    let items = phase.read().expect("phase").at("item").expect("items");
    let entry = items.read().expect("items").at("0").expect("first item");
    add_integer(&entry, "period", 60);

    let imported = read_action_items(
        100,
        "1",
        &phase,
        &BTreeSet::from([4_000_000]),
        &BTreeSet::new(),
    )
    .expect("relative item expiration");

    assert_eq!(
        imported.fixed[0].expiration,
        Some(QuestItemExpiration::RelativeMilliseconds(3_600_000))
    );

    let permanent = action_phase(None);
    let items = permanent.read().expect("phase").at("item").expect("items");
    let entry = items.read().expect("items").at("0").expect("first item");
    add_integer(&entry, "period", 0);
    let imported = read_action_items(
        100,
        "1",
        &permanent,
        &BTreeSet::from([4_000_000]),
        &BTreeSet::new(),
    )
    .expect("zero period is permanent");
    assert_eq!(imported.fixed[0].expiration, None);

    let negative = action_phase(None);
    let items = negative.read().expect("phase").at("item").expect("items");
    let entry = items.read().expect("items").at("0").expect("first item");
    add_integer(&entry, "period", -1);
    assert!(matches!(
        read_action_items(
            100,
            "1",
            &negative,
            &BTreeSet::from([4_000_000]),
            &BTreeSet::new(),
        ),
        Err(QuestContentError::Invalid { .. })
    ));
}

#[test]
fn selectable_item_rewards_preserve_order_count_and_eligibility() {
    let phase = property("1");
    let items = property("item");
    let later = property("1");
    add_integer(&later, "id", 4_000_001);
    add_integer(&later, "count", 3);
    add_integer(&later, "prop", -1);
    add_child(&items, &later);
    let earlier = property("0");
    add_integer(&earlier, "id", 4_000_000);
    add_integer(&earlier, "count", 2);
    add_integer(&earlier, "prop", -1);
    add_integer(&earlier, "job", 2);
    add_integer(&earlier, "gender", 1);
    add_integer(&earlier, "period", 60);
    add_child(&items, &earlier);
    add_child(&phase, &items);

    let imported = read_action_items(
        100,
        "1",
        &phase,
        &BTreeSet::from([4_000_000, 4_000_001]),
        &BTreeSet::new(),
    )
    .expect("selectable rewards");

    assert!(imported.fixed.is_empty());
    assert_eq!(
        imported
            .selectable
            .iter()
            .map(|reward| (reward.item_id, reward.count))
            .collect::<Vec<_>>(),
        vec![(4_000_000, 2), (4_000_001, 3)]
    );
    assert_eq!(
        imported.selectable[0].eligibility,
        QuestRewardEligibility {
            job_mask: Some(2),
            gender: Some(QuestRewardGender::Female),
        }
    );
    assert_eq!(
        imported.selectable[0].expiration,
        Some(QuestItemExpiration::RelativeMilliseconds(3_600_000))
    );

    let fixed = read_action_items(
        100,
        "1",
        &action_phase(None),
        &BTreeSet::from([4_000_000]),
        &BTreeSet::new(),
    )
    .expect("an item without prop remains a fixed grant");
    assert_eq!(fixed.fixed.len(), 1);
    assert!(fixed.conditional.is_empty());
    assert!(fixed.weighted.is_empty());
    assert!(fixed.selectable.is_empty());
}

#[test]
fn omitted_action_item_count_defaults_to_one_and_explicit_zero_is_a_noop() {
    let phase = property("1");
    let items = property("item");
    let defaulted = property("0");
    add_integer(&defaulted, "id", 4_000_000);
    add_child(&items, &defaulted);
    let removal = property("1");
    add_integer(&removal, "id", 4_000_001);
    add_integer(&removal, "count", -2);
    add_child(&items, &removal);
    let selectable = property("2");
    add_integer(&selectable, "id", 4_000_002);
    add_integer(&selectable, "prop", -1);
    add_child(&items, &selectable);
    add_child(&phase, &items);
    let item_ids = BTreeSet::from([4_000_000, 4_000_001, 4_000_002]);

    let imported = read_action_items(100, "1", &phase, &item_ids, &BTreeSet::new())
        .expect("omitted counts use the WZ default");

    assert_eq!(
        imported.fixed,
        vec![
            QuestItemDelta {
                item_id: 4_000_000,
                count: 1,
                expiration: None,
            },
            QuestItemDelta {
                item_id: 4_000_001,
                count: -2,
                expiration: None,
            },
        ]
    );
    assert_eq!(imported.selectable[0].count, 1);

    add_integer(&defaulted, "count", 0);
    let imported = read_action_items(100, "1", &phase, &item_ids, &BTreeSet::new())
        .expect("an explicit zero count is inert");
    assert_eq!(imported.fixed.len(), 1);
    assert_eq!(imported.fixed[0].item_id, 4_000_001);
    assert_eq!(imported.selectable.len(), 1);
}

#[test]
fn exact_inert_action_item_metadata_is_retained_without_changing_actions() {
    let phase = property("0");
    let items = property("item");
    for (index, item_id, var) in [(0, 4_000_000, 1), (1, 4_000_001, 2)] {
        let entry = property(&index.to_string());
        add_integer(&entry, "id", item_id);
        add_integer(&entry, "name", 1);
        add_integer(&entry, "var", var);
        add_child(&items, &entry);
    }
    add_child(&phase, &items);
    let action = property("quest");
    add_child(&action, &phase);

    let (actions, retained) = read_action_phase(
        100,
        &action,
        "0",
        &BTreeSet::from([4_000_000, 4_000_001]),
        &BTreeSet::new(),
    )
    .expect("exact inert item metadata");

    assert_eq!(
        retained,
        vec![
            "act/0/item/0/name",
            "act/0/item/0/var",
            "act/0/item/1/name",
            "act/0/item/1/var",
        ]
    );
    assert_eq!(
        actions.fixed_items,
        vec![
            QuestItemDelta {
                item_id: 4_000_000,
                count: 1,
                expiration: None,
            },
            QuestItemDelta {
                item_id: 4_000_001,
                count: 1,
                expiration: None,
            },
        ]
    );
}

#[test]
fn unknown_or_noninteger_action_item_metadata_fails_closed() {
    for (field, value) in [("name", 0), ("name", 2), ("var", 0), ("var", 3)] {
        let phase = action_phase(None);
        let items = phase.read().expect("phase").at("item").expect("items");
        let entry = items.read().expect("items").at("0").expect("entry");
        add_integer(&entry, field, value);

        assert!(
            read_action_items(
                100,
                "1",
                &phase,
                &BTreeSet::from([4_000_000]),
                &BTreeSet::new(),
            )
            .is_err(),
            "{field}={value} must fail",
        );
    }

    for field in ["name", "var"] {
        let phase = action_phase(None);
        let items = phase.read().expect("phase").at("item").expect("items");
        let entry = items.read().expect("items").at("0").expect("entry");
        add_string(&entry, field, "1");

        assert!(
            read_action_items(
                100,
                "1",
                &phase,
                &BTreeSet::from([4_000_000]),
                &BTreeSet::new(),
            )
            .is_err(),
            "string {field} metadata must fail",
        );
    }
}

#[test]
fn zero_count_actions_do_not_enter_any_item_action_category() {
    let phase = property("1");
    let items = property("item");
    for (index, prop, job) in [
        (0, None, None),
        (1, None, Some(1)),
        (2, Some(10), None),
        (3, Some(-1), None),
    ] {
        let entry = property(&index.to_string());
        add_integer(&entry, "id", 4_000_000 + index);
        add_integer(&entry, "count", 0);
        if let Some(prop) = prop {
            add_integer(&entry, "prop", prop);
        }
        if let Some(job) = job {
            add_integer(&entry, "job", job);
        }
        add_child(&items, &entry);
    }
    add_child(&phase, &items);
    let item_ids = BTreeSet::from([4_000_000, 4_000_001, 4_000_002, 4_000_003]);

    let imported = read_action_items(100, "1", &phase, &item_ids, &BTreeSet::new())
        .expect("zero-count no-ops");

    assert!(imported.fixed.is_empty());
    assert!(imported.conditional.is_empty());
    assert!(imported.weighted.is_empty());
    assert!(imported.selectable.is_empty());

    let unknown = action_phase(None);
    let items = unknown.read().expect("phase").at("item").expect("items");
    let entry = items.read().expect("items").at("0").expect("entry");
    add_integer(&entry, "id", 4_000_099);
    add_integer(&entry, "count", 0);
    assert!(matches!(
        read_action_items(100, "1", &unknown, &BTreeSet::new(), &BTreeSet::new()),
        Err(QuestContentError::Unsupported { category, .. })
            if category == "unknown item reference"
    ));
}

#[test]
fn selectable_rewards_require_a_positive_count() {
    let phase = action_phase(Some(-1));
    let items = phase.read().expect("phase").at("item").expect("items");
    let entry = items.read().expect("items").at("0").expect("first item");
    add_integer(&entry, "count", -1);

    let error = match read_action_items(
        100,
        "1",
        &phase,
        &BTreeSet::from([4_000_000]),
        &BTreeSet::new(),
    ) {
        Err(error) => error,
        Ok(_) => panic!("negative selectable count must fail"),
    };

    assert!(matches!(
        error,
        QuestContentError::Invalid { quest_id: 100, .. }
    ));
}

#[test]
fn expiration_is_preserved_for_conditional_and_weighted_rewards() {
    let phase = property("1");
    let items = property("item");
    let conditional = property("0");
    add_integer(&conditional, "id", 4_000_000);
    add_integer(&conditional, "job", 1);
    add_string(&conditional, "dateExpire", "2030011500");
    add_child(&items, &conditional);
    let weighted = property("1");
    add_integer(&weighted, "id", 4_000_001);
    add_integer(&weighted, "prop", 10);
    add_integer(&weighted, "period", 30);
    add_child(&items, &weighted);
    add_child(&phase, &items);

    let imported = read_action_items(
        100,
        "1",
        &phase,
        &BTreeSet::from([4_000_000, 4_000_001]),
        &BTreeSet::new(),
    )
    .expect("filtered expiring rewards");

    assert_eq!(
        imported.conditional[0].expiration,
        Some(QuestItemExpiration::AbsoluteUnixMilliseconds(
            item_expiration_unix_ms("2030011500").expect("deadline"),
        ))
    );
    assert_eq!(
        imported.weighted[0].expiration,
        Some(QuestItemExpiration::RelativeMilliseconds(1_800_000))
    );
}

#[test]
fn expiring_equipment_is_preserved_for_fixed_weighted_and_selectable_rewards() {
    let phase = property("1");
    let items = property("item");
    let fixed = property("0");
    add_integer(&fixed, "id", 1_040_002);
    add_string(&fixed, "dateExpire", "2030011500");
    add_child(&items, &fixed);
    let weighted = property("1");
    add_integer(&weighted, "id", 1_060_002);
    add_integer(&weighted, "prop", 10);
    add_integer(&weighted, "period", 30);
    add_child(&items, &weighted);
    let selectable = property("2");
    add_integer(&selectable, "id", 1_072_000);
    add_integer(&selectable, "prop", -1);
    add_string(&selectable, "dateExpire", "2030011600");
    add_child(&items, &selectable);
    add_child(&phase, &items);
    let item_ids = BTreeSet::from([1_040_002, 1_060_002, 1_072_000]);

    let imported = read_action_items(100, "1", &phase, &item_ids, &item_ids)
        .expect("expiring equipment rewards");

    assert_eq!(
        imported.fixed[0].expiration,
        Some(QuestItemExpiration::AbsoluteUnixMilliseconds(
            item_expiration_unix_ms("2030011500").expect("fixed deadline"),
        ))
    );
    assert_eq!(
        imported.weighted[0].expiration,
        Some(QuestItemExpiration::RelativeMilliseconds(1_800_000))
    );
    assert_eq!(
        imported.selectable[0].expiration,
        Some(QuestItemExpiration::AbsoluteUnixMilliseconds(
            item_expiration_unix_ms("2030011600").expect("selectable deadline"),
        ))
    );
}

#[test]
fn invalid_expiration_metadata_is_explicit() {
    let phase = action_phase(None);
    let items = phase.read().expect("phase").at("item").expect("items");
    let entry = items.read().expect("items").at("0").expect("first item");
    add_integer(&entry, "period", 1);
    add_string(&entry, "dateExpire", "2030011500");
    assert!(matches!(
        read_action_items(
            100,
            "1",
            &phase,
            &BTreeSet::from([4_000_000]),
            &BTreeSet::new(),
        ),
        Err(QuestContentError::Invalid { .. })
    ));

    let removal = action_phase(None);
    let items = removal.read().expect("phase").at("item").expect("items");
    let entry = items.read().expect("items").at("0").expect("first item");
    add_integer(&entry, "count", -1);
    add_integer(&entry, "period", 0);
    assert!(matches!(
        read_action_items(
            100,
            "1",
            &removal,
            &BTreeSet::from([4_000_000]),
            &BTreeSet::new(),
        ),
        Err(QuestContentError::Invalid { .. })
    ));
}

#[test]
fn selectable_start_rewards_are_rejected_but_completion_rewards_can_follow_quizzes() {
    let selectable = QuestSelectableItemReward {
        item_id: 4_000_000,
        count: 1,
        expiration: None,
        eligibility: QuestRewardEligibility::default(),
    };
    let start = QuestActions {
        selectable_items: vec![selectable],
        ..QuestActions::default()
    };
    let start_error = validate_selectable_reward_flow(
        100,
        &start,
        &QuestActions::default(),
        &QuestDialogue::default(),
    )
    .expect_err("start reward selection");
    assert!(matches!(
        start_error,
        QuestContentError::Unsupported { category, .. }
            if category == "selectable reward in start actions"
    ));

    let completion = QuestActions {
        selectable_items: vec![selectable],
        ..QuestActions::default()
    };
    validate_selectable_reward_flow(
        100,
        &QuestActions::default(),
        &completion,
        &QuestDialogue::default(),
    )
    .expect("completion quiz followed by reward selection");
}
