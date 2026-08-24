#![forbid(unsafe_code)]

pub mod v1 {
    include!(concat!(env!("OUT_DIR"), "/oozems.v1.rs"));
}

pub const PROTOBUF_CONTENT_TYPE: &str = "application/x-protobuf";

#[cfg(test)]
mod tests {
    use prost::Message;

    use crate::v1::EquipmentSlot;
    use crate::v1::EquippedItem;
    use crate::v1::LearnedSkill;
    use crate::v1::MonsterBookCard;
    use crate::v1::Npc;
    use crate::v1::NpcAnimation;
    use crate::v1::NpcAnimationEvent;
    use crate::v1::NpcFrame;
    use crate::v1::NpcInteractionResponse;
    use crate::v1::PlayerSkill;
    use crate::v1::PlayerState;
    use crate::v1::QuestRecord;
    use crate::v1::QuestRecordEntry;

    #[test]
    fn absent_legacy_record_field_decodes_as_empty() {
        let legacy = PlayerState {
            id: "legacy".to_owned(),
            ..PlayerState::default()
        };
        let encoded = legacy.encode_to_vec();

        let decoded = PlayerState::decode(encoded.as_slice()).expect("decode legacy player");

        assert!(decoded.quest_records.is_empty());
    }

    #[test]
    fn quest_records_round_trip_sparse_entries_and_exact_values() {
        let player = PlayerState {
            quest_records: vec![QuestRecord {
                quest_id: 2_236,
                entries: vec![
                    QuestRecordEntry {
                        index: 0,
                        value: "000000".to_owned(),
                    },
                    QuestRecordEntry {
                        index: 42,
                        value: "AbC".to_owned(),
                    },
                ],
            }],
            ..PlayerState::default()
        };

        let decoded =
            PlayerState::decode(player.encode_to_vec().as_slice()).expect("decode player records");

        assert_eq!(decoded, player);
    }

    #[test]
    fn monster_book_cards_are_additive_and_round_trip() {
        let legacy = PlayerState {
            id: "legacy".to_owned(),
            ..PlayerState::default()
        };
        assert!(
            PlayerState::decode(legacy.encode_to_vec().as_slice())
                .expect("decode legacy player")
                .monster_book_cards
                .is_empty()
        );

        let player = PlayerState {
            monster_book_cards: vec![MonsterBookCard {
                card_item_id: 2_380_000,
                count: 5,
            }],
            ..PlayerState::default()
        };
        assert_eq!(
            PlayerState::decode(player.encode_to_vec().as_slice())
                .expect("decode Monster Book cards"),
            player
        );
    }

    #[test]
    fn skill_master_levels_are_additive_and_round_trip() {
        let legacy_learned = LearnedSkill {
            skill_id: 1_000,
            level: 2,
            master_level: 0,
        };
        let legacy_player_skill = PlayerSkill {
            level: 2,
            master_level: 0,
            ..PlayerSkill::default()
        };
        assert_eq!(
            LearnedSkill::decode(legacy_learned.encode_to_vec().as_slice())
                .expect("decode legacy learned skill")
                .master_level,
            0
        );
        assert_eq!(
            PlayerSkill::decode(legacy_player_skill.encode_to_vec().as_slice())
                .expect("decode legacy player skill")
                .master_level,
            0
        );

        let mastered = LearnedSkill {
            skill_id: 2_321_003,
            level: 0,
            master_level: 15,
        };
        assert_eq!(
            LearnedSkill::decode(mastered.encode_to_vec().as_slice())
                .expect("decode mastered skill"),
            mastered
        );
    }

    #[test]
    fn equipped_item_expiration_is_additive_and_round_trips_exactly() {
        let legacy = EquippedItem {
            slot: EquipmentSlot::Top as i32,
            item_id: 1_040_002,
            expires_at_unix_ms: 0,
        };
        assert_eq!(
            EquippedItem::decode(legacy.encode_to_vec().as_slice())
                .expect("decode legacy equipped item")
                .expires_at_unix_ms,
            0
        );

        let expiring = EquippedItem {
            expires_at_unix_ms: 1_900_000_000_123,
            ..legacy
        };
        assert_eq!(
            EquippedItem::decode(expiring.encode_to_vec().as_slice())
                .expect("decode expiring equipped item"),
            expiring
        );
    }

    #[test]
    fn named_npc_animations_and_one_shot_events_round_trip_additively() {
        let legacy = Npc {
            frames: vec![NpcFrame {
                asset_id: "stand".to_owned(),
                delay_ms: 100,
                ..NpcFrame::default()
            }],
            ..Npc::default()
        };
        let decoded = Npc::decode(legacy.encode_to_vec().as_slice()).expect("decode legacy NPC");
        assert_eq!(decoded.frames, legacy.frames);
        assert!(decoded.animations.is_empty());

        let named = Npc {
            animations: vec![NpcAnimation {
                name: "quest".to_owned(),
                frames: legacy.frames.clone(),
            }],
            ..legacy
        };
        assert_eq!(
            Npc::decode(named.encode_to_vec().as_slice()).expect("decode named NPC"),
            named
        );

        let response = NpcInteractionResponse {
            npc_animation: Some(NpcAnimationEvent {
                map_id: 100_000_000,
                npc_spawn_id: 7,
                npc_id: 1_092_003,
                action_name: "quest".to_owned(),
                player_revision: 42,
            }),
            ..NpcInteractionResponse::default()
        };
        assert_eq!(
            NpcInteractionResponse::decode(response.encode_to_vec().as_slice())
                .expect("decode NPC animation response"),
            response
        );
    }
}
