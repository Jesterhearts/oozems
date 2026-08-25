use oozems_proto::v1::CashShopOffer;
use oozems_proto::v1::GameGui;
use oozems_proto::v1::GuiWindow;

use crate::game_gui;
use crate::game_gui::CanvasPoint;
use crate::game_gui::PointerButton;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CashShopAction {
    Close,
    Buy { offer_id: u32 },
    Consume,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScreenTransform {
    pub origin_x: f32,
    pub origin_y: f32,
    pub scale: f32,
}

pub struct CashShopState {
    pub open: bool,
    pub offers: Option<Vec<CashShopOffer>>,
    pub currency_name: String,
    pub load_error: Option<String>,
}

impl Default for CashShopState {
    fn default() -> Self {
        Self {
            open: false,
            offers: None,
            currency_name: "Ooze".to_owned(),
            load_error: None,
        }
    }
}

impl CashShopState {
    pub fn begin_open(&mut self) {
        self.open = true;
        self.offers = None;
        self.load_error = None;
    }

    pub fn close(&mut self) {
        self.open = false;
        self.offers = None;
        self.load_error = None;
    }

    pub fn install_catalog(
        &mut self,
        offers: Vec<CashShopOffer>,
        currency_name: String,
    ) {
        if self.open {
            self.offers = Some(offers);
            self.currency_name = currency_name;
            self.load_error = None;
        }
    }

    pub fn install_load_error(
        &mut self,
        error: String,
    ) {
        if self.open {
            self.offers = None;
            self.load_error = Some(error);
        }
    }
}

pub fn action_at(
    state: &CashShopState,
    gui: &GameGui,
    viewport_width: f32,
    viewport_height: f32,
    point: CanvasPoint,
    button: PointerButton,
    request_in_flight: bool,
) -> Option<CashShopAction> {
    if !state.open {
        return None;
    }
    if button != PointerButton::Left {
        return Some(CashShopAction::Consume);
    }
    let window = valid_window(gui)?;
    let logical_point = logical_point(window, viewport_width, viewport_height, point)?;
    let layout = window.layout.as_ref()?;
    if region_contains(layout, "cash-shop-exit", logical_point) {
        return Some(CashShopAction::Close);
    }
    if request_in_flight {
        return Some(CashShopAction::Consume);
    }
    state
        .offers
        .as_ref()
        .and_then(|offers| {
            offers
                .iter()
                .enumerate()
                .find(|(index, _)| {
                    region_contains(layout, &format!("cash-shop-buy-{index}"), logical_point)
                })
                .map(|(_, offer)| CashShopAction::Buy {
                    offer_id: offer.offer_id,
                })
        })
        .or(Some(CashShopAction::Consume))
}

pub fn screen_transform(
    window: &GuiWindow,
    viewport_width: f32,
    viewport_height: f32,
) -> Option<ScreenTransform> {
    let layout = window.layout.as_ref()?;
    if !game_gui::valid_layout(layout)
        || !viewport_width.is_finite()
        || !viewport_height.is_finite()
        || viewport_width <= 0.0
        || viewport_height <= 0.0
    {
        return None;
    }
    let scale = (viewport_width / layout.width)
        .min(viewport_height / layout.height)
        .max(f32::MIN_POSITIVE);
    Some(ScreenTransform {
        origin_x: (viewport_width - layout.width * scale) / 2.0,
        origin_y: (viewport_height - layout.height * scale) / 2.0,
        scale,
    })
}

fn valid_window(gui: &GameGui) -> Option<&GuiWindow> {
    gui.cash_shop_window
        .as_ref()
        .filter(|window| window.layout.as_ref().is_some_and(game_gui::valid_layout))
}

fn logical_point(
    window: &GuiWindow,
    viewport_width: f32,
    viewport_height: f32,
    point: CanvasPoint,
) -> Option<CanvasPoint> {
    let layout = window.layout.as_ref()?;
    let transform = screen_transform(window, viewport_width, viewport_height)?;
    let point = CanvasPoint {
        x: (point.x - transform.origin_x) / transform.scale - window.x,
        y: (point.y - transform.origin_y) / transform.scale - window.y,
    };
    (point.x >= 0.0 && point.x <= layout.width && point.y >= 0.0 && point.y <= layout.height)
        .then_some(point)
}

fn region_contains(
    layout: &oozems_proto::v1::GuiLayout,
    name: &str,
    point: CanvasPoint,
) -> bool {
    game_gui::named_region(layout, name).is_some_and(|region| {
        crate::hit_test::contains_inclusive(
            crate::hit_test::Rect {
                x: f64::from(region.x),
                y: f64::from(region.y),
                width: f64::from(region.width),
                height: f64::from(region.height),
            },
            crate::hit_test::Point {
                x: f64::from(point.x),
                y: f64::from(point.y),
            },
        )
    })
}

#[cfg(test)]
mod tests {
    use oozems_proto::v1::CashShopOffer;
    use oozems_proto::v1::GameGui;
    use oozems_proto::v1::GuiLayout;
    use oozems_proto::v1::GuiRegion;
    use oozems_proto::v1::GuiSprite;
    use oozems_proto::v1::GuiWindow;

    use super::CashShopAction;
    use super::CashShopState;
    use super::action_at;
    use super::screen_transform;
    use crate::game_gui::CanvasPoint;
    use crate::game_gui::PointerButton;

    #[test]
    fn classic_screen_is_scaled_uniformly_and_centered() {
        let window = cash_shop_window();

        let transform = screen_transform(&window, 1_600.0, 900.0).expect("valid screen");

        assert_eq!(transform.scale, 1.5);
        assert_eq!(transform.origin_x, 200.0);
        assert_eq!(transform.origin_y, 0.0);
    }

    #[test]
    fn catalog_name_defaults_to_ooze_and_accepts_the_server_value() {
        let mut state = CashShopState::default();
        assert_eq!(state.currency_name, "Ooze");

        state.open = true;
        state.install_catalog(Vec::new(), "Slime Tokens".to_owned());

        assert_eq!(state.currency_name, "Slime Tokens");
    }

    #[test]
    fn scaled_buy_and_exit_regions_select_the_expected_action() {
        let state = CashShopState {
            open: true,
            offers: Some(vec![CashShopOffer {
                offer_id: 7,
                item_id: 5_010_000,
                price: 1_200,
                duration_ms: 1_000,
            }]),
            ..CashShopState::default()
        };
        let gui = GameGui {
            cash_shop_window: Some(cash_shop_window()),
            ..GameGui::default()
        };

        assert_eq!(
            action_at(
                &state,
                &gui,
                1_600.0,
                900.0,
                CanvasPoint { x: 740.0, y: 240.0 },
                PointerButton::Left,
                false,
            ),
            Some(CashShopAction::Buy { offer_id: 7 })
        );
        assert_eq!(
            action_at(
                &state,
                &gui,
                1_600.0,
                900.0,
                CanvasPoint {
                    x: 1_250.0,
                    y: 840.0,
                },
                PointerButton::Left,
                false,
            ),
            Some(CashShopAction::Close)
        );
    }

    #[test]
    fn non_left_and_in_flight_clicks_are_consumed() {
        let state = CashShopState {
            open: true,
            offers: Some(vec![CashShopOffer {
                offer_id: 7,
                item_id: 5_010_000,
                price: 1_200,
                duration_ms: 1_000,
            }]),
            ..CashShopState::default()
        };
        let gui = GameGui {
            cash_shop_window: Some(cash_shop_window()),
            ..GameGui::default()
        };
        let point = CanvasPoint { x: 740.0, y: 240.0 };

        assert_eq!(
            action_at(
                &state,
                &gui,
                1_600.0,
                900.0,
                point,
                PointerButton::Right,
                false,
            ),
            Some(CashShopAction::Consume)
        );
        assert_eq!(
            action_at(
                &state,
                &gui,
                1_600.0,
                900.0,
                point,
                PointerButton::Left,
                true,
            ),
            Some(CashShopAction::Consume)
        );
    }

    fn cash_shop_window() -> GuiWindow {
        GuiWindow {
            layout: Some(GuiLayout {
                width: 800.0,
                height: 600.0,
                background: Some(GuiSprite {
                    name: "background".to_owned(),
                    asset_id: "background".to_owned(),
                    width: 800.0,
                    height: 600.0,
                    ..GuiSprite::default()
                }),
                regions: vec![
                    GuiRegion {
                        name: "cash-shop-buy-0".to_owned(),
                        x: 355.0,
                        y: 155.0,
                        width: 37.0,
                        height: 19.0,
                    },
                    GuiRegion {
                        name: "cash-shop-exit".to_owned(),
                        x: 632.0,
                        y: 535.0,
                        width: 168.0,
                        height: 49.0,
                    },
                ],
                ..GuiLayout::default()
            }),
            ..GuiWindow::default()
        }
    }
}
