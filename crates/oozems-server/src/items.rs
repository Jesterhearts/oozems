use std::sync::Mutex;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use oozems_proto::v1::DroppedItem;
use oozems_proto::v1::EquipmentSlot;
use oozems_proto::v1::EquippedItem;
use oozems_proto::v1::InventoryState;
use oozems_proto::v1::ItemDefinition;
use oozems_proto::v1::PlayerState;
use oozems_proto::v1::Vec2;
use thiserror::Error;

pub const INVENTORY_CAPACITY: u32 = 24;
pub const STARTER_TOP_ID: u32 = 1_040_002;
pub const STARTER_BOTTOM_ID: u32 = 1_060_002;
pub const STARTER_SHOES_ID: u32 = 1_072_000;
pub const SPARE_TOP_ID: u32 = 1_040_003;
pub const SPARE_BOTTOM_ID: u32 = 1_060_001;
pub const SPARE_SHOES_ID: u32 = 1_072_001;
pub const PICK_UP_RADIUS: f32 = 80.0;

pub struct DropStore {
    drops: Mutex<Vec<MapDrop>>,
    lifespan: Duration,
    next_id: AtomicU64,
}

#[derive(Clone, Debug)]
struct MapDrop {
    map_id: u32,
    item: DroppedItem,
}

#[derive(Clone, Debug)]
pub struct RemovedItem {
    pub item_id: u32,
    pub map_id: u32,
    pub position: Vec2,
    pub player: PlayerState,
}

#[derive(Clone, Debug)]
pub struct PickedUpItem {
    pub drop: DroppedItem,
    pub player: PlayerState,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ItemRuleError {
    #[error("the player does not have inventory data")]
    MissingInventory,
    #[error("inventory index {index} does not exist")]
    InvalidInventoryIndex { index: u32 },
    #[error("item {item_id} is not available")]
    UnknownItem { item_id: u32 },
    #[error("item {item_id} cannot be equipped")]
    InvalidEquipment { item_id: u32 },
    #[error("equipment slot is invalid")]
    InvalidEquipmentSlot,
    #[error("equipment slot {slot:?} is empty")]
    EmptyEquipmentSlot { slot: EquipmentSlot },
    #[error("the inventory is full")]
    InventoryFull,
    #[error("the player does not have a valid map position")]
    MissingPosition,
    #[error("there is no dropped item close enough to pick up")]
    NoNearbyDrop,
}

#[derive(Debug, Error)]
pub enum DropStoreError {
    #[error("system time is before the Unix epoch")]
    InvalidSystemTime,
    #[error("drop expiry is outside the supported time range")]
    ExpiryOverflow,
    #[error("the dropped-item store lock was poisoned")]
    Lock,
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
            drops: Mutex::new(Vec::new()),
            lifespan,
            next_id: AtomicU64::new(0),
        }
    }
}

pub fn starter_inventory() -> InventoryState {
    InventoryState {
        item_ids: vec![SPARE_TOP_ID, SPARE_BOTTOM_ID, SPARE_SHOES_ID],
        equipment: vec![
            EquippedItem {
                slot: EquipmentSlot::Top as i32,
                item_id: STARTER_TOP_ID,
            },
            EquippedItem {
                slot: EquipmentSlot::Bottom as i32,
                item_id: STARTER_BOTTOM_ID,
            },
            EquippedItem {
                slot: EquipmentSlot::Shoes as i32,
                item_id: STARTER_SHOES_ID,
            },
        ],
        capacity: INVENTORY_CAPACITY,
    }
}

pub fn equip_inventory_item(
    mut player: PlayerState,
    inventory_index: u32,
    definitions: &[ItemDefinition],
) -> Result<PlayerState, ItemRuleError> {
    let inventory = player
        .inventory
        .as_mut()
        .ok_or(ItemRuleError::MissingInventory)?;
    let index = valid_inventory_index(inventory, inventory_index)?;
    let item_id = inventory.item_ids[index];
    let definition = find_definition(definitions, item_id)?;
    let slot = EquipmentSlot::try_from(definition.slot)
        .ok()
        .filter(|slot| *slot != EquipmentSlot::Unspecified)
        .ok_or(ItemRuleError::InvalidEquipment { item_id })?;

    inventory.item_ids.remove(index);
    if let Some(equipped) = inventory
        .equipment
        .iter_mut()
        .find(|equipped| equipped.slot == slot as i32)
    {
        inventory.item_ids.push(equipped.item_id);
        equipped.item_id = item_id;
    } else {
        inventory.equipment.push(EquippedItem {
            slot: slot as i32,
            item_id,
        });
    }
    inventory.equipment.sort_by_key(|equipped| equipped.slot);
    Ok(player)
}

pub fn unequip_item(
    mut player: PlayerState,
    slot_value: i32,
) -> Result<PlayerState, ItemRuleError> {
    let slot = EquipmentSlot::try_from(slot_value)
        .ok()
        .filter(|slot| *slot != EquipmentSlot::Unspecified)
        .ok_or(ItemRuleError::InvalidEquipmentSlot)?;
    let inventory = player
        .inventory
        .as_mut()
        .ok_or(ItemRuleError::MissingInventory)?;
    if inventory.item_ids.len() >= inventory.capacity as usize {
        return Err(ItemRuleError::InventoryFull);
    }
    let index = inventory
        .equipment
        .iter()
        .position(|equipped| equipped.slot == slot as i32)
        .ok_or(ItemRuleError::EmptyEquipmentSlot { slot })?;
    let equipped = inventory.equipment.remove(index);
    inventory.item_ids.push(equipped.item_id);
    Ok(player)
}

pub fn remove_inventory_item(
    mut player: PlayerState,
    inventory_index: u32,
    definitions: &[ItemDefinition],
) -> Result<RemovedItem, ItemRuleError> {
    let position = player
        .position
        .as_ref()
        .cloned()
        .filter(|position| position.x.is_finite() && position.y.is_finite())
        .ok_or(ItemRuleError::MissingPosition)?;
    let inventory = player
        .inventory
        .as_mut()
        .ok_or(ItemRuleError::MissingInventory)?;
    let index = valid_inventory_index(inventory, inventory_index)?;
    let item_id = inventory.item_ids[index];
    find_definition(definitions, item_id)?;
    inventory.item_ids.remove(index);

    Ok(RemovedItem {
        item_id,
        map_id: player.map_id,
        position,
        player,
    })
}

pub fn create_drop(
    store: &DropStore,
    removed: &RemovedItem,
) -> Result<DroppedItem, DropStoreError> {
    let now_ms = unix_time_ms()?;
    create_drop_at(store, removed, now_ms)
}

fn create_drop_at(
    store: &DropStore,
    removed: &RemovedItem,
    now_ms: u64,
) -> Result<DroppedItem, DropStoreError> {
    let lifespan_ms =
        u64::try_from(store.lifespan.as_millis()).map_err(|_| DropStoreError::ExpiryOverflow)?;
    let despawn_at_unix_ms = now_ms
        .checked_add(lifespan_ms)
        .ok_or(DropStoreError::ExpiryOverflow)?;
    let sequence = store.next_id.fetch_add(1, Ordering::Relaxed);
    let item = DroppedItem {
        id: format!("drop-{now_ms:x}-{sequence:x}"),
        item_id: removed.item_id,
        position: Some(removed.position),
        despawn_at_unix_ms,
    };
    let mut drops = store.drops.lock().map_err(|_| DropStoreError::Lock)?;
    retain_active_drops(&mut drops, now_ms);
    drops.push(MapDrop {
        map_id: removed.map_id,
        item: item.clone(),
    });
    Ok(item)
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
) -> Result<PickedUpItem, PickUpError> {
    if !position.x.is_finite() || !position.y.is_finite() {
        return Err(ItemRuleError::MissingPosition.into());
    }
    let inventory = player
        .inventory
        .as_mut()
        .ok_or(ItemRuleError::MissingInventory)?;
    if inventory.item_ids.len() >= inventory.capacity as usize {
        return Err(ItemRuleError::InventoryFull.into());
    }
    let now_ms = unix_time_ms()?;
    let mut drops = store.drops.lock().map_err(|_| DropStoreError::Lock)?;
    retain_active_drops(&mut drops, now_ms);
    let radius_squared = PICK_UP_RADIUS * PICK_UP_RADIUS;
    let index = drops
        .iter()
        .enumerate()
        .filter(|(_, drop)| drop.map_id == player.map_id)
        .filter_map(|(index, drop)| {
            let drop_position = drop.item.position.as_ref()?;
            let dx = drop_position.x - position.x;
            let dy = drop_position.y - position.y;
            let distance_squared = dx * dx + dy * dy;
            (distance_squared <= radius_squared).then_some((index, distance_squared))
        })
        .min_by(|(_, left), (_, right)| left.total_cmp(right))
        .map(|(index, _)| index)
        .ok_or(ItemRuleError::NoNearbyDrop)?;
    let drop = drops.remove(index).item;
    inventory.item_ids.push(drop.item_id);
    player.position = Some(position);
    Ok(PickedUpItem { drop, player })
}

pub fn restore_drop(
    store: &DropStore,
    map_id: u32,
    item: DroppedItem,
) -> Result<(), DropStoreError> {
    let mut drops = store.drops.lock().map_err(|_| DropStoreError::Lock)?;
    if !drops.iter().any(|drop| drop.item.id == item.id) {
        drops.push(MapDrop { map_id, item });
    }
    Ok(())
}

fn map_drops_at(
    store: &DropStore,
    map_id: u32,
    now_ms: u64,
) -> Result<Vec<DroppedItem>, DropStoreError> {
    let mut drops = store.drops.lock().map_err(|_| DropStoreError::Lock)?;
    retain_active_drops(&mut drops, now_ms);
    Ok(drops
        .iter()
        .filter(|drop| drop.map_id == map_id)
        .map(|drop| drop.item.clone())
        .collect())
}

fn valid_inventory_index(
    inventory: &InventoryState,
    inventory_index: u32,
) -> Result<usize, ItemRuleError> {
    let index = inventory_index as usize;
    (index < inventory.item_ids.len()).then_some(index).ok_or(
        ItemRuleError::InvalidInventoryIndex {
            index: inventory_index,
        },
    )
}

fn find_definition(
    definitions: &[ItemDefinition],
    item_id: u32,
) -> Result<&ItemDefinition, ItemRuleError> {
    definitions
        .iter()
        .find(|definition| definition.item_id == item_id)
        .ok_or(ItemRuleError::UnknownItem { item_id })
}

fn retain_active_drops(
    drops: &mut Vec<MapDrop>,
    now_ms: u64,
) {
    drops.retain(|drop| drop.item.despawn_at_unix_ms > now_ms);
}

fn unix_time_ms() -> Result<u64, DropStoreError> {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| DropStoreError::InvalidSystemTime)?;
    u64::try_from(elapsed.as_millis()).map_err(|_| DropStoreError::ExpiryOverflow)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use oozems_proto::v1::EquipmentSlot;
    use oozems_proto::v1::ItemDefinition;
    use oozems_proto::v1::PlayerState;
    use oozems_proto::v1::Vec2;

    use super::DropStore;
    use super::SPARE_TOP_ID;
    use super::STARTER_TOP_ID;
    use super::create_drop;
    use super::create_drop_at;
    use super::equip_inventory_item;
    use super::map_drops;
    use super::map_drops_at;
    use super::pick_up_nearest;
    use super::remove_inventory_item;
    use super::starter_inventory;
    use super::unequip_item;

    #[test]
    fn equipment_moves_between_inventory_and_its_slot() {
        let player = player();
        let definitions = vec![definition(SPARE_TOP_ID, EquipmentSlot::Top)];

        let player = equip_inventory_item(player, 0, &definitions).expect("equip top");
        let inventory = player.inventory.as_ref().expect("inventory");
        assert_eq!(inventory.item_ids.last(), Some(&STARTER_TOP_ID));
        assert!(inventory.equipment.iter().any(|item| {
            item.slot == EquipmentSlot::Top as i32 && item.item_id == SPARE_TOP_ID
        }));

        let player = unequip_item(player, EquipmentSlot::Top as i32).expect("unequip top");
        let inventory = player.inventory.expect("inventory");
        assert!(
            !inventory
                .equipment
                .iter()
                .any(|item| item.slot == EquipmentSlot::Top as i32)
        );
        assert_eq!(inventory.item_ids.last(), Some(&SPARE_TOP_ID));
    }

    #[test]
    fn dropped_items_are_scoped_to_their_map() {
        let definitions = vec![definition(SPARE_TOP_ID, EquipmentSlot::Top)];
        let removed = remove_inventory_item(player(), 0, &definitions).expect("remove item");
        let store = DropStore::new(Duration::from_secs(600));

        let drop = create_drop(&store, &removed).expect("create drop");

        assert_eq!(map_drops(&store, 100).expect("map drops"), vec![drop]);
        assert!(map_drops(&store, 101).expect("other map drops").is_empty());
    }

    #[test]
    fn nearest_dropped_item_moves_back_into_inventory() {
        let definitions = vec![definition(SPARE_TOP_ID, EquipmentSlot::Top)];
        let removed = remove_inventory_item(player(), 0, &definitions).expect("remove item");
        let store = DropStore::new(Duration::from_secs(600));
        let drop = create_drop(&store, &removed).expect("create drop");
        let position = drop.position.expect("drop position");

        let picked = pick_up_nearest(&store, removed.player, position).expect("pick up drop");

        assert_eq!(picked.drop.id, drop.id);
        assert_eq!(
            picked.player.inventory.expect("inventory").item_ids.last(),
            Some(&SPARE_TOP_ID)
        );
        assert!(map_drops(&store, 100).expect("map drops").is_empty());
    }

    #[test]
    fn expired_drops_are_removed_at_the_configured_deadline() {
        let definitions = vec![definition(SPARE_TOP_ID, EquipmentSlot::Top)];
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

    fn player() -> PlayerState {
        PlayerState {
            id: "local".to_owned(),
            map_id: 100,
            position: Some(Vec2 { x: 20.0, y: 30.0 }),
            inventory: Some(starter_inventory()),
            ..PlayerState::default()
        }
    }

    fn definition(
        item_id: u32,
        slot: EquipmentSlot,
    ) -> ItemDefinition {
        ItemDefinition {
            item_id,
            slot: slot as i32,
            ..ItemDefinition::default()
        }
    }
}
