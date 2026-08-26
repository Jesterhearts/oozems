#![forbid(unsafe_code)]

pub mod v1 {
    include!(concat!(env!("OUT_DIR"), "/oozems.v1.rs"));
}

pub const FILE_DESCRIPTOR_SET: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/oozems_descriptor.bin"));
pub const PROTOBUF_CONTENT_TYPE: &str = "application/x-protobuf";

#[cfg(test)]
mod tests {
    use prost::Message;

    use crate::v1::PlayerState;
    use crate::v1::QuestRecord;
    use crate::v1::QuestRecordEntry;

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
}
