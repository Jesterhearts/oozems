use std::collections::HashMap;
use std::fs;
use std::io::ErrorKind;
use std::path::Path;
use std::path::PathBuf;

use serde::Deserialize;
use thiserror::Error;

use crate::content::ContentCatalog;
use crate::content::ContentError;

const MAXIMUM_SHOP_OFFERS: usize = 5;
const MAXIMUM_TAXI_DESTINATIONS: usize = 4;

#[derive(Clone, Debug)]
pub struct InteractionCatalog {
    shops: HashMap<InteractionKey, ShopDefinition>,
    taxis: HashMap<InteractionKey, TaxiDefinition>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct InteractionKey {
    map_id: u32,
    npc_spawn_id: u32,
}

#[derive(Clone, Debug)]
pub struct ShopDefinition {
    pub offers: Vec<ShopOffer>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShopOffer {
    pub item_id: u32,
    pub buy_price: u64,
}

#[derive(Clone, Debug)]
pub struct TaxiDefinition {
    pub destinations: Vec<TaxiDestination>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaxiDestination {
    pub map_id: u32,
    pub portal_name: String,
    pub label: String,
    pub fare: u64,
}

#[derive(Debug, Error)]
pub enum InteractionConfigError {
    #[error("failed to read NPC interaction configuration {path}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse NPC interaction configuration {path}")]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("NPC interaction configuration {path} is invalid: {message}")]
    Invalid { path: PathBuf, message: String },
    #[error(transparent)]
    Content(#[from] ContentError),
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct InteractionFile {
    shops: Vec<ShopFile>,
    taxis: Vec<TaxiFile>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ShopFile {
    map_id: u32,
    npc_spawn_id: u32,
    offers: Vec<ShopOfferFile>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ShopOfferFile {
    item_id: u32,
    buy_price: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TaxiFile {
    map_id: u32,
    npc_spawn_id: u32,
    destinations: Vec<TaxiDestinationFile>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TaxiDestinationFile {
    map_id: u32,
    portal_name: String,
    label: String,
    fare: u64,
}

impl InteractionCatalog {
    pub fn load(
        path: &Path,
        content: &ContentCatalog,
    ) -> Result<Self, InteractionConfigError> {
        let source = match fs::read_to_string(path) {
            Ok(source) => source,
            Err(source) if source.kind() == ErrorKind::NotFound => String::new(),
            Err(source) => {
                return Err(InteractionConfigError::Read {
                    path: path.to_owned(),
                    source,
                });
            }
        };
        let file = toml::from_str::<InteractionFile>(&source).map_err(|source| {
            InteractionConfigError::Parse {
                path: path.to_owned(),
                source,
            }
        })?;
        build_catalog(path, content, file)
    }

    pub fn shop(
        &self,
        map_id: u32,
        npc_spawn_id: u32,
    ) -> Option<&ShopDefinition> {
        self.shops.get(&InteractionKey {
            map_id,
            npc_spawn_id,
        })
    }

    pub fn taxi(
        &self,
        map_id: u32,
        npc_spawn_id: u32,
    ) -> Option<&TaxiDefinition> {
        self.taxis.get(&InteractionKey {
            map_id,
            npc_spawn_id,
        })
    }

    pub fn item_reference_ids(&self) -> impl Iterator<Item = u32> + '_ {
        self.shops
            .values()
            .flat_map(|shop| shop.offers.iter().map(|offer| offer.item_id))
    }
}

fn build_catalog(
    path: &Path,
    content: &ContentCatalog,
    file: InteractionFile,
) -> Result<InteractionCatalog, InteractionConfigError> {
    let mut shops = HashMap::new();
    for shop in file.shops {
        let key = InteractionKey {
            map_id: shop.map_id,
            npc_spawn_id: shop.npc_spawn_id,
        };
        let map = configured_map(path, content, key)?;
        configured_npc(path, &map, key)?;
        if shop.offers.is_empty() {
            return invalid(path, format!("shop on map {} has no offers", shop.map_id));
        }
        if shop.offers.len() > MAXIMUM_SHOP_OFFERS {
            return invalid(
                path,
                format!(
                    "shop on map {} has more than {MAXIMUM_SHOP_OFFERS} offers",
                    shop.map_id
                ),
            );
        }
        let mut offers = Vec::with_capacity(shop.offers.len());
        for offer in shop.offers {
            if offer.buy_price == 0 {
                return invalid(
                    path,
                    format!("shop item {} has a zero buy price", offer.item_id),
                );
            }
            let definition = content.item_definition(offer.item_id)?.ok_or_else(|| {
                InteractionConfigError::Invalid {
                    path: path.to_owned(),
                    message: format!("shop item {} is not in the item catalog", offer.item_id),
                }
            })?;
            if definition.sale_price > offer.buy_price {
                return invalid(
                    path,
                    format!(
                        "shop item {} buys for less than its sale price",
                        offer.item_id
                    ),
                );
            }
            if offers
                .iter()
                .any(|existing: &ShopOffer| existing.item_id == offer.item_id)
            {
                return invalid(path, format!("shop item {} is duplicated", offer.item_id));
            }
            offers.push(ShopOffer {
                item_id: offer.item_id,
                buy_price: offer.buy_price,
            });
        }
        if shops.insert(key, ShopDefinition { offers }).is_some() {
            return invalid(
                path,
                format!(
                    "map {} NPC spawn {} has more than one shop",
                    key.map_id, key.npc_spawn_id
                ),
            );
        }
    }

    let mut taxis = HashMap::new();
    for taxi in file.taxis {
        let key = InteractionKey {
            map_id: taxi.map_id,
            npc_spawn_id: taxi.npc_spawn_id,
        };
        let map = configured_map(path, content, key)?;
        configured_npc(path, &map, key)?;
        if shops.contains_key(&key) {
            return invalid(path, "one NPC spawn cannot be both a shop and a taxi");
        }
        if taxi.destinations.is_empty() {
            return invalid(
                path,
                format!("taxi on map {} has no destinations", taxi.map_id),
            );
        }
        if taxi.destinations.len() > MAXIMUM_TAXI_DESTINATIONS {
            return invalid(
                path,
                format!(
                    "taxi on map {} has more than {MAXIMUM_TAXI_DESTINATIONS} destinations",
                    taxi.map_id
                ),
            );
        }
        let mut destinations = Vec::with_capacity(taxi.destinations.len());
        for destination in taxi.destinations {
            if destination.label.trim().is_empty() || destination.portal_name.is_empty() {
                return invalid(path, "taxi destinations require labels and portal names");
            }
            if destinations
                .iter()
                .any(|existing: &TaxiDestination| existing.map_id == destination.map_id)
            {
                return invalid(
                    path,
                    format!("taxi destination map {} is duplicated", destination.map_id),
                );
            }
            let target = content.get_map(destination.map_id)?.ok_or_else(|| {
                InteractionConfigError::Invalid {
                    path: path.to_owned(),
                    message: format!("taxi destination map {} does not exist", destination.map_id),
                }
            })?;
            if !target
                .portals
                .iter()
                .any(|portal| portal.name == destination.portal_name)
            {
                return invalid(
                    path,
                    format!(
                        "taxi destination map {} has no portal {:?}",
                        destination.map_id, destination.portal_name
                    ),
                );
            }
            destinations.push(TaxiDestination {
                map_id: destination.map_id,
                portal_name: destination.portal_name,
                label: destination.label,
                fare: destination.fare,
            });
        }
        if taxis.insert(key, TaxiDefinition { destinations }).is_some() {
            return invalid(
                path,
                format!(
                    "map {} NPC spawn {} has more than one taxi definition",
                    key.map_id, key.npc_spawn_id
                ),
            );
        }
    }

    Ok(InteractionCatalog { shops, taxis })
}

fn configured_map(
    path: &Path,
    content: &ContentCatalog,
    key: InteractionKey,
) -> Result<oozems_proto::v1::Map, InteractionConfigError> {
    content
        .get_map(key.map_id)?
        .ok_or_else(|| InteractionConfigError::Invalid {
            path: path.to_owned(),
            message: format!("interaction map {} does not exist", key.map_id),
        })
}

fn configured_npc(
    path: &Path,
    map: &oozems_proto::v1::Map,
    key: InteractionKey,
) -> Result<(), InteractionConfigError> {
    if map.npcs.iter().any(|npc| npc.spawn_id == key.npc_spawn_id) {
        Ok(())
    } else {
        invalid(
            path,
            format!("map {} has no NPC spawn {}", key.map_id, key.npc_spawn_id),
        )
    }
}

fn invalid<T>(
    path: &Path,
    message: impl Into<String>,
) -> Result<T, InteractionConfigError> {
    Err(InteractionConfigError::Invalid {
        path: path.to_owned(),
        message: message.into(),
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use oozems_proto::v1::ItemCategory;

    use super::InteractionCatalog;
    use crate::content::ContentCatalog;
    use crate::content::ContentConfig;

    #[test]
    fn bundled_interactions_reference_available_wz_content() {
        let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let wz_dir = manifest_dir.join("../../data");
        if !["Map.wz", "Npc.wz", "Character.wz"]
            .iter()
            .all(|name| wz_dir.join(name).exists())
        {
            return;
        }
        let content = ContentCatalog::load(
            &wz_dir,
            &ContentConfig::load(&manifest_dir.join("../../config/content.toml"))
                .expect("content configuration"),
        )
        .expect("content catalog");

        let interactions = InteractionCatalog::load(
            &manifest_dir.join("../../config/interactions.toml"),
            &content,
        )
        .expect("interaction catalog");

        assert_eq!(
            interactions
                .shop(100_000_101, 1)
                .expect("Sam's shop")
                .offers
                .len(),
            5
        );
        assert_eq!(
            interactions
                .taxi(100_000_000, 2)
                .expect("Henesys taxi")
                .destinations[0]
                .map_id,
            104_000_000
        );
    }

    #[test]
    fn shops_accept_indexed_non_eager_items_and_reject_unknown_items() {
        let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let wz_dir = manifest_dir.join("../../data");
        if !["Map.wz", "Npc.wz", "Character.wz", "Item.wz"]
            .iter()
            .all(|name| wz_dir.join(name).exists())
        {
            return;
        }
        let content = ContentCatalog::load(
            &wz_dir,
            &ContentConfig::load(&manifest_dir.join("../../config/content.toml"))
                .expect("content configuration"),
        )
        .expect("content catalog");
        let item_id = content
            .indexed_item_ids()
            .find(|item_id| {
                !content
                    .item_definition_slice()
                    .iter()
                    .any(|definition| definition.item_id == *item_id)
            })
            .expect("item source index should contain a non-eager item");
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("interactions.toml");
        fs::write(
            &path,
            format!(
                "[[shops]]\nmap_id = 100000101\nnpc_spawn_id = 1\n[[shops.offers]]\nitem_id = \
                 {item_id}\nbuy_price = 9223372036854775807\n"
            ),
        )
        .expect("write interaction configuration");

        let interactions =
            InteractionCatalog::load(&path, &content).expect("indexed shop item should load");

        assert_eq!(
            interactions
                .shop(100_000_101, 1)
                .expect("configured shop")
                .offers[0]
                .item_id,
            item_id
        );
        let definition = content
            .item_definition(item_id)
            .expect("item metadata lookup")
            .expect("indexed item definition");
        assert!(!definition.name.is_empty());
        assert!(definition.stack_max > 0);
        assert!(ItemCategory::try_from(definition.category).is_ok());
        assert!(
            !content
                .item_definition_slice()
                .iter()
                .any(|definition| definition.item_id == item_id)
        );

        fs::write(
            &path,
            "[[shops]]\nmap_id = 100000101\nnpc_spawn_id = 1\n[[shops.offers]]\nitem_id = \
             4294967295\nbuy_price = 1\n",
        )
        .expect("write unknown shop item");
        let error = InteractionCatalog::load(&path, &content)
            .expect_err("unknown shop item should be rejected");
        assert!(
            error
                .to_string()
                .contains("shop item 4294967295 is not in the item catalog")
        );
    }
}
