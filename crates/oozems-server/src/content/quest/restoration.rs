use super::model::QuestRestorationEligibility;
use super::model::QuestRestorationProvenance;
use super::model::QuestStateRequirement;
use super::model::RequiredQuestState;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct AuditedRestorationRule {
    pub quest_id: u32,
    pub item_id: u32,
    pub target_count: u64,
    pub provenance: QuestRestorationProvenance,
    pub eligibility: QuestRestorationEligibility,
}

const OWNER_STARTED: QuestRestorationEligibility = QuestRestorationEligibility {
    owner_state: RequiredQuestState::Started,
    required_quests: &[],
    forbidden_quests: &[],
    absent_skill_ids: &[],
    absent_item_ids: &[4_031_698],
};

const OWNER_COMPLETED_2208: QuestRestorationEligibility = completed(&[], &[], &[], &[4_031_890]);
const OWNER_COMPLETED_4007: QuestRestorationEligibility = completed(
    &[QuestStateRequirement {
        quest_id: 4_006,
        state: RequiredQuestState::Started,
    }],
    &[],
    &[],
    &[4_031_291],
);
const OWNER_COMPLETED_4008: QuestRestorationEligibility = completed(
    &[QuestStateRequirement {
        quest_id: 4_007,
        state: RequiredQuestState::Started,
    }],
    &[],
    &[],
    &[4_031_303],
);
const OWNER_COMPLETED_4009: QuestRestorationEligibility = completed(
    &[QuestStateRequirement {
        quest_id: 4_006,
        state: RequiredQuestState::Started,
    }],
    &[],
    &[],
    &[4_031_292],
);
const OWNER_COMPLETED_4010: QuestRestorationEligibility = completed(
    &[QuestStateRequirement {
        quest_id: 4_006,
        state: RequiredQuestState::Started,
    }],
    &[],
    &[],
    &[4_031_293],
);
const OWNER_COMPLETED_4944: QuestRestorationEligibility = completed(&[], &[], &[], &[4_031_771]);
const OWNER_COMPLETED_4960: QuestRestorationEligibility = completed(&[], &[], &[], &[4_031_771]);
const OWNER_COMPLETED_4946: QuestRestorationEligibility = completed(
    &[],
    &[QuestStateRequirement {
        quest_id: 4_947,
        state: RequiredQuestState::Completed,
    }],
    &[],
    &[4_031_767],
);
const OWNER_COMPLETED_4953: QuestRestorationEligibility = completed(
    &[QuestStateRequirement {
        quest_id: 4_948,
        state: RequiredQuestState::NotStarted,
    }],
    &[],
    &[],
    &[4_031_768],
);
const OWNER_COMPLETED_4954: QuestRestorationEligibility = completed(
    &[],
    &[QuestStateRequirement {
        quest_id: 4_942,
        state: RequiredQuestState::Completed,
    }],
    &[],
    &[4_031_772],
);
const OWNER_COMPLETED_6263: QuestRestorationEligibility =
    completed(&[], &[], &[2_221_003], &[2_280_011, 4_031_450]);
const OWNER_COMPLETED_6273: QuestRestorationEligibility =
    completed(&[], &[], &[2_121_003], &[2_280_000, 4_001_109]);

pub(super) const AUDITED_RESTORATION_RULES: &[AuditedRestorationRule] = &[
    completion_rule(2_208, 4_031_890, OWNER_COMPLETED_2208),
    AuditedRestorationRule {
        quest_id: 3_310,
        item_id: 4_031_698,
        target_count: 1,
        // Reactor 2619000 consumes this device to produce quest objective 4031709.
        provenance: QuestRestorationProvenance::AuditedReactorDevice,
        eligibility: OWNER_STARTED,
    },
    completion_rule(4_007, 4_031_291, OWNER_COMPLETED_4007),
    completion_rule(4_008, 4_031_303, OWNER_COMPLETED_4008),
    completion_rule(4_009, 4_031_292, OWNER_COMPLETED_4009),
    completion_rule(4_010, 4_031_293, OWNER_COMPLETED_4010),
    completion_rule(4_944, 4_031_771, OWNER_COMPLETED_4944),
    completion_rule(4_946, 4_031_767, OWNER_COMPLETED_4946),
    completion_rule(4_953, 4_031_768, OWNER_COMPLETED_4953),
    completion_rule(4_954, 4_031_772, OWNER_COMPLETED_4954),
    completion_rule(4_960, 4_031_771, OWNER_COMPLETED_4960),
    completion_rule(6_263, 4_031_450, OWNER_COMPLETED_6263),
    completion_rule(6_273, 4_001_109, OWNER_COMPLETED_6273),
];

const fn completed(
    required_quests: &'static [QuestStateRequirement],
    forbidden_quests: &'static [QuestStateRequirement],
    absent_skill_ids: &'static [u32],
    absent_item_ids: &'static [u32],
) -> QuestRestorationEligibility {
    QuestRestorationEligibility {
        owner_state: RequiredQuestState::Completed,
        required_quests,
        forbidden_quests,
        absent_skill_ids,
        absent_item_ids,
    }
}

const fn completion_rule(
    quest_id: u32,
    item_id: u32,
    eligibility: QuestRestorationEligibility,
) -> AuditedRestorationRule {
    AuditedRestorationRule {
        quest_id,
        item_id,
        target_count: 1,
        provenance: QuestRestorationProvenance::AuditedCompletionGrant,
        eligibility,
    }
}

pub(super) fn audited_rule(quest_id: u32) -> Option<&'static AuditedRestorationRule> {
    AUDITED_RESTORATION_RULES
        .iter()
        .find(|rule| rule.quest_id == quest_id)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use wz_reader::WzNodeArc;
    use wz_reader::WzNodeCast;

    #[test]
    fn audited_restoration_rules_have_the_exact_reviewed_shape() {
        let actual = super::AUDITED_RESTORATION_RULES
            .iter()
            .map(|rule| {
                (
                    rule.quest_id,
                    rule.item_id,
                    rule.target_count,
                    rule.provenance,
                    rule.eligibility.owner_state,
                    rule.eligibility
                        .required_quests
                        .iter()
                        .map(|requirement| (requirement.quest_id, requirement.state))
                        .collect::<Vec<_>>(),
                    rule.eligibility
                        .forbidden_quests
                        .iter()
                        .map(|requirement| (requirement.quest_id, requirement.state))
                        .collect::<Vec<_>>(),
                    rule.eligibility.absent_skill_ids,
                    rule.eligibility.absent_item_ids,
                )
            })
            .collect::<Vec<_>>();

        use super::QuestRestorationProvenance as Provenance;
        use super::RequiredQuestState as State;
        assert_eq!(
            actual,
            vec![
                (
                    2_208,
                    4_031_890,
                    1,
                    Provenance::AuditedCompletionGrant,
                    State::Completed,
                    vec![],
                    vec![],
                    &[][..],
                    &[4_031_890][..]
                ),
                (
                    3_310,
                    4_031_698,
                    1,
                    Provenance::AuditedReactorDevice,
                    State::Started,
                    vec![],
                    vec![],
                    &[][..],
                    &[4_031_698][..]
                ),
                (
                    4_007,
                    4_031_291,
                    1,
                    Provenance::AuditedCompletionGrant,
                    State::Completed,
                    vec![(4_006, State::Started)],
                    vec![],
                    &[][..],
                    &[4_031_291][..]
                ),
                (
                    4_008,
                    4_031_303,
                    1,
                    Provenance::AuditedCompletionGrant,
                    State::Completed,
                    vec![(4_007, State::Started)],
                    vec![],
                    &[][..],
                    &[4_031_303][..]
                ),
                (
                    4_009,
                    4_031_292,
                    1,
                    Provenance::AuditedCompletionGrant,
                    State::Completed,
                    vec![(4_006, State::Started)],
                    vec![],
                    &[][..],
                    &[4_031_292][..]
                ),
                (
                    4_010,
                    4_031_293,
                    1,
                    Provenance::AuditedCompletionGrant,
                    State::Completed,
                    vec![(4_006, State::Started)],
                    vec![],
                    &[][..],
                    &[4_031_293][..]
                ),
                (
                    4_944,
                    4_031_771,
                    1,
                    Provenance::AuditedCompletionGrant,
                    State::Completed,
                    vec![],
                    vec![],
                    &[][..],
                    &[4_031_771][..]
                ),
                (
                    4_946,
                    4_031_767,
                    1,
                    Provenance::AuditedCompletionGrant,
                    State::Completed,
                    vec![],
                    vec![(4_947, State::Completed)],
                    &[][..],
                    &[4_031_767][..]
                ),
                (
                    4_953,
                    4_031_768,
                    1,
                    Provenance::AuditedCompletionGrant,
                    State::Completed,
                    vec![(4_948, State::NotStarted)],
                    vec![],
                    &[][..],
                    &[4_031_768][..]
                ),
                (
                    4_954,
                    4_031_772,
                    1,
                    Provenance::AuditedCompletionGrant,
                    State::Completed,
                    vec![],
                    vec![(4_942, State::Completed)],
                    &[][..],
                    &[4_031_772][..]
                ),
                (
                    4_960,
                    4_031_771,
                    1,
                    Provenance::AuditedCompletionGrant,
                    State::Completed,
                    vec![],
                    vec![],
                    &[][..],
                    &[4_031_771][..]
                ),
                (
                    6_263,
                    4_031_450,
                    1,
                    Provenance::AuditedCompletionGrant,
                    State::Completed,
                    vec![],
                    vec![],
                    &[2_221_003][..],
                    &[2_280_011, 4_031_450][..]
                ),
                (
                    6_273,
                    4_001_109,
                    1,
                    Provenance::AuditedCompletionGrant,
                    State::Completed,
                    vec![],
                    vec![],
                    &[2_121_003][..],
                    &[2_280_000, 4_001_109][..]
                ),
            ]
        );
    }

    #[test]
    fn local_reactor_2619000_consumes_the_3310_device() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/Reactor.wz");
        if !path.exists() {
            return;
        }
        let root = crate::content::wz::open_archive(&path).expect("reactor archive");
        crate::content::wz::parse(&root, "reactor archive root".to_owned())
            .expect("parse reactor archive");
        let image = child(&root, "2619000.img");
        crate::content::wz::parse(&image, "reactor image 2619000".to_owned())
            .expect("parse reactor image");

        for (state, next_state) in [("0", 1), ("1", 2)] {
            let event = child(&child(&child(&image, state), "event"), "0");
            assert_eq!(integer(&child(&event, "type")), 100);
            assert_eq!(integer(&child(&event, "0")), 4_031_698);
            assert_eq!(integer(&child(&event, "1")), 1);
            assert_eq!(integer(&child(&event, "state")), next_state);
        }
    }

    fn child(
        node: &WzNodeArc,
        name: &str,
    ) -> WzNodeArc {
        crate::content::wz::child(node, name)
            .expect("reactor child lookup")
            .unwrap_or_else(|| panic!("missing reactor field {name}"))
    }

    fn integer(node: &WzNodeArc) -> i64 {
        let read = node.read().expect("reactor node lock");
        read.try_as_int()
            .map(|value| i64::from(*value))
            .or_else(|| read.try_as_short().map(|value| i64::from(*value)))
            .or_else(|| read.try_as_long().copied())
            .expect("reactor integer")
    }
}
