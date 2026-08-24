use oozems_proto::v1::PlayerState;
use oozems_proto::v1::QuestRecord;
use oozems_proto::v1::QuestRecordEntry;
use thiserror::Error;

pub const MAXIMUM_VALUE_BYTES: usize = 15;

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum QuestRecordError {
    #[error("quest record ID must be nonzero")]
    ZeroQuestId,
    #[error("quest record {quest_id} appears more than once")]
    DuplicateQuestId { quest_id: u32 },
    #[error("quest record {quest_id} index {index} appears more than once")]
    DuplicateIndex { quest_id: u32, index: u32 },
    #[error("quest record values must be ASCII and at most {MAXIMUM_VALUE_BYTES} bytes")]
    InvalidValue,
}

pub fn canonicalize(mut records: Vec<QuestRecord>) -> Result<Vec<QuestRecord>, QuestRecordError> {
    for record in &mut records {
        validate_quest_id(record.quest_id)?;
        record.entries.sort_by_key(|entry| entry.index);
        for entry in &record.entries {
            validate_value(&entry.value)?;
        }
        if let Some(entries) = record
            .entries
            .windows(2)
            .find(|pair| pair[0].index == pair[1].index)
        {
            return Err(QuestRecordError::DuplicateIndex {
                quest_id: record.quest_id,
                index: entries[0].index,
            });
        }
    }
    records.sort_by_key(|record| record.quest_id);
    if let Some(records) = records
        .windows(2)
        .find(|pair| pair[0].quest_id == pair[1].quest_id)
    {
        return Err(QuestRecordError::DuplicateQuestId {
            quest_id: records[0].quest_id,
        });
    }
    Ok(records)
}

pub fn get(
    player: &PlayerState,
    quest_id: u32,
    index: u32,
) -> Option<&str> {
    let record_index = player
        .quest_records
        .binary_search_by_key(&quest_id, |record| record.quest_id)
        .ok()?;
    let record = &player.quest_records[record_index];
    let entry_index = record
        .entries
        .binary_search_by_key(&index, |entry| entry.index)
        .ok()?;
    Some(&record.entries[entry_index].value)
}

pub fn set(
    player: &mut PlayerState,
    quest_id: u32,
    index: u32,
    value: String,
) -> Result<(), QuestRecordError> {
    validate_quest_id(quest_id)?;
    validate_value(&value)?;
    let record_index = match player
        .quest_records
        .binary_search_by_key(&quest_id, |record| record.quest_id)
    {
        Ok(index) => index,
        Err(index) => {
            player.quest_records.insert(
                index,
                QuestRecord {
                    quest_id,
                    entries: Vec::new(),
                },
            );
            index
        }
    };
    let entries = &mut player.quest_records[record_index].entries;
    match entries.binary_search_by_key(&index, |entry| entry.index) {
        Ok(entry_index) => entries[entry_index].value = value,
        Err(entry_index) => entries.insert(entry_index, QuestRecordEntry { index, value }),
    }
    Ok(())
}

pub fn clear(
    player: &mut PlayerState,
    quest_id: u32,
) {
    if let Ok(index) = player
        .quest_records
        .binary_search_by_key(&quest_id, |record| record.quest_id)
    {
        player.quest_records.remove(index);
    }
}

pub fn validate_quest_id(quest_id: u32) -> Result<(), QuestRecordError> {
    (quest_id != 0)
        .then_some(())
        .ok_or(QuestRecordError::ZeroQuestId)
}

pub fn validate_value(value: &str) -> Result<(), QuestRecordError> {
    (value.len() <= MAXIMUM_VALUE_BYTES && value.is_ascii())
        .then_some(())
        .ok_or(QuestRecordError::InvalidValue)
}

pub fn strict_decimal(value: &str) -> Option<u64> {
    (!value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| value.parse().ok())
        .flatten()
}

#[cfg(test)]
mod tests {
    use oozems_proto::v1::PlayerState;
    use oozems_proto::v1::QuestRecord;
    use oozems_proto::v1::QuestRecordEntry;

    use super::QuestRecordError;
    use super::canonicalize;
    use super::clear;
    use super::get;
    use super::set;
    use super::strict_decimal;

    #[test]
    fn get_set_and_clear_keep_records_canonical() {
        let mut player = PlayerState::default();

        set(&mut player, 200, 7, "late".to_owned()).expect("set record");
        set(&mut player, 100, 9, "09".to_owned()).expect("set record");
        set(&mut player, 100, 2, "two".to_owned()).expect("set record");
        set(&mut player, 100, 9, "009".to_owned()).expect("replace record");

        assert_eq!(get(&player, 100, 9), Some("009"));
        assert_eq!(get(&player, 100, 8), None);
        assert_eq!(player.quest_records[0].quest_id, 100);
        assert_eq!(player.quest_records[0].entries[0].index, 2);
        clear(&mut player, 100);
        assert_eq!(get(&player, 100, 9), None);
        assert_eq!(get(&player, 200, 7), Some("late"));
    }

    #[test]
    fn canonicalization_sorts_and_rejects_invalid_records() {
        let records = canonicalize(vec![
            QuestRecord {
                quest_id: 2,
                entries: vec![QuestRecordEntry {
                    index: 8,
                    value: "B".to_owned(),
                }],
            },
            QuestRecord {
                quest_id: 1,
                entries: vec![
                    QuestRecordEntry {
                        index: 9,
                        value: "09".to_owned(),
                    },
                    QuestRecordEntry {
                        index: 3,
                        value: "03".to_owned(),
                    },
                ],
            },
        ])
        .expect("canonical records");
        assert_eq!(records[0].quest_id, 1);
        assert_eq!(records[0].entries[0].index, 3);

        assert_eq!(
            canonicalize(vec![
                QuestRecord {
                    quest_id: 1,
                    entries: Vec::new(),
                },
                QuestRecord {
                    quest_id: 1,
                    entries: Vec::new(),
                },
            ]),
            Err(QuestRecordError::DuplicateQuestId { quest_id: 1 })
        );
        assert_eq!(
            canonicalize(vec![QuestRecord {
                quest_id: 1,
                entries: vec![
                    QuestRecordEntry {
                        index: 7,
                        value: "first".to_owned(),
                    },
                    QuestRecordEntry {
                        index: 7,
                        value: "second".to_owned(),
                    },
                ],
            }]),
            Err(QuestRecordError::DuplicateIndex {
                quest_id: 1,
                index: 7,
            })
        );
        assert_eq!(
            canonicalize(vec![QuestRecord {
                quest_id: 0,
                entries: Vec::new(),
            }]),
            Err(QuestRecordError::ZeroQuestId)
        );
        assert!(
            canonicalize(vec![QuestRecord {
                quest_id: 1,
                entries: vec![QuestRecordEntry {
                    index: 0,
                    value: "1234567890123456".to_owned(),
                }],
            }])
            .is_err()
        );
        assert!(
            canonicalize(vec![QuestRecord {
                quest_id: 1,
                entries: vec![QuestRecordEntry {
                    index: 0,
                    value: String::from_utf8(vec![0xc3, 0xa9]).expect("valid UTF-8"),
                }],
            }])
            .is_err()
        );
    }

    #[test]
    fn decimal_parsing_is_strict_and_preserves_storage_strings() {
        assert_eq!(strict_decimal("0009"), Some(9));
        assert_eq!(strict_decimal(""), None);
        assert_eq!(strict_decimal("+9"), None);
        assert_eq!(strict_decimal(" 9"), None);
        assert_eq!(strict_decimal("9a"), None);
    }
}
