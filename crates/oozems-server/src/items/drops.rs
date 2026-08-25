use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Mutex;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use oozems_proto::v1::DroppedItem;
use oozems_proto::v1::PlayerState;
use oozems_proto::v1::Vec2;
use thiserror::Error;

use super::inventory::ItemDefinitionLookup;
use super::inventory::ItemRuleError;
use super::inventory::RemovedItem;
use super::inventory::apply_item_grant;
use super::inventory::canonicalize_inventory;

pub const PICK_UP_RADIUS: f32 = 80.0;

pub struct DropStore {
    drops: Mutex<DropIndex>,
    lifespan: Duration,
    next_id: AtomicU64,
}

#[derive(Default)]
struct DropIndex {
    maps: HashMap<u32, MapDropIndex>,
    drop_maps: HashMap<String, u32>,
    expirations: BTreeMap<u64, HashSet<String>>,
}

#[derive(Default)]
struct MapDropIndex {
    drops: HashMap<String, MapDrop>,
    order: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
struct MapDrop {
    item: DroppedItem,
    owner_player_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StagedDropGrant {
    map_id: u32,
    item: DroppedItem,
    owner_player_id: Option<String>,
}

impl StagedDropGrant {
    pub fn item(&self) -> &DroppedItem {
        &self.item
    }
}

#[derive(Clone, Debug)]
pub struct PickedUpItem {
    pub drop: DroppedItem,
    pub owner_player_id: Option<String>,
    pub player: PlayerState,
}

#[derive(Debug, Error)]
pub enum DropStoreError {
    #[error("system time is before the Unix epoch")]
    InvalidSystemTime,
    #[error("drop expiry is outside the supported time range")]
    ExpiryOverflow,
    #[error("the dropped-item store lock was poisoned")]
    Lock,
    #[error("dropped items changed during a player transaction")]
    Conflict,
}

#[derive(Debug, Error)]
pub enum PickUpError {
    #[error(transparent)]
    Rule(#[from] ItemRuleError),
    #[error(transparent)]
    Store(#[from] DropStoreError),
}

impl DropStore {
    pub fn new(lifespan: Duration) -> Self {
        Self {
            drops: Mutex::new(DropIndex::default()),
            lifespan,
            next_id: AtomicU64::new(0),
        }
    }
}

#[cfg(test)]
pub fn create_drop(
    store: &DropStore,
    removed: &RemovedItem,
) -> Result<DroppedItem, DropStoreError> {
    let now_ms = unix_time_ms()?;
    create_drop_at(store, removed, now_ms)
}

pub fn stage_inventory_drop(
    store: &DropStore,
    removed: &RemovedItem,
) -> Result<StagedDropGrant, DropStoreError> {
    let now_ms = unix_time_ms()?;
    let despawn_at_unix_ms = drop_expiry(store, now_ms)?;
    Ok(StagedDropGrant {
        map_id: removed.map_id,
        item: new_drop(
            store,
            removed.item_id,
            removed.quantity,
            removed.expires_at_unix_ms,
            removed.position,
            now_ms,
            despawn_at_unix_ms,
        ),
        owner_player_id: None,
    })
}

#[cfg(test)]
fn create_drop_at(
    store: &DropStore,
    removed: &RemovedItem,
    now_ms: u64,
) -> Result<DroppedItem, DropStoreError> {
    let despawn_at_unix_ms = drop_expiry(store, now_ms)?;
    let item = new_drop(
        store,
        removed.item_id,
        removed.quantity,
        removed.expires_at_unix_ms,
        removed.position,
        now_ms,
        despawn_at_unix_ms,
    );
    let mut drops = store.drops.lock().map_err(|_| DropStoreError::Lock)?;
    retain_active_drops(&mut drops, now_ms);
    insert_drop(&mut drops, removed.map_id, item.clone(), None);
    Ok(item)
}

#[cfg(test)]
pub fn create_mob_drops(
    store: &DropStore,
    map_id: u32,
    position: Vec2,
    item_ids: &[u32],
    owner_player_id: &str,
) -> Result<Vec<DroppedItem>, DropStoreError> {
    let staged = stage_mob_drops(store, map_id, position, item_ids, owner_player_id)?;
    let items = staged.iter().map(|grant| grant.item.clone()).collect();
    commit_staged_drops(store, &staged)?;
    Ok(items)
}

pub fn stage_mob_drops(
    store: &DropStore,
    map_id: u32,
    position: Vec2,
    item_ids: &[u32],
    owner_player_id: &str,
) -> Result<Vec<StagedDropGrant>, DropStoreError> {
    if item_ids.is_empty() {
        return Ok(Vec::new());
    }
    stage_mob_drops_at(
        store,
        map_id,
        position,
        item_ids,
        owner_player_id,
        unix_time_ms()?,
    )
}

fn stage_mob_drops_at(
    store: &DropStore,
    map_id: u32,
    position: Vec2,
    item_ids: &[u32],
    owner_player_id: &str,
    now_ms: u64,
) -> Result<Vec<StagedDropGrant>, DropStoreError> {
    let despawn_at_unix_ms = drop_expiry(store, now_ms)?;
    let owner_player_id = owner_player_id.to_owned();
    Ok(item_ids
        .iter()
        .enumerate()
        .map(|(index, item_id)| StagedDropGrant {
            map_id,
            item: new_drop(
                store,
                *item_id,
                1,
                0,
                spread_position(position, index, item_ids.len()),
                now_ms,
                despawn_at_unix_ms,
            ),
            owner_player_id: Some(owner_player_id.clone()),
        })
        .collect())
}

#[cfg(test)]
pub fn commit_staged_drops(
    store: &DropStore,
    staged: &[StagedDropGrant],
) -> Result<(), DropStoreError> {
    if staged.is_empty() {
        return Ok(());
    }
    let mut drops = store.drops.lock().map_err(|_| DropStoreError::Lock)?;
    if staged.iter().any(|grant| {
        drops
            .drop_maps
            .get(&grant.item.id)
            .is_some_and(|map_id| *map_id != grant.map_id)
            || drops
                .maps
                .get(&grant.map_id)
                .and_then(|map| map.drops.get(&grant.item.id))
                .is_some_and(|drop| {
                    drop.item != grant.item || drop.owner_player_id != grant.owner_player_id
                })
    }) {
        return Err(DropStoreError::Conflict);
    }
    for grant in staged {
        insert_drop(
            &mut drops,
            grant.map_id,
            grant.item.clone(),
            grant.owner_player_id.clone(),
        );
    }
    Ok(())
}

pub fn commit_new_staged_drops(
    store: &DropStore,
    staged: &[StagedDropGrant],
) -> Result<(), DropStoreError> {
    if staged.is_empty() {
        return Ok(());
    }
    let mut drops = store.drops.lock().map_err(|_| DropStoreError::Lock)?;
    let unique_ids = staged
        .iter()
        .map(|grant| grant.item.id.as_str())
        .collect::<HashSet<_>>();
    if unique_ids.len() != staged.len()
        || staged
            .iter()
            .any(|grant| drops.drop_maps.contains_key(&grant.item.id))
    {
        return Err(DropStoreError::Conflict);
    }
    for grant in staged {
        let inserted = insert_drop(
            &mut drops,
            grant.map_id,
            grant.item.clone(),
            grant.owner_player_id.clone(),
        );
        debug_assert!(inserted);
    }
    Ok(())
}

pub fn rollback_staged_drops(
    store: &DropStore,
    staged: &[StagedDropGrant],
) -> Result<(), DropStoreError> {
    if staged.is_empty() {
        return Ok(());
    }
    let mut drops = store.drops.lock().map_err(|_| DropStoreError::Lock)?;
    let all_match = staged.iter().all(|grant| {
        let current = drops
            .maps
            .get(&grant.map_id)
            .and_then(|map| map.drops.get(&grant.item.id));
        let expected = MapDrop {
            item: grant.item.clone(),
            owner_player_id: grant.owner_player_id.clone(),
        };
        current == Some(&expected)
    });
    if !all_match {
        return Err(DropStoreError::Conflict);
    }
    for grant in staged.iter().rev() {
        remove_drop(&mut drops, grant.map_id, &grant.item.id)
            .expect("the checked staged drop remains indexed");
    }
    Ok(())
}

pub fn map_drops(
    store: &DropStore,
    map_id: u32,
) -> Result<Vec<DroppedItem>, DropStoreError> {
    let now_ms = unix_time_ms()?;
    map_drops_at(store, map_id, now_ms)
}

pub fn pick_up_nearest(
    store: &DropStore,
    mut player: PlayerState,
    position: Vec2,
    definitions: &(impl ItemDefinitionLookup + ?Sized),
) -> Result<PickedUpItem, PickUpError> {
    if !position.x.is_finite() || !position.y.is_finite() {
        return Err(ItemRuleError::MissingPosition.into());
    }
    let player_id = player.id.clone();
    let now_ms = unix_time_ms()?;
    let mut drops = store.drops.lock().map_err(|_| DropStoreError::Lock)?;
    retain_active_drops(&mut drops, now_ms);
    let radius_squared = PICK_UP_RADIUS * PICK_UP_RADIUS;
    let drop_id = drops
        .maps
        .get(&player.map_id)
        .and_then(|map_drops| {
            map_drops
                .order
                .iter()
                .filter_map(|drop_id| {
                    let drop = map_drops.drops.get(drop_id)?;
                    if drop
                        .owner_player_id
                        .as_deref()
                        .is_some_and(|owner| owner != player_id)
                    {
                        return None;
                    }
                    let drop_position = drop.item.position.as_ref()?;
                    let dx = drop_position.x - position.x;
                    let dy = drop_position.y - position.y;
                    let distance_squared = dx * dx + dy * dy;
                    (distance_squared <= radius_squared).then_some((drop_id, distance_squared))
                })
                .min_by(|(_, left), (_, right)| left.total_cmp(right))
                .map(|(drop_id, _)| drop_id.clone())
        })
        .ok_or(ItemRuleError::NoNearbyDrop)?;
    let selected = drops
        .maps
        .get(&player.map_id)
        .and_then(|map_drops| map_drops.drops.get(&drop_id))
        .cloned()
        .expect("the selected drop is indexed on its map");
    let item_id = selected.item.item_id;
    let quantity = selected.item.quantity;
    if quantity == 0 {
        return Err(ItemRuleError::InvalidQuantity { item_id }.into());
    }
    if let Some(card) = definitions.monster_book_card(item_id) {
        debug_assert_eq!(card.item_id, item_id);
        debug_assert_eq!(card.max_count, crate::monster_book::MAX_CARD_COUNT);
        crate::monster_book::add_card(&mut player.monster_book_cards, item_id);
    } else {
        let inventory = player
            .inventory
            .as_mut()
            .ok_or(ItemRuleError::MissingInventory)?;
        canonicalize_inventory(inventory, definitions)?;
        apply_item_grant(
            inventory,
            definitions,
            item_id,
            u64::from(quantity),
            selected.item.expires_at_unix_ms,
        )?;
    }
    let removed = remove_drop(&mut drops, player.map_id, &drop_id)
        .expect("the selected drop is indexed on its map");
    let drop = removed.item;
    player.position = Some(position);
    Ok(PickedUpItem {
        drop,
        owner_player_id: removed.owner_player_id,
        player,
    })
}

#[cfg(test)]
pub fn restore_drop(
    store: &DropStore,
    map_id: u32,
    item: DroppedItem,
    owner_player_id: Option<String>,
) -> Result<(), DropStoreError> {
    let mut drops = store.drops.lock().map_err(|_| DropStoreError::Lock)?;
    insert_drop(&mut drops, map_id, item, owner_player_id);
    Ok(())
}

pub fn restore_picked_up_drop(
    store: &DropStore,
    map_id: u32,
    item: DroppedItem,
    owner_player_id: Option<String>,
) -> Result<(), DropStoreError> {
    let mut drops = store.drops.lock().map_err(|_| DropStoreError::Lock)?;
    if drops.drop_maps.contains_key(&item.id) {
        return Err(DropStoreError::Conflict);
    }
    insert_drop(&mut drops, map_id, item, owner_player_id);
    Ok(())
}

fn map_drops_at(
    store: &DropStore,
    map_id: u32,
    now_ms: u64,
) -> Result<Vec<DroppedItem>, DropStoreError> {
    let mut drops = store.drops.lock().map_err(|_| DropStoreError::Lock)?;
    retain_active_drops(&mut drops, now_ms);
    let Some(map_drops) = drops.maps.get(&map_id) else {
        return Ok(Vec::new());
    };
    Ok(map_drops
        .order
        .iter()
        .filter_map(|drop_id| map_drops.drops.get(drop_id))
        .map(|drop| drop.item.clone())
        .collect())
}

fn drop_expiry(
    store: &DropStore,
    now_ms: u64,
) -> Result<u64, DropStoreError> {
    let lifespan_ms =
        u64::try_from(store.lifespan.as_millis()).map_err(|_| DropStoreError::ExpiryOverflow)?;
    now_ms
        .checked_add(lifespan_ms)
        .ok_or(DropStoreError::ExpiryOverflow)
}

fn new_drop(
    store: &DropStore,
    item_id: u32,
    quantity: u32,
    expires_at_unix_ms: u64,
    position: Vec2,
    now_ms: u64,
    despawn_at_unix_ms: u64,
) -> DroppedItem {
    let sequence = store.next_id.fetch_add(1, Ordering::Relaxed);
    DroppedItem {
        id: format!("drop-{now_ms:x}-{sequence:x}"),
        item_id,
        position: Some(position),
        despawn_at_unix_ms,
        quantity,
        expires_at_unix_ms,
    }
}

fn spread_position(
    center: Vec2,
    index: usize,
    count: usize,
) -> Vec2 {
    const SPACING: f32 = 28.0;

    let center_index = count.saturating_sub(1) as f32 / 2.0;
    Vec2 {
        x: center.x + (index as f32 - center_index) * SPACING,
        y: center.y,
    }
}

fn insert_drop(
    drops: &mut DropIndex,
    map_id: u32,
    item: DroppedItem,
    owner_player_id: Option<String>,
) -> bool {
    if drops.drop_maps.contains_key(&item.id) {
        return false;
    }
    let drop_id = item.id.clone();
    let deadline = drop_deadline(&item);
    let map_drops = drops.maps.entry(map_id).or_default();
    map_drops.order.push(drop_id.clone());
    let previous = map_drops.drops.insert(
        drop_id.clone(),
        MapDrop {
            item,
            owner_player_id,
        },
    );
    debug_assert!(previous.is_none());
    drops.drop_maps.insert(drop_id.clone(), map_id);
    drops
        .expirations
        .entry(deadline)
        .or_default()
        .insert(drop_id);
    true
}

fn remove_drop(
    drops: &mut DropIndex,
    map_id: u32,
    drop_id: &str,
) -> Option<MapDrop> {
    if drops.drop_maps.get(drop_id) != Some(&map_id) {
        return None;
    }
    drops.drop_maps.remove(drop_id);
    let map_drops = drops
        .maps
        .get_mut(&map_id)
        .expect("a globally indexed drop has a map index");
    let removed = map_drops
        .drops
        .remove(drop_id)
        .expect("a globally indexed drop exists on its map");
    let map_is_empty = map_drops.drops.is_empty();
    if !map_is_empty && map_drops.order.len() > map_drops.drops.len().saturating_mul(2) + 16 {
        map_drops
            .order
            .retain(|candidate| map_drops.drops.contains_key(candidate));
    }
    if map_is_empty {
        drops.maps.remove(&map_id);
    }
    let deadline = drop_deadline(&removed.item);
    let expiration_is_empty = drops
        .expirations
        .get_mut(&deadline)
        .is_some_and(|drop_ids| {
            drop_ids.remove(drop_id);
            drop_ids.is_empty()
        });
    if expiration_is_empty {
        drops.expirations.remove(&deadline);
    }
    Some(removed)
}

fn retain_active_drops(
    drops: &mut DropIndex,
    now_ms: u64,
) {
    let expired_drop_ids = drops
        .expirations
        .range(..=now_ms)
        .flat_map(|(_, drop_ids)| drop_ids.iter().cloned())
        .collect::<Vec<_>>();
    for drop_id in expired_drop_ids {
        if let Some(map_id) = drops.drop_maps.get(&drop_id).copied() {
            remove_drop(drops, map_id, &drop_id)
                .expect("an expiration entry refers to an indexed drop");
        }
    }
}

fn drop_deadline(item: &DroppedItem) -> u64 {
    if item.expires_at_unix_ms == 0 {
        item.despawn_at_unix_ms
    } else {
        item.despawn_at_unix_ms.min(item.expires_at_unix_ms)
    }
}

fn unix_time_ms() -> Result<u64, DropStoreError> {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| DropStoreError::InvalidSystemTime)?;
    u64::try_from(elapsed.as_millis()).map_err(|_| DropStoreError::ExpiryOverflow)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use std::sync::Barrier;
    use std::time::Duration;

    use oozems_proto::v1::DroppedItem;
    use oozems_proto::v1::EquipmentSlot;
    use oozems_proto::v1::InventoryItemStack;
    use oozems_proto::v1::InventoryState;
    use oozems_proto::v1::ItemDefinition;
    use oozems_proto::v1::PlayerState;
    use oozems_proto::v1::Vec2;

    use super::DropStore;
    use super::DropStoreError;
    use super::PickUpError;
    use super::StagedDropGrant;
    use super::commit_new_staged_drops;
    use super::create_drop;
    use super::create_drop_at;
    use super::create_mob_drops;
    use super::map_drops;
    use super::map_drops_at;
    use super::pick_up_nearest;
    use super::restore_drop;
    use crate::items::ItemDefinitionLookup;
    use crate::items::ItemRuleError;
    use crate::items::SPARE_BOTTOM_ID;
    use crate::items::SPARE_SHOES_ID;
    use crate::items::SPARE_TOP_ID;
    use crate::items::STARTER_TOP_ID;
    use crate::items::remove_inventory_item;
    use crate::items::starter_inventory;

    const STACKABLE_ITEM_ID: u32 = 2_000_000;
    const CARD_ITEM_ID: u32 = 2_380_000;

    fn staged_drop(
        map_id: u32,
        id: &str,
    ) -> StagedDropGrant {
        StagedDropGrant {
            map_id,
            item: DroppedItem {
                id: id.to_owned(),
                item_id: STACKABLE_ITEM_ID,
                position: Some(Vec2 { x: 10.0, y: 20.0 }),
                despawn_at_unix_ms: u64::MAX,
                quantity: 1,
                expires_at_unix_ms: 0,
            },
            owner_player_id: None,
        }
    }

    struct TestCatalog {
        definitions: Vec<ItemDefinition>,
        cards: BTreeMap<u32, crate::content::MonsterBookCardDefinition>,
    }

    impl ItemDefinitionLookup for TestCatalog {
        fn item_definition(
            &self,
            item_id: u32,
        ) -> Result<Option<&ItemDefinition>, ItemRuleError> {
            Ok(self
                .definitions
                .iter()
                .find(|definition| definition.item_id == item_id))
        }

        fn monster_book_card(
            &self,
            item_id: u32,
        ) -> Option<crate::content::MonsterBookCardDefinition> {
            self.cards.get(&item_id).copied()
        }
    }

    #[test]
    fn one_drop_batch_rejects_duplicate_ids_atomically() {
        let store = DropStore::new(Duration::from_secs(60));
        let grant = staged_drop(1, "duplicate");

        assert!(matches!(
            commit_new_staged_drops(&store, &[grant.clone(), grant]),
            Err(DropStoreError::Conflict)
        ));
        assert!(map_drops(&store, 1).expect("map drops").is_empty());
    }

    #[test]
    fn one_drop_batch_rejects_duplicate_ids_across_maps_atomically() {
        let store = DropStore::new(Duration::from_secs(60));

        assert!(matches!(
            commit_new_staged_drops(
                &store,
                &[staged_drop(1, "duplicate"), staged_drop(2, "duplicate")],
            ),
            Err(DropStoreError::Conflict)
        ));
        assert!(map_drops(&store, 1).expect("first map drops").is_empty());
        assert!(map_drops(&store, 2).expect("second map drops").is_empty());
    }

    #[test]
    fn dropped_items_are_scoped_to_their_map_and_have_a_quantity() {
        let definitions = equipment_definitions();
        let removed = remove_inventory_item(player(), 0, &definitions).expect("remove item");
        let store = DropStore::new(Duration::from_secs(600));

        let drop = create_drop(&store, &removed).expect("create drop");

        assert_eq!(drop.quantity, 1);
        assert_eq!(drop.expires_at_unix_ms, 0);
        assert_eq!(map_drops(&store, 100).expect("map drops"), vec![drop]);
        assert!(map_drops(&store, 101).expect("other map drops").is_empty());
    }

    #[test]
    fn map_drop_order_ignores_interleaved_drops_on_other_maps() {
        let store = DropStore::new(Duration::from_secs(600));
        let first = card_drop("first", Vec2 { x: 10.0, y: 20.0 });
        let other = card_drop("other", Vec2 { x: 20.0, y: 20.0 });
        let second = card_drop("second", Vec2 { x: 30.0, y: 20.0 });

        restore_drop(&store, 100, first.clone(), None).expect("restore first drop");
        restore_drop(&store, 200, other.clone(), None).expect("restore other map drop");
        restore_drop(&store, 100, second.clone(), None).expect("restore second drop");

        assert_eq!(
            map_drops(&store, 100).expect("first map drops"),
            vec![first, second]
        );
        assert_eq!(
            map_drops(&store, 200).expect("other map drops"),
            vec![other]
        );
    }

    #[test]
    fn concurrent_restore_keeps_each_drop_id_on_exactly_one_map() {
        let store = Arc::new(DropStore::new(Duration::from_secs(600)));
        let barrier = Arc::new(Barrier::new(3));
        let drop = card_drop("shared-id", Vec2 { x: 20.0, y: 30.0 });
        let workers = [100, 200].map(|map_id| {
            let store = store.clone();
            let barrier = barrier.clone();
            let drop = drop.clone();
            std::thread::spawn(move || {
                barrier.wait();
                restore_drop(&store, map_id, drop, None).expect("restore shared drop");
            })
        });

        barrier.wait();
        for worker in workers {
            worker.join().expect("restore worker");
        }

        let first_map = map_drops(&store, 100).expect("first map drops");
        let second_map = map_drops(&store, 200).expect("second map drops");
        assert_eq!(first_map.len() + second_map.len(), 1);
        assert_eq!(
            first_map
                .into_iter()
                .chain(second_map)
                .next()
                .expect("one restored drop"),
            drop
        );
    }

    #[test]
    fn pickup_rejects_zero_quantity_without_consuming_the_drop() {
        let definitions = vec![stackable_definition()];
        let store = DropStore::new(Duration::from_secs(600));
        let drop = DroppedItem {
            id: "zero-quantity-drop".to_owned(),
            item_id: STACKABLE_ITEM_ID,
            position: Some(Vec2 { x: 20.0, y: 30.0 }),
            despawn_at_unix_ms: u64::MAX,
            quantity: 0,
            expires_at_unix_ms: 0,
        };
        restore_drop(&store, 100, drop.clone(), None).expect("restore drop");
        let position = drop.position.expect("drop position");

        assert!(matches!(
            pick_up_nearest(&store, player(), position, &definitions),
            Err(PickUpError::Rule(ItemRuleError::InvalidQuantity {
                item_id: STACKABLE_ITEM_ID
            }))
        ));
        assert_eq!(map_drops(&store, 100).expect("map drops"), vec![drop]);
    }

    #[test]
    fn pickup_merges_a_multi_unit_drop() {
        let definitions = vec![stackable_definition()];
        let mut player = player();
        player.inventory = Some(InventoryState {
            capacity: 1,
            stacks: vec![InventoryItemStack {
                item_id: STACKABLE_ITEM_ID,
                quantity: 4,
                expires_at_unix_ms: 0,
            }],
            ..InventoryState::default()
        });
        let store = DropStore::new(Duration::from_secs(600));
        let drop = DroppedItem {
            id: "quantity-drop".to_owned(),
            item_id: STACKABLE_ITEM_ID,
            position: Some(Vec2 { x: 20.0, y: 30.0 }),
            despawn_at_unix_ms: u64::MAX,
            quantity: 6,
            expires_at_unix_ms: 0,
        };
        restore_drop(&store, 100, drop, None).expect("restore drop");

        let picked = pick_up_nearest(&store, player, Vec2 { x: 20.0, y: 30.0 }, &definitions)
            .expect("pick up quantity");

        assert_eq!(
            picked.player.inventory.expect("inventory").stacks[0].quantity,
            10
        );
    }

    #[test]
    fn monster_book_pickup_inserts_increments_and_consumes_at_the_cap() {
        let catalog = card_catalog();
        let store = DropStore::new(Duration::from_secs(600));
        let position = Vec2 { x: 20.0, y: 30.0 };
        let mut player = player();
        player.inventory = None;

        for expected in 1..=5 {
            let drop = card_drop(&format!("card-{expected}"), position);
            restore_drop(&store, 100, drop, None).expect("restore card drop");
            let picked = pick_up_nearest(&store, player, position, &catalog).expect("pick up card");
            player = picked.player;
            assert_eq!(player.monster_book_cards[0].count, expected);
            assert!(map_drops(&store, 100).expect("map drops").is_empty());
        }

        let capped_drop = card_drop("capped-card", position);
        restore_drop(&store, 100, capped_drop, None).expect("restore capped card");
        let picked =
            pick_up_nearest(&store, player, position, &catalog).expect("consume capped card");
        assert_eq!(picked.player.monster_book_cards[0].count, 5);
        assert!(map_drops(&store, 100).expect("map drops").is_empty());
    }

    #[test]
    fn monster_book_pickup_bypasses_full_inventory_without_changing_it() {
        let catalog = card_catalog();
        let store = DropStore::new(Duration::from_secs(600));
        let position = Vec2 { x: 20.0, y: 30.0 };
        let mut player = player();
        player.inventory = Some(InventoryState {
            capacity: 1,
            stacks: vec![InventoryItemStack {
                item_id: STACKABLE_ITEM_ID,
                quantity: 10,
                expires_at_unix_ms: 0,
            }],
            ..InventoryState::default()
        });
        let inventory = player.inventory.clone();
        restore_drop(&store, 100, card_drop("full-card", position), None).expect("restore card");

        let picked = pick_up_nearest(&store, player, position, &catalog).expect("pick up card");

        assert_eq!(picked.player.inventory, inventory);
        assert_eq!(picked.player.monster_book_cards[0].count, 1);
    }

    #[test]
    fn monster_book_pickup_restores_the_drop_after_a_simulated_save_failure() {
        let catalog = card_catalog();
        let store = DropStore::new(Duration::from_secs(600));
        let position = Vec2 { x: 20.0, y: 30.0 };
        let drop = card_drop("rollback-card", position);
        let persisted_player = player();
        restore_drop(
            &store,
            persisted_player.map_id,
            drop.clone(),
            Some(persisted_player.id.clone()),
        )
        .expect("restore card");

        let picked = pick_up_nearest(&store, persisted_player.clone(), position, &catalog)
            .expect("stage card pickup");
        assert_eq!(picked.player.monster_book_cards[0].count, 1);
        assert!(
            map_drops(&store, persisted_player.map_id)
                .expect("map drops")
                .is_empty()
        );

        restore_drop(
            &store,
            persisted_player.map_id,
            picked.drop,
            picked.owner_player_id,
        )
        .expect("roll back failed save");
        assert!(persisted_player.monster_book_cards.is_empty());
        assert_eq!(
            map_drops(&store, persisted_player.map_id).expect("restored drops"),
            vec![drop]
        );
    }

    #[test]
    fn dropping_and_picking_up_preserves_item_expiration() {
        let definitions = vec![stackable_definition()];
        let mut source = player();
        source.inventory.as_mut().expect("inventory").stacks = vec![InventoryItemStack {
            item_id: STACKABLE_ITEM_ID,
            quantity: 1,
            expires_at_unix_ms: u64::MAX,
        }];
        let removed = remove_inventory_item(source, 0, &definitions).expect("remove item");
        let store = DropStore::new(Duration::from_secs(600));

        let drop = create_drop(&store, &removed).expect("create expiring drop");
        assert_eq!(drop.expires_at_unix_ms, u64::MAX);
        assert_ne!(drop.despawn_at_unix_ms, drop.expires_at_unix_ms);
        let picked = pick_up_nearest(
            &store,
            removed.player,
            drop.position.expect("drop position"),
            &definitions,
        )
        .expect("pick up expiring drop");

        assert_eq!(
            picked.player.inventory.expect("inventory").stacks[0].expires_at_unix_ms,
            u64::MAX
        );
    }

    #[test]
    fn item_expiration_removes_a_drop_before_its_normal_despawn() {
        let definitions = vec![stackable_definition()];
        let store = DropStore::new(Duration::from_secs(600));
        let drop = DroppedItem {
            id: "expired-item-drop".to_owned(),
            item_id: STACKABLE_ITEM_ID,
            position: Some(Vec2 { x: 20.0, y: 30.0 }),
            despawn_at_unix_ms: u64::MAX,
            quantity: 1,
            expires_at_unix_ms: 1,
        };
        restore_drop(&store, 100, drop, None).expect("restore expired drop");
        let mut player = player();
        player.inventory = Some(InventoryState {
            capacity: 1,
            ..InventoryState::default()
        });

        assert!(matches!(
            pick_up_nearest(&store, player, Vec2 { x: 20.0, y: 30.0 }, &definitions,),
            Err(PickUpError::Rule(ItemRuleError::NoNearbyDrop))
        ));
        assert!(map_drops(&store, 100).expect("map drops").is_empty());
    }

    #[test]
    fn failed_pickup_keeps_the_complete_drop_in_the_store() {
        let definitions = vec![stackable_definition()];
        let mut player = player();
        player.inventory = Some(InventoryState {
            capacity: 1,
            stacks: vec![InventoryItemStack {
                item_id: STACKABLE_ITEM_ID,
                quantity: 10,
                expires_at_unix_ms: 0,
            }],
            ..InventoryState::default()
        });
        let store = DropStore::new(Duration::from_secs(600));
        let drop = DroppedItem {
            id: "full-inventory-drop".to_owned(),
            item_id: STACKABLE_ITEM_ID,
            position: Some(Vec2 { x: 20.0, y: 30.0 }),
            despawn_at_unix_ms: u64::MAX,
            quantity: 3,
            expires_at_unix_ms: 0,
        };
        restore_drop(&store, 100, drop.clone(), Some("local".to_owned())).expect("restore drop");

        assert!(matches!(
            pick_up_nearest(&store, player, Vec2 { x: 20.0, y: 30.0 }, &definitions,),
            Err(PickUpError::Rule(ItemRuleError::InventoryFull))
        ));
        assert_eq!(map_drops(&store, 100).expect("map drops"), vec![drop]);
    }

    #[test]
    fn expired_drops_are_removed_at_the_configured_deadline() {
        let definitions = equipment_definitions();
        let removed = remove_inventory_item(player(), 0, &definitions).expect("remove item");
        let store = DropStore::new(Duration::from_secs(600));
        let drop = create_drop_at(&store, &removed, 1_000).expect("create drop");

        assert_eq!(drop.despawn_at_unix_ms, 601_000);
        assert_eq!(
            map_drops_at(&store, 100, 600_999).expect("active drops"),
            vec![drop]
        );
        assert!(
            map_drops_at(&store, 100, 601_000)
                .expect("expired drops")
                .is_empty()
        );
    }

    #[test]
    fn mob_drops_can_only_be_picked_up_by_the_final_attacker() {
        let store = DropStore::new(Duration::from_secs(600));
        let position = Vec2 { x: 20.0, y: 30.0 };
        let drops = create_mob_drops(&store, 100, position, &[SPARE_TOP_ID], "owner")
            .expect("create mob drop");
        let definitions = equipment_definitions();
        let mut stranger = player();
        stranger.id = "stranger".to_owned();

        assert!(matches!(
            pick_up_nearest(&store, stranger, position, &definitions),
            Err(PickUpError::Rule(ItemRuleError::NoNearbyDrop))
        ));
        let mut owner = player();
        owner.id = "owner".to_owned();
        let picked = pick_up_nearest(&store, owner, position, &definitions).expect("owner pickup");

        assert_eq!(drops[0].quantity, 1);
        assert_eq!(picked.drop.id, drops[0].id);
        assert_eq!(picked.owner_player_id.as_deref(), Some("owner"));
    }

    fn player() -> PlayerState {
        PlayerState {
            id: "local".to_owned(),
            map_id: 100,
            position: Some(Vec2 { x: 20.0, y: 30.0 }),
            inventory: Some(starter_inventory()),
            ..PlayerState::default()
        }
    }

    fn equipment_definitions() -> Vec<ItemDefinition> {
        vec![
            equipment_definition(SPARE_TOP_ID, EquipmentSlot::Top),
            equipment_definition(STARTER_TOP_ID, EquipmentSlot::Top),
            equipment_definition(SPARE_BOTTOM_ID, EquipmentSlot::Bottom),
            equipment_definition(SPARE_SHOES_ID, EquipmentSlot::Shoes),
        ]
    }

    fn equipment_definition(
        item_id: u32,
        slot: EquipmentSlot,
    ) -> ItemDefinition {
        ItemDefinition {
            item_id,
            slot: slot as i32,
            stack_max: 1,
            appearance_supported: true,
            ..ItemDefinition::default()
        }
    }

    fn stackable_definition() -> ItemDefinition {
        ItemDefinition {
            item_id: STACKABLE_ITEM_ID,
            stack_max: 10,
            ..ItemDefinition::default()
        }
    }

    fn card_catalog() -> TestCatalog {
        TestCatalog {
            definitions: vec![stackable_definition()],
            cards: BTreeMap::from([(
                CARD_ITEM_ID,
                crate::content::MonsterBookCardDefinition {
                    item_id: CARD_ITEM_ID,
                    source_mob_id: 100_100,
                    max_count: 5,
                },
            )]),
        }
    }

    fn card_drop(
        id: &str,
        position: Vec2,
    ) -> DroppedItem {
        DroppedItem {
            id: id.to_owned(),
            item_id: CARD_ITEM_ID,
            position: Some(position),
            despawn_at_unix_ms: u64::MAX,
            quantity: 1,
            expires_at_unix_ms: 0,
        }
    }
}
