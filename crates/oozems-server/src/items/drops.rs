use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU8;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use crossbeam_skiplist::SkipMap;
use oozems_proto::v1::DroppedInventoryItem;
use oozems_proto::v1::DroppedItem;
use oozems_proto::v1::PlayerState;
use oozems_proto::v1::Vec2;
use oozems_proto::v1::dropped_item;
use thiserror::Error;

use super::inventory::ItemDefinitionLookup;
use super::inventory::ItemRuleError;
use super::inventory::RemovedItem;
use super::inventory::apply_item_grant;
use super::inventory::canonicalize_inventory;

pub const PICK_UP_RADIUS: f32 = 80.0;
const BATCH_PREPARING: u8 = 0;
const BATCH_COMMITTED: u8 = 1;

pub struct DropStore {
    maps: SkipMap<u32, Arc<MapDrops>>,
    ids: SkipMap<String, Arc<MapDrop>>,
    expirations: SkipMap<(u32, u64, u64), Arc<MapDrop>>,
    lifespan: Duration,
    next_id: AtomicU64,
}

struct MapDrops {
    order: SkipMap<u64, Arc<MapDrop>>,
    mutation: Mutex<()>,
    version: AtomicU64,
    next_order: AtomicU64,
}

struct DropBatch {
    mutation: Mutex<()>,
    state: AtomicU8,
}

struct MapDrop {
    map_id: u32,
    item: DroppedItem,
    owner_player_id: Option<String>,
    order: u64,
    batch: Arc<DropBatch>,
    active: AtomicBool,
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
    #[error("the dropped-item map order is exhausted")]
    OrderOverflow,
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
            maps: SkipMap::new(),
            ids: SkipMap::new(),
            expirations: SkipMap::new(),
            lifespan,
            next_id: AtomicU64::new(0),
        }
    }
}

impl Default for MapDrops {
    fn default() -> Self {
        Self {
            order: SkipMap::new(),
            mutation: Mutex::new(()),
            version: AtomicU64::new(0),
            next_order: AtomicU64::new(0),
        }
    }
}

impl DropBatch {
    fn preparing() -> Self {
        Self {
            mutation: Mutex::new(()),
            state: AtomicU8::new(BATCH_PREPARING),
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
        item: new_item_drop(
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
    let item = new_item_drop(
        store,
        removed.item_id,
        removed.quantity,
        removed.expires_at_unix_ms,
        removed.position,
        now_ms,
        despawn_at_unix_ms,
    );
    retain_active_map(store, removed.map_id, now_ms)?;
    insert_single_drop(store, removed.map_id, item.clone(), None, true)?;
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
    let loot = item_ids
        .iter()
        .map(|item_id| crate::loot::RolledLoot::Item {
            item_id: *item_id,
            quantity: 1,
        })
        .collect::<Vec<_>>();
    let staged = stage_combat_drops(store, map_id, position, &loot, owner_player_id)?;
    let items = staged.iter().map(|grant| grant.item.clone()).collect();
    commit_staged_drops(store, &staged)?;
    Ok(items)
}

pub fn stage_combat_drops(
    store: &DropStore,
    map_id: u32,
    position: Vec2,
    loot: &[crate::loot::RolledLoot],
    owner_player_id: &str,
) -> Result<Vec<StagedDropGrant>, DropStoreError> {
    if loot.is_empty() {
        return Ok(Vec::new());
    }
    stage_combat_drops_at(
        store,
        map_id,
        position,
        loot,
        owner_player_id,
        unix_time_ms()?,
    )
}

fn stage_combat_drops_at(
    store: &DropStore,
    map_id: u32,
    position: Vec2,
    loot: &[crate::loot::RolledLoot],
    owner_player_id: &str,
    now_ms: u64,
) -> Result<Vec<StagedDropGrant>, DropStoreError> {
    let despawn_at_unix_ms = drop_expiry(store, now_ms)?;
    let owner_player_id = owner_player_id.to_owned();
    let count = loot.len();
    Ok(loot
        .iter()
        .enumerate()
        .map(|(index, loot)| StagedDropGrant {
            map_id,
            item: match loot {
                crate::loot::RolledLoot::Item { item_id, quantity } => new_item_drop(
                    store,
                    *item_id,
                    *quantity,
                    0,
                    spread_position(position, index, count),
                    now_ms,
                    despawn_at_unix_ms,
                ),
                crate::loot::RolledLoot::Mesos { amount } => new_meso_drop(
                    store,
                    *amount,
                    spread_position(position, index, count),
                    now_ms,
                    despawn_at_unix_ms,
                ),
            },
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
    commit_drop_batch(store, staged, true)
}

pub fn commit_new_staged_drops(
    store: &DropStore,
    staged: &[StagedDropGrant],
) -> Result<(), DropStoreError> {
    if staged.is_empty() {
        return Ok(());
    }
    let unique_ids = staged
        .iter()
        .map(|grant| grant.item.id.as_str())
        .collect::<HashSet<_>>();
    if unique_ids.len() != staged.len() {
        return Err(DropStoreError::Conflict);
    }
    commit_drop_batch(store, staged, false)
}

pub fn rollback_staged_drops(
    store: &DropStore,
    staged: &[StagedDropGrant],
) -> Result<(), DropStoreError> {
    if staged.is_empty() {
        return Ok(());
    }
    let unique_ids = staged
        .iter()
        .map(|grant| grant.item.id.as_str())
        .collect::<HashSet<_>>();
    if unique_ids.len() != staged.len() {
        return Err(DropStoreError::Conflict);
    }
    rollback_drop_batch(store, staged)
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
    player: PlayerState,
    position: Vec2,
    definitions: &(impl ItemDefinitionLookup + ?Sized),
) -> Result<PickedUpItem, PickUpError> {
    if !position.x.is_finite() || !position.y.is_finite() {
        return Err(ItemRuleError::MissingPosition.into());
    }
    let now_ms = unix_time_ms()?;
    retain_active_map(store, player.map_id, now_ms)?;
    loop {
        let selected = select_nearest_drop(store, player.map_id, &player.id, position, now_ms)?
            .ok_or(ItemRuleError::NoNearbyDrop)?;
        let updated_player =
            apply_picked_up_item(player.clone(), &selected, position, definitions)?;
        let Some(removed) = remove_drop_if_unchanged(store, player.map_id, &selected)? else {
            continue;
        };
        return Ok(PickedUpItem {
            drop: removed.item.clone(),
            owner_player_id: removed.owner_player_id.clone(),
            player: updated_player,
        });
    }
}

#[cfg(test)]
pub fn restore_drop(
    store: &DropStore,
    map_id: u32,
    item: DroppedItem,
    owner_player_id: Option<String>,
) -> Result<(), DropStoreError> {
    insert_single_drop(store, map_id, item, owner_player_id, true)?;
    Ok(())
}

pub fn restore_picked_up_drop(
    store: &DropStore,
    map_id: u32,
    item: DroppedItem,
    owner_player_id: Option<String>,
) -> Result<(), DropStoreError> {
    insert_single_drop(store, map_id, item, owner_player_id, false)?;
    Ok(())
}

fn map_drops_at(
    store: &DropStore,
    map_id: u32,
    now_ms: u64,
) -> Result<Vec<DroppedItem>, DropStoreError> {
    if map_has_expired_drops(store, map_id, now_ms) {
        retain_active_map(store, map_id, now_ms)?;
    }
    Ok(store
        .maps
        .get(&map_id)
        .map_or_else(Vec::new, |entry| map_items(entry.value(), now_ms)))
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

fn new_item_drop(
    store: &DropStore,
    item_id: u32,
    quantity: u32,
    expires_at_unix_ms: u64,
    position: Vec2,
    now_ms: u64,
    despawn_at_unix_ms: u64,
) -> DroppedItem {
    new_drop(
        store,
        dropped_item::Content::Item(DroppedInventoryItem {
            item_id,
            quantity,
            expires_at_unix_ms,
        }),
        position,
        now_ms,
        despawn_at_unix_ms,
    )
}

fn new_meso_drop(
    store: &DropStore,
    amount: u64,
    position: Vec2,
    now_ms: u64,
    despawn_at_unix_ms: u64,
) -> DroppedItem {
    new_drop(
        store,
        dropped_item::Content::Mesos(amount),
        position,
        now_ms,
        despawn_at_unix_ms,
    )
}

fn new_drop(
    store: &DropStore,
    content: dropped_item::Content,
    position: Vec2,
    now_ms: u64,
    despawn_at_unix_ms: u64,
) -> DroppedItem {
    let sequence = store.next_id.fetch_add(1, Ordering::Relaxed);
    DroppedItem {
        id: format!("drop-{now_ms:x}-{sequence:x}"),
        position: Some(position),
        despawn_at_unix_ms,
        content: Some(content),
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

struct MapWrite<'a> {
    maps: &'a [(u32, Arc<MapDrops>)],
}

impl<'a> MapWrite<'a> {
    fn begin(maps: &'a [(u32, Arc<MapDrops>)]) -> Self {
        for (_, map) in maps {
            let previous = map.version.fetch_add(1, Ordering::AcqRel);
            debug_assert_eq!(previous & 1, 0);
        }
        Self { maps }
    }
}

impl Drop for MapWrite<'_> {
    fn drop(&mut self) {
        for (_, map) in self.maps {
            let previous = map.version.fetch_add(1, Ordering::Release);
            debug_assert_eq!(previous & 1, 1);
        }
    }
}

fn commit_drop_batch(
    store: &DropStore,
    staged: &[StagedDropGrant],
    allow_existing: bool,
) -> Result<(), DropStoreError> {
    let staged = canonical_grants(staged)?;
    let maps = map_cells(store, staged.iter().map(|grant| grant.map_id));
    let _map_guards = lock_maps(&maps)?;

    let mut new_grants = Vec::new();
    for grant in staged {
        let Some(existing) = store.ids.get(&grant.item.id) else {
            new_grants.push(grant);
            continue;
        };
        if !allow_existing
            || !drop_is_visible(existing.value())
            || !drop_matches_grant(existing.value(), grant)
        {
            return Err(DropStoreError::Conflict);
        }
    }
    if new_grants.is_empty() {
        return Ok(());
    }

    check_order_capacity(&maps, &new_grants)?;
    let batch = Arc::new(DropBatch::preparing());
    let records = new_grants
        .into_iter()
        .map(|grant| {
            let map = map_by_id(&maps, grant.map_id);
            let order = map.next_order.fetch_add(1, Ordering::Relaxed);
            Arc::new(MapDrop {
                map_id: grant.map_id,
                item: grant.item.clone(),
                owner_player_id: grant.owner_player_id.clone(),
                order,
                batch: batch.clone(),
                active: AtomicBool::new(true),
            })
        })
        .collect::<Vec<_>>();

    let mut reserved = Vec::with_capacity(records.len());
    for record in &records {
        let entry = store
            .ids
            .get_or_insert(record.item.id.clone(), record.clone());
        if !Arc::ptr_eq(entry.value(), record) {
            release_id_reservations(store, &reserved);
            return Err(DropStoreError::Conflict);
        }
        reserved.push(record.clone());
    }

    let _write = MapWrite::begin(&maps);
    for record in &records {
        let map = map_by_id(&maps, record.map_id);
        map.order.insert(record.order, record.clone());
        store.expirations.insert(
            (record.map_id, drop_deadline(&record.item), record.order),
            record.clone(),
        );
    }
    batch.state.store(BATCH_COMMITTED, Ordering::Release);
    Ok(())
}

fn rollback_drop_batch(
    store: &DropStore,
    staged: &[StagedDropGrant],
) -> Result<(), DropStoreError> {
    let records = staged
        .iter()
        .map(|grant| {
            store
                .ids
                .get(&grant.item.id)
                .map(|entry| entry.value().clone())
                .ok_or(DropStoreError::Conflict)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let maps = existing_map_cells(store, staged.iter().map(|grant| grant.map_id))?;
    let _map_guards = lock_maps(&maps)?;
    let batches = unique_batches(&records);
    let _batch_guards = lock_batches(&batches)?;
    if records.iter().zip(staged).any(|(record, grant)| {
        record.batch.state.load(Ordering::Acquire) != BATCH_COMMITTED
            || !record_is_indexed(store, record)
            || !record.active.load(Ordering::Acquire)
            || !drop_matches_grant(record, grant)
    }) {
        return Err(DropStoreError::Conflict);
    }

    let _write = MapWrite::begin(&maps);
    for record in records.iter().rev() {
        remove_record_indexes(store, map_by_id(&maps, record.map_id), record);
    }
    Ok(())
}

fn insert_single_drop(
    store: &DropStore,
    map_id: u32,
    item: DroppedItem,
    owner_player_id: Option<String>,
    allow_existing: bool,
) -> Result<(), DropStoreError> {
    let grant = StagedDropGrant {
        map_id,
        item,
        owner_player_id,
    };
    match commit_drop_batch(store, &[grant], false) {
        Err(DropStoreError::Conflict) if allow_existing => Ok(()),
        result => result,
    }
}

fn canonical_grants(staged: &[StagedDropGrant]) -> Result<Vec<&StagedDropGrant>, DropStoreError> {
    let mut by_id = HashMap::with_capacity(staged.len());
    let mut canonical = Vec::with_capacity(staged.len());
    for grant in staged {
        match by_id.get(grant.item.id.as_str()) {
            Some(existing) if *existing == grant => {}
            Some(_) => return Err(DropStoreError::Conflict),
            None => {
                by_id.insert(grant.item.id.as_str(), grant);
                canonical.push(grant);
            }
        }
    }
    Ok(canonical)
}

fn map_cells(
    store: &DropStore,
    map_ids: impl IntoIterator<Item = u32>,
) -> Vec<(u32, Arc<MapDrops>)> {
    let mut map_ids = map_ids.into_iter().collect::<Vec<_>>();
    map_ids.sort_unstable();
    map_ids.dedup();
    map_ids
        .into_iter()
        .map(|map_id| {
            let map = store
                .maps
                .get_or_insert_with(map_id, || Arc::new(MapDrops::default()))
                .value()
                .clone();
            (map_id, map)
        })
        .collect()
}

fn existing_map_cells(
    store: &DropStore,
    map_ids: impl IntoIterator<Item = u32>,
) -> Result<Vec<(u32, Arc<MapDrops>)>, DropStoreError> {
    let mut map_ids = map_ids.into_iter().collect::<Vec<_>>();
    map_ids.sort_unstable();
    map_ids.dedup();
    map_ids
        .into_iter()
        .map(|map_id| {
            store
                .maps
                .get(&map_id)
                .map(|entry| (map_id, entry.value().clone()))
                .ok_or(DropStoreError::Conflict)
        })
        .collect()
}

fn lock_maps(maps: &[(u32, Arc<MapDrops>)]) -> Result<Vec<MutexGuard<'_, ()>>, DropStoreError> {
    maps.iter()
        .map(|(_, map)| map.mutation.lock().map_err(|_| DropStoreError::Lock))
        .collect()
}

fn map_by_id(
    maps: &[(u32, Arc<MapDrops>)],
    map_id: u32,
) -> &MapDrops {
    maps.binary_search_by_key(&map_id, |(candidate, _)| *candidate)
        .map(|index| maps[index].1.as_ref())
        .expect("a locked map batch contains every requested map")
}

fn check_order_capacity(
    maps: &[(u32, Arc<MapDrops>)],
    grants: &[&StagedDropGrant],
) -> Result<(), DropStoreError> {
    let mut counts = HashMap::<u32, u64>::new();
    for grant in grants {
        *counts.entry(grant.map_id).or_default() += 1;
    }
    for (map_id, count) in counts {
        map_by_id(maps, map_id)
            .next_order
            .load(Ordering::Relaxed)
            .checked_add(count)
            .ok_or(DropStoreError::OrderOverflow)?;
    }
    Ok(())
}

fn release_id_reservations(
    store: &DropStore,
    records: &[Arc<MapDrop>],
) {
    for record in records {
        if store
            .ids
            .get(&record.item.id)
            .is_some_and(|entry| Arc::ptr_eq(entry.value(), record))
        {
            store.ids.remove(&record.item.id);
        }
    }
}

fn record_is_indexed(
    store: &DropStore,
    record: &Arc<MapDrop>,
) -> bool {
    store
        .ids
        .get(&record.item.id)
        .is_some_and(|entry| Arc::ptr_eq(entry.value(), record))
}

fn drop_matches_grant(
    drop: &MapDrop,
    grant: &StagedDropGrant,
) -> bool {
    drop.map_id == grant.map_id
        && drop.item == grant.item
        && drop.owner_player_id == grant.owner_player_id
}

fn drop_is_visible(drop: &MapDrop) -> bool {
    drop.active.load(Ordering::Acquire)
        && drop.batch.state.load(Ordering::Acquire) == BATCH_COMMITTED
}

fn map_items(
    map: &MapDrops,
    now_ms: u64,
) -> Vec<DroppedItem> {
    read_stable_map(map, || {
        map.order
            .iter()
            .filter_map(|entry| {
                let drop = entry.value();
                (drop_is_visible(drop) && drop_deadline(&drop.item) > now_ms)
                    .then(|| drop.item.clone())
            })
            .collect()
    })
}

fn select_nearest_drop(
    store: &DropStore,
    map_id: u32,
    player_id: &str,
    position: Vec2,
    now_ms: u64,
) -> Result<Option<Arc<MapDrop>>, DropStoreError> {
    let Some(map) = store.maps.get(&map_id) else {
        return Ok(None);
    };
    Ok(read_stable_map(map.value(), || {
        map.value()
            .order
            .iter()
            .filter_map(|entry| {
                let drop = entry.value();
                if !drop_is_visible(drop)
                    || drop_deadline(&drop.item) <= now_ms
                    || drop
                        .owner_player_id
                        .as_deref()
                        .is_some_and(|owner| owner != player_id)
                {
                    return None;
                }
                let drop_position = drop.item.position?;
                let distance_squared = squared_distance(position, drop_position);
                (distance_squared <= PICK_UP_RADIUS * PICK_UP_RADIUS)
                    .then(|| (distance_squared, drop.clone()))
            })
            .min_by(|(left, _), (right, _)| left.total_cmp(right))
            .map(|(_, drop)| drop)
    }))
}

fn remove_drop_if_unchanged(
    store: &DropStore,
    map_id: u32,
    selected: &Arc<MapDrop>,
) -> Result<Option<Arc<MapDrop>>, DropStoreError> {
    let Some(map_entry) = store.maps.get(&map_id) else {
        return Ok(None);
    };
    let map = map_entry.value().clone();
    drop(map_entry);
    let _map_guard = map.mutation.lock().map_err(|_| DropStoreError::Lock)?;
    let _batch_guard = selected
        .batch
        .mutation
        .lock()
        .map_err(|_| DropStoreError::Lock)?;
    if selected.map_id != map_id
        || !drop_is_visible(selected)
        || !record_is_indexed(store, selected)
    {
        return Ok(None);
    }

    let maps = [(map_id, map.clone())];
    let _write = MapWrite::begin(&maps);
    selected.active.store(false, Ordering::Release);
    remove_record_indexes(store, &map, selected);
    Ok(Some(selected.clone()))
}

fn remove_record_indexes(
    store: &DropStore,
    map: &MapDrops,
    record: &Arc<MapDrop>,
) {
    store.ids.remove(&record.item.id);
    map.order.remove(&record.order);
    store
        .expirations
        .remove(&(record.map_id, drop_deadline(&record.item), record.order));
    record.active.store(false, Ordering::Release);
}

fn retain_active_map(
    store: &DropStore,
    map_id: u32,
    now_ms: u64,
) -> Result<(), DropStoreError> {
    let Some(map_entry) = store.maps.get(&map_id) else {
        return Ok(());
    };
    let map = map_entry.value().clone();
    drop(map_entry);
    let _map_guard = map.mutation.lock().map_err(|_| DropStoreError::Lock)?;
    let expired = store
        .expirations
        .range((map_id, 0, 0)..=(map_id, now_ms, u64::MAX))
        .map(|entry| entry.value().clone())
        .collect::<Vec<_>>();
    if expired.is_empty() {
        return Ok(());
    }

    let batches = unique_batches(&expired);
    let _batch_guards = lock_batches(&batches)?;
    let expired = expired
        .into_iter()
        .filter(|record| {
            record.map_id == map_id
                && record.active.load(Ordering::Acquire)
                && record.batch.state.load(Ordering::Acquire) == BATCH_COMMITTED
                && drop_deadline(&record.item) <= now_ms
                && record_is_indexed(store, record)
        })
        .collect::<Vec<_>>();
    if expired.is_empty() {
        return Ok(());
    }

    let maps = [(map_id, map.clone())];
    let _write = MapWrite::begin(&maps);
    for record in expired {
        remove_record_indexes(store, &map, &record);
    }
    Ok(())
}

fn unique_batches(records: &[Arc<MapDrop>]) -> Vec<Arc<DropBatch>> {
    let mut batches = records
        .iter()
        .map(|record| record.batch.clone())
        .collect::<Vec<_>>();
    batches.sort_unstable_by_key(|batch| Arc::as_ptr(batch) as usize);
    batches.dedup_by(|left, right| Arc::ptr_eq(left, right));
    batches
}

fn lock_batches(batches: &[Arc<DropBatch>]) -> Result<Vec<MutexGuard<'_, ()>>, DropStoreError> {
    batches
        .iter()
        .map(|batch| batch.mutation.lock().map_err(|_| DropStoreError::Lock))
        .collect()
}

fn map_has_expired_drops(
    store: &DropStore,
    map_id: u32,
    now_ms: u64,
) -> bool {
    store
        .expirations
        .range((map_id, 0, 0)..=(map_id, now_ms, u64::MAX))
        .next()
        .is_some()
}

fn read_stable_map<T>(
    map: &MapDrops,
    read: impl Fn() -> T,
) -> T {
    loop {
        let before = map.version.load(Ordering::Acquire);
        if before & 1 != 0 {
            std::thread::yield_now();
            continue;
        }
        let value = read();
        if map.version.load(Ordering::Acquire) == before {
            return value;
        }
    }
}

fn apply_picked_up_item(
    mut player: PlayerState,
    selected: &MapDrop,
    position: Vec2,
    definitions: &(impl ItemDefinitionLookup + ?Sized),
) -> Result<PlayerState, ItemRuleError> {
    match selected
        .item
        .content
        .as_ref()
        .ok_or(ItemRuleError::MissingDropContent)?
    {
        dropped_item::Content::Item(item) => {
            if item.quantity == 0 {
                return Err(ItemRuleError::InvalidQuantity {
                    item_id: item.item_id,
                });
            }
            if let Some(card) = definitions.monster_book_card(item.item_id) {
                debug_assert_eq!(card.item_id, item.item_id);
                debug_assert_eq!(card.max_count, crate::monster_book::MAX_CARD_COUNT);
                crate::monster_book::add_cards(
                    &mut player.monster_book_cards,
                    item.item_id,
                    item.quantity,
                );
            } else {
                let inventory = player
                    .inventory
                    .as_mut()
                    .ok_or(ItemRuleError::MissingInventory)?;
                canonicalize_inventory(inventory, definitions)?;
                apply_item_grant(
                    inventory,
                    definitions,
                    item.item_id,
                    u64::from(item.quantity),
                    item.expires_at_unix_ms,
                )?;
            }
        }
        dropped_item::Content::Mesos(amount) => {
            if *amount == 0 {
                return Err(ItemRuleError::InvalidMesoAmount);
            }
            let mesos = player
                .mesos
                .checked_add(*amount)
                .filter(|mesos| *mesos <= i64::MAX as u64)
                .ok_or(ItemRuleError::MesosOverflow)?;
            player.mesos = mesos;
        }
    }
    player.position = Some(position);
    Ok(player)
}

fn squared_distance(
    left: Vec2,
    right: Vec2,
) -> f32 {
    let x = left.x - right.x;
    let y = left.y - right.y;
    x * x + y * y
}

fn drop_deadline(item: &DroppedItem) -> u64 {
    match item.content.as_ref() {
        Some(dropped_item::Content::Item(content)) if content.expires_at_unix_ms != 0 => {
            item.despawn_at_unix_ms.min(content.expires_at_unix_ms)
        }
        _ => item.despawn_at_unix_ms,
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
    use std::sync::mpsc;
    use std::time::Duration;

    use oozems_proto::v1::DroppedInventoryItem;
    use oozems_proto::v1::DroppedItem;
    use oozems_proto::v1::EquipmentSlot;
    use oozems_proto::v1::InventoryItemStack;
    use oozems_proto::v1::InventoryState;
    use oozems_proto::v1::ItemDefinition;
    use oozems_proto::v1::PlayerState;
    use oozems_proto::v1::Vec2;
    use oozems_proto::v1::dropped_item;

    use super::DropStore;
    use super::DropStoreError;
    use super::PickUpError;
    use super::StagedDropGrant;
    use super::commit_new_staged_drops;
    use super::commit_staged_drops;
    use super::create_drop;
    use super::create_drop_at;
    use super::create_mob_drops;
    use super::map_drops;
    use super::map_drops_at;
    use super::pick_up_nearest;
    use super::restore_drop;
    use super::rollback_staged_drops;
    use super::stage_combat_drops;
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
            item: item_drop(id, STACKABLE_ITEM_ID, 1, 0, Vec2 { x: 10.0, y: 20.0 }),
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
    fn multi_map_batch_commit_is_idempotent_and_rollback_removes_every_drop() {
        let store = DropStore::new(Duration::from_secs(60));
        let staged = [staged_drop(1, "first"), staged_drop(2, "second")];

        commit_staged_drops(&store, &staged).expect("commit drops");
        commit_staged_drops(&store, &staged).expect("repeat commit");

        assert_eq!(map_drops(&store, 1).expect("first map drops").len(), 1);
        assert_eq!(map_drops(&store, 2).expect("second map drops").len(), 1);

        rollback_staged_drops(&store, &staged).expect("roll back drops");

        assert!(map_drops(&store, 1).expect("first map drops").is_empty());
        assert!(map_drops(&store, 2).expect("second map drops").is_empty());
    }

    #[test]
    fn rollback_accepts_a_batch_subset_and_drops_from_multiple_batches() {
        let store = DropStore::new(Duration::from_secs(60));
        let first_batch = [staged_drop(1, "first"), staged_drop(2, "second")];
        let third = staged_drop(3, "third");
        commit_new_staged_drops(&store, &first_batch).expect("commit first batch");
        commit_new_staged_drops(&store, std::slice::from_ref(&third)).expect("commit third drop");

        rollback_staged_drops(&store, &[first_batch[0].clone(), third])
            .expect("roll back selected drops");

        assert!(map_drops(&store, 1).expect("first map drops").is_empty());
        assert_eq!(
            map_drops(&store, 2).expect("second map drops"),
            vec![first_batch[1].item.clone()]
        );
        assert!(map_drops(&store, 3).expect("third map drops").is_empty());
        rollback_staged_drops(&store, &first_batch[1..]).expect("roll back remaining drop");
    }

    #[test]
    fn conflicting_batch_does_not_insert_its_new_drops() {
        let store = DropStore::new(Duration::from_secs(60));
        let existing = staged_drop(1, "existing");
        commit_new_staged_drops(&store, std::slice::from_ref(&existing))
            .expect("commit existing drop");

        assert!(matches!(
            commit_new_staged_drops(&store, &[existing.clone(), staged_drop(2, "new")]),
            Err(DropStoreError::Conflict)
        ));
        assert_eq!(
            map_drops(&store, 1).expect("first map drops"),
            vec![existing.item]
        );
        assert!(map_drops(&store, 2).expect("second map drops").is_empty());
    }

    #[test]
    fn batch_rollback_conflict_does_not_remove_the_unchanged_drops() {
        let definitions = vec![stackable_definition()];
        let store = DropStore::new(Duration::from_secs(60));
        let staged = [staged_drop(1, "picked"), staged_drop(2, "unchanged")];
        commit_new_staged_drops(&store, &staged).expect("commit drops");
        let mut picker = player();
        picker.map_id = 1;
        picker.inventory = Some(InventoryState {
            capacity: 1,
            ..InventoryState::default()
        });

        pick_up_nearest(&store, picker, Vec2 { x: 10.0, y: 20.0 }, &definitions)
            .expect("pick up first drop");

        assert!(matches!(
            rollback_staged_drops(&store, &staged),
            Err(DropStoreError::Conflict)
        ));
        assert!(map_drops(&store, 1).expect("first map drops").is_empty());
        assert_eq!(
            map_drops(&store, 2).expect("second map drops"),
            vec![staged[1].item.clone()]
        );
    }

    #[test]
    fn a_blocked_map_mutation_does_not_block_another_map() {
        let store = Arc::new(DropStore::new(Duration::from_secs(60)));
        restore_drop(&store, 1, staged_drop(1, "first-map").item, None).expect("create first map");
        let first_map = store.maps.get(&1).expect("first map").value().clone();
        let first_map_guard = first_map.mutation.lock().expect("lock first map");
        let (completed_tx, completed_rx) = mpsc::channel();
        let worker_store = store.clone();
        let worker = std::thread::spawn(move || {
            let result = restore_drop(&worker_store, 2, staged_drop(2, "second-map").item, None);
            completed_tx.send(result).expect("report completion");
        });

        completed_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("the second map must make independent progress")
            .expect("mutate second map");

        drop(first_map_guard);
        worker.join().expect("map worker");
        assert_eq!(map_drops(&store, 2).expect("second map drops").len(), 1);
    }

    #[test]
    fn concurrent_pickups_consume_one_drop_exactly_once() {
        let definitions = Arc::new(vec![stackable_definition()]);
        let store = Arc::new(DropStore::new(Duration::from_secs(60)));
        let drop = staged_drop(100, "contested");
        restore_drop(&store, 100, drop.item, None).expect("restore drop");
        let barrier = Arc::new(Barrier::new(3));
        let workers = ["first", "second"].map(|player_id| {
            let definitions = definitions.clone();
            let store = store.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let mut picker = player();
                picker.id = player_id.to_owned();
                picker.inventory = Some(InventoryState {
                    capacity: 1,
                    ..InventoryState::default()
                });
                barrier.wait();
                pick_up_nearest(
                    &store,
                    picker,
                    Vec2 { x: 10.0, y: 20.0 },
                    definitions.as_ref(),
                )
            })
        });

        barrier.wait();
        let results = workers.map(|worker| worker.join().expect("pickup worker"));

        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(
                    result,
                    Err(PickUpError::Rule(ItemRuleError::NoNearbyDrop))
                ))
                .count(),
            1
        );
        assert!(map_drops(&store, 100).expect("map drops").is_empty());
    }

    #[test]
    fn dropped_items_are_scoped_to_their_map_and_have_a_quantity() {
        let definitions = equipment_definitions();
        let removed = remove_inventory_item(player(), 0, &definitions).expect("remove item");
        let store = DropStore::new(Duration::from_secs(600));

        let drop = create_drop(&store, &removed).expect("create drop");

        assert_eq!(inventory_item(&drop).quantity, 1);
        assert_eq!(inventory_item(&drop).expires_at_unix_ms, 0);
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
        let drop = item_drop(
            "zero-quantity-drop",
            STACKABLE_ITEM_ID,
            0,
            0,
            Vec2 { x: 20.0, y: 30.0 },
        );
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
        let drop = item_drop(
            "quantity-drop",
            STACKABLE_ITEM_ID,
            6,
            0,
            Vec2 { x: 20.0, y: 30.0 },
        );
        restore_drop(&store, 100, drop, None).expect("restore drop");

        let picked = pick_up_nearest(&store, player, Vec2 { x: 20.0, y: 30.0 }, &definitions)
            .expect("pick up quantity");

        assert_eq!(
            picked.player.inventory.expect("inventory").stacks[0].quantity,
            10
        );
    }

    #[test]
    fn combat_loot_stages_item_quantities_and_mesos_as_typed_drops() {
        let store = DropStore::new(Duration::from_secs(600));
        let staged = stage_combat_drops(
            &store,
            100,
            Vec2 { x: 20.0, y: 30.0 },
            &[
                crate::loot::RolledLoot::Item {
                    item_id: STACKABLE_ITEM_ID,
                    quantity: 3,
                },
                crate::loot::RolledLoot::Mesos { amount: 25 },
            ],
            "owner",
        )
        .expect("stage combat loot");

        assert_eq!(inventory_item(staged[0].item()).quantity, 3);
        assert!(matches!(
            staged[1].item().content,
            Some(dropped_item::Content::Mesos(25))
        ));
    }

    #[test]
    fn pickup_credits_mesos_without_requiring_inventory() {
        let store = DropStore::new(Duration::from_secs(600));
        let position = Vec2 { x: 20.0, y: 30.0 };
        let mut player = player();
        player.inventory = None;
        player.mesos = 75;
        restore_drop(&store, 100, meso_drop("mesos", 25, position), None)
            .expect("restore meso drop");

        let picked = pick_up_nearest(&store, player, position, &[]).expect("pick up mesos");

        assert_eq!(picked.player.mesos, 100);
        assert!(map_drops(&store, 100).expect("map drops").is_empty());
    }

    #[test]
    fn pickup_rejects_invalid_meso_content_without_consuming_the_drop() {
        let position = Vec2 { x: 20.0, y: 30.0 };
        for (amount, starting_mesos, expected_error) in [
            (0, 0, ItemRuleError::InvalidMesoAmount),
            (1, i64::MAX as u64, ItemRuleError::MesosOverflow),
        ] {
            let store = DropStore::new(Duration::from_secs(600));
            let drop = meso_drop("invalid-mesos", amount, position);
            restore_drop(&store, 100, drop.clone(), None).expect("restore meso drop");
            let mut player = player();
            player.mesos = starting_mesos;

            assert!(matches!(
                pick_up_nearest(&store, player, position, &[]),
                Err(PickUpError::Rule(error)) if error == expected_error
            ));
            assert_eq!(map_drops(&store, 100).expect("map drops"), vec![drop]);
        }
    }

    #[test]
    fn pickup_rejects_missing_content_without_consuming_the_drop() {
        let store = DropStore::new(Duration::from_secs(600));
        let position = Vec2 { x: 20.0, y: 30.0 };
        let drop = DroppedItem {
            id: "missing-content".to_owned(),
            position: Some(position),
            despawn_at_unix_ms: u64::MAX,
            content: None,
        };
        restore_drop(&store, 100, drop.clone(), None).expect("restore malformed drop");

        assert!(matches!(
            pick_up_nearest(&store, player(), position, &[]),
            Err(PickUpError::Rule(ItemRuleError::MissingDropContent))
        ));
        assert_eq!(map_drops(&store, 100).expect("map drops"), vec![drop]);
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
        assert_eq!(inventory_item(&drop).expires_at_unix_ms, u64::MAX);
        assert_ne!(
            drop.despawn_at_unix_ms,
            inventory_item(&drop).expires_at_unix_ms
        );
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
        let drop = item_drop(
            "expired-item-drop",
            STACKABLE_ITEM_ID,
            1,
            1,
            Vec2 { x: 20.0, y: 30.0 },
        );
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
        let drop = item_drop(
            "full-inventory-drop",
            STACKABLE_ITEM_ID,
            3,
            0,
            Vec2 { x: 20.0, y: 30.0 },
        );
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

        assert_eq!(inventory_item(&drops[0]).quantity, 1);
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
        item_drop(id, CARD_ITEM_ID, 1, 0, position)
    }

    fn item_drop(
        id: &str,
        item_id: u32,
        quantity: u32,
        expires_at_unix_ms: u64,
        position: Vec2,
    ) -> DroppedItem {
        DroppedItem {
            id: id.to_owned(),
            position: Some(position),
            despawn_at_unix_ms: u64::MAX,
            content: Some(dropped_item::Content::Item(DroppedInventoryItem {
                item_id,
                quantity,
                expires_at_unix_ms,
            })),
        }
    }

    fn meso_drop(
        id: &str,
        amount: u64,
        position: Vec2,
    ) -> DroppedItem {
        DroppedItem {
            id: id.to_owned(),
            position: Some(position),
            despawn_at_unix_ms: u64::MAX,
            content: Some(dropped_item::Content::Mesos(amount)),
        }
    }

    fn inventory_item(drop: &DroppedItem) -> &DroppedInventoryItem {
        match drop.content.as_ref() {
            Some(dropped_item::Content::Item(item)) => item,
            Some(dropped_item::Content::Mesos(_)) | None => panic!("expected inventory item drop"),
        }
    }
}
