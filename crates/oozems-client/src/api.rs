use gloo_net::http::Request;
use js_sys::Uint8Array;
use oozems_proto::PROTOBUF_CONTENT_TYPE;
use oozems_proto::v1::BootstrapRequest;
use oozems_proto::v1::BootstrapResponse;
use oozems_proto::v1::CharacterAppearance;
use oozems_proto::v1::CharacterSpriteSet;
use oozems_proto::v1::CreateCharacterRequest;
use oozems_proto::v1::CreateCharacterResponse;
use oozems_proto::v1::DropItemRequest;
use oozems_proto::v1::EquipItemRequest;
use oozems_proto::v1::EquippedItem;
use oozems_proto::v1::ErrorResponse;
use oozems_proto::v1::GameGui;
use oozems_proto::v1::GetCharacterSpritesRequest;
use oozems_proto::v1::GetCharacterSpritesResponse;
use oozems_proto::v1::GetGuiRequest;
use oozems_proto::v1::GetGuiResponse;
use oozems_proto::v1::GetMapRequest;
use oozems_proto::v1::GetMapResponse;
use oozems_proto::v1::ItemActionResponse;
use oozems_proto::v1::Map;
use oozems_proto::v1::PickUpItemRequest;
use oozems_proto::v1::PlayerState;
use oozems_proto::v1::SavePlayerRequest;
use oozems_proto::v1::SavePlayerResponse;
use oozems_proto::v1::UnequipItemRequest;
use oozems_proto::v1::Vec2;
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
}

pub async fn bootstrap(player_id: &str) -> Result<BootstrapResponse, ClientError> {
    post_protobuf(
        "/api/v1/bootstrap",
        BootstrapRequest {
            player_id: player_id.to_owned(),
        },
    )
    .await
}

pub async fn create_character(
    player_id: &str,
    name: &str,
    appearance: CharacterAppearance,
) -> Result<PlayerState, ClientError> {
    let response: CreateCharacterResponse = post_protobuf(
        "/api/v1/characters/create",
        CreateCharacterRequest {
            player_id: player_id.to_owned(),
            name: name.to_owned(),
            appearance: Some(appearance),
        },
    )
    .await?;

    response.player.ok_or(ClientError::MissingData("player"))
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

    response
        .sprites
        .ok_or(ClientError::MissingData("character sprites"))
}

pub async fn equip_item(
    player_id: &str,
    inventory_index: u32,
) -> Result<ItemActionResponse, ClientError> {
    post_protobuf(
        "/api/v1/items/equip",
        EquipItemRequest {
            player_id: player_id.to_owned(),
            inventory_index,
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
) -> Result<ItemActionResponse, ClientError> {
    post_protobuf(
        "/api/v1/items/drop",
        DropItemRequest {
            player_id: player_id.to_owned(),
            inventory_index,
        },
    )
    .await
}

pub async fn pick_up_item(
    player_id: &str,
    map_id: u32,
    position: Vec2,
) -> Result<ItemActionResponse, ClientError> {
    post_protobuf(
        "/api/v1/items/pick-up",
        PickUpItemRequest {
            player_id: player_id.to_owned(),
            map_id,
            position: Some(position),
        },
    )
    .await
}

pub async fn get_map(map_id: u32) -> Result<Map, ClientError> {
    let response: GetMapResponse =
        post_protobuf("/api/v1/maps/get", GetMapRequest { map_id }).await?;

    response.map.ok_or(ClientError::MissingData("map"))
}

pub async fn get_gui() -> Result<GameGui, ClientError> {
    let response: GetGuiResponse = post_protobuf("/api/v1/gui/get", GetGuiRequest {}).await?;

    response.gui.ok_or(ClientError::MissingData("game GUI"))
}

pub async fn save_player(player: PlayerState) -> Result<PlayerState, ClientError> {
    let response: SavePlayerResponse = post_protobuf(
        "/api/v1/players/save",
        SavePlayerRequest {
            player: Some(player),
        },
    )
    .await?;

    response.player.ok_or(ClientError::MissingData("player"))
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
    let request = Request::post(url)
        .header("Content-Type", PROTOBUF_CONTENT_TYPE)
        .header("Accept", PROTOBUF_CONTENT_TYPE)
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
        return Err(ClientError::Server {
            status,
            code: error.code,
            message: error.message,
        });
    }

    O::decode(bytes.as_slice()).map_err(|error| ClientError::InvalidResponse(error.to_string()))
}
