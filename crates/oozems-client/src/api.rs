use std::cell::Cell;
use std::cell::RefCell;

use gloo_net::http::Request;
use js_sys::Uint8Array;
use oozems_proto::PROTOBUF_CONTENT_TYPE;
use oozems_proto::v1::AbilityStat;
use oozems_proto::v1::ActiveBuffState;
use oozems_proto::v1::AllocateAbilityPointRequest;
use oozems_proto::v1::AllocateAbilityPointResponse;
use oozems_proto::v1::AllocateSkillPointRequest;
use oozems_proto::v1::AllocateSkillPointResponse;
use oozems_proto::v1::BasicAttackRequest;
use oozems_proto::v1::BasicAttackResponse;
use oozems_proto::v1::BootstrapRequest;
use oozems_proto::v1::BootstrapResponse;
use oozems_proto::v1::CharacterAppearance;
use oozems_proto::v1::CharacterSpriteSet;
use oozems_proto::v1::CreateCharacterRequest;
use oozems_proto::v1::CreateCharacterResponse;
use oozems_proto::v1::DropItemRequest;
use oozems_proto::v1::EnterPortalRequest;
use oozems_proto::v1::EquipItemRequest;
use oozems_proto::v1::EquippedItem;
use oozems_proto::v1::ErrorResponse;
use oozems_proto::v1::GameGui;
use oozems_proto::v1::GameplaySessionGrant;
use oozems_proto::v1::GetCashShopRequest;
use oozems_proto::v1::GetCashShopResponse;
use oozems_proto::v1::GetCharacterSpritesRequest;
use oozems_proto::v1::GetCharacterSpritesResponse;
use oozems_proto::v1::GetGuiRequest;
use oozems_proto::v1::GetGuiResponse;
use oozems_proto::v1::GetMapRequest;
use oozems_proto::v1::GetMapResponse;
use oozems_proto::v1::GetMorphRequest;
use oozems_proto::v1::GetMorphResponse;
use oozems_proto::v1::GetMovementRulesRequest;
use oozems_proto::v1::GetMovementRulesResponse;
use oozems_proto::v1::GetSkillBookRequest;
use oozems_proto::v1::GetSkillBookResponse;
use oozems_proto::v1::ItemActionResponse;
use oozems_proto::v1::KeyBinding;
use oozems_proto::v1::Map;
use oozems_proto::v1::MovementRules;
use oozems_proto::v1::MovementSnapshot;
use oozems_proto::v1::MovementUpdateResponse;
use oozems_proto::v1::NpcInteractionRequest;
use oozems_proto::v1::NpcInteractionResponse;
use oozems_proto::v1::PickUpItemRequest;
use oozems_proto::v1::PlayerState;
use oozems_proto::v1::PurchaseCashShopItemRequest;
use oozems_proto::v1::PurchaseCashShopItemResponse;
use oozems_proto::v1::RecoverPlayerRequest;
use oozems_proto::v1::RecoverPlayerResponse;
use oozems_proto::v1::RespawnPlayerRequest;
use oozems_proto::v1::RespawnPlayerResponse;
use oozems_proto::v1::SavePlayerRequest;
use oozems_proto::v1::SavePlayerResponse;
use oozems_proto::v1::SkillBook;
use oozems_proto::v1::StartingEquipmentSelection;
use oozems_proto::v1::SubmitMovementRequest;
use oozems_proto::v1::UnequipItemRequest;
use oozems_proto::v1::UseItemRequest;
use oozems_proto::v1::UseSkillRequest;
use oozems_proto::v1::UseSkillResponse;
use prost::Message;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("request failed: {0}")]
    Network(String),
    #[error("server returned {status}: {code}: {message}")]
    Server {
        status: u16,
        code: String,
        message: String,
    },
    #[error("invalid protobuf response: {0}")]
    InvalidResponse(String),
    #[error("response did not contain {0}")]
    MissingData(&'static str),
    #[error("the gameplay session is no longer current")]
    GameplaySessionInvalidated,
    #[error("player state requires server reconciliation; restart the server and reload the game")]
    PlayerReconciliationRequired,
}

impl ClientError {
    pub(crate) fn operation_outcome_unknown(&self) -> bool {
        matches!(self, Self::Network(_) | Self::InvalidResponse(_))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ClientRecovery {
    Bootstrap,
    ServerRestart,
}

pub struct LoadedSkillBook {
    pub skill_book: SkillBook,
    pub active_buffs: ActiveBuffState,
}

thread_local! {
    static GAMEPLAY_SESSION: RefCell<Option<String>> = const { RefCell::new(None) };
    static RECOVERY_REQUIRED: Cell<Option<ClientRecovery>> = const { Cell::new(None) };
}

pub(crate) fn require_data<T>(
    data: Option<T>,
    name: &'static str,
) -> Result<T, ClientError> {
    data.ok_or(ClientError::MissingData(name))
}

pub async fn bootstrap(player_id: &str) -> Result<BootstrapResponse, ClientError> {
    let mut response: BootstrapResponse = post_protobuf(
        "/api/v1/bootstrap",
        BootstrapRequest {
            player_id: player_id.to_owned(),
        },
    )
    .await?;
    let grant = require_data(response.gameplay_session.take(), "gameplay session")?;
    install_gameplay_session(grant, player_id)?;
    Ok(response)
}

fn install_gameplay_session(
    grant: GameplaySessionGrant,
    expected_player_id: &str,
) -> Result<(), ClientError> {
    if grant.player_id != expected_player_id || grant.token.is_empty() {
        return Err(ClientError::InvalidResponse(
            "gameplay session does not match the bootstrap request".to_owned(),
        ));
    }
    GAMEPLAY_SESSION.with(|current| {
        *current.borrow_mut() = Some(format!("Bearer {}", grant.token));
    });
    RECOVERY_REQUIRED.with(|required| required.set(None));
    Ok(())
}

pub(crate) fn take_recovery_requirement() -> Option<ClientRecovery> {
    RECOVERY_REQUIRED.with(|required| required.replace(None))
}

fn require_recovery(recovery: ClientRecovery) {
    RECOVERY_REQUIRED.with(|required| {
        if required.get() != Some(ClientRecovery::ServerRestart) {
            required.set(Some(recovery));
        }
    });
}

pub async fn create_character(
    player_id: &str,
    name: &str,
    appearance: CharacterAppearance,
    equipment: Vec<StartingEquipmentSelection>,
) -> Result<PlayerState, ClientError> {
    let response: CreateCharacterResponse = post_protobuf(
        "/api/v1/characters/create",
        CreateCharacterRequest {
            player_id: player_id.to_owned(),
            name: name.to_owned(),
            appearance: Some(appearance),
            equipment,
        },
    )
    .await?;

    require_data(response.player, "player")
}

pub async fn get_character_sprites(
    appearance: CharacterAppearance,
    equipment: Option<&[EquippedItem]>,
) -> Result<CharacterSpriteSet, ClientError> {
    let use_starter_equipment = equipment.is_none();
    let response: GetCharacterSpritesResponse = post_protobuf(
        "/api/v1/characters/sprites",
        GetCharacterSpritesRequest {
            appearance: Some(appearance),
            equipment: equipment.unwrap_or_default().to_vec(),
            use_starter_equipment,
        },
    )
    .await?;

    require_data(response.sprites, "character sprites")
}

pub async fn get_morph(morph_id: u32) -> Result<oozems_proto::v1::MorphDefinition, ClientError> {
    let response: GetMorphResponse =
        post_protobuf("/api/v1/morphs/get", GetMorphRequest { morph_id }).await?;
    require_data(response.morph, "morph")
}

pub async fn equip_item(
    player_id: &str,
    inventory_index: u32,
    expected_item_id: u32,
    expected_expires_at_unix_ms: u64,
) -> Result<ItemActionResponse, ClientError> {
    post_protobuf(
        "/api/v1/items/equip",
        EquipItemRequest {
            player_id: player_id.to_owned(),
            inventory_index,
            expected_item_id,
            expected_expires_at_unix_ms,
        },
    )
    .await
}

pub async fn unequip_item(
    player_id: &str,
    slot: i32,
) -> Result<ItemActionResponse, ClientError> {
    post_protobuf(
        "/api/v1/items/unequip",
        UnequipItemRequest {
            player_id: player_id.to_owned(),
            slot,
        },
    )
    .await
}

pub async fn drop_item(
    player_id: &str,
    inventory_index: u32,
    expected_item_id: u32,
    expected_expires_at_unix_ms: u64,
) -> Result<ItemActionResponse, ClientError> {
    post_protobuf(
        "/api/v1/items/drop",
        DropItemRequest {
            player_id: player_id.to_owned(),
            inventory_index,
            expected_item_id,
            expected_expires_at_unix_ms,
        },
    )
    .await
}

pub async fn use_item(
    player_id: &str,
    inventory_index: u32,
    expected_item_id: u32,
    expected_expires_at_unix_ms: u64,
) -> Result<ItemActionResponse, ClientError> {
    post_protobuf(
        "/api/v1/items/use",
        UseItemRequest {
            player_id: player_id.to_owned(),
            inventory_index,
            expected_item_id,
            expected_expires_at_unix_ms,
        },
    )
    .await
}

pub async fn pick_up_item(player_id: &str) -> Result<ItemActionResponse, ClientError> {
    post_protobuf(
        "/api/v1/items/pick-up",
        PickUpItemRequest {
            player_id: player_id.to_owned(),
        },
    )
    .await
}

pub async fn interact_npc(
    request: NpcInteractionRequest
) -> Result<NpcInteractionResponse, ClientError> {
    post_protobuf("/api/v1/npcs/interact", request).await
}

pub async fn get_map(
    player_id: &str,
    map_id: u32,
) -> Result<Map, ClientError> {
    let response: GetMapResponse = post_protobuf(
        "/api/v1/maps/get",
        GetMapRequest {
            map_id,
            player_id: player_id.to_owned(),
        },
    )
    .await?;

    require_data(response.map, "map")
}

pub async fn get_movement_rules() -> Result<MovementRules, ClientError> {
    let response: GetMovementRulesResponse =
        post_protobuf("/api/v1/movement/rules", GetMovementRulesRequest {}).await?;

    require_data(response.rules, "movement rules")
}

pub async fn submit_movement(
    player_id: &str,
    snapshot: MovementSnapshot,
) -> Result<MovementUpdateResponse, ClientError> {
    post_protobuf(
        "/api/v1/movement/submit",
        SubmitMovementRequest {
            player_id: player_id.to_owned(),
            snapshot: Some(snapshot),
        },
    )
    .await
}

pub async fn enter_portal(
    player_id: &str,
    source: MovementSnapshot,
    target_map_id: u32,
    target_portal_name: &str,
) -> Result<MovementUpdateResponse, ClientError> {
    post_protobuf(
        "/api/v1/movement/portal",
        EnterPortalRequest {
            player_id: player_id.to_owned(),
            source: Some(source),
            target_map_id,
            target_portal_name: target_portal_name.to_owned(),
        },
    )
    .await
}

pub async fn get_gui(
    player_id: &str,
    observed_item_ids: Vec<u32>,
) -> Result<GameGui, ClientError> {
    let response: GetGuiResponse = post_protobuf(
        "/api/v1/gui/get",
        GetGuiRequest {
            player_id: player_id.to_owned(),
            observed_item_ids,
        },
    )
    .await?;

    require_data(response.gui, "game GUI")
}

pub async fn get_cash_shop() -> Result<GetCashShopResponse, ClientError> {
    post_protobuf("/api/v1/cash-shop/get", GetCashShopRequest {}).await
}

pub async fn purchase_cash_shop_item(
    player_id: &str,
    offer_id: u32,
) -> Result<PurchaseCashShopItemResponse, ClientError> {
    post_protobuf(
        "/api/v1/cash-shop/purchase",
        PurchaseCashShopItemRequest {
            player_id: player_id.to_owned(),
            offer_id,
        },
    )
    .await
}

pub async fn get_skill_book(player_id: &str) -> Result<LoadedSkillBook, ClientError> {
    let response: GetSkillBookResponse = post_protobuf(
        "/api/v1/skills/book",
        GetSkillBookRequest {
            player_id: player_id.to_owned(),
        },
    )
    .await?;

    let skill_book = require_data(response.skill_book, "skill book")?;
    let active_buffs = require_data(response.active_buffs, "active buffs")?;
    Ok(LoadedSkillBook {
        skill_book,
        active_buffs,
    })
}

pub async fn allocate_skill_point(
    player_id: &str,
    skill_id: u32,
) -> Result<AllocateSkillPointResponse, ClientError> {
    post_protobuf(
        "/api/v1/skills/allocate",
        AllocateSkillPointRequest {
            player_id: player_id.to_owned(),
            skill_id,
        },
    )
    .await
}

pub async fn allocate_ability_point(
    player_id: &str,
    stat: AbilityStat,
) -> Result<AllocateAbilityPointResponse, ClientError> {
    post_protobuf(
        "/api/v1/abilities/allocate",
        AllocateAbilityPointRequest {
            player_id: player_id.to_owned(),
            stat: stat as i32,
        },
    )
    .await
}

pub async fn use_skill(
    player_id: &str,
    skill_id: u32,
    target_mob_id: &str,
    facing_left: bool,
    movement: MovementSnapshot,
) -> Result<UseSkillResponse, ClientError> {
    post_protobuf(
        "/api/v1/skills/use",
        UseSkillRequest {
            player_id: player_id.to_owned(),
            skill_id,
            target_mob_id: target_mob_id.to_owned(),
            facing_left,
            movement: Some(movement),
        },
    )
    .await
}

pub async fn use_basic_attack(
    player_id: &str,
    facing_left: bool,
    movement: MovementSnapshot,
) -> Result<BasicAttackResponse, ClientError> {
    post_protobuf(
        "/api/v1/combat/basic-attack",
        BasicAttackRequest {
            player_id: player_id.to_owned(),
            facing_left,
            movement: Some(movement),
        },
    )
    .await
}

pub async fn recover_player(player_id: &str) -> Result<RecoverPlayerResponse, ClientError> {
    post_protobuf(
        "/api/v1/players/recover",
        RecoverPlayerRequest {
            player_id: player_id.to_owned(),
        },
    )
    .await
}

pub async fn respawn_player(player_id: &str) -> Result<RespawnPlayerResponse, ClientError> {
    post_protobuf(
        "/api/v1/players/respawn",
        RespawnPlayerRequest {
            player_id: player_id.to_owned(),
        },
    )
    .await
}

pub async fn save_player(
    player_id: &str,
    key_bindings: Vec<KeyBinding>,
) -> Result<SavePlayerResponse, ClientError> {
    post_protobuf(
        "/api/v1/players/save",
        SavePlayerRequest {
            player_id: player_id.to_owned(),
            key_bindings,
        },
    )
    .await
}

async fn post_protobuf<I, O>(
    url: &str,
    input: I,
) -> Result<O, ClientError>
where
    I: Message,
    O: Message + Default,
{
    let encoded = input.encode_to_vec();
    let body = Uint8Array::from(encoded.as_slice());
    let mut request = Request::post(url)
        .header("Content-Type", PROTOBUF_CONTENT_TYPE)
        .header("Accept", PROTOBUF_CONTENT_TYPE);
    if let Some(authorization) = GAMEPLAY_SESSION.with(|current| current.borrow().clone()) {
        request = request.header("Authorization", &authorization);
    }
    let request = request
        .body(body)
        .map_err(|error| ClientError::Network(error.to_string()))?;
    let response = request
        .send()
        .await
        .map_err(|error| ClientError::Network(error.to_string()))?;
    let status = response.status();
    let bytes = response
        .binary()
        .await
        .map_err(|error| ClientError::Network(error.to_string()))?;

    if !response.ok() {
        let error = ErrorResponse::decode(bytes.as_slice()).map_err(|decode_error| {
            ClientError::InvalidResponse(format!(
                "HTTP {status} error was not valid protobuf: {decode_error}"
            ))
        })?;
        if error.code == "invalid_gameplay_session" {
            require_recovery(ClientRecovery::Bootstrap);
            return Err(ClientError::GameplaySessionInvalidated);
        }
        if error.code == "player_reconciliation_required" {
            require_recovery(ClientRecovery::ServerRestart);
            return Err(ClientError::PlayerReconciliationRequired);
        }
        return Err(ClientError::Server {
            status,
            code: error.code,
            message: error.message,
        });
    }

    O::decode(bytes.as_slice()).map_err(|error| ClientError::InvalidResponse(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::ClientRecovery;
    use super::RECOVERY_REQUIRED;
    use super::require_recovery;
    use super::take_recovery_requirement;

    #[test]
    fn server_restart_recovery_takes_precedence_over_bootstrap() {
        RECOVERY_REQUIRED.with(|required| required.set(None));

        require_recovery(ClientRecovery::Bootstrap);
        require_recovery(ClientRecovery::ServerRestart);
        require_recovery(ClientRecovery::Bootstrap);

        assert_eq!(
            take_recovery_requirement(),
            Some(ClientRecovery::ServerRestart)
        );
        assert_eq!(take_recovery_requirement(), None);
    }
}
