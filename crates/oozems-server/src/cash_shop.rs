use std::collections::HashSet;
use std::fs;
use std::io::ErrorKind;
use std::path::Path;
use std::path::PathBuf;

use oozems_proto::v1::PlayerState;
use serde::Deserialize;
use thiserror::Error;

use crate::items::ItemDefinitionLookup;
use crate::items::ItemRuleError;

const MAXIMUM_OFFERS: usize = 10;
const MAXIMUM_CURRENCY_NAME_CHARACTERS: usize = 24;
const DEFAULT_CURRENCY_NAME: &str = "Ooze";

#[derive(Clone, Debug)]
pub struct CashShopCatalog {
    currency_name: String,
    offers: Vec<CashShopOffer>,
}

impl Default for CashShopCatalog {
    fn default() -> Self {
        Self {
            currency_name: DEFAULT_CURRENCY_NAME.to_owned(),
            offers: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CashShopOffer {
    pub offer_id: u32,
    pub item_id: u32,
    pub price: u64,
    pub lifetime: OfferLifetime,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OfferLifetime {
    Permanent,
    Timed { duration_ms: u64 },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Purchase {
    pub player: PlayerState,
    pub offer_id: u32,
    pub item_id: u32,
    pub expires_at_unix_ms: u64,
}

#[derive(Debug, Error)]
pub enum CashShopConfigError {
    #[error("failed to read cash-shop configuration {path}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse cash-shop configuration {path}")]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("cash-shop configuration {path} is invalid: {message}")]
    Invalid { path: PathBuf, message: String },
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CashShopPurchaseError {
    #[error("the player does not have enough cash points")]
    InsufficientCashPoints,
    #[error("the cash-shop item expiration exceeds the persisted range")]
    ExpirationOverflow,
    #[error(transparent)]
    Item(#[from] ItemRuleError),
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct CashShopFile {
    currency_name: String,
    offers: Vec<CashShopOfferFile>,
}

impl Default for CashShopFile {
    fn default() -> Self {
        Self {
            currency_name: DEFAULT_CURRENCY_NAME.to_owned(),
            offers: Vec::new(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CashShopOfferFile {
    offer_id: u32,
    item_id: u32,
    price: u64,
    duration: String,
}

impl CashShopCatalog {
    pub fn currency_name(&self) -> &str {
        &self.currency_name
    }

    pub fn load(
        path: &Path,
        definitions: &(impl ItemDefinitionLookup + ?Sized),
    ) -> Result<Self, CashShopConfigError> {
        let source = match fs::read_to_string(path) {
            Ok(source) => source,
            Err(source) if source.kind() == ErrorKind::NotFound => String::new(),
            Err(source) => {
                return Err(CashShopConfigError::Read {
                    path: path.to_owned(),
                    source,
                });
            }
        };
        let file = toml::from_str::<CashShopFile>(&source).map_err(|source| {
            CashShopConfigError::Parse {
                path: path.to_owned(),
                source,
            }
        })?;
        build_catalog(path, definitions, file)
    }

    pub fn offers(&self) -> &[CashShopOffer] {
        &self.offers
    }

    pub fn offer(
        &self,
        offer_id: u32,
    ) -> Option<&CashShopOffer> {
        self.offers.iter().find(|offer| offer.offer_id == offer_id)
    }

    pub fn item_reference_ids(&self) -> impl Iterator<Item = u32> + '_ {
        self.offers.iter().map(|offer| offer.item_id)
    }
}

pub fn purchase(
    mut player: PlayerState,
    offer: &CashShopOffer,
    definitions: &(impl ItemDefinitionLookup + ?Sized),
    purchased_at_unix_ms: u64,
) -> Result<Purchase, CashShopPurchaseError> {
    if player.cash_points < offer.price {
        return Err(CashShopPurchaseError::InsufficientCashPoints);
    }
    let expires_at_unix_ms = expiration_deadline(offer.lifetime, purchased_at_unix_ms)?;
    let inventory = player
        .inventory
        .as_mut()
        .ok_or(ItemRuleError::MissingInventory)?;
    crate::items::apply_item_grant(inventory, definitions, offer.item_id, 1, expires_at_unix_ms)?;
    player.cash_points -= offer.price;
    Ok(Purchase {
        player,
        offer_id: offer.offer_id,
        item_id: offer.item_id,
        expires_at_unix_ms,
    })
}

fn build_catalog(
    path: &Path,
    definitions: &(impl ItemDefinitionLookup + ?Sized),
    file: CashShopFile,
) -> Result<CashShopCatalog, CashShopConfigError> {
    let currency_name = validate_currency_name(path, file.currency_name)?;
    if file.offers.len() > MAXIMUM_OFFERS {
        return invalid(
            path,
            format!("the cash shop has more than {MAXIMUM_OFFERS} offers"),
        );
    }
    let mut offer_ids = HashSet::new();
    let mut offers = Vec::with_capacity(file.offers.len());
    for offer in file.offers {
        if offer.offer_id == 0 {
            return invalid(path, "cash-shop offer IDs must be positive");
        }
        if !offer_ids.insert(offer.offer_id) {
            return invalid(
                path,
                format!("cash-shop offer ID {} is duplicated", offer.offer_id),
            );
        }
        if offer.price == 0 {
            return invalid(
                path,
                format!("cash-shop offer {} has a zero price", offer.offer_id),
            );
        }
        if i64::try_from(offer.price).is_err() {
            return invalid(
                path,
                format!(
                    "cash-shop offer {} price exceeds the persisted balance range",
                    offer.offer_id
                ),
            );
        }
        match definitions.item_definition(offer.item_id) {
            Ok(Some(_)) => {}
            Ok(None) => {
                return invalid(
                    path,
                    format!(
                        "cash-shop item {} is not in the item catalog",
                        offer.item_id
                    ),
                );
            }
            Err(error) => {
                return invalid(
                    path,
                    format!(
                        "cash-shop item {} metadata could not be loaded: {error}",
                        offer.item_id
                    ),
                );
            }
        }
        offers.push(CashShopOffer {
            offer_id: offer.offer_id,
            item_id: offer.item_id,
            price: offer.price,
            lifetime: parse_lifetime(path, offer.offer_id, &offer.duration)?,
        });
    }
    Ok(CashShopCatalog {
        currency_name,
        offers,
    })
}

fn validate_currency_name(
    path: &Path,
    currency_name: String,
) -> Result<String, CashShopConfigError> {
    if currency_name.is_empty() {
        return invalid(path, "cash-shop currency_name must not be empty");
    }
    if currency_name.trim() != currency_name {
        return invalid(
            path,
            "cash-shop currency_name must not have leading or trailing whitespace",
        );
    }
    if currency_name.chars().any(char::is_control) {
        return invalid(
            path,
            "cash-shop currency_name must not contain control characters",
        );
    }
    if currency_name.chars().count() > MAXIMUM_CURRENCY_NAME_CHARACTERS {
        return invalid(
            path,
            format!(
                "cash-shop currency_name must contain at most {MAXIMUM_CURRENCY_NAME_CHARACTERS} \
                 characters"
            ),
        );
    }
    Ok(currency_name)
}

fn parse_lifetime(
    path: &Path,
    offer_id: u32,
    value: &str,
) -> Result<OfferLifetime, CashShopConfigError> {
    if value == "permanent" {
        return Ok(OfferLifetime::Permanent);
    }
    let duration =
        humantime::parse_duration(value).map_err(|error| CashShopConfigError::Invalid {
            path: path.to_owned(),
            message: format!("cash-shop offer {offer_id} duration is invalid: {error}"),
        })?;
    let duration_ms =
        u64::try_from(duration.as_millis()).map_err(|_| CashShopConfigError::Invalid {
            path: path.to_owned(),
            message: format!("cash-shop offer {offer_id} duration exceeds the supported range"),
        })?;
    if duration_ms == 0 {
        return invalid(
            path,
            format!("cash-shop offer {offer_id} duration must be greater than zero"),
        );
    }
    Ok(OfferLifetime::Timed { duration_ms })
}

fn expiration_deadline(
    lifetime: OfferLifetime,
    purchased_at_unix_ms: u64,
) -> Result<u64, CashShopPurchaseError> {
    match lifetime {
        OfferLifetime::Permanent => Ok(0),
        OfferLifetime::Timed { duration_ms } => purchased_at_unix_ms
            .checked_add(duration_ms)
            .filter(|deadline| i64::try_from(*deadline).is_ok())
            .ok_or(CashShopPurchaseError::ExpirationOverflow),
    }
}

fn invalid<T>(
    path: &Path,
    message: impl Into<String>,
) -> Result<T, CashShopConfigError> {
    Err(CashShopConfigError::Invalid {
        path: path.to_owned(),
        message: message.into(),
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use oozems_proto::v1::InventoryState;
    use oozems_proto::v1::ItemDefinition;
    use oozems_proto::v1::PlayerState;

    use super::CashShopCatalog;
    use super::CashShopConfigError;
    use super::CashShopFile;
    use super::CashShopOffer;
    use super::CashShopOfferFile;
    use super::CashShopPurchaseError;
    use super::OfferLifetime;
    use super::build_catalog;
    use super::expiration_deadline;
    use super::purchase;

    const ITEM_ID: u32 = 5_010_000;

    #[test]
    fn missing_configuration_file_loads_an_empty_catalog() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let catalog = CashShopCatalog::load(
            &directory.path().join("missing-cash-shop.toml"),
            &vec![definition()],
        )
        .expect("missing catalog");

        assert!(catalog.offers().is_empty());
        assert_eq!(catalog.currency_name(), "Ooze");
    }

    #[test]
    fn file_loader_uses_the_configured_currency_name() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("cash-shop.toml");
        fs::write(&path, "currency_name = \"Slime Tokens\"\n").expect("write cash-shop fixture");

        let catalog = CashShopCatalog::load(&path, &vec![definition()]).expect("valid catalog");

        assert_eq!(catalog.currency_name(), "Slime Tokens");
    }

    #[test]
    fn file_loader_rejects_unknown_fields_and_invalid_offer_values() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("cash-shop.toml");
        for source in [
            "currency_name = \"\"\n",
            "currency_name = \" Ooze\"\n",
            "currency_name = \"This premium currency name is too long\"\n",
            "[[offers]]\noffer_id = 1\nitem_id = 5010000\nprice = 100\nduration = \"1d\"\nextra = \
             true\n",
            "[[offers]]\noffer_id = 0\nitem_id = 5010000\nprice = 100\nduration = \"1d\"\n",
            "[[offers]]\noffer_id = 1\nitem_id = 5010000\nprice = 0\nduration = \"1d\"\n",
            "[[offers]]\noffer_id = 1\nitem_id = 9999999\nprice = 100\nduration = \"1d\"\n",
            "[[offers]]\noffer_id = 1\nitem_id = 5010000\nprice = 100\nduration = \"0s\"\n",
        ] {
            fs::write(&path, source).expect("write cash-shop fixture");
            assert!(CashShopCatalog::load(&path, &vec![definition()]).is_err());
        }
    }

    #[test]
    fn catalog_preserves_order_and_parses_offer_lifetimes() {
        let definitions = vec![definition()];
        let catalog = build_catalog(
            Path::new("cash-shop.toml"),
            &definitions,
            CashShopFile {
                offers: vec![
                    offer_file(1, "30d"),
                    CashShopOfferFile {
                        offer_id: 2,
                        duration: "permanent".to_owned(),
                        ..offer_file(2, "1d")
                    },
                ],
                ..CashShopFile::default()
            },
        )
        .expect("valid catalog");

        assert_eq!(catalog.offers()[0].offer_id, 1);
        assert_eq!(
            catalog.offers()[0].lifetime,
            OfferLifetime::Timed {
                duration_ms: 30 * 24 * 60 * 60 * 1_000,
            }
        );
        assert_eq!(catalog.offers()[1].lifetime, OfferLifetime::Permanent);
    }

    #[test]
    fn catalog_rejects_duplicate_ids_and_more_than_ten_offers() {
        let definitions = vec![definition()];
        let duplicate = build_catalog(
            Path::new("cash-shop.toml"),
            &definitions,
            CashShopFile {
                offers: vec![offer_file(1, "1d"), offer_file(1, "2d")],
                ..CashShopFile::default()
            },
        );
        assert!(matches!(
            duplicate,
            Err(CashShopConfigError::Invalid { .. })
        ));

        let oversized = build_catalog(
            Path::new("cash-shop.toml"),
            &definitions,
            CashShopFile {
                offers: (1..=11)
                    .map(|offer_id| offer_file(offer_id, "1d"))
                    .collect(),
                ..CashShopFile::default()
            },
        );
        assert!(matches!(
            oversized,
            Err(CashShopConfigError::Invalid { .. })
        ));
    }

    #[test]
    fn timed_purchase_debits_points_and_persists_exact_deadline() {
        let result = purchase(
            player(2_000),
            &CashShopOffer {
                offer_id: 7,
                item_id: ITEM_ID,
                price: 1_200,
                lifetime: OfferLifetime::Timed { duration_ms: 500 },
            },
            &vec![definition()],
            10_000,
        )
        .expect("purchase timed item");

        assert_eq!(result.player.cash_points, 800);
        assert_eq!(result.offer_id, 7);
        assert_eq!(result.item_id, ITEM_ID);
        assert_eq!(result.expires_at_unix_ms, 10_500);
        assert_eq!(
            result.player.inventory.as_ref().expect("inventory").stacks[0].expires_at_unix_ms,
            10_500
        );
    }

    #[test]
    fn purchase_rejects_insufficient_points_before_granting() {
        let result = purchase(
            player(1_199),
            &CashShopOffer {
                offer_id: 1,
                item_id: ITEM_ID,
                price: 1_200,
                lifetime: OfferLifetime::Permanent,
            },
            &vec![definition()],
            10_000,
        );

        assert_eq!(result, Err(CashShopPurchaseError::InsufficientCashPoints));
    }

    #[test]
    fn timed_deadline_must_fit_the_persisted_integer_range() {
        assert_eq!(
            expiration_deadline(OfferLifetime::Timed { duration_ms: 1 }, i64::MAX as u64,),
            Err(CashShopPurchaseError::ExpirationOverflow)
        );
    }

    fn offer_file(
        offer_id: u32,
        duration: &str,
    ) -> CashShopOfferFile {
        CashShopOfferFile {
            offer_id,
            item_id: ITEM_ID,
            price: 100,
            duration: duration.to_owned(),
        }
    }

    fn definition() -> ItemDefinition {
        ItemDefinition {
            item_id: ITEM_ID,
            name: "Cash item".to_owned(),
            stack_max: 1,
            ..ItemDefinition::default()
        }
    }

    fn player(cash_points: u64) -> PlayerState {
        PlayerState {
            cash_points,
            inventory: Some(InventoryState {
                capacity: 4,
                ..InventoryState::default()
            }),
            ..PlayerState::default()
        }
    }
}
