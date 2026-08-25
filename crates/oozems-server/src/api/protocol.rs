use axum::body::Bytes;
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::http::header;
use axum::response::IntoResponse;
use axum::response::Response;
use oozems_proto::PROTOBUF_CONTENT_TYPE;
use oozems_proto::v1::ErrorResponse;
use prost::Message;
use thiserror::Error;

pub(super) fn decode_request<T>(
    headers: &HeaderMap,
    body: Bytes,
) -> Result<T, ApiError>
where
    T: Message + Default,
{
    let has_protobuf_content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case(PROTOBUF_CONTENT_TYPE));
    if !has_protobuf_content_type {
        return Err(ApiError::bad_request(
            "invalid_content_type",
            format!("Content-Type must be {PROTOBUF_CONTENT_TYPE}"),
        ));
    }

    T::decode(body).map_err(|error| {
        ApiError::bad_request(
            "invalid_protobuf",
            format!("invalid protobuf body: {error}"),
        )
    })
}

pub struct Protobuf<T>(pub T);

impl<T: Message> IntoResponse for Protobuf<T> {
    fn into_response(self) -> Response {
        (
            [(header::CONTENT_TYPE, PROTOBUF_CONTENT_TYPE)],
            self.0.encode_to_vec(),
        )
            .into_response()
    }
}

#[derive(Debug, Error)]
pub enum ApiError {
    #[error("{message}")]
    Client {
        status: StatusCode,
        code: &'static str,
        message: String,
    },
    #[error("database operation failed")]
    Database(#[from] surrealdb::Error),
    #[error("content operation failed")]
    Content(#[from] crate::content::ContentError),
    #[error("content worker failed")]
    Worker(#[from] tokio::task::JoinError),
    #[error("persisted player data is invalid: {0}")]
    PlayerData(String),
    #[error("game rules could not be applied")]
    GameRules(#[from] crate::experience::ExperienceRuleError),
    #[error("attack rules could not be applied")]
    AttackRules(#[from] crate::attacks::AttackRuleError),
    #[error("dropped-item operation failed")]
    Drops(#[from] crate::items::DropStoreError),
    #[error("mob spawning failed")]
    Mobs(#[from] crate::mobs::MobStoreError),
    #[error("skill rules could not be applied")]
    SkillRules(#[from] crate::skills::SkillRuleError),
    #[error("recovery rules could not be applied")]
    Recovery(#[from] crate::recovery::RecoveryError),
    #[error("movement rules could not be applied")]
    Movement(#[from] crate::movement::MovementError),
    #[error("active effects could not be accessed")]
    Effects(#[from] crate::effects::EffectStoreError),
    #[error("player operations could not be serialized")]
    PlayerLock(#[from] crate::player_lock::PlayerLockError),
    #[error("player transaction failed")]
    PlayerTransaction(Box<crate::player_transaction::PlayerTransactionError>),
    #[error("system time is earlier than the Unix epoch")]
    Clock,
}

impl ApiError {
    pub(super) fn bad_request(
        code: &'static str,
        message: impl Into<String>,
    ) -> Self {
        Self::Client {
            status: StatusCode::BAD_REQUEST,
            code,
            message: message.into(),
        }
    }

    pub(super) fn not_found(
        code: &'static str,
        message: impl Into<String>,
    ) -> Self {
        Self::Client {
            status: StatusCode::NOT_FOUND,
            code,
            message: message.into(),
        }
    }

    pub(super) fn conflict(
        code: &'static str,
        message: impl Into<String>,
    ) -> Self {
        Self::Client {
            status: StatusCode::CONFLICT,
            code,
            message: message.into(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            Self::Client {
                status,
                code,
                message,
            } => (status, code, message),
            Self::Database(error) => {
                tracing::error!(%error, "database request failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "database_error",
                    "the server could not access player data".to_owned(),
                )
            }
            Self::Content(error) => {
                tracing::error!(%error, "content request failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "content_error",
                    "the server could not load game content".to_owned(),
                )
            }
            Self::Worker(error) => {
                tracing::error!(%error, "content worker failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "content_worker_error",
                    "the server could not load game content".to_owned(),
                )
            }
            Self::PlayerData(error) => {
                tracing::error!(%error, "persisted player data is invalid");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "player_data_error",
                    "the server could not load valid player data".to_owned(),
                )
            }
            Self::GameRules(error) => {
                tracing::error!(%error, "game rules could not be applied");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "game_rules_error",
                    "the server could not apply its game rules".to_owned(),
                )
            }
            Self::AttackRules(error) => {
                tracing::error!(%error, "attack rules could not be applied");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "attack_rules_error",
                    "the server could not apply its attack rules".to_owned(),
                )
            }
            Self::Drops(error) => {
                tracing::error!(%error, "dropped-item operation failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "drop_store_error",
                    "the server could not access dropped items".to_owned(),
                )
            }
            Self::Mobs(error) => {
                tracing::error!(%error, "mob spawning failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "mob_store_error",
                    "the server could not access live mobs".to_owned(),
                )
            }
            Self::SkillRules(error) => {
                tracing::error!(%error, "skill rules could not be applied");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "skill_rules_error",
                    "the server could not apply its skill rules".to_owned(),
                )
            }
            Self::Recovery(error) => {
                tracing::error!(%error, "recovery rules could not be applied");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "recovery_rules_error",
                    "the server could not apply its recovery rules".to_owned(),
                )
            }
            Self::Movement(error) => {
                tracing::error!(%error, "movement rules could not be applied");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "movement_rules_error",
                    "the server could not apply its movement rules".to_owned(),
                )
            }
            Self::Effects(error) => {
                tracing::error!(%error, "active effects could not be accessed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "effect_store_error",
                    "the server could not access active effects".to_owned(),
                )
            }
            Self::PlayerLock(error) => {
                tracing::error!(%error, "player operation lock failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "player_lock_error",
                    "the server could not serialize player operations".to_owned(),
                )
            }
            Self::PlayerTransaction(error) => {
                let reconciliation = matches!(
                    *error,
                    crate::player_transaction::PlayerTransactionError::Reconciliation { .. }
                );
                tracing::error!(%error, reconciliation, "player transaction failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    if reconciliation {
                        "player_reconciliation_required"
                    } else {
                        "player_transaction_error"
                    },
                    if reconciliation {
                        "player state could not be reconciled across server stores"
                    } else {
                        "the server could not apply the player operation"
                    }
                    .to_owned(),
                )
            }
            Self::Clock => {
                tracing::error!("system time is earlier than the Unix epoch");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "clock_error",
                    "the server clock is unavailable".to_owned(),
                )
            }
        };

        (
            status,
            Protobuf(ErrorResponse {
                code: code.to_owned(),
                message,
            }),
        )
            .into_response()
    }
}

impl From<crate::player_transaction::PlayerTransactionError> for ApiError {
    fn from(error: crate::player_transaction::PlayerTransactionError) -> Self {
        Self::PlayerTransaction(Box::new(error))
    }
}
