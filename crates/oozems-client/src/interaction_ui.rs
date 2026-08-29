use oozems_proto::v1::GameGui;
use oozems_proto::v1::InventoryState;
use oozems_proto::v1::NpcDialogChoiceKind;
use oozems_proto::v1::NpcDialogView;
use oozems_proto::v1::NpcInteraction;
use oozems_proto::v1::NpcShopCurrency;
use oozems_proto::v1::NpcShopView;
use oozems_proto::v1::npc_interaction;

use crate::game_gui::CanvasPoint;

pub const SHOP_PAGE_SIZE: usize = 5;
pub const DIALOG_CHOICE_PAGE_SIZE: usize = 4;
const DIALOG_PAGE_CHARACTER_LIMIT: usize = 360;

#[derive(Default)]
pub struct InteractionState {
    pub interaction: Option<NpcInteraction>,
    pub page: usize,
    pub choice_page: usize,
    pub selected_offer: Option<usize>,
    pub selected_inventory: Option<usize>,
    pub inventory_page: usize,
    pub generation: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InteractionUiAction {
    Consume,
    Close,
    PreviousPage,
    NextPage,
    PreviousChoicePage,
    NextChoicePage,
    SelectChoice { quest_id: u32, choice_id: u32 },
    SelectOffer { index: usize },
    SelectInventory { index: usize },
    PreviousInventoryPage,
    NextInventoryPage,
    Buy,
    Sell,
    TakeTaxi { map_id: u32 },
}

impl InteractionState {
    pub fn is_open(&self) -> bool {
        self.interaction.is_some()
    }

    pub fn install(
        &mut self,
        interaction: Option<NpcInteraction>,
    ) {
        self.interaction = interaction;
        self.page = 0;
        self.choice_page = 0;
        self.selected_offer = None;
        self.selected_inventory = None;
        self.inventory_page = 0;
        self.generation = self.generation.wrapping_add(1);
    }

    pub fn close(&mut self) {
        self.install(None);
    }
}

pub fn click_action(
    gui: &GameGui,
    state: &InteractionState,
    inventory: Option<&InventoryState>,
    point: CanvasPoint,
) -> Option<InteractionUiAction> {
    let interaction = state.interaction.as_ref()?;
    match interaction.view.as_ref()? {
        npc_interaction::View::Dialog(dialog) => {
            let window = gui.npc_dialog_window.as_ref()?;
            if !contains_window(window, point) {
                return Some(InteractionUiAction::Consume);
            }
            let pages = visual_dialog_pages(dialog);
            let previous_region = dialog_previous_region(state, dialog, pages.len());
            if state.page + 1 < pages.len() && contains_region(window, "npc-next", point) {
                return Some(InteractionUiAction::NextPage);
            }
            if state.page + 1 == pages.len() && !dialog.choices.is_empty() {
                if state.choice_page > 0 && contains_region(window, previous_region, point) {
                    return Some(InteractionUiAction::PreviousChoicePage);
                }
                if (state.choice_page + 1) * DIALOG_CHOICE_PAGE_SIZE < dialog.choices.len()
                    && contains_region(window, "npc-next", point)
                {
                    return Some(InteractionUiAction::NextChoicePage);
                }
            }
            if state.page > 0 && contains_region(window, previous_region, point) {
                return Some(InteractionUiAction::PreviousPage);
            }
            if state.page + 1 < pages.len() {
                return Some(InteractionUiAction::Consume);
            }
            if is_quest_decision(dialog) {
                if contains_region(window, "npc-accept", point) {
                    return dialog.choices.iter().find_map(|choice| {
                        (NpcDialogChoiceKind::try_from(choice.kind).ok()
                            == Some(NpcDialogChoiceKind::AcceptQuest))
                        .then_some(InteractionUiAction::SelectChoice {
                            quest_id: dialog.quest_id,
                            choice_id: choice.choice_id,
                        })
                    });
                }
                if contains_region(window, "npc-decline", point) {
                    return dialog.choices.iter().find_map(|choice| {
                        (NpcDialogChoiceKind::try_from(choice.kind).ok()
                            == Some(NpcDialogChoiceKind::DeclineQuest))
                        .then_some(InteractionUiAction::SelectChoice {
                            quest_id: dialog.quest_id,
                            choice_id: choice.choice_id,
                        })
                    });
                }
                return Some(InteractionUiAction::Consume);
            }
            if let Some(index) =
                indexed_region_at(window, "npc-choice-row", DIALOG_CHOICE_PAGE_SIZE, point)
                && let Some(choice) = dialog
                    .choices
                    .get(state.choice_page * DIALOG_CHOICE_PAGE_SIZE + index)
            {
                return Some(InteractionUiAction::SelectChoice {
                    quest_id: dialog.quest_id,
                    choice_id: choice.choice_id,
                });
            }
            if dialog.choices.is_empty() && contains_region(window, "npc-ok", point) {
                return Some(InteractionUiAction::Close);
            }
            Some(InteractionUiAction::Consume)
        }
        npc_interaction::View::Shop(shop) => {
            let cash_point_shop = is_cash_point_shop(shop);
            let window = gui.shop_window.as_ref()?;
            if !contains_window(window, point) {
                return Some(InteractionUiAction::Consume);
            }
            if contains_region(window, "shop-close", point) {
                return Some(InteractionUiAction::Close);
            }
            if contains_region(window, "shop-buy", point) {
                return Some(InteractionUiAction::Buy);
            }
            if !cash_point_shop && contains_region(window, "shop-sell", point) {
                return Some(InteractionUiAction::Sell);
            }
            let inventory_len = inventory.map_or(0, |inventory| inventory.stacks.len());
            if !cash_point_shop
                && state.inventory_page > 0
                && contains_region(window, "shop-inventory-previous", point)
            {
                return Some(InteractionUiAction::PreviousInventoryPage);
            }
            if !cash_point_shop
                && (state.inventory_page + 1) * SHOP_PAGE_SIZE < inventory_len
                && contains_region(window, "shop-inventory-next", point)
            {
                return Some(InteractionUiAction::NextInventoryPage);
            }
            if let Some(index) = indexed_region_at(window, "shop-stock-row", SHOP_PAGE_SIZE, point)
                && index < shop.offers.len()
            {
                return Some(InteractionUiAction::SelectOffer { index });
            }
            if !cash_point_shop
                && let Some(index) =
                    indexed_region_at(window, "shop-inventory-row", SHOP_PAGE_SIZE, point)
            {
                let index = state.inventory_page * SHOP_PAGE_SIZE + index;
                if index < inventory_len {
                    return Some(InteractionUiAction::SelectInventory { index });
                }
            }
            Some(InteractionUiAction::Consume)
        }
        npc_interaction::View::Taxi(taxi) => {
            let window = gui.npc_dialog_window.as_ref()?;
            if !contains_window(window, point) {
                return Some(InteractionUiAction::Consume);
            }
            if contains_region(window, "npc-close", point) {
                return Some(InteractionUiAction::Close);
            }
            if let Some(index) =
                indexed_region_at(window, "npc-choice-row", DIALOG_CHOICE_PAGE_SIZE, point)
                && let Some(destination) = taxi.destinations.get(index)
            {
                return Some(InteractionUiAction::TakeTaxi {
                    map_id: destination.map_id,
                });
            }
            Some(InteractionUiAction::Consume)
        }
    }
}

pub(crate) fn is_cash_point_shop(shop: &NpcShopView) -> bool {
    NpcShopCurrency::try_from(shop.currency).ok() == Some(NpcShopCurrency::CashPoints)
}

pub fn apply_local_action(
    state: &mut InteractionState,
    action: InteractionUiAction,
) -> bool {
    match action {
        InteractionUiAction::Consume => true,
        InteractionUiAction::Close => {
            state.close();
            true
        }
        InteractionUiAction::PreviousPage => {
            state.page = state.page.saturating_sub(1);
            state.choice_page = 0;
            true
        }
        InteractionUiAction::NextPage => {
            state.page = state.page.saturating_add(1);
            state.choice_page = 0;
            true
        }
        InteractionUiAction::PreviousChoicePage => {
            state.choice_page = state.choice_page.saturating_sub(1);
            true
        }
        InteractionUiAction::NextChoicePage => {
            state.choice_page = state.choice_page.saturating_add(1);
            true
        }
        InteractionUiAction::SelectOffer { index } => {
            state.selected_offer = Some(index);
            true
        }
        InteractionUiAction::SelectInventory { index } => {
            state.selected_inventory = Some(index);
            true
        }
        InteractionUiAction::PreviousInventoryPage => {
            state.inventory_page = state.inventory_page.saturating_sub(1);
            state.selected_inventory = None;
            true
        }
        InteractionUiAction::NextInventoryPage => {
            state.inventory_page = state.inventory_page.saturating_add(1);
            state.selected_inventory = None;
            true
        }
        InteractionUiAction::SelectChoice { .. }
        | InteractionUiAction::Buy
        | InteractionUiAction::Sell
        | InteractionUiAction::TakeTaxi { .. } => false,
    }
}

pub fn visual_dialog_pages(dialog: &NpcDialogView) -> Vec<String> {
    let mut pages = dialog
        .pages
        .iter()
        .flat_map(|page| split_dialog_page(page))
        .collect::<Vec<_>>();
    if pages.is_empty() {
        pages.push(String::new());
    }
    pages
}

pub(crate) fn is_quest_decision(dialog: &NpcDialogView) -> bool {
    dialog.choices.len() == 2
        && [
            NpcDialogChoiceKind::AcceptQuest,
            NpcDialogChoiceKind::DeclineQuest,
        ]
        .into_iter()
        .all(|kind| {
            dialog
                .choices
                .iter()
                .any(|choice| NpcDialogChoiceKind::try_from(choice.kind).ok() == Some(kind))
        })
}

pub(crate) fn dialog_previous_region(
    state: &InteractionState,
    dialog: &NpcDialogView,
    page_count: usize,
) -> &'static str {
    if state.page.checked_add(1) == Some(page_count) && is_quest_decision(dialog) {
        "npc-decision-previous"
    } else {
        "npc-previous"
    }
}

fn split_dialog_page(source: &str) -> Vec<String> {
    let mut pages = Vec::new();
    let mut page = String::new();
    for word in source.split_whitespace() {
        let additional = word.chars().count() + usize::from(!page.is_empty());
        if !page.is_empty() && page.chars().count() + additional > DIALOG_PAGE_CHARACTER_LIMIT {
            pages.push(std::mem::take(&mut page));
        }
        if !page.is_empty() {
            page.push(' ');
        }
        page.push_str(word);
    }
    if !page.is_empty() || pages.is_empty() {
        pages.push(page);
    }
    pages
}

fn contains_window(
    window: &oozems_proto::v1::GuiWindow,
    point: CanvasPoint,
) -> bool {
    window.layout.as_ref().is_some_and(|layout| {
        crate::hit_test::contains(
            crate::hit_test::Rect {
                x: f64::from(window.x),
                y: f64::from(window.y),
                width: f64::from(layout.width),
                height: f64::from(layout.height),
            },
            crate::hit_test::Point {
                x: f64::from(point.x),
                y: f64::from(point.y),
            },
        )
    })
}

fn contains_region(
    window: &oozems_proto::v1::GuiWindow,
    name: &str,
    point: CanvasPoint,
) -> bool {
    window.layout.as_ref().is_some_and(|layout| {
        layout.regions.iter().any(|region| {
            region.name == name
                && crate::hit_test::contains(
                    crate::hit_test::Rect {
                        x: f64::from(window.x + region.x),
                        y: f64::from(window.y + region.y),
                        width: f64::from(region.width),
                        height: f64::from(region.height),
                    },
                    crate::hit_test::Point {
                        x: f64::from(point.x),
                        y: f64::from(point.y),
                    },
                )
        })
    })
}

fn indexed_region_at(
    window: &oozems_proto::v1::GuiWindow,
    prefix: &str,
    count: usize,
    point: CanvasPoint,
) -> Option<usize> {
    (0..count).find(|index| contains_region(window, &format!("{prefix}-{index}"), point))
}

#[cfg(test)]
mod tests {
    use oozems_proto::v1::GameGui;
    use oozems_proto::v1::GuiLayout;
    use oozems_proto::v1::GuiRegion;
    use oozems_proto::v1::GuiWindow;
    use oozems_proto::v1::InventoryItemStack;
    use oozems_proto::v1::InventoryState;
    use oozems_proto::v1::NpcDialogChoice;
    use oozems_proto::v1::NpcDialogChoiceKind;
    use oozems_proto::v1::NpcDialogView;
    use oozems_proto::v1::NpcInteraction;
    use oozems_proto::v1::NpcShopCurrency;
    use oozems_proto::v1::NpcShopOffer;
    use oozems_proto::v1::NpcShopView;
    use oozems_proto::v1::npc_interaction;

    use super::InteractionState;
    use super::InteractionUiAction;
    use super::click_action;
    use super::dialog_previous_region;
    use super::indexed_region_at;
    use super::visual_dialog_pages;
    use crate::game_gui::CanvasPoint;

    #[test]
    fn indexed_regions_use_window_and_region_offsets() {
        let window = GuiWindow {
            x: 100.0,
            y: 50.0,
            layout: Some(GuiLayout {
                regions: vec![GuiRegion {
                    name: "rows-2".to_owned(),
                    x: 20.0,
                    y: 78.0,
                    width: 100.0,
                    height: 24.0,
                }],
                ..GuiLayout::default()
            }),
        };

        assert_eq!(
            indexed_region_at(&window, "rows", 4, CanvasPoint { x: 125.0, y: 129.0 }),
            Some(2)
        );
        assert_eq!(
            indexed_region_at(&window, "rows", 4, CanvasPoint { x: 110.0, y: 129.0 }),
            None
        );
        assert_eq!(
            indexed_region_at(&window, "rows", 4, CanvasPoint { x: 125.0, y: 152.0 }),
            None
        );
    }

    #[test]
    fn long_dialogue_is_split_into_visual_pages() {
        let dialog = oozems_proto::v1::NpcDialogView {
            pages: vec!["word ".repeat(100)],
            ..oozems_proto::v1::NpcDialogView::default()
        };

        assert!(visual_dialog_pages(&dialog).len() > 1);
    }

    #[test]
    fn bottom_padding_cannot_select_a_hidden_fifth_dialog_choice() {
        let window = GuiWindow {
            layout: Some(GuiLayout {
                width: 200.0,
                height: 140.0,
                regions: vec![GuiRegion {
                    name: "npc-choices".to_owned(),
                    x: 10.0,
                    y: 20.0,
                    width: 100.0,
                    height: 100.0,
                }],
                ..GuiLayout::default()
            }),
            ..GuiWindow::default()
        };
        let gui = GameGui {
            npc_dialog_window: Some(window),
            ..GameGui::default()
        };
        let state = InteractionState {
            interaction: Some(NpcInteraction {
                view: Some(npc_interaction::View::Dialog(NpcDialogView {
                    quest_id: 100,
                    choices: (0..5)
                        .map(|choice_id| NpcDialogChoice {
                            choice_id,
                            ..NpcDialogChoice::default()
                        })
                        .collect(),
                    ..NpcDialogView::default()
                })),
                ..NpcInteraction::default()
            }),
            ..InteractionState::default()
        };

        for y in [116.0, 117.0, 118.0, 119.0] {
            assert_eq!(
                click_action(&gui, &state, None, CanvasPoint { x: 20.0, y }),
                Some(InteractionUiAction::Consume)
            );
        }
    }

    #[test]
    fn final_quest_decision_has_distinct_back_accept_and_decline_actions() {
        let dialog = NpcDialogView {
            quest_id: 100,
            pages: vec!["one".to_owned(), "two".to_owned()],
            choices: vec![
                NpcDialogChoice {
                    choice_id: 1,
                    kind: NpcDialogChoiceKind::AcceptQuest as i32,
                    ..NpcDialogChoice::default()
                },
                NpcDialogChoice {
                    choice_id: 2,
                    kind: NpcDialogChoiceKind::DeclineQuest as i32,
                    ..NpcDialogChoice::default()
                },
            ],
            ..NpcDialogView::default()
        };
        let state = InteractionState {
            page: 1,
            interaction: Some(NpcInteraction {
                view: Some(npc_interaction::View::Dialog(dialog.clone())),
                ..NpcInteraction::default()
            }),
            ..InteractionState::default()
        };
        let gui = GameGui {
            npc_dialog_window: Some(GuiWindow {
                layout: Some(GuiLayout {
                    width: 529.0,
                    height: 286.0,
                    regions: vec![
                        GuiRegion {
                            name: "npc-decision-previous".to_owned(),
                            x: 329.0,
                            y: 253.0,
                            width: 46.0,
                            height: 20.0,
                        },
                        GuiRegion {
                            name: "npc-accept".to_owned(),
                            x: 383.0,
                            y: 253.0,
                            width: 60.0,
                            height: 20.0,
                        },
                        GuiRegion {
                            name: "npc-decline".to_owned(),
                            x: 448.0,
                            y: 253.0,
                            width: 60.0,
                            height: 20.0,
                        },
                        GuiRegion {
                            name: "npc-choices".to_owned(),
                            x: 154.0,
                            y: 108.0,
                            width: 345.0,
                            height: 100.0,
                        },
                    ],
                    ..GuiLayout::default()
                }),
                ..GuiWindow::default()
            }),
            ..GameGui::default()
        };

        assert_eq!(
            dialog_previous_region(&state, &dialog, 2),
            "npc-decision-previous"
        );
        assert_eq!(
            dialog_previous_region(&InteractionState::default(), &dialog, 2),
            "npc-previous"
        );
        assert_eq!(
            dialog_previous_region(
                &state,
                &NpcDialogView {
                    pages: dialog.pages.clone(),
                    ..NpcDialogView::default()
                },
                2
            ),
            "npc-previous"
        );
        assert_eq!(
            click_action(&gui, &state, None, CanvasPoint { x: 340.0, y: 260.0 }),
            Some(InteractionUiAction::PreviousPage)
        );
        assert_eq!(
            click_action(&gui, &state, None, CanvasPoint { x: 410.0, y: 260.0 }),
            Some(InteractionUiAction::SelectChoice {
                quest_id: 100,
                choice_id: 1,
            })
        );
        assert_eq!(
            click_action(&gui, &state, None, CanvasPoint { x: 470.0, y: 260.0 }),
            Some(InteractionUiAction::SelectChoice {
                quest_id: 100,
                choice_id: 2,
            })
        );
        assert_eq!(
            click_action(&gui, &state, None, CanvasPoint { x: 170.0, y: 120.0 }),
            Some(InteractionUiAction::Consume)
        );
    }

    #[test]
    fn cash_point_shops_enable_buying_and_ignore_selling_controls() {
        let window = GuiWindow {
            layout: Some(GuiLayout {
                width: 200.0,
                height: 200.0,
                regions: vec![
                    GuiRegion {
                        name: "shop-sell".to_owned(),
                        x: 10.0,
                        y: 10.0,
                        width: 40.0,
                        height: 20.0,
                    },
                    GuiRegion {
                        name: "shop-inventory".to_owned(),
                        x: 10.0,
                        y: 40.0,
                        width: 100.0,
                        height: 40.0,
                    },
                    GuiRegion {
                        name: "shop-buy".to_owned(),
                        x: 60.0,
                        y: 10.0,
                        width: 40.0,
                        height: 20.0,
                    },
                    GuiRegion {
                        name: "shop-close".to_owned(),
                        x: 110.0,
                        y: 10.0,
                        width: 40.0,
                        height: 20.0,
                    },
                    GuiRegion {
                        name: "shop-stock".to_owned(),
                        x: 10.0,
                        y: 90.0,
                        width: 100.0,
                        height: 40.0,
                    },
                    GuiRegion {
                        name: "shop-stock-row-0".to_owned(),
                        x: 10.0,
                        y: 90.0,
                        width: 100.0,
                        height: 37.0,
                    },
                ],
                ..GuiLayout::default()
            }),
            ..GuiWindow::default()
        };
        let gui = GameGui {
            shop_window: Some(window),
            ..GameGui::default()
        };
        let state = InteractionState {
            interaction: Some(NpcInteraction {
                view: Some(npc_interaction::View::Shop(NpcShopView {
                    currency: NpcShopCurrency::CashPoints as i32,
                    currency_name: "Ooze".to_owned(),
                    offers: vec![NpcShopOffer {
                        item_id: 5_000_001,
                        buy_price: 250,
                    }],
                })),
                ..NpcInteraction::default()
            }),
            ..InteractionState::default()
        };
        let inventory = InventoryState {
            stacks: vec![InventoryItemStack {
                item_id: 5_000_001,
                quantity: 1,
                ..InventoryItemStack::default()
            }],
            ..InventoryState::default()
        };

        assert_eq!(
            click_action(
                &gui,
                &state,
                Some(&inventory),
                CanvasPoint { x: 20.0, y: 20.0 },
            ),
            Some(InteractionUiAction::Consume)
        );
        assert_eq!(
            click_action(
                &gui,
                &state,
                Some(&inventory),
                CanvasPoint { x: 20.0, y: 50.0 },
            ),
            Some(InteractionUiAction::Consume)
        );
        assert_eq!(
            click_action(
                &gui,
                &state,
                Some(&inventory),
                CanvasPoint { x: 70.0, y: 20.0 },
            ),
            Some(InteractionUiAction::Buy)
        );
        assert_eq!(
            click_action(
                &gui,
                &state,
                Some(&inventory),
                CanvasPoint { x: 120.0, y: 20.0 },
            ),
            Some(InteractionUiAction::Close)
        );
        assert_eq!(
            click_action(
                &gui,
                &state,
                Some(&inventory),
                CanvasPoint { x: 20.0, y: 100.0 },
            ),
            Some(InteractionUiAction::SelectOffer { index: 0 })
        );
    }
}
