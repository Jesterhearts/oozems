use oozems_proto::v1::ActiveBuffState;
use oozems_proto::v1::AllocateSkillPointResponse;
use oozems_proto::v1::BasicAttackResponse;
use oozems_proto::v1::ItemActionResponse;
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

#[cfg(test)]
mod tests {
    use oozems_proto::v1::ActiveBuff;
    use oozems_proto::v1::ActiveBuffState;
    use oozems_proto::v1::PlayerState;
    use oozems_proto::v1::SavePlayerResponse;

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
}
