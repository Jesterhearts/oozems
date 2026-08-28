use oozems_proto::v1::CharacterEquipmentOption;
use oozems_proto::v1::EquipmentSlot;
use oozems_proto::v1::EquippedItem;
use oozems_proto::v1::InventoryItemStack;
use oozems_proto::v1::InventoryState;
use oozems_proto::v1::ItemCategory;
use oozems_proto::v1::ItemDefinition;
use oozems_proto::v1::PlayerState;
use oozems_proto::v1::StartingEquipmentSelection;
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
pub const STARTING_TOP_IDS: [u32; 2] = [1_040_002, 1_040_003];
pub const STARTING_BOTTOM_IDS: [u32; 2] = [1_060_002, 1_060_001];
pub const STARTING_SHOES_IDS: [u32; 3] = [1_072_001, 1_072_005, 1_072_038];
pub const STARTING_WEAPON_IDS: [u32; 3] = [1_302_000, 1_312_004, 1_322_005];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EquipmentStats {
    pub weapon_attack: i32,
    pub weapon_defense: i32,
    pub magic_defense: i32,
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

#[derive(Clone, Debug, PartialEq)]
pub struct UsedInventoryItem {
    pub item_id: u32,
    pub category: ItemCategory,
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
    #[error("item {item_id} cannot be used")]
    UnusableItem { item_id: u32 },
    #[error("equipment slot is invalid")]
    InvalidEquipmentSlot,
    #[error("the starter equipment selection must contain one supported item for every slot")]
    InvalidStarterEquipment,
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

pub fn equipment_stats(
    player: &PlayerState,
    definitions: &(impl ItemDefinitionLookup + ?Sized),
) -> Result<EquipmentStats, ItemRuleError> {
    let inventory = player
        .inventory
        .as_ref()
        .ok_or(ItemRuleError::MissingInventory)?;
    inventory
        .equipment
        .iter()
        .try_fold(EquipmentStats::default(), |mut stats, equipped| {
            let definition = find_definition(definitions, equipped.item_id)?;
            let slot =
                supported_equipment_slot(definition).ok_or(ItemRuleError::InvalidEquipment {
                    item_id: equipped.item_id,
                })?;
            if equipped.slot != slot as i32 {
                return Err(ItemRuleError::InvalidEquipment {
                    item_id: equipped.item_id,
                });
            }
            stats.weapon_attack = stats
                .weapon_attack
                .saturating_add(bounded_equipment_stat(definition.weapon_attack));
            stats.weapon_defense = stats
                .weapon_defense
                .saturating_add(bounded_equipment_stat(definition.weapon_defense));
            stats.magic_defense = stats
                .magic_defense
                .saturating_add(bounded_equipment_stat(definition.magic_defense));
            Ok(stats)
        })
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

#[cfg(test)]
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

pub fn default_starter_equipment() -> Vec<EquippedItem> {
    [
        (EquipmentSlot::Top, STARTING_TOP_IDS[0]),
        (EquipmentSlot::Bottom, STARTING_BOTTOM_IDS[0]),
        (EquipmentSlot::Shoes, STARTING_SHOES_IDS[0]),
        (EquipmentSlot::Weapon, STARTING_WEAPON_IDS[0]),
    ]
    .map(|(slot, item_id)| EquippedItem {
        slot: slot as i32,
        item_id,
        expires_at_unix_ms: 0,
    })
    .into()
}

pub fn selected_starter_inventory(
    selections: &[StartingEquipmentSelection]
) -> Result<InventoryState, ItemRuleError> {
    let expected = [
        (EquipmentSlot::Top, STARTING_TOP_IDS.as_slice()),
        (EquipmentSlot::Bottom, STARTING_BOTTOM_IDS.as_slice()),
        (EquipmentSlot::Shoes, STARTING_SHOES_IDS.as_slice()),
        (EquipmentSlot::Weapon, STARTING_WEAPON_IDS.as_slice()),
    ];
    if selections.len() != expected.len() {
        return Err(ItemRuleError::InvalidStarterEquipment);
    }

    let equipment = expected
        .into_iter()
        .map(|(slot, allowed_ids)| {
            let mut matching = selections
                .iter()
                .filter(|selection| selection.slot == slot as i32);
            let selection = matching
                .next()
                .filter(|_| matching.next().is_none())
                .filter(|selection| allowed_ids.contains(&selection.item_id))
                .ok_or(ItemRuleError::InvalidStarterEquipment)?;
            Ok(EquippedItem {
                slot: slot as i32,
                item_id: selection.item_id,
                expires_at_unix_ms: 0,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(InventoryState {
        equipment,
        capacity: INVENTORY_CAPACITY,
        stacks: Vec::new(),
    })
}

pub fn starter_equipment_options(definitions: &[ItemDefinition]) -> Vec<CharacterEquipmentOption> {
    [
        (EquipmentSlot::Top, STARTING_TOP_IDS.as_slice()),
        (EquipmentSlot::Bottom, STARTING_BOTTOM_IDS.as_slice()),
        (EquipmentSlot::Shoes, STARTING_SHOES_IDS.as_slice()),
        (EquipmentSlot::Weapon, STARTING_WEAPON_IDS.as_slice()),
    ]
    .into_iter()
    .flat_map(|(slot, item_ids)| {
        item_ids.iter().filter_map(move |item_id| {
            definitions
                .iter()
                .find(|definition| definition.item_id == *item_id)
                .map(|definition| CharacterEquipmentOption {
                    item_id: *item_id,
                    label: definition.name.clone(),
                    slot: slot as i32,
                })
        })
    })
    .collect()
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

pub fn use_inventory_item(
    mut player: PlayerState,
    inventory_index: u32,
    definitions: &(impl ItemDefinitionLookup + ?Sized),
) -> Result<UsedInventoryItem, ItemRuleError> {
    let inventory = player
        .inventory
        .as_mut()
        .ok_or(ItemRuleError::MissingInventory)?;
    let mut next = inventory.clone();
    canonicalize_inventory(&mut next, definitions)?;
    let index = valid_inventory_index(&next, inventory_index)?;
    let item_id = next.stacks[index].item_id;
    let definition = find_definition(definitions, item_id)?;
    let category = ItemCategory::try_from(definition.category)
        .ok()
        .filter(|category| matches!(category, ItemCategory::Consume | ItemCategory::Install))
        .filter(|_| definition.usable)
        .ok_or(ItemRuleError::UnusableItem { item_id })?;
    if category == ItemCategory::Consume {
        remove_stack_quantity(&mut next, definitions, index, 1)?;
    }
    *inventory = next;
    Ok(UsedInventoryItem {
        item_id,
        category,
        player,
    })
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

fn bounded_equipment_stat(value: u32) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}

fn supported_equipment_slot(definition: &ItemDefinition) -> Option<EquipmentSlot> {
    if !definition.appearance_supported {
        return None;
    }
    EquipmentSlot::try_from(definition.slot)
        .ok()
        .filter(|slot| is_supported_slot(*slot))
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

fn is_supported_slot(slot: EquipmentSlot) -> bool {
    matches!(
        slot,
        EquipmentSlot::Top | EquipmentSlot::Bottom | EquipmentSlot::Shoes | EquipmentSlot::Weapon
    )
}

#[cfg(test)]
mod tests {
    use oozems_proto::v1::EquipmentSlot;
    use oozems_proto::v1::EquippedItem;
    use oozems_proto::v1::InventoryItemStack;
    use oozems_proto::v1::InventoryState;
    use oozems_proto::v1::ItemCategory;
    use oozems_proto::v1::ItemDefinition;
    use oozems_proto::v1::PlayerState;
    use oozems_proto::v1::StartingEquipmentSelection;
    use oozems_proto::v1::Vec2;

    use super::ItemRuleError;
    use super::SPARE_BOTTOM_ID;
    use super::SPARE_SHOES_ID;
    use super::SPARE_TOP_ID;
    use super::STARTER_BOTTOM_ID;
    use super::STARTER_SHOES_ID;
    use super::STARTER_TOP_ID;
    use super::STARTING_BOTTOM_IDS;
    use super::STARTING_SHOES_IDS;
    use super::STARTING_TOP_IDS;
    use super::STARTING_WEAPON_IDS;
    use super::apply_item_delta;
    use super::apply_item_grant;
    use super::buy_shop_item;
    use super::canonicalize_inventory;
    use super::count_item_quantity;
    use super::equip_inventory_item;
    use super::equipment_stats;
    use super::prune_and_validate_inventory;
    use super::prune_expired_inventory;
    use super::remove_inventory_item;
    use super::selected_starter_inventory;
    use super::sell_inventory_item;
    use super::starter_inventory;
    use super::unequip_item;
    use super::use_inventory_item;
    use super::validate_inventory;
    use super::validate_inventory_selection;
    use crate::interactions::ShopCurrency;

    const STACKABLE_ITEM_ID: u32 = 2_000_000;

    #[test]
    fn selected_starter_inventory_equips_exactly_one_item_per_slot() {
        let selections = [
            selection(EquipmentSlot::Top, STARTING_TOP_IDS[1]),
            selection(EquipmentSlot::Bottom, STARTING_BOTTOM_IDS[1]),
            selection(EquipmentSlot::Shoes, STARTING_SHOES_IDS[2]),
            selection(EquipmentSlot::Weapon, STARTING_WEAPON_IDS[1]),
        ];

        let inventory = selected_starter_inventory(&selections).expect("valid starter equipment");

        assert_eq!(inventory.capacity, super::INVENTORY_CAPACITY);
        assert!(inventory.stacks.is_empty());
        assert_eq!(inventory.equipment.len(), 4);
        for selection in selections {
            assert!(inventory.equipment.iter().any(|equipped| {
                equipped.slot == selection.slot && equipped.item_id == selection.item_id
            }));
        }
    }

    #[test]
    fn selected_starter_inventory_rejects_missing_duplicate_and_unsupported_items() {
        let valid = [
            selection(EquipmentSlot::Top, STARTING_TOP_IDS[0]),
            selection(EquipmentSlot::Bottom, STARTING_BOTTOM_IDS[0]),
            selection(EquipmentSlot::Shoes, STARTING_SHOES_IDS[0]),
            selection(EquipmentSlot::Weapon, STARTING_WEAPON_IDS[0]),
        ];
        assert_eq!(
            selected_starter_inventory(&valid[..3]),
            Err(ItemRuleError::InvalidStarterEquipment)
        );

        let mut duplicate = valid;
        duplicate[3] = duplicate[0];
        assert_eq!(
            selected_starter_inventory(&duplicate),
            Err(ItemRuleError::InvalidStarterEquipment)
        );

        let mut unsupported = valid;
        unsupported[3].item_id = 1;
        assert_eq!(
            selected_starter_inventory(&unsupported),
            Err(ItemRuleError::InvalidStarterEquipment)
        );
    }

    fn selection(
        slot: EquipmentSlot,
        item_id: u32,
    ) -> StartingEquipmentSelection {
        StartingEquipmentSelection {
            slot: slot as i32,
            item_id,
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
                    item_id: STARTER_BOTTOM_ID,
                    expires_at_unix_ms: 1_000,
                },
                EquippedItem {
                    slot: EquipmentSlot::Shoes as i32,
                    item_id: STARTER_SHOES_ID,
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
    fn using_consumables_removes_one_but_setup_items_remain() {
        const CONSUME_ID: u32 = 2_022_070;
        const SETUP_ID: u32 = 3_010_072;
        let definitions = [
            ItemDefinition {
                item_id: CONSUME_ID,
                category: ItemCategory::Consume as i32,
                stack_max: 100,
                usable: true,
                ..ItemDefinition::default()
            },
            ItemDefinition {
                item_id: SETUP_ID,
                category: ItemCategory::Install as i32,
                stack_max: 1,
                usable: true,
                ..ItemDefinition::default()
            },
        ];
        let player = PlayerState {
            inventory: Some(InventoryState {
                capacity: 24,
                stacks: vec![
                    InventoryItemStack {
                        item_id: CONSUME_ID,
                        quantity: 2,
                        expires_at_unix_ms: 0,
                    },
                    InventoryItemStack {
                        item_id: SETUP_ID,
                        quantity: 1,
                        expires_at_unix_ms: 0,
                    },
                ],
                ..InventoryState::default()
            }),
            ..PlayerState::default()
        };

        let consumed = use_inventory_item(player, 0, &definitions).expect("use consumable");
        assert_eq!(consumed.item_id, CONSUME_ID);
        assert_eq!(consumed.category, ItemCategory::Consume);
        assert_eq!(
            consumed
                .player
                .inventory
                .as_ref()
                .expect("inventory")
                .stacks[0]
                .quantity,
            1
        );

        let setup = use_inventory_item(consumed.player, 1, &definitions).expect("use setup item");
        assert_eq!(setup.item_id, SETUP_ID);
        assert_eq!(setup.category, ItemCategory::Install);
        assert_eq!(setup.player.inventory.expect("inventory").stacks.len(), 2);
    }

    #[test]
    fn item_use_rejects_categories_and_definitions_without_behavior() {
        let player = PlayerState {
            inventory: Some(InventoryState {
                capacity: 1,
                stacks: vec![InventoryItemStack {
                    item_id: STACKABLE_ITEM_ID,
                    quantity: 1,
                    expires_at_unix_ms: 0,
                }],
                ..InventoryState::default()
            }),
            ..PlayerState::default()
        };
        let unusable_consume = [ItemDefinition {
            item_id: STACKABLE_ITEM_ID,
            category: ItemCategory::Consume as i32,
            stack_max: 10,
            ..ItemDefinition::default()
        }];
        assert_eq!(
            use_inventory_item(player.clone(), 0, &unusable_consume),
            Err(ItemRuleError::UnusableItem {
                item_id: STACKABLE_ITEM_ID
            })
        );

        let etc = [ItemDefinition {
            item_id: STACKABLE_ITEM_ID,
            category: ItemCategory::Etc as i32,
            stack_max: 10,
            usable: true,
            ..ItemDefinition::default()
        }];
        assert_eq!(
            use_inventory_item(player, 0, &etc),
            Err(ItemRuleError::UnusableItem {
                item_id: STACKABLE_ITEM_ID
            })
        );
    }

    #[test]
    fn weapon_slots_are_equippable_and_equipment_stats_are_aggregated() {
        const WEAPON_ID: u32 = 1_302_000;
        let definitions = [
            ItemDefinition {
                weapon_attack: 17,
                ..equipment_definition(WEAPON_ID, EquipmentSlot::Weapon)
            },
            ItemDefinition {
                weapon_defense: 8,
                magic_defense: 5,
                ..equipment_definition(STARTER_TOP_ID, EquipmentSlot::Top)
            },
        ];
        let player = PlayerState {
            inventory: Some(InventoryState {
                equipment: vec![
                    EquippedItem {
                        slot: EquipmentSlot::Weapon as i32,
                        item_id: WEAPON_ID,
                        expires_at_unix_ms: 0,
                    },
                    EquippedItem {
                        slot: EquipmentSlot::Top as i32,
                        item_id: STARTER_TOP_ID,
                        expires_at_unix_ms: 0,
                    },
                ],
                ..InventoryState::default()
            }),
            ..PlayerState::default()
        };

        assert_eq!(
            equipment_stats(&player, &definitions),
            Ok(super::EquipmentStats {
                weapon_attack: 17,
                weapon_defense: 8,
                magic_defense: 5,
            })
        );

        let player = PlayerState {
            inventory: Some(InventoryState {
                capacity: 1,
                stacks: vec![InventoryItemStack {
                    item_id: WEAPON_ID,
                    quantity: 1,
                    expires_at_unix_ms: 0,
                }],
                ..InventoryState::default()
            }),
            ..PlayerState::default()
        };
        let equipped = equip_inventory_item(player, 0, &definitions).expect("equip weapon");
        assert_eq!(
            equipped.inventory.expect("inventory").equipment,
            vec![EquippedItem {
                slot: EquipmentSlot::Weapon as i32,
                item_id: WEAPON_ID,
                expires_at_unix_ms: 0,
            }]
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
                item_id: SPARE_BOTTOM_ID,
                quantity: 1,
                expires_at_unix_ms: 2,
            },
            InventoryItemStack {
                item_id: SPARE_SHOES_ID,
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
}
