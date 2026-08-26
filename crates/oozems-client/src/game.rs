use std::cell::RefCell;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::rc::Rc;

use oozems_proto::v1::ActiveBuffState;
use oozems_proto::v1::CharacterSpriteSet;
use oozems_proto::v1::GameGui;
use oozems_proto::v1::KeyBinding;
use oozems_proto::v1::Map;
use oozems_proto::v1::MorphDefinition;
use oozems_proto::v1::MovementRules;
use oozems_proto::v1::PlayerState;
use oozems_proto::v1::SkillBook;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use web_sys::CanvasRenderingContext2d;
use web_sys::HtmlCanvasElement;

use crate::api;
use crate::assets;
use crate::assets::BrowserAsset;
use crate::cash_shop_ui::CashShopState;
use crate::character_render::CharacterAnimation;
use crate::game_gui::CanvasPoint;
use crate::game_gui::GuiState;
use crate::game_gui::KeyDrag;
use crate::game_gui::WindowDrag;
use crate::interaction_ui::InteractionState;
use crate::js_error;
use crate::keymap;
use crate::keymap::KeyboardState;
use crate::mob_render::MobRenderState;
use crate::movement;
use crate::movement::MotionState;
use crate::render;
use crate::show_status;
use crate::skill_effects::SkillEffectState;

pub(crate) mod buffs;
mod cash_shop_actions;
mod input;
mod interaction_actions;
mod item_actions;
mod movement_actions;
mod player_updates;
mod recovery_actions;
mod refresh;
mod request_dispatch;
mod requests;
mod respawn_actions;
mod responses;
mod runtime;
mod skill_actions;

use input::GameInput;
use player_updates::PlayerDomains;
use player_updates::PlayerInstallation;
use player_updates::PlayerRevisions;
use player_updates::install_player_update;
use player_updates::synchronize_skill_book;
use player_updates::visible_appearance_identity;
use request_dispatch::AppearanceRefreshState;
use request_dispatch::GuiRefreshState;
use request_dispatch::MorphRefreshState;
use request_dispatch::RequestState;
use request_dispatch::SAVE_INTERVAL_MS;
use request_dispatch::dispatch_requests;
use request_dispatch::queue_appearance_refresh;
use request_dispatch::synchronize_morph;
use runtime::CharacterAnimationState;
use runtime::PersistenceState;
pub(crate) use runtime::character_animation_elapsed_ms;
use runtime::new_character_animation_state;
use runtime::update;

pub(crate) fn monotonic_time_ms() -> f64 {
    web_sys::window()
        .and_then(|window| window.performance())
        .map_or(0.0, |performance| performance.now())
}

pub struct Game {
    pub surface: BrowserSurface,
    pub clock: FrameClock,
    pub player: PlayerRuntime,
    pub world: WorldRuntime,
    pub ui: UiRuntime,
    input: GameInput,
    persistence: PersistenceState,
    requests: RequestState,
}

pub struct BrowserSurface {
    pub canvas: HtmlCanvasElement,
    pub context: CanvasRenderingContext2d,
    pub images: HashMap<String, BrowserAsset>,
}

pub struct FrameClock {
    pub now_ms: f64,
    last_frame_ms: f64,
}

pub struct PlayerRuntime {
    pub state: PlayerState,
    pub skill_book: SkillBook,
    pub(crate) active_buffs: buffs::TrackedBuffs,
    key_bindings: KeyBindingState,
    revisions: PlayerRevisions,
}

impl std::ops::Deref for PlayerRuntime {
    type Target = PlayerState;

    fn deref(&self) -> &Self::Target {
        &self.state
    }
}

impl std::ops::DerefMut for PlayerRuntime {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.state
    }
}

pub struct WorldRuntime {
    pub map: Map,
    pub mob_render: MobRenderState,
    pub movement_rules: MovementRules,
    pub motion: MotionState,
    pub facing_left: bool,
    pub world_layers: Vec<i32>,
    pub character_animation: CharacterAnimationState,
    pub character_sprites: CharacterSpriteSet,
    pub(crate) npc_animations: render::npc::NpcAnimationPlaybackState,
    pub(crate) morph_definition: Option<MorphDefinition>,
    pub(crate) skill_effect_state: SkillEffectState,
}

pub struct UiRuntime {
    pub cash_shop: CashShopState,
    pub(crate) death: crate::death_ui::DeathUiState,
    pub gui: GameGui,
    pub gui_state: Rc<RefCell<GuiState>>,
    pub interaction: InteractionState,
    pub key_drag: Option<KeyDrag>,
    pub window_drag: Option<WindowDrag>,
    pub pointer: Option<CanvasPoint>,
    pub(crate) selected_buff: Option<buffs::BuffKey>,
}

impl Game {
    pub(crate) fn key_bindings(&self) -> std::cell::Ref<'_, Vec<KeyBinding>> {
        self.player.key_bindings.current.borrow()
    }

    pub(crate) fn cash_shop_request_in_flight(&self) -> bool {
        self.requests
            .admission
            .is_active(requests::RequestKind::CashShopCatalog)
    }
}

struct KeyBindingState {
    current: Rc<RefCell<Vec<KeyBinding>>>,
    generation: u64,
    pending: bool,
}

fn install_full_player_update(
    game: &mut Game,
    update: PlayerState,
) -> PlayerInstallation {
    let mut domains = PlayerDomains::FULL;
    domains.key_bindings = !game.player.key_bindings.pending;
    let installed = install_player_update(
        &mut game.player.state,
        &mut game.player.revisions,
        update,
        domains,
    );
    if installed.domains.skills {
        synchronize_skill_book(&mut game.player.skill_book, &game.player.state);
        if game.player.key_bindings.pending {
            let updated = keymap::retain_learned_skill_bindings(
                &game.player.key_bindings.current.borrow(),
                &game.player.state.learned_skills,
            );
            if updated != *game.player.key_bindings.current.borrow() {
                *game.player.key_bindings.current.borrow_mut() = updated.clone();
                game.player.state.key_bindings = updated;
                game.player.key_bindings.generation =
                    game.player.key_bindings.generation.saturating_add(1);
                game.persistence.dirty = true;
            }
        }
        let dragged_skill_was_removed = game.ui.key_drag.as_ref().is_some_and(|drag| {
            let keymap::BindingTarget::Skill(skill_id) = drag.target else {
                return false;
            };
            !game
                .player
                .state
                .learned_skills
                .iter()
                .any(|skill| skill.skill_id == skill_id && skill.level > 0)
        });
        if dragged_skill_was_removed {
            game.ui.key_drag = None;
        }
    }
    if installed.domains.key_bindings {
        *game.player.key_bindings.current.borrow_mut() = game.player.state.key_bindings.clone();
    }
    queue_appearance_refresh(game, installed);
    installed
}

fn install_active_buffs(
    game: &mut Game,
    state: buffs::ValidatedState,
    request_started_ms: f64,
) {
    let received_at_ms = monotonic_time_ms();
    buffs::install(
        &mut game.player.active_buffs,
        state,
        received_at_ms,
        elapsed_since(request_started_ms, received_at_ms),
    );
    synchronize_morph(game);
}

fn elapsed_since(
    started_ms: f64,
    finished_ms: f64,
) -> f64 {
    (finished_ms - started_ms).max(0.0)
}

pub async fn run(
    player: PlayerState,
    character_sprites: CharacterSpriteSet,
    bootstrap_active_buffs: ActiveBuffState,
    bootstrap_requested_at_ms: f64,
) -> Result<(), String> {
    show_status("Loading map, GUI, and skills...", false);
    let map = api::get_map(&player.id, player.map_id)
        .await
        .map_err(|error| error.to_string())?;
    let movement_rules = api::get_movement_rules()
        .await
        .map_err(|error| error.to_string())?;
    movement::validate_rules(&movement_rules)?;
    let gui_result = api::get_gui(&player.id, Vec::new()).await;
    let gui_refresh_required = gui_result.is_err();
    let gui_warning = gui_result.as_ref().err().map(ToString::to_string);
    let gui = gui_result.unwrap_or_default();
    let skill_requested_at_ms = monotonic_time_ms();
    let skill_result = api::get_skill_book(&player.id).await;
    let skill_warning = skill_result.as_ref().err().map(ToString::to_string);
    let loaded_skills = skill_result.unwrap_or_else(|_| api::LoadedSkillBook {
        skill_book: SkillBook {
            job_id: player.stats.as_ref().map_or(0, |stats| stats.job_id),
            name: "Skills".to_owned(),
            ..SkillBook::default()
        },
        active_buffs: ActiveBuffState::default(),
    });
    let game = build_game(
        player,
        map,
        movement_rules,
        character_sprites,
        gui,
        loaded_skills.skill_book,
        bootstrap_active_buffs,
        bootstrap_requested_at_ms,
        loaded_skills.active_buffs,
        skill_requested_at_ms,
    )?;
    if !gui_refresh_required {
        game.borrow_mut().requests.gui.cached.insert((), ());
    }
    let asset_count = game.borrow().surface.images.len();

    let warnings = [
        gui_warning.map(|error| format!("GUI unavailable: {error}")),
        skill_warning.map(|error| format!("skill book unavailable: {error}")),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    if warnings.is_empty() {
        show_status(
            &format!("Ready. Streaming {asset_count} assets as needed."),
            false,
        );
    } else {
        show_status(
            &format!(
                "Ready. Streaming {asset_count} assets as needed. {}",
                warnings.join("; ")
            ),
            true,
        );
    }
    schedule_frame(game)?;
    Ok(())
}

fn build_game(
    mut player: PlayerState,
    map: Map,
    movement_rules: MovementRules,
    character_sprites: CharacterSpriteSet,
    gui: GameGui,
    skill_book: SkillBook,
    bootstrap_active_buffs: ActiveBuffState,
    bootstrap_requested_at_ms: f64,
    skill_active_buffs: ActiveBuffState,
    skill_requested_at_ms: f64,
) -> Result<Rc<RefCell<Game>>, String> {
    if let Some(position) = player.position {
        player.position = Some(movement::constrain_position(&map, position));
    }
    let window = web_sys::window().ok_or("browser window is unavailable")?;
    let document = window.document().ok_or("browser document is unavailable")?;
    let canvas = document
        .get_element_by_id("game")
        .ok_or("game canvas is missing")?
        .dyn_into::<HtmlCanvasElement>()
        .map_err(|_| "game element is not a canvas")?;
    let context = canvas
        .get_context("2d")
        .map_err(js_error)?
        .ok_or("2D canvas is unavailable")?
        .dyn_into::<CanvasRenderingContext2d>()
        .map_err(|_| "could not create a 2D canvas context")?;
    context.set_image_smoothing_enabled(false);

    let input = Rc::new(RefCell::new(KeyboardState::default()));
    let key_bindings = Rc::new(RefCell::new(player.key_bindings.clone()));
    let game_input = input::install(&window, &canvas, input, key_bindings.clone())?;
    let gui_state = Rc::new(RefCell::new(GuiState::default()));
    let images = prepare_game_assets(&map, &character_sprites, &gui, &skill_book)?;
    let motion = player
        .position
        .as_ref()
        .map_or_else(MotionState::default, |position| {
            movement::initial_motion_state(&map, position)
        });
    let world_layers = render::world_layers(&map);
    let simulation_sequence = map.simulation_sequence;
    let player_revisions = PlayerRevisions::new(player.revision);
    let mut appearance_refresh_state = AppearanceRefreshState::default();
    if let Some(identity) = visible_appearance_identity(&player) {
        appearance_refresh_state
            .cached
            .insert(identity, character_sprites.clone());
    }

    let now_local_ms = monotonic_time_ms();
    let mut active_buffs = buffs::from_state(
        bootstrap_active_buffs,
        now_local_ms,
        elapsed_since(bootstrap_requested_at_ms, now_local_ms),
    )?;
    buffs::install(
        &mut active_buffs,
        buffs::validate_state(skill_active_buffs)?,
        now_local_ms,
        elapsed_since(skill_requested_at_ms, now_local_ms),
    );
    let morph_refresh_state = MorphRefreshState {
        pending: active_buffs.morph_id,
        ..MorphRefreshState::default()
    };
    let game = Rc::new(RefCell::new(Game {
        surface: BrowserSurface {
            canvas: canvas.clone(),
            context,
            images,
        },
        clock: FrameClock {
            now_ms: 0.0,
            last_frame_ms: 0.0,
        },
        player: PlayerRuntime {
            state: player,
            skill_book,
            active_buffs,
            key_bindings: KeyBindingState {
                current: key_bindings,
                generation: 0,
                pending: false,
            },
            revisions: player_revisions,
        },
        world: WorldRuntime {
            map,
            mob_render: crate::mob_render::new_map_state(simulation_sequence),
            movement_rules,
            motion,
            facing_left: false,
            world_layers,
            character_animation: new_character_animation_state(CharacterAnimation::Idle, true, 0.0),
            character_sprites,
            npc_animations: render::npc::NpcAnimationPlaybackState::default(),
            morph_definition: None,
            skill_effect_state: SkillEffectState::default(),
        },
        ui: UiRuntime {
            cash_shop: CashShopState::default(),
            death: crate::death_ui::DeathUiState::default(),
            gui,
            gui_state,
            interaction: InteractionState::default(),
            key_drag: None,
            window_drag: None,
            pointer: None,
            selected_buff: None,
        },
        input: game_input,
        persistence: PersistenceState {
            dirty: false,
            next_save_ms: SAVE_INTERVAL_MS,
        },
        requests: RequestState {
            admission: requests::RequestAdmission::default(),
            movement: movement_actions::MovementSyncState::default(),
            recovery: recovery_actions::RecoveryState::default(),
            deferred_transitions: VecDeque::new(),
            appearance: appearance_refresh_state,
            morph: morph_refresh_state,
            gui: GuiRefreshState::default(),
        },
    }));
    crate::gui_dump::install(&game);
    Ok(game)
}

fn prepare_game_assets(
    map: &Map,
    character_sprites: &CharacterSpriteSet,
    gui: &GameGui,
    skill_book: &SkillBook,
) -> Result<HashMap<String, BrowserAsset>, String> {
    assets::prepare_assets(
        map.assets
            .iter()
            .chain(character_sprites.assets.iter())
            .chain(gui.assets.iter())
            .chain(skill_book.assets.iter()),
    )
}

fn schedule_frame(game: Rc<RefCell<Game>>) -> Result<(), String> {
    let window = web_sys::window().ok_or("browser window is unavailable")?;
    let callback = Closure::once_into_js(move |timestamp_ms: f64| {
        let pending = update(&mut game.borrow_mut(), timestamp_ms);
        {
            let game = game.borrow();
            render::draw(&game);
        }
        dispatch_requests(game.clone(), pending);
        if let Err(error) = schedule_frame(game) {
            show_status(&format!("Animation stopped: {error}"), true);
        }
    });
    window
        .request_animation_frame(callback.unchecked_ref())
        .map_err(js_error)?;
    Ok(())
}
