use std::cell::Cell;
use std::cell::RefCell;
use std::future::Future;
use std::rc::Rc;

use wasm_bindgen_futures::spawn_local;

use super::Game;
use crate::show_status;

const PLAYER_MUTATION: u32 = 1 << 0;
const MOVEMENT: u32 = 1 << 1;
const CASH_SHOP: u32 = 1 << 2;
const APPEARANCE: u32 = 1 << 3;
const MORPH: u32 = 1 << 4;
const GUI: u32 = 1 << 5;
const KEY_BINDING_SAVE: u32 = 1 << 6;
const ITEM: u32 = 1 << 7;
const SKILL: u32 = 1 << 8;
const TRANSITION: u32 = 1 << 9;
const RECOVERY: u32 = 1 << 10;
const PURCHASE: u32 = 1 << 11;
const INTERACTION: u32 = 1 << 12;
const RESPAWN: u32 = 1 << 13;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RequestKind {
    KeyBindingSave,
    Item,
    Skill,
    Transition,
    Recovery,
    Respawn,
    CashShopCatalog,
    CashShopPurchase,
    Interaction,
    Taxi,
    Movement,
    Appearance,
    Morph,
    Gui,
}

impl RequestKind {
    fn lanes(self) -> u32 {
        match self {
            Self::KeyBindingSave => PLAYER_MUTATION | KEY_BINDING_SAVE,
            Self::Item => PLAYER_MUTATION | ITEM,
            Self::Skill => PLAYER_MUTATION | SKILL,
            Self::Transition => PLAYER_MUTATION | MOVEMENT | TRANSITION,
            Self::Recovery => PLAYER_MUTATION | RECOVERY,
            Self::Respawn => PLAYER_MUTATION | MOVEMENT | RESPAWN,
            Self::CashShopCatalog => CASH_SHOP,
            Self::CashShopPurchase => PLAYER_MUTATION | CASH_SHOP | PURCHASE,
            Self::Interaction => PLAYER_MUTATION | INTERACTION,
            Self::Taxi => PLAYER_MUTATION | MOVEMENT | INTERACTION,
            Self::Movement => MOVEMENT,
            Self::Appearance => APPEARANCE,
            Self::Morph => MORPH,
            Self::Gui => GUI,
        }
    }

    fn identity(self) -> u32 {
        self.lanes() & !PLAYER_MUTATION
    }

    pub fn requires_gameplay_session(self) -> bool {
        !matches!(self, Self::CashShopCatalog | Self::Appearance | Self::Morph)
    }
}

#[derive(Clone, Default)]
pub(super) struct RequestAdmission {
    occupied: Rc<Cell<u32>>,
}

impl RequestAdmission {
    pub fn admit(
        &self,
        kind: RequestKind,
    ) -> Option<RequestPermit> {
        let occupied = self.occupied.get();
        let lanes = kind.lanes();
        if occupied & lanes != 0 {
            return None;
        }
        self.occupied.set(occupied | lanes);
        Some(RequestPermit {
            occupied: self.occupied.clone(),
            lanes,
        })
    }

    pub fn is_active(
        &self,
        kind: RequestKind,
    ) -> bool {
        let identity = kind.identity();
        self.occupied.get() & identity == identity
    }

    pub fn player_mutation_is_active(&self) -> bool {
        self.occupied.get() & PLAYER_MUTATION != 0
    }
}

pub(super) struct RequestPermit {
    occupied: Rc<Cell<u32>>,
    lanes: u32,
}

impl Drop for RequestPermit {
    fn drop(&mut self) {
        self.occupied.set(self.occupied.get() & !self.lanes);
    }
}

pub(super) struct RequestStatus {
    message: Option<String>,
    is_error: bool,
}

impl RequestStatus {
    pub fn silent() -> Self {
        Self {
            message: None,
            is_error: false,
        }
    }

    pub fn success(message: impl Into<String>) -> Self {
        Self {
            message: Some(message.into()),
            is_error: false,
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            message: Some(message.into()),
            is_error: true,
        }
    }
}

pub(super) fn spawn_request<T, E, Fut, Request, Complete>(
    game: Rc<RefCell<Game>>,
    permit: RequestPermit,
    request: Request,
    complete: Complete,
) where
    T: 'static,
    E: 'static,
    Fut: Future<Output = Result<T, E>> + 'static,
    Request: FnOnce() -> Fut + 'static,
    Complete: FnOnce(&mut Game, Result<T, E>, f64) -> RequestStatus + 'static,
{
    spawn_local(async move {
        let request_started_ms = super::monotonic_time_ms();
        let result = request().await;
        let status = complete(&mut game.borrow_mut(), result, request_started_ms);
        drop(permit);
        if let Some(message) = status.message {
            show_status(&message, status.is_error);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::RequestAdmission;
    use super::RequestKind;

    #[test]
    fn player_mutations_share_one_admission_lane() {
        let admission = RequestAdmission::default();
        let permit = admission
            .admit(RequestKind::Item)
            .expect("first mutation is admitted");

        for kind in [
            RequestKind::KeyBindingSave,
            RequestKind::Skill,
            RequestKind::Transition,
            RequestKind::Recovery,
            RequestKind::Respawn,
            RequestKind::CashShopPurchase,
            RequestKind::Interaction,
            RequestKind::Taxi,
        ] {
            assert!(admission.admit(kind).is_none(), "{kind:?} must wait");
        }

        drop(permit);
        assert!(admission.admit(RequestKind::KeyBindingSave).is_some());
    }

    #[test]
    fn independent_observation_and_refresh_lanes_can_overlap() {
        let admission = RequestAdmission::default();
        let _mutation = admission
            .admit(RequestKind::Skill)
            .expect("mutation permit");
        let _movement = admission
            .admit(RequestKind::Movement)
            .expect("movement permit");
        let _appearance = admission
            .admit(RequestKind::Appearance)
            .expect("appearance permit");
        let _morph = admission.admit(RequestKind::Morph).expect("morph permit");
        let _gui = admission.admit(RequestKind::Gui).expect("GUI permit");
    }

    #[test]
    fn cash_shop_catalog_and_purchase_share_the_cash_shop_lane() {
        let admission = RequestAdmission::default();
        let catalog = admission
            .admit(RequestKind::CashShopCatalog)
            .expect("catalog permit");

        assert!(admission.admit(RequestKind::CashShopPurchase).is_none());
        assert!(admission.is_active(RequestKind::CashShopCatalog));
        drop(catalog);
        assert!(admission.admit(RequestKind::CashShopPurchase).is_some());
    }

    #[test]
    fn respawn_waits_for_movement_and_blocks_new_snapshots() {
        let admission = RequestAdmission::default();
        let movement = admission
            .admit(RequestKind::Movement)
            .expect("movement permit");

        assert!(admission.admit(RequestKind::Respawn).is_none());
        drop(movement);

        let respawn = admission
            .admit(RequestKind::Respawn)
            .expect("respawn permit");
        assert!(admission.admit(RequestKind::Movement).is_none());
        drop(respawn);
        assert!(admission.admit(RequestKind::Movement).is_some());
    }

    #[test]
    fn relocations_wait_for_movement_and_block_new_snapshots() {
        for kind in [RequestKind::Transition, RequestKind::Taxi] {
            let admission = RequestAdmission::default();
            let movement = admission
                .admit(RequestKind::Movement)
                .expect("movement permit");

            assert!(admission.admit(kind).is_none(), "{kind:?} must wait");
            assert!(!admission.is_active(kind));
            drop(movement);

            let relocation = admission.admit(kind).expect("relocation permit");
            assert!(admission.is_active(kind));
            assert!(admission.is_active(RequestKind::Movement));
            assert!(admission.admit(RequestKind::Movement).is_none());
            drop(relocation);
            assert!(admission.admit(RequestKind::Movement).is_some());
        }
    }

    #[test]
    fn plain_movement_is_not_reported_as_a_relocation() {
        let admission = RequestAdmission::default();
        let _movement = admission
            .admit(RequestKind::Movement)
            .expect("movement permit");

        assert!(!admission.is_active(RequestKind::Transition));
        assert!(!admission.is_active(RequestKind::Interaction));
        assert!(!admission.is_active(RequestKind::Respawn));
    }

    #[test]
    fn ordinary_interactions_can_overlap_movement() {
        let admission = RequestAdmission::default();
        let _movement = admission
            .admit(RequestKind::Movement)
            .expect("movement permit");

        assert!(admission.admit(RequestKind::Interaction).is_some());
        assert!(admission.admit(RequestKind::Taxi).is_none());
    }
}
