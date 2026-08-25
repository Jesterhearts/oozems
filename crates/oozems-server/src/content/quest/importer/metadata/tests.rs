use crate::content::quest::importer::test_support::*;

#[test]
fn calendar_values_become_unix_milliseconds() {
    assert_eq!(calendar_unix_ms("19700101"), Ok(0));
    assert_eq!(calendar_unix_ms("19700102010203"), Ok(90_123_000));
    assert!(calendar_unix_ms("20230229").is_err());
}

#[test]
fn both_archive_timer_fields_are_seconds() {
    let info = property("info");
    add_integer(&info, "timeLimit", 3_600);
    add_integer(&info, "timeLimit2", 2_592_000);

    let info = read_info(100, &info).expect("quest timer metadata");

    assert_eq!(info.time_limit_ms, Some(3_600_000));
    assert_eq!(info.time_limit2_ms, Some(2_592_000_000));
    assert_eq!(
        quest_timer_milliseconds(15, "timeLimit", 100).expect("seconds duration"),
        15_000
    );
    assert!(quest_timer_milliseconds(u64::MAX, "timeLimit2", 100).is_err());
}

#[test]
fn quest_info_area_is_typed_instead_of_retained_as_unknown_metadata() {
    let info = property("info");
    add_integer(&info, "area", 51);

    let info = read_info(100, &info).expect("typed quest area");

    assert_eq!(info.area, Some(51));
    assert!(
        !info
            .retained_metadata_fields
            .iter()
            .any(|field| field == "area")
    );
}

#[test]
fn selected_skill_metadata_is_positive_authoritative_and_named() {
    let info = property("2415");
    add_integer(&info, "selectedSkillID", 1_001_004);
    let root = property("QuestInfo");

    let parsed = read_info_with_skills(
        2_415,
        &info,
        &root,
        &BTreeSet::from([1_001_004]),
        &BTreeMap::from([(1_001_004, "Power Strike".to_owned())]),
    )
    .expect("selected skill");
    let selected = parsed.selected_skill.expect("typed selected skill");
    assert_eq!(selected.id.get(), 1_001_004);
    assert_eq!(selected.name.as_deref(), Some("Power Strike"));

    let known_skills = BTreeSet::from([1_001_004]);
    let no_skills = BTreeSet::new();
    for (value, known) in [(0, true), (-1, true), (1_001_005, false)] {
        let info = property("2415");
        add_integer(&info, "selectedSkillID", value);
        assert!(
            read_info_with_skills(
                2_415,
                &info,
                &root,
                if known { &known_skills } else { &no_skills },
                &BTreeMap::new(),
            )
            .is_err()
        );
    }
}
