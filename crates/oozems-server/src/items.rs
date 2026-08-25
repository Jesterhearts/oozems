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
use oozems_proto::v1::EquipmentSlot;
use oozems_proto::v1::EquippedItem;
use oozems_proto::v1::InventoryItemStack;
use oozems_proto::v1::InventoryState;
use oozems_proto::v1::ItemDefinition;
use oozems_proto::v1::PlayerState;
use oozems_proto::v1::Vec2;
use thiserror::Error;

use crate::interactions::ShopCurrency;

pub const INVENTORY_CAPACITY: u32 = 24;
pub const STARTER_TOP_ID: u32 = 1_040_002;
pub const STARTER_BOTTOM_ID: u32 = 1_060_002;
pub const STARTER_SHOES_ID: u32 = 1_072_000;
pub const SPARE_TOP_ID: u32 = 1_040_003;
pub const SPARE_BOTTOM_ID: u32 = 1_060_001;
pub const SPARE_SHOES_ID: u32 = 1_072_001;
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
pub struct RemovedItem {
    pub item_id: u32,
    pub quantity: u32,
    pub expires_at_unix_ms: u64,
    pub map_id: u32,
    pub position: Vec2,
    pub player: PlayerState,
}

#[derive(Clone, Debug)]
pub struct PickedUpItem {
    pub drop: DroppedItem,
    pub owner_player_id: Option<String>,
    pub player: PlayerState,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ItemRuleError {
    #[error("the player does not have inventory data")]
    MissingInventory,
    #[error("inventory index {index} does not exist")]
    InvalidInventoryIndex { index: u32 },
    #[error("the item at inventory index {index} changed before the action was applied")]
    InventorySelectionChanged { index: u32 },
    #[error("the inventory stack layout is not canonical")]
    NonCanonicalInventory,
    #[error("item {item_id} is not available")]
    UnknownItem { item_id: u32 },
    #[error("item {item_id} metadata could not be loaded: {message}")]
    DefinitionLoad { item_id: u32, message: String },
    #[error("item {item_id} has an invalid stack maximum")]
    InvalidStackMaximum { item_id: u32 },
    #[error("item {item_id} quantity must be greater than zero")]
    InvalidQuantity { item_id: u32 },
    #[error("item {item_id} quantity exceeds the supported range")]
    QuantityOverflow { item_id: u32 },
    #[error(
        "the inventory only contains {available} of item {item_id}, but {requested} were requested"
    )]
    InsufficientItems {
        item_id: u32,
        requested: u64,
        available: u64,
    },
    #[error("item {item_id} cannot be equipped")]
    InvalidEquipment { item_id: u32 },
    #[error("equipment slot is invalid")]
    InvalidEquipmentSlot,
    #[error("equipment slot {slot:?} is empty")]
    EmptyEquipmentSlot { slot: EquipmentSlot },
    #[error("the inventory is full")]
    InventoryFull,
    #[error("the player does not have enough mesos")]
    InsufficientMesos,
    #[error("the player does not have enough cash points")]
    InsufficientCashPoints,
    #[error("item {item_id} cannot be sold")]
    UnsellableItem { item_id: u32 },
    #[error("the mesos balance exceeds the supported range")]
    MesosOverflow,
    #[error("the player does not have a valid map position")]
    MissingPosition,
    #[error("there is no dropped item close enough to pick up")]
    NoNearbyDrop,
}

pub trait ItemDefinitionLookup {
    fn item_definition(
        &self,
        item_id: u32,
    ) -> Result<Option<&ItemDefinition>, ItemRuleError>;

    fn monster_book_card(
        &self,
        _item_id: u32,
    ) -> Option<crate::content::MonsterBookCardDefinition> {
        None
    }
}

pub fn validate_inventory_selection(
    player: &PlayerState,
    inventory_index: u32,
    expected_item_id: u32,
    expected_expires_at_unix_ms: u64,
) -> Result<(), ItemRuleError> {
    let inventory = player
        .inventory
        .as_ref()
        .ok_or(ItemRuleError::MissingInventory)?;
    let index = valid_inventory_index(inventory, inventory_index)?;
    let stack = &inventory.stacks[index];
    if expected_item_id == 0
        || stack.item_id != expected_item_id
        || stack.expires_at_unix_ms != expected_expires_at_unix_ms
    {
        return Err(ItemRuleError::InventorySelectionChanged {
            index: inventory_index,
        });
    }
    Ok(())
}

impl ItemDefinitionLookup for [ItemDefinition] {
    fn item_definition(
        &self,
        item_id: u32,
    ) -> Result<Option<&ItemDefinition>, ItemRuleError> {
        Ok(self.iter().find(|definition| definition.item_id == item_id))
    }
}

impl ItemDefinitionLookup for Vec<ItemDefinition> {
    fn item_definition(
        &self,
        item_id: u32,
    ) -> Result<Option<&ItemDefinition>, ItemRuleError> {
        self.as_slice().item_definition(item_id)
    }
}

impl<const N: usize> ItemDefinitionLookup for [ItemDefinition; N] {
    fn item_definition(
        &self,
        item_id: u32,
    ) -> Result<Option<&ItemDefinition>, ItemRuleError> {
        self.as_slice().item_definition(item_id)
    }
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

pub fn find_definition(
    definitions: &(impl ItemDefinitionLookup + ?Sized),
    item_id: u32,
) -> Result<&ItemDefinition, ItemRuleError> {
    definitions
        .item_definition(item_id)?
        .ok_or(ItemRuleError::UnknownItem { item_id })
}

pub fn canonicalize_inventory(
    inventory: &mut InventoryState,
    definitions: &(impl ItemDefinitionLookup + ?Sized),
) -> Result<(), ItemRuleError> {
    let stacks = canonical_stacks(inventory, definitions)?;
    inventory.stacks = stacks;
    Ok(())
}

pub fn validate_inventory(
    inventory: &InventoryState,
    definitions: &(impl ItemDefinitionLookup + ?Sized),
) -> Result<(), ItemRuleError> {
    if canonical_stacks(inventory, definitions)? != inventory.stacks {
        return Err(ItemRuleError::NonCanonicalInventory);
    }
    for equipped in &inventory.equipment {
        let definition = find_definition(definitions, equipped.item_id)?;
        let slot = supported_equipment_slot(definition).ok_or(ItemRuleError::InvalidEquipment {
            item_id: equipped.item_id,
        })?;
        if equipped.slot != slot as i32 {
            return Err(ItemRuleError::InvalidEquipment {
                item_id: equipped.item_id,
            });
        }
    }
    Ok(())
}

pub fn count_item_quantity(
    stacks: &[InventoryItemStack],
    item_id: u32,
) -> Result<u64, ItemRuleError> {
    stacks
        .iter()
        .filter(|stack| stack.item_id == item_id)
        .try_fold(0_u64, |total, stack| {
            if stack.quantity == 0 {
                return Err(ItemRuleError::InvalidQuantity { item_id });
            }
            total
                .checked_add(u64::from(stack.quantity))
                .ok_or(ItemRuleError::QuantityOverflow { item_id })
        })
}

pub fn count_inventory_item(
    inventory: &InventoryState,
    definitions: &(impl ItemDefinitionLookup + ?Sized),
    item_id: u32,
) -> Result<u64, ItemRuleError> {
    let stacks = canonical_stacks(inventory, definitions)?;
    count_item_quantity(&stacks, item_id)
}

pub fn prune_expired_inventory(
    inventory: &mut InventoryState,
    now_unix_ms: u64,
) -> bool {
    let original_stack_len = inventory.stacks.len();
    let original_equipment_len = inventory.equipment.len();
    inventory
        .stacks
        .retain(|stack| stack.expires_at_unix_ms == 0 || stack.expires_at_unix_ms > now_unix_ms);
    inventory.equipment.retain(|equipped| {
        equipped.expires_at_unix_ms == 0 || equipped.expires_at_unix_ms > now_unix_ms
    });
    inventory.stacks.len() != original_stack_len
        || inventory.equipment.len() != original_equipment_len
}

pub fn prune_and_validate_inventory(
    inventory: &mut InventoryState,
    definitions: &(impl ItemDefinitionLookup + ?Sized),
    now_unix_ms: u64,
) -> Result<bool, ItemRuleError> {
    let pruned = prune_expired_inventory(inventory, now_unix_ms);
    validate_inventory(inventory, definitions)?;
    Ok(pruned)
}

pub fn preflight_item_grant(
    inventory: &InventoryState,
    definitions: &(impl ItemDefinitionLookup + ?Sized),
    item_id: u32,
    quantity: u64,
    expires_at_unix_ms: u64,
) -> Result<Vec<InventoryItemStack>, ItemRuleError> {
    if quantity == 0 {
        return Err(ItemRuleError::InvalidQuantity { item_id });
    }
    let definition = find_definition(definitions, item_id)?;
    let stack_max = definition_stack_max(definition)?;
    let mut stacks = canonical_stacks(inventory, definitions)?;
    add_to_stacks(
        &mut stacks,
        inventory.capacity,
        item_id,
        quantity,
        stack_max,
        expires_at_unix_ms,
    )?;
    Ok(stacks)
}

pub fn apply_item_grant(
    inventory: &mut InventoryState,
    definitions: &(impl ItemDefinitionLookup + ?Sized),
    item_id: u32,
    quantity: u64,
    expires_at_unix_ms: u64,
) -> Result<(), ItemRuleError> {
    let stacks = preflight_item_grant(
        inventory,
        definitions,
        item_id,
        quantity,
        expires_at_unix_ms,
    )?;
    inventory.stacks = stacks;
    Ok(())
}

pub fn preflight_item_delta(
    inventory: &InventoryState,
    definitions: &(impl ItemDefinitionLookup + ?Sized),
    item_id: u32,
    delta: i64,
) -> Result<Vec<InventoryItemStack>, ItemRuleError> {
    inventory_after_delta(inventory, definitions, item_id, delta)
}

pub fn apply_item_delta(
    inventory: &mut InventoryState,
    definitions: &(impl ItemDefinitionLookup + ?Sized),
    item_id: u32,
    delta: i64,
) -> Result<(), ItemRuleError> {
    let stacks = preflight_item_delta(inventory, definitions, item_id, delta)?;
    inventory.stacks = stacks;
    Ok(())
}

pub fn starter_inventory() -> InventoryState {
    InventoryState {
        equipment: vec![
            EquippedItem {
                slot: EquipmentSlot::Top as i32,
                item_id: STARTER_TOP_ID,
                expires_at_unix_ms: 0,
            },
            EquippedItem {
                slot: EquipmentSlot::Bottom as i32,
                item_id: STARTER_BOTTOM_ID,
                expires_at_unix_ms: 0,
            },
            EquippedItem {
                slot: EquipmentSlot::Shoes as i32,
                item_id: STARTER_SHOES_ID,
                expires_at_unix_ms: 0,
            },
        ],
        capacity: INVENTORY_CAPACITY,
        stacks: [SPARE_TOP_ID, SPARE_BOTTOM_ID, SPARE_SHOES_ID]
            .into_iter()
            .map(|item_id| InventoryItemStack {
                item_id,
                quantity: 1,
                expires_at_unix_ms: 0,
            })
            .collect(),
    }
}

pub fn equip_inventory_item(
    mut player: PlayerState,
    inventory_index: u32,
    definitions: &(impl ItemDefinitionLookup + ?Sized),
) -> Result<PlayerState, ItemRuleError> {
    let inventory = player
        .inventory
        .as_mut()
        .ok_or(ItemRuleError::MissingInventory)?;
    let mut next = inventory.clone();
    canonicalize_inventory(&mut next, definitions)?;
    let index = valid_inventory_index(&next, inventory_index)?;
    let item_id = next.stacks[index].item_id;
    let expires_at_unix_ms = next.stacks[index].expires_at_unix_ms;
    let definition = find_definition(definitions, item_id)?;
    let slot =
        supported_equipment_slot(definition).ok_or(ItemRuleError::InvalidEquipment { item_id })?;
    remove_stack_quantity(&mut next, definitions, index, 1)?;

    if let Some(equipped) = next
        .equipment
        .iter()
        .find(|equipped| equipped.slot == slot as i32)
        .copied()
    {
        apply_item_grant(
            &mut next,
            definitions,
            equipped.item_id,
            1,
            equipped.expires_at_unix_ms,
        )?;
        *next
            .equipment
            .iter_mut()
            .find(|candidate| candidate.slot == slot as i32)
            .expect("equipped item disappeared") = EquippedItem {
            slot: slot as i32,
            item_id,
            expires_at_unix_ms,
        };
    } else {
        next.equipment.push(EquippedItem {
            slot: slot as i32,
            item_id,
            expires_at_unix_ms,
        });
    }
    next.equipment.sort_by_key(|equipped| equipped.slot);
    *inventory = next;
    Ok(player)
}

pub fn unequip_item(
    mut player: PlayerState,
    slot_value: i32,
    definitions: &(impl ItemDefinitionLookup + ?Sized),
) -> Result<PlayerState, ItemRuleError> {
    let slot = EquipmentSlot::try_from(slot_value)
        .ok()
        .filter(|slot| is_supported_slot(*slot))
        .ok_or(ItemRuleError::InvalidEquipmentSlot)?;
    let inventory = player
        .inventory
        .as_mut()
        .ok_or(ItemRuleError::MissingInventory)?;
    let mut next = inventory.clone();
    canonicalize_inventory(&mut next, definitions)?;
    let index = next
        .equipment
        .iter()
        .position(|equipped| equipped.slot == slot as i32)
        .ok_or(ItemRuleError::EmptyEquipmentSlot { slot })?;
    let equipped = next.equipment[index];
    apply_item_grant(
        &mut next,
        definitions,
        equipped.item_id,
        1,
        equipped.expires_at_unix_ms,
    )?;
    next.equipment.remove(index);
    *inventory = next;
    Ok(player)
}

pub fn buy_shop_item(
    mut player: PlayerState,
    item_id: u32,
    price: u64,
    currency: ShopCurrency,
    definitions: &(impl ItemDefinitionLookup + ?Sized),
) -> Result<PlayerState, ItemRuleError> {
    find_definition(definitions, item_id)?;
    match currency {
        ShopCurrency::Mesos if player.mesos < price => {
            return Err(ItemRuleError::InsufficientMesos);
        }
        ShopCurrency::CashPoints if player.cash_points < price => {
            return Err(ItemRuleError::InsufficientCashPoints);
        }
        ShopCurrency::Mesos | ShopCurrency::CashPoints => {}
    }
    let inventory = player
        .inventory
        .as_mut()
        .ok_or(ItemRuleError::MissingInventory)?;
    apply_item_delta(inventory, definitions, item_id, 1)?;
    match currency {
        ShopCurrency::Mesos => player.mesos -= price,
        ShopCurrency::CashPoints => player.cash_points -= price,
    }
    Ok(player)
}

pub fn sell_inventory_item(
    mut player: PlayerState,
    inventory_index: u32,
    definitions: &(impl ItemDefinitionLookup + ?Sized),
) -> Result<PlayerState, ItemRuleError> {
    let inventory = player
        .inventory
        .as_mut()
        .ok_or(ItemRuleError::MissingInventory)?;
    let mut next = inventory.clone();
    canonicalize_inventory(&mut next, definitions)?;
    let index = valid_inventory_index(&next, inventory_index)?;
    let item_id = next.stacks[index].item_id;
    let price = find_definition(definitions, item_id)?.sale_price;
    if price == 0 {
        return Err(ItemRuleError::UnsellableItem { item_id });
    }
    let mesos = player
        .mesos
        .checked_add(price)
        .ok_or(ItemRuleError::MesosOverflow)?;
    remove_stack_quantity(&mut next, definitions, index, 1)?;
    *inventory = next;
    player.mesos = mesos;
    Ok(player)
}

pub fn remove_inventory_item(
    mut player: PlayerState,
    inventory_index: u32,
    definitions: &(impl ItemDefinitionLookup + ?Sized),
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
    let mut next = inventory.clone();
    canonicalize_inventory(&mut next, definitions)?;
    let index = valid_inventory_index(&next, inventory_index)?;
    let item_id = next.stacks[index].item_id;
    let expires_at_unix_ms = next.stacks[index].expires_at_unix_ms;
    remove_stack_quantity(&mut next, definitions, index, 1)?;
    *inventory = next;

    Ok(RemovedItem {
        item_id,
        quantity: 1,
        expires_at_unix_ms,
        map_id: player.map_id,
        position,
        player,
    })
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

fn canonical_stacks(
    inventory: &InventoryState,
    definitions: &(impl ItemDefinitionLookup + ?Sized),
) -> Result<Vec<InventoryItemStack>, ItemRuleError> {
    let mut stacks = Vec::new();
    for stack in &inventory.stacks {
        if stack.quantity == 0 {
            return Err(ItemRuleError::InvalidQuantity {
                item_id: stack.item_id,
            });
        }
        let definition = find_definition(definitions, stack.item_id)?;
        let stack_max = definition_stack_max(definition)?;
        add_to_stacks(
            &mut stacks,
            inventory.capacity,
            stack.item_id,
            u64::from(stack.quantity),
            stack_max,
            stack.expires_at_unix_ms,
        )?;
    }
    Ok(stacks)
}

fn inventory_after_delta(
    inventory: &InventoryState,
    definitions: &(impl ItemDefinitionLookup + ?Sized),
    item_id: u32,
    delta: i64,
) -> Result<Vec<InventoryItemStack>, ItemRuleError> {
    if delta == 0 {
        return Err(ItemRuleError::InvalidQuantity { item_id });
    }
    if delta == i64::MIN {
        return Err(ItemRuleError::QuantityOverflow { item_id });
    }
    let definition = find_definition(definitions, item_id)?;
    let stack_max = definition_stack_max(definition)?;
    let mut stacks = canonical_stacks(inventory, definitions)?;
    if delta > 0 {
        add_to_stacks(
            &mut stacks,
            inventory.capacity,
            item_id,
            delta.unsigned_abs(),
            stack_max,
            0,
        )?;
    } else {
        remove_from_stacks(&mut stacks, item_id, delta.unsigned_abs())?;
    }
    Ok(stacks)
}

fn add_to_stacks(
    stacks: &mut Vec<InventoryItemStack>,
    capacity: u32,
    item_id: u32,
    mut quantity: u64,
    stack_max: u32,
    expires_at_unix_ms: u64,
) -> Result<(), ItemRuleError> {
    for stack in stacks.iter_mut().filter(|stack| {
        stack.item_id == item_id
            && stack.expires_at_unix_ms == expires_at_unix_ms
            && stack.quantity < stack_max
    }) {
        let added = quantity.min(u64::from(stack_max - stack.quantity));
        stack.quantity = stack
            .quantity
            .checked_add(
                u32::try_from(added).map_err(|_| ItemRuleError::QuantityOverflow { item_id })?,
            )
            .ok_or(ItemRuleError::QuantityOverflow { item_id })?;
        quantity -= added;
        if quantity == 0 {
            return Ok(());
        }
    }

    let required_slots = quantity.div_ceil(u64::from(stack_max));
    let available_slots = u64::from(capacity).saturating_sub(stacks.len() as u64);
    if required_slots > available_slots {
        return Err(ItemRuleError::InventoryFull);
    }
    while quantity > 0 {
        let stack_quantity = quantity.min(u64::from(stack_max));
        stacks.push(InventoryItemStack {
            item_id,
            quantity: u32::try_from(stack_quantity)
                .map_err(|_| ItemRuleError::QuantityOverflow { item_id })?,
            expires_at_unix_ms,
        });
        quantity -= stack_quantity;
    }
    Ok(())
}

fn remove_from_stacks(
    stacks: &mut Vec<InventoryItemStack>,
    item_id: u32,
    mut quantity: u64,
) -> Result<(), ItemRuleError> {
    let available = count_item_quantity(stacks, item_id)?;
    if available < quantity {
        return Err(ItemRuleError::InsufficientItems {
            item_id,
            requested: quantity,
            available,
        });
    }
    while quantity > 0 {
        let index = stacks
            .iter()
            .enumerate()
            .filter(|(_, stack)| stack.item_id == item_id)
            .min_by_key(|(index, stack)| {
                (
                    stack.expires_at_unix_ms == 0,
                    stack.expires_at_unix_ms,
                    *index,
                )
            })
            .map(|(index, _)| index)
            .expect("item availability was checked before removal");
        let removed = quantity.min(u64::from(stacks[index].quantity));
        stacks[index].quantity -=
            u32::try_from(removed).map_err(|_| ItemRuleError::QuantityOverflow { item_id })?;
        quantity -= removed;
        if stacks[index].quantity == 0 {
            stacks.remove(index);
        }
    }
    Ok(())
}

fn remove_stack_quantity(
    inventory: &mut InventoryState,
    definitions: &(impl ItemDefinitionLookup + ?Sized),
    index: usize,
    quantity: u32,
) -> Result<u32, ItemRuleError> {
    let mut stacks = canonical_stacks(inventory, definitions)?;
    let stack = stacks
        .get(index)
        .ok_or(ItemRuleError::InvalidInventoryIndex {
            index: u32::try_from(index).unwrap_or(u32::MAX),
        })?;
    let item_id = stack.item_id;
    if quantity == 0 {
        return Err(ItemRuleError::InvalidQuantity { item_id });
    }
    if stack.quantity < quantity {
        return Err(ItemRuleError::InsufficientItems {
            item_id,
            requested: u64::from(quantity),
            available: u64::from(stack.quantity),
        });
    }
    stacks[index].quantity -= quantity;
    if stacks[index].quantity == 0 {
        stacks.remove(index);
    }
    inventory.stacks = stacks;
    Ok(item_id)
}

fn definition_stack_max(definition: &ItemDefinition) -> Result<u32, ItemRuleError> {
    (definition.stack_max > 0)
        .then_some(definition.stack_max)
        .ok_or(ItemRuleError::InvalidStackMaximum {
            item_id: definition.item_id,
        })
}

fn supported_equipment_slot(definition: &ItemDefinition) -> Option<EquipmentSlot> {
    if !definition.appearance_supported {
        return None;
    }
    EquipmentSlot::try_from(definition.slot)
        .ok()
        .filter(|slot| is_supported_slot(*slot))
}

fn is_supported_slot(slot: EquipmentSlot) -> bool {
    matches!(
        slot,
        EquipmentSlot::Top | EquipmentSlot::Bottom | EquipmentSlot::Shoes
    )
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

fn valid_inventory_index(
    inventory: &InventoryState,
    inventory_index: u32,
) -> Result<usize, ItemRuleError> {
    let index = inventory_index as usize;
    (index < inventory.stacks.len())
        .then_some(index)
        .ok_or(ItemRuleError::InvalidInventoryIndex {
            index: inventory_index,
        })
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
    use oozems_proto::v1::EquippedItem;
    use oozems_proto::v1::InventoryItemStack;
    use oozems_proto::v1::InventoryState;
    use oozems_proto::v1::ItemDefinition;
    use oozems_proto::v1::PlayerState;
    use oozems_proto::v1::Vec2;

    use super::DropStore;
    use super::DropStoreError;
    use super::ItemDefinitionLookup;
    use super::ItemRuleError;
    use super::PickUpError;
    use super::SPARE_TOP_ID;
    use super::STARTER_TOP_ID;
    use super::StagedDropGrant;
    use super::apply_item_delta;
    use super::apply_item_grant;
    use super::buy_shop_item;
    use super::canonicalize_inventory;
    use super::commit_new_staged_drops;
    use super::count_item_quantity;
    use super::create_drop;
    use super::create_drop_at;
    use super::create_mob_drops;
    use super::equip_inventory_item;
    use super::map_drops;
    use super::map_drops_at;
    use super::pick_up_nearest;
    use super::prune_and_validate_inventory;
    use super::prune_expired_inventory;
    use super::remove_inventory_item;
    use super::sell_inventory_item;
    use super::starter_inventory;
    use super::unequip_item;
    use super::validate_inventory;
    use super::validate_inventory_selection;
    use crate::interactions::ShopCurrency;

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
    fn stacks_are_merged_and_split_using_catalog_limits() {
        let definitions = vec![stackable_definition()];
        let mut inventory = InventoryState {
            stacks: (0..12)
                .map(|_| InventoryItemStack {
                    item_id: STACKABLE_ITEM_ID,
                    quantity: 1,
                    expires_at_unix_ms: 0,
                })
                .collect(),
            capacity: 2,
            ..InventoryState::default()
        };

        canonicalize_inventory(&mut inventory, &definitions).expect("canonicalize inventory");

        assert_eq!(
            inventory.stacks,
            vec![
                InventoryItemStack {
                    item_id: STACKABLE_ITEM_ID,
                    quantity: 10,
                    expires_at_unix_ms: 0,
                },
                InventoryItemStack {
                    item_id: STACKABLE_ITEM_ID,
                    quantity: 2,
                    expires_at_unix_ms: 0,
                },
            ]
        );
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
    fn persisted_inventory_must_already_match_the_current_catalog() {
        let definitions = vec![stackable_definition()];
        let inventory = InventoryState {
            capacity: 2,
            stacks: vec![
                InventoryItemStack {
                    item_id: STACKABLE_ITEM_ID,
                    quantity: 1,
                    expires_at_unix_ms: 0,
                },
                InventoryItemStack {
                    item_id: STACKABLE_ITEM_ID,
                    quantity: 1,
                    expires_at_unix_ms: 0,
                },
            ],
            ..InventoryState::default()
        };

        assert_eq!(
            validate_inventory(&inventory, &definitions),
            Err(ItemRuleError::NonCanonicalInventory)
        );

        let definitions = equipment_definitions();
        let mut inventory = starter_inventory();
        inventory.equipment[0].slot = EquipmentSlot::Bottom as i32;
        assert_eq!(
            validate_inventory(&inventory, &definitions),
            Err(ItemRuleError::InvalidEquipment {
                item_id: STARTER_TOP_ID,
            })
        );
    }

    #[test]
    fn expired_items_are_pruned_before_catalog_validation() {
        let mut inventory = InventoryState {
            capacity: 1,
            stacks: vec![InventoryItemStack {
                item_id: 999,
                quantity: 1,
                expires_at_unix_ms: 100,
            }],
            ..InventoryState::default()
        };

        assert_eq!(
            prune_and_validate_inventory(&mut inventory, &Vec::new(), 100),
            Ok(true)
        );
        assert!(inventory.stacks.is_empty());
    }

    #[test]
    fn item_deltas_merge_before_allocating_and_fail_atomically() {
        let definitions = vec![stackable_definition()];
        let mut inventory = InventoryState {
            capacity: 2,
            stacks: vec![InventoryItemStack {
                item_id: STACKABLE_ITEM_ID,
                quantity: 8,
                expires_at_unix_ms: 0,
            }],
            ..InventoryState::default()
        };

        apply_item_delta(&mut inventory, &definitions, STACKABLE_ITEM_ID, 5).expect("add quantity");
        assert_eq!(inventory.stacks[0].quantity, 10);
        assert_eq!(inventory.stacks[1].quantity, 3);
        assert_eq!(
            count_item_quantity(&inventory.stacks, STACKABLE_ITEM_ID),
            Ok(13)
        );

        let unchanged = inventory.clone();
        assert_eq!(
            apply_item_delta(&mut inventory, &definitions, STACKABLE_ITEM_ID, 8),
            Err(ItemRuleError::InventoryFull)
        );
        assert_eq!(inventory, unchanged);
        assert!(matches!(
            apply_item_delta(&mut inventory, &definitions, STACKABLE_ITEM_ID, -14),
            Err(ItemRuleError::InsufficientItems { .. })
        ));
        assert_eq!(inventory, unchanged);
    }

    #[test]
    fn invalid_item_deltas_are_rejected_without_mutation() {
        let definitions = vec![stackable_definition()];
        let mut inventory = InventoryState {
            capacity: 2,
            stacks: vec![InventoryItemStack {
                item_id: STACKABLE_ITEM_ID,
                quantity: 1,
                expires_at_unix_ms: 0,
            }],
            ..InventoryState::default()
        };
        let unchanged = inventory.clone();

        assert_eq!(
            apply_item_delta(&mut inventory, &definitions, STACKABLE_ITEM_ID, 0),
            Err(ItemRuleError::InvalidQuantity {
                item_id: STACKABLE_ITEM_ID,
            })
        );
        assert_eq!(
            apply_item_delta(&mut inventory, &definitions, 9_999_999, 1),
            Err(ItemRuleError::UnknownItem { item_id: 9_999_999 })
        );
        assert_eq!(
            apply_item_delta(&mut inventory, &definitions, STACKABLE_ITEM_ID, i64::MIN,),
            Err(ItemRuleError::QuantityOverflow {
                item_id: STACKABLE_ITEM_ID,
            })
        );
        assert_eq!(inventory, unchanged);
    }

    #[test]
    fn stack_expiration_is_part_of_stack_identity_and_capacity() {
        let definitions = vec![stackable_definition()];
        let mut inventory = InventoryState {
            capacity: 3,
            ..InventoryState::default()
        };

        apply_item_grant(&mut inventory, &definitions, STACKABLE_ITEM_ID, 5, 10_000)
            .expect("first expiring grant");
        apply_item_grant(&mut inventory, &definitions, STACKABLE_ITEM_ID, 7, 10_000)
            .expect("matching expiration grant");
        apply_item_grant(&mut inventory, &definitions, STACKABLE_ITEM_ID, 1, 20_000)
            .expect("different expiration grant");

        assert_eq!(
            inventory
                .stacks
                .iter()
                .map(|stack| (stack.quantity, stack.expires_at_unix_ms))
                .collect::<Vec<_>>(),
            vec![(10, 10_000), (2, 10_000), (1, 20_000)]
        );
        let unchanged = inventory.clone();
        assert_eq!(
            apply_item_grant(&mut inventory, &definitions, STACKABLE_ITEM_ID, 1, 30_000),
            Err(ItemRuleError::InventoryFull)
        );
        assert_eq!(inventory, unchanged);
    }

    #[test]
    fn item_removal_consumes_earliest_expiration_and_permanent_last() {
        let definitions = vec![stackable_definition()];
        let mut inventory = InventoryState {
            capacity: 3,
            stacks: vec![
                InventoryItemStack {
                    item_id: STACKABLE_ITEM_ID,
                    quantity: 2,
                    expires_at_unix_ms: 0,
                },
                InventoryItemStack {
                    item_id: STACKABLE_ITEM_ID,
                    quantity: 2,
                    expires_at_unix_ms: u64::MAX,
                },
                InventoryItemStack {
                    item_id: STACKABLE_ITEM_ID,
                    quantity: 2,
                    expires_at_unix_ms: 200,
                },
            ],
            ..InventoryState::default()
        };

        apply_item_delta(&mut inventory, &definitions, STACKABLE_ITEM_ID, -3)
            .expect("remove earliest items");

        assert_eq!(
            inventory
                .stacks
                .iter()
                .map(|stack| (stack.quantity, stack.expires_at_unix_ms))
                .collect::<Vec<_>>(),
            vec![(2, 0), (1, u64::MAX)]
        );
    }

    #[test]
    fn pruning_treats_deadline_equality_as_expired_and_reports_changes_once() {
        let mut inventory = InventoryState {
            stacks: vec![
                InventoryItemStack {
                    item_id: STACKABLE_ITEM_ID,
                    quantity: 1,
                    expires_at_unix_ms: 0,
                },
                InventoryItemStack {
                    item_id: STACKABLE_ITEM_ID,
                    quantity: 1,
                    expires_at_unix_ms: 1_000,
                },
                InventoryItemStack {
                    item_id: STACKABLE_ITEM_ID,
                    quantity: 1,
                    expires_at_unix_ms: 1_001,
                },
            ],
            equipment: vec![
                EquippedItem {
                    slot: EquipmentSlot::Top as i32,
                    item_id: STARTER_TOP_ID,
                    expires_at_unix_ms: 0,
                },
                EquippedItem {
                    slot: EquipmentSlot::Bottom as i32,
                    item_id: super::STARTER_BOTTOM_ID,
                    expires_at_unix_ms: 1_000,
                },
                EquippedItem {
                    slot: EquipmentSlot::Shoes as i32,
                    item_id: super::STARTER_SHOES_ID,
                    expires_at_unix_ms: 1_001,
                },
            ],
            ..InventoryState::default()
        };

        assert!(prune_expired_inventory(&mut inventory, 1_000));
        assert_eq!(inventory.stacks.len(), 2);
        assert_eq!(inventory.equipment.len(), 2);
        assert!(
            inventory
                .stacks
                .iter()
                .any(|stack| stack.expires_at_unix_ms == 0)
        );
        assert!(
            inventory
                .stacks
                .iter()
                .any(|stack| stack.expires_at_unix_ms == 1_001)
        );
        assert!(
            inventory
                .equipment
                .iter()
                .any(|equipped| equipped.expires_at_unix_ms == 0)
        );
        assert!(
            inventory
                .equipment
                .iter()
                .any(|equipped| equipped.expires_at_unix_ms == 1_001)
        );
        assert!(!prune_expired_inventory(&mut inventory, 1_000));
    }

    #[test]
    fn equipment_moves_one_item_between_inventory_and_its_slot() {
        let player = player();
        let definitions = equipment_definitions();

        let player = equip_inventory_item(player, 0, &definitions).expect("equip top");
        let inventory = player.inventory.as_ref().expect("inventory");
        assert_eq!(
            inventory.stacks.last().map(|stack| stack.item_id),
            Some(STARTER_TOP_ID)
        );
        assert!(inventory.equipment.iter().any(|item| {
            item.slot == EquipmentSlot::Top as i32 && item.item_id == SPARE_TOP_ID
        }));

        let player =
            unequip_item(player, EquipmentSlot::Top as i32, &definitions).expect("unequip top");
        let inventory = player.inventory.expect("inventory");
        assert!(
            !inventory
                .equipment
                .iter()
                .any(|item| item.slot == EquipmentSlot::Top as i32)
        );
        assert_eq!(
            inventory.stacks.last().map(|stack| stack.item_id),
            Some(SPARE_TOP_ID)
        );
    }

    #[test]
    fn equipment_replacement_unequip_and_drop_preserve_exact_deadlines() {
        let mut player = player();
        let inventory = player.inventory.as_mut().expect("inventory");
        inventory.stacks[0].expires_at_unix_ms = 5_000;
        inventory
            .equipment
            .iter_mut()
            .find(|equipped| equipped.slot == EquipmentSlot::Top as i32)
            .expect("starter top")
            .expires_at_unix_ms = 4_000;
        let definitions = equipment_definitions();

        let equipped = equip_inventory_item(player, 0, &definitions).expect("equip expiring top");
        let inventory = equipped.inventory.as_ref().expect("inventory");
        assert!(inventory.equipment.iter().any(|item| {
            item.slot == EquipmentSlot::Top as i32
                && item.item_id == SPARE_TOP_ID
                && item.expires_at_unix_ms == 5_000
        }));
        assert!(
            inventory.stacks.iter().any(|stack| {
                stack.item_id == STARTER_TOP_ID && stack.expires_at_unix_ms == 4_000
            })
        );

        let unequipped = unequip_item(equipped, EquipmentSlot::Top as i32, &definitions)
            .expect("unequip expiring top");
        let index = unequipped
            .inventory
            .as_ref()
            .expect("inventory")
            .stacks
            .iter()
            .position(|stack| stack.item_id == SPARE_TOP_ID && stack.expires_at_unix_ms == 5_000)
            .expect("unequipped expiring stack");
        let removed = remove_inventory_item(unequipped, index as u32, &definitions)
            .expect("drop expiring top");
        assert_eq!(removed.item_id, SPARE_TOP_ID);
        assert_eq!(removed.expires_at_unix_ms, 5_000);
    }

    #[test]
    fn equipment_replacement_and_unequip_fail_atomically_when_inventory_is_full() {
        let mut definitions = equipment_definitions();
        definitions
            .iter_mut()
            .find(|definition| definition.item_id == SPARE_TOP_ID)
            .expect("spare top definition")
            .stack_max = 2;
        let mut replacement = player();
        replacement.inventory.as_mut().expect("inventory").capacity = 1;
        replacement.inventory.as_mut().expect("inventory").stacks = vec![InventoryItemStack {
            item_id: SPARE_TOP_ID,
            quantity: 2,
            expires_at_unix_ms: 5_000,
        }];
        let unchanged = replacement.clone();

        assert_eq!(
            equip_inventory_item(replacement, 0, &definitions),
            Err(ItemRuleError::InventoryFull)
        );
        assert_eq!(
            unchanged.inventory.as_ref().expect("inventory").stacks[0].quantity,
            2
        );

        let mut unequip = player();
        unequip.inventory.as_mut().expect("inventory").capacity = 3;
        unequip.inventory.as_mut().expect("inventory").stacks = vec![
            InventoryItemStack {
                item_id: SPARE_TOP_ID,
                quantity: 1,
                expires_at_unix_ms: 1,
            },
            InventoryItemStack {
                item_id: super::SPARE_BOTTOM_ID,
                quantity: 1,
                expires_at_unix_ms: 2,
            },
            InventoryItemStack {
                item_id: super::SPARE_SHOES_ID,
                quantity: 1,
                expires_at_unix_ms: 3,
            },
        ];
        let unchanged = unequip.clone();
        assert_eq!(
            unequip_item(unequip, EquipmentSlot::Top as i32, &definitions),
            Err(ItemRuleError::InventoryFull)
        );
        assert_eq!(
            unchanged
                .inventory
                .as_ref()
                .expect("inventory")
                .equipment
                .len(),
            3
        );
    }

    #[test]
    fn equipment_replacement_fails_atomically_when_the_prior_item_is_invalid() {
        let definitions = equipment_definitions()
            .into_iter()
            .filter(|definition| definition.item_id != STARTER_TOP_ID)
            .collect::<Vec<_>>();

        assert_eq!(
            equip_inventory_item(player(), 0, &definitions),
            Err(ItemRuleError::UnknownItem {
                item_id: STARTER_TOP_ID,
            })
        );
    }

    #[test]
    fn unsupported_appearance_cannot_be_equipped() {
        let mut definitions = equipment_definitions();
        definitions
            .iter_mut()
            .find(|definition| definition.item_id == SPARE_TOP_ID)
            .expect("spare top definition")
            .appearance_supported = false;

        assert_eq!(
            equip_inventory_item(player(), 0, &definitions),
            Err(ItemRuleError::InvalidEquipment {
                item_id: SPARE_TOP_ID,
            })
        );
    }

    #[test]
    fn shop_transactions_exchange_single_stack_units_and_mesos() {
        let mut player = player();
        player.mesos = 200;
        player.inventory.as_mut().expect("inventory").stacks = vec![InventoryItemStack {
            item_id: STACKABLE_ITEM_ID,
            quantity: 2,
            expires_at_unix_ms: 0,
        }];
        let definitions = vec![ItemDefinition {
            sale_price: 40,
            ..stackable_definition()
        }];

        let bought = buy_shop_item(
            player,
            STACKABLE_ITEM_ID,
            100,
            ShopCurrency::Mesos,
            &definitions,
        )
        .expect("buy item");
        assert_eq!(bought.mesos, 100);
        assert_eq!(
            bought.inventory.as_ref().expect("inventory").stacks[0].quantity,
            3
        );

        let sold = sell_inventory_item(bought, 0, &definitions).expect("sell item");
        assert_eq!(sold.mesos, 140);
        assert_eq!(
            sold.inventory.as_ref().expect("inventory").stacks[0].quantity,
            2
        );
    }

    #[test]
    fn shops_grant_permanent_items_and_sell_the_selected_stack() {
        let mut player = player();
        player.mesos = 200;
        player.inventory = Some(InventoryState {
            capacity: 2,
            stacks: vec![InventoryItemStack {
                item_id: STACKABLE_ITEM_ID,
                quantity: 2,
                expires_at_unix_ms: 10_000,
            }],
            ..InventoryState::default()
        });
        let definitions = vec![ItemDefinition {
            sale_price: 40,
            ..stackable_definition()
        }];

        let bought = buy_shop_item(
            player,
            STACKABLE_ITEM_ID,
            100,
            ShopCurrency::Mesos,
            &definitions,
        )
        .expect("buy permanent item");
        let stacks = &bought.inventory.as_ref().expect("inventory").stacks;
        assert_eq!(stacks.len(), 2);
        assert_eq!(stacks[0].expires_at_unix_ms, 10_000);
        assert_eq!(stacks[1].expires_at_unix_ms, 0);

        let sold = sell_inventory_item(bought, 0, &definitions).expect("sell selected stack");
        let stacks = &sold.inventory.as_ref().expect("inventory").stacks;
        assert_eq!(stacks[0].quantity, 1);
        assert_eq!(stacks[0].expires_at_unix_ms, 10_000);
        assert_eq!(stacks[1].quantity, 1);
        assert_eq!(stacks[1].expires_at_unix_ms, 0);
    }

    #[test]
    fn cash_point_shop_purchases_only_debit_cash_points() {
        let mut player = player();
        player.mesos = 500;
        player.cash_points = 200;
        player.inventory.as_mut().expect("inventory").stacks.clear();
        let definitions = vec![stackable_definition()];

        let bought = buy_shop_item(
            player.clone(),
            STACKABLE_ITEM_ID,
            125,
            ShopCurrency::CashPoints,
            &definitions,
        )
        .expect("buy item with cash points");

        assert_eq!(bought.mesos, 500);
        assert_eq!(bought.cash_points, 75);
        assert_eq!(
            bought.inventory.as_ref().expect("inventory").stacks[0].quantity,
            1
        );
        player.cash_points = 124;
        assert_eq!(
            buy_shop_item(
                player,
                STACKABLE_ITEM_ID,
                125,
                ShopCurrency::CashPoints,
                &definitions,
            ),
            Err(ItemRuleError::InsufficientCashPoints)
        );
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

        super::restore_drop(&store, 100, first.clone(), None).expect("restore first drop");
        super::restore_drop(&store, 200, other.clone(), None).expect("restore other map drop");
        super::restore_drop(&store, 100, second.clone(), None).expect("restore second drop");

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
                super::restore_drop(&store, map_id, drop, None).expect("restore shared drop");
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
        let drop = oozems_proto::v1::DroppedItem {
            id: "zero-quantity-drop".to_owned(),
            item_id: STACKABLE_ITEM_ID,
            position: Some(Vec2 { x: 20.0, y: 30.0 }),
            despawn_at_unix_ms: u64::MAX,
            quantity: 0,
            expires_at_unix_ms: 0,
        };
        super::restore_drop(&store, 100, drop.clone(), None).expect("restore drop");
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
        let drop = oozems_proto::v1::DroppedItem {
            id: "quantity-drop".to_owned(),
            item_id: STACKABLE_ITEM_ID,
            position: Some(Vec2 { x: 20.0, y: 30.0 }),
            despawn_at_unix_ms: u64::MAX,
            quantity: 6,
            expires_at_unix_ms: 0,
        };
        super::restore_drop(&store, 100, drop, None).expect("restore drop");

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
            super::restore_drop(&store, 100, drop, None).expect("restore card drop");
            let picked = pick_up_nearest(&store, player, position, &catalog).expect("pick up card");
            player = picked.player;
            assert_eq!(player.monster_book_cards[0].count, expected);
            assert!(map_drops(&store, 100).expect("map drops").is_empty());
        }

        let capped_drop = card_drop("capped-card", position);
        super::restore_drop(&store, 100, capped_drop, None).expect("restore capped card");
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
        super::restore_drop(&store, 100, card_drop("full-card", position), None)
            .expect("restore card");

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
        super::restore_drop(
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

        super::restore_drop(
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
        let drop = oozems_proto::v1::DroppedItem {
            id: "expired-item-drop".to_owned(),
            item_id: STACKABLE_ITEM_ID,
            position: Some(Vec2 { x: 20.0, y: 30.0 }),
            despawn_at_unix_ms: u64::MAX,
            quantity: 1,
            expires_at_unix_ms: 1,
        };
        super::restore_drop(&store, 100, drop, None).expect("restore expired drop");
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
        let drop = oozems_proto::v1::DroppedItem {
            id: "full-inventory-drop".to_owned(),
            item_id: STACKABLE_ITEM_ID,
            position: Some(Vec2 { x: 20.0, y: 30.0 }),
            despawn_at_unix_ms: u64::MAX,
            quantity: 3,
            expires_at_unix_ms: 0,
        };
        super::restore_drop(&store, 100, drop.clone(), Some("local".to_owned()))
            .expect("restore drop");

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

    #[test]
    fn inventory_selection_identity_prevents_index_retargeting() {
        let player = player();
        let stack = &player.inventory.as_ref().expect("inventory").stacks[0];

        validate_inventory_selection(&player, 0, stack.item_id, stack.expires_at_unix_ms)
            .expect("matching selection");
        assert_eq!(
            validate_inventory_selection(&player, 0, stack.item_id + 1, stack.expires_at_unix_ms),
            Err(ItemRuleError::InventorySelectionChanged { index: 0 })
        );
        assert_eq!(
            validate_inventory_selection(&player, 0, stack.item_id, 123),
            Err(ItemRuleError::InventorySelectionChanged { index: 0 })
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

    fn equipment_definitions() -> Vec<ItemDefinition> {
        vec![
            equipment_definition(SPARE_TOP_ID, EquipmentSlot::Top),
            equipment_definition(STARTER_TOP_ID, EquipmentSlot::Top),
            equipment_definition(super::SPARE_BOTTOM_ID, EquipmentSlot::Bottom),
            equipment_definition(super::SPARE_SHOES_ID, EquipmentSlot::Shoes),
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
    ) -> oozems_proto::v1::DroppedItem {
        oozems_proto::v1::DroppedItem {
            id: id.to_owned(),
            item_id: CARD_ITEM_ID,
            position: Some(position),
            despawn_at_unix_ms: u64::MAX,
            quantity: 1,
            expires_at_unix_ms: 0,
        }
    }
}
