use oozems_proto::v1::ActiveBuffState;
use oozems_proto::v1::AllocateSkillPointResponse;
use oozems_proto::v1::BasicAttackResponse;
use oozems_proto::v1::ItemActionResponse;
use oozems_proto::v1::Map;
use oozems_proto::v1::MovementSnapshot;
use oozems_proto::v1::MovementUpdateResponse;
use oozems_proto::v1::NpcInteractionResponse;
use oozems_proto::v1::PlayerState;
use oozems_proto::v1::PurchaseCashShopItemResponse;
use oozems_proto::v1::RecoverPlayerResponse;
use oozems_proto::v1::RespawnPlayerResponse;
use oozems_proto::v1::SavePlayerResponse;
use oozems_proto::v1::UseSkillResponse;

use super::buffs;
use crate::api;

pub(super) trait PlayerBuffResponse {
    fn player(&mut self) -> &mut Option<PlayerState>;
    fn active_buffs(&mut self) -> &mut Option<ActiveBuffState>;
}

macro_rules! player_buff_response {
    ($($response:ty),+ $(,)?) => {
        $(
            impl PlayerBuffResponse for $response {
                fn player(&mut self) -> &mut Option<PlayerState> {
                    &mut self.player
                }

                fn active_buffs(&mut self) -> &mut Option<ActiveBuffState> {
                    &mut self.active_buffs
                }
            }
        )+
    };
}

player_buff_response!(
    AllocateSkillPointResponse,
    BasicAttackResponse,
    ItemActionResponse,
    MovementUpdateResponse,
    NpcInteractionResponse,
    PurchaseCashShopItemResponse,
    RecoverPlayerResponse,
    RespawnPlayerResponse,
    SavePlayerResponse,
    UseSkillResponse,
);

pub(super) fn take_player_and_active_buffs(
    response: &mut impl PlayerBuffResponse
) -> Result<(PlayerState, buffs::ValidatedState), String> {
    let player =
        api::require_data(response.player().take(), "player").map_err(|error| error.to_string())?;
    let active_buffs = api::require_data(response.active_buffs().take(), "active buffs")
        .map_err(|error| error.to_string())?;
    let active_buffs = buffs::validate_state(active_buffs)?;
    Ok((player, active_buffs))
}

pub(super) struct PreparedRelocation {
    pub player: PlayerState,
    pub active_buffs: buffs::ValidatedState,
    pub map: Map,
    pub authoritative: MovementSnapshot,
}

pub(super) fn prepare_relocation(
    player: PlayerState,
    active_buffs: buffs::ValidatedState,
    map: Map,
    authoritative: MovementSnapshot,
    expected_player_id: &str,
    expected_map_id: u32,
) -> Result<PreparedRelocation, String> {
    if player.id != expected_player_id {
        return Err("relocation response belongs to a different player".to_owned());
    }
    if map.id != expected_map_id {
        return Err("relocation response targets an unexpected map".to_owned());
    }
    if player.map_id != map.id || authoritative.map_id != map.id {
        return Err("relocation response map IDs do not agree".to_owned());
    }
    let player_position = player
        .position
        .ok_or("relocation response player does not contain a position")?;
    let authoritative_position = authoritative
        .position
        .ok_or("relocation response does not contain an authoritative position")?;
    if player_position != authoritative_position {
        return Err("relocation response positions do not agree".to_owned());
    }
    Ok(PreparedRelocation {
        player,
        active_buffs,
        map,
        authoritative,
    })
}

pub(super) fn validate_player_authoritative_map(
    player: &PlayerState,
    authoritative: &MovementSnapshot,
) -> Result<(), String> {
    if player.map_id != authoritative.map_id {
        return Err("response player and authoritative movement maps do not agree".to_owned());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use oozems_proto::v1::ActiveBuff;
    use oozems_proto::v1::ActiveBuffState;
    use oozems_proto::v1::Map;
    use oozems_proto::v1::MovementSnapshot;
    use oozems_proto::v1::PlayerState;
    use oozems_proto::v1::SavePlayerResponse;
    use oozems_proto::v1::Vec2;

    use super::prepare_relocation;
    use super::take_player_and_active_buffs;

    #[test]
    fn shared_response_transform_requires_both_authoritative_values() {
        let mut missing_buffs = SavePlayerResponse {
            player: Some(PlayerState::default()),
            active_buffs: None,
        };
        assert_eq!(
            take_player_and_active_buffs(&mut missing_buffs)
                .err()
                .expect("missing buffs must fail"),
            "response did not contain active buffs"
        );

        let mut missing_player = SavePlayerResponse {
            player: None,
            active_buffs: Some(ActiveBuffState::default()),
        };
        assert_eq!(
            take_player_and_active_buffs(&mut missing_player)
                .err()
                .expect("missing player must fail"),
            "response did not contain player"
        );
    }

    #[test]
    fn shared_response_transform_validates_buffs_before_installation() {
        let mut response = SavePlayerResponse {
            player: Some(PlayerState::default()),
            active_buffs: Some(ActiveBuffState {
                buffs: vec![ActiveBuff::default()],
                ..ActiveBuffState::default()
            }),
        };

        assert_eq!(
            take_player_and_active_buffs(&mut response)
                .err()
                .expect("invalid buff source must fail"),
            "active buff does not contain a source"
        );
    }

    #[test]
    fn relocation_preparation_validates_all_authoritative_identity() {
        let position = Vec2 { x: 10.0, y: 20.0 };
        let player = PlayerState {
            id: "player".to_owned(),
            map_id: 200,
            position: Some(position),
            ..PlayerState::default()
        };
        let map = Map {
            id: 200,
            ..Map::default()
        };
        let authoritative = MovementSnapshot {
            map_id: 200,
            position: Some(position),
            ..MovementSnapshot::default()
        };

        assert!(
            prepare_relocation(
                player.clone(),
                super::buffs::validate_state(ActiveBuffState::default())
                    .expect("empty buffs are valid"),
                map.clone(),
                authoritative.clone(),
                "player",
                200,
            )
            .is_ok()
        );
        let mut mismatched = player;
        mismatched.map_id = 100;
        assert_eq!(
            prepare_relocation(
                mismatched,
                super::buffs::validate_state(ActiveBuffState::default())
                    .expect("empty buffs are valid"),
                map,
                authoritative,
                "player",
                200,
            )
            .err()
            .expect("mismatched map must fail"),
            "relocation response map IDs do not agree"
        );
    }

    #[test]
    fn relocation_preparation_rejects_position_disagreement() {
        let player = PlayerState {
            id: "player".to_owned(),
            map_id: 200,
            position: Some(Vec2 { x: 10.0, y: 20.0 }),
            ..PlayerState::default()
        };
        let map = Map {
            id: 200,
            ..Map::default()
        };
        let authoritative = MovementSnapshot {
            map_id: 200,
            position: Some(Vec2 { x: 11.0, y: 20.0 }),
            ..MovementSnapshot::default()
        };

        assert_eq!(
            prepare_relocation(
                player,
                super::buffs::validate_state(ActiveBuffState::default())
                    .expect("empty buffs are valid"),
                map,
                authoritative,
                "player",
                200,
            )
            .err()
            .expect("mismatched position must fail"),
            "relocation response positions do not agree"
        );
    }
}
