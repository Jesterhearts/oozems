use std::cell::RefCell;
use std::rc::Rc;

use oozems_proto::v1::AssetDescriptor;
use oozems_proto::v1::MapAudio;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use wasm_bindgen_futures::JsFuture;
use wasm_bindgen_futures::spawn_local;
use web_sys::HtmlAudioElement;
use web_sys::KeyboardEvent;

use crate::js_error;

const AUDIO_LIFETIME_MS: f64 = 15_000.0;
const BGM_VOLUME: f64 = 0.45;
const SOUND_EFFECT_VOLUME: f64 = 0.8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MapSound {
    Jump,
    Portal,
    PickUpItem,
    DropItem,
    UseItem,
    Tombstone,
    LevelUp,
    QuestClear,
}

#[derive(Default)]
pub(crate) struct AudioState {
    bgm: Option<HtmlAudioElement>,
    bgm_url: Option<String>,
    sounds: Vec<ActiveSound>,
}

struct ActiveSound {
    element: HtmlAudioElement,
    expires_at_ms: f64,
}

pub(crate) struct AudioEventHandlers {
    window: web_sys::Window,
    keydown: Closure<dyn FnMut(KeyboardEvent)>,
    pointer_down: Closure<dyn FnMut(web_sys::Event)>,
}

impl Drop for AudioEventHandlers {
    fn drop(&mut self) {
        let _ = self
            .window
            .remove_event_listener_with_callback("keydown", self.keydown.as_ref().unchecked_ref());
        let _ = self.window.remove_event_listener_with_callback(
            "pointerdown",
            self.pointer_down.as_ref().unchecked_ref(),
        );
    }
}

pub(crate) fn install_input(
    window: &web_sys::Window,
    state: Rc<RefCell<AudioState>>,
) -> Result<AudioEventHandlers, String> {
    let keyboard_state = state.clone();
    let keydown = Closure::<dyn FnMut(KeyboardEvent)>::new(move |_| {
        resume_audio(&keyboard_state.borrow());
    });
    window
        .add_event_listener_with_callback("keydown", keydown.as_ref().unchecked_ref())
        .map_err(js_error)?;

    let pointer_down = Closure::<dyn FnMut(web_sys::Event)>::new(move |_| {
        resume_audio(&state.borrow());
    });
    window
        .add_event_listener_with_callback("pointerdown", pointer_down.as_ref().unchecked_ref())
        .map_err(js_error)?;
    Ok(AudioEventHandlers {
        window: window.clone(),
        keydown,
        pointer_down,
    })
}

pub(crate) fn set_bgm(
    state: &mut AudioState,
    descriptor: Option<&AssetDescriptor>,
) {
    let next_url = descriptor.map(|descriptor| descriptor.url.as_str());
    if !bgm_change_required(state.bgm_url.as_deref(), next_url) {
        return;
    }
    if let Some(current) = state.bgm.take() {
        let _ = current.pause();
    }
    state.bgm_url = None;
    let Some(url) = next_url else {
        return;
    };
    let audio = match HtmlAudioElement::new_with_src(url) {
        Ok(audio) => audio,
        Err(error) => {
            warn(&format!(
                "Could not create the BGM player: {}",
                js_error(error)
            ));
            return;
        }
    };
    audio.set_loop(true);
    audio.set_preload("auto");
    audio.set_volume(BGM_VOLUME);
    request_play(&audio);
    state.bgm_url = Some(url.to_owned());
    state.bgm = Some(audio);
}

pub(crate) fn play_map_sound(
    state: &mut AudioState,
    audio: Option<&MapAudio>,
    sound: MapSound,
    timestamp_ms: f64,
) {
    if let Some(descriptor) = audio.and_then(|audio| map_sound_descriptor(audio, sound)) {
        play_sound_url(state, &descriptor.url, timestamp_ms);
    }
}

pub(crate) fn play_sound_url(
    state: &mut AudioState,
    url: &str,
    timestamp_ms: f64,
) {
    let audio = match HtmlAudioElement::new_with_src(url) {
        Ok(audio) => audio,
        Err(error) => {
            warn(&format!(
                "Could not create a sound-effect player: {}",
                js_error(error)
            ));
            return;
        }
    };
    audio.set_preload("auto");
    audio.set_volume(SOUND_EFFECT_VOLUME);
    request_play(&audio);
    state.sounds.push(ActiveSound {
        element: audio,
        expires_at_ms: timestamp_ms + AUDIO_LIFETIME_MS,
    });
}

pub(crate) fn update(
    state: &mut AudioState,
    timestamp_ms: f64,
) {
    state
        .sounds
        .retain(|sound| timestamp_ms < sound.expires_at_ms && !sound.element.ended());
}

pub(crate) fn clear_sound_effects(state: &mut AudioState) {
    for sound in state.sounds.drain(..) {
        let _ = sound.element.pause();
    }
}

fn resume_audio(state: &AudioState) {
    if let Some(audio) = &state.bgm
        && audio.paused()
    {
        request_play(audio);
    }
    for sound in &state.sounds {
        if sound.element.paused() && !sound.element.ended() {
            request_play(&sound.element);
        }
    }
}

fn bgm_change_required(
    current_url: Option<&str>,
    next_url: Option<&str>,
) -> bool {
    current_url != next_url
}

fn request_play(audio: &HtmlAudioElement) {
    match audio.play() {
        Ok(promise) => spawn_local(async move {
            // Autoplay rejection is expected until the first keyboard or pointer input.
            let _ = JsFuture::from(promise).await;
        }),
        Err(error) => warn(&format!(
            "Could not start audio playback: {}",
            js_error(error)
        )),
    }
}

fn map_sound_descriptor(
    audio: &MapAudio,
    sound: MapSound,
) -> Option<&AssetDescriptor> {
    match sound {
        MapSound::Jump => audio.jump.as_ref(),
        MapSound::Portal => audio.portal.as_ref(),
        MapSound::PickUpItem => audio.pick_up_item.as_ref(),
        MapSound::DropItem => audio.drop_item.as_ref(),
        MapSound::UseItem => audio.use_item.as_ref(),
        MapSound::Tombstone => audio.tombstone.as_ref(),
        MapSound::LevelUp => audio.level_up.as_ref(),
        MapSound::QuestClear => audio.quest_clear.as_ref(),
    }
}

fn warn(message: &str) {
    web_sys::console::warn_1(&message.into());
}

#[cfg(test)]
mod tests {
    use oozems_proto::v1::AssetDescriptor;
    use oozems_proto::v1::MapAudio;

    use super::MapSound;
    use super::bgm_change_required;
    use super::map_sound_descriptor;

    #[test]
    fn map_sound_selection_returns_only_the_requested_cue() {
        let jump = AssetDescriptor {
            id: "jump".to_owned(),
            url: "/jump.mp3".to_owned(),
        };
        let portal = AssetDescriptor {
            id: "portal".to_owned(),
            url: "/portal.mp3".to_owned(),
        };
        let level_up = AssetDescriptor {
            id: "level-up".to_owned(),
            url: "/level-up.mp3".to_owned(),
        };
        let audio = MapAudio {
            jump: Some(jump.clone()),
            portal: Some(portal.clone()),
            level_up: Some(level_up.clone()),
            ..MapAudio::default()
        };

        assert_eq!(map_sound_descriptor(&audio, MapSound::Jump), Some(&jump));
        assert_eq!(
            map_sound_descriptor(&audio, MapSound::Portal),
            Some(&portal)
        );
        assert_eq!(
            map_sound_descriptor(&audio, MapSound::LevelUp),
            Some(&level_up)
        );
    }

    #[test]
    fn bgm_changes_only_when_its_url_changes() {
        assert!(!bgm_change_required(Some("/bgm.mp3"), Some("/bgm.mp3")));
        assert!(bgm_change_required(Some("/first.mp3"), Some("/next.mp3")));
        assert!(bgm_change_required(None, Some("/bgm.mp3")));
        assert!(bgm_change_required(Some("/bgm.mp3"), None));
    }
}
