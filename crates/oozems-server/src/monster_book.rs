use oozems_proto::v1::MonsterBookCard;
use thiserror::Error;

pub const MAX_CARD_COUNT: u32 = 5;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum MonsterBookError {
    #[error("Monster Book card item ID must be positive")]
    InvalidCardItemId,
    #[error("Monster Book card {card_item_id} count must be between 1 and {MAX_CARD_COUNT}")]
    InvalidCount { card_item_id: u32 },
    #[error("Monster Book card {card_item_id} appears more than once")]
    DuplicateCard { card_item_id: u32 },
}

pub fn canonicalize(
    mut cards: Vec<MonsterBookCard>
) -> Result<Vec<MonsterBookCard>, MonsterBookError> {
    cards.sort_unstable_by_key(|card| card.card_item_id);
    let mut previous = None;
    for card in &cards {
        if card.card_item_id == 0 {
            return Err(MonsterBookError::InvalidCardItemId);
        }
        if !(1..=MAX_CARD_COUNT).contains(&card.count) {
            return Err(MonsterBookError::InvalidCount {
                card_item_id: card.card_item_id,
            });
        }
        if previous == Some(card.card_item_id) {
            return Err(MonsterBookError::DuplicateCard {
                card_item_id: card.card_item_id,
            });
        }
        previous = Some(card.card_item_id);
    }
    Ok(cards)
}

pub fn add_card(
    cards: &mut Vec<MonsterBookCard>,
    card_item_id: u32,
) {
    match cards.binary_search_by_key(&card_item_id, |card| card.card_item_id) {
        Ok(index) => {
            cards[index].count = cards[index].count.saturating_add(1).min(MAX_CARD_COUNT);
        }
        Err(index) => cards.insert(
            index,
            MonsterBookCard {
                card_item_id,
                count: 1,
            },
        ),
    }
}

pub fn count(
    cards: &[MonsterBookCard],
    card_item_id: u32,
) -> u32 {
    cards
        .binary_search_by_key(&card_item_id, |card| card.card_item_id)
        .ok()
        .map_or(0, |index| cards[index].count)
}

#[cfg(test)]
mod tests {
    use oozems_proto::v1::MonsterBookCard;

    #[test]
    fn canonical_cards_are_sorted_and_reject_invalid_collections_atomically() {
        let cards = super::canonicalize(vec![
            MonsterBookCard {
                card_item_id: 2_380_001,
                count: 5,
            },
            MonsterBookCard {
                card_item_id: 2_380_000,
                count: 1,
            },
        ])
        .expect("canonical cards");
        assert_eq!(
            cards
                .iter()
                .map(|card| (card.card_item_id, card.count))
                .collect::<Vec<_>>(),
            vec![(2_380_000, 1), (2_380_001, 5)]
        );

        for malformed in [
            vec![MonsterBookCard {
                card_item_id: 0,
                count: 1,
            }],
            vec![MonsterBookCard {
                card_item_id: 2_380_000,
                count: 0,
            }],
            vec![
                MonsterBookCard {
                    card_item_id: 2_380_000,
                    count: 1,
                },
                MonsterBookCard {
                    card_item_id: 2_380_000,
                    count: 2,
                },
            ],
        ] {
            assert!(super::canonicalize(malformed).is_err());
        }
    }

    #[test]
    fn structurally_valid_unknown_cards_are_preserved() {
        let cards = vec![MonsterBookCard {
            card_item_id: 2_389_999,
            count: 1,
        }];

        assert_eq!(super::canonicalize(cards.clone()), Ok(cards));
    }

    #[test]
    fn adding_cards_inserts_in_order_and_caps_at_five() {
        let mut cards = vec![MonsterBookCard {
            card_item_id: 2_380_001,
            count: 5,
        }];
        super::add_card(&mut cards, 2_380_000);
        super::add_card(&mut cards, 2_380_001);

        assert_eq!(super::count(&cards, 2_380_000), 1);
        assert_eq!(super::count(&cards, 2_380_001), 5);
    }
}
