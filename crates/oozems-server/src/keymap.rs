use std::collections::HashSet;

use oozems_proto::v1::KeyAction;
use oozems_proto::v1::KeyBinding;
use thiserror::Error;

#[derive(Clone, Copy, Debug)]
pub struct KeyActionSpec {
    pub action: KeyAction,
    pub label: &'static str,
    pub icon_id: &'static str,
    pub palette_index: usize,
}

#[derive(Clone, Copy, Debug)]
pub struct KeySlotSpec {
    pub code: &'static str,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum KeyBindingError {
    #[error("keyboard code {code:?} is not assignable")]
    UnsupportedCode { code: String },
    #[error("keyboard action {action} is not supported")]
    UnsupportedAction { action: i32 },
    #[error("keyboard binding for {code:?} must contain exactly one action or skill")]
    InvalidTarget { code: String },
    #[error("keyboard code {code:?} has more than one action")]
    DuplicateCode { code: String },
    #[error("keyboard action {action:?} has more than one key")]
    DuplicateAction { action: KeyAction },
    #[error("skill {skill_id} has more than one key")]
    DuplicateSkill { skill_id: u32 },
}

pub const ACTIONS: &[KeyActionSpec] = &[
    KeyActionSpec {
        action: KeyAction::Jump,
        label: "Jump",
        icon_id: "53",
        palette_index: 0,
    },
    KeyActionSpec {
        action: KeyAction::PickUp,
        label: "Pick up",
        icon_id: "50",
        palette_index: 1,
    },
    KeyActionSpec {
        action: KeyAction::OpenCharacter,
        label: "Character",
        icon_id: "2",
        palette_index: 2,
    },
    KeyActionSpec {
        action: KeyAction::OpenEquipment,
        label: "Equipment",
        icon_id: "0",
        palette_index: 3,
    },
    KeyActionSpec {
        action: KeyAction::OpenInventory,
        label: "Inventory",
        icon_id: "1",
        palette_index: 4,
    },
    KeyActionSpec {
        action: KeyAction::OpenKeyConfig,
        label: "Key settings",
        icon_id: "9",
        palette_index: 5,
    },
    KeyActionSpec {
        action: KeyAction::OpenSkills,
        label: "Skills",
        icon_id: "3",
        palette_index: 6,
    },
    KeyActionSpec {
        action: KeyAction::BasicAttack,
        label: "Basic Attack",
        icon_id: "52",
        palette_index: 7,
    },
];

pub const SLOTS: &[KeySlotSpec] = &[
    slot("Escape", 13.0, 28.0, 32.0, 32.0),
    slot("F1", 80.0, 28.0, 32.0, 32.0),
    slot("F2", 114.0, 28.0, 32.0, 32.0),
    slot("F3", 148.0, 28.0, 32.0, 32.0),
    slot("F4", 182.0, 28.0, 32.0, 32.0),
    slot("F5", 225.0, 28.0, 32.0, 32.0),
    slot("F6", 259.0, 28.0, 32.0, 32.0),
    slot("F7", 293.0, 28.0, 32.0, 32.0),
    slot("F8", 327.0, 28.0, 32.0, 32.0),
    slot("F9", 369.0, 28.0, 32.0, 32.0),
    slot("F10", 403.0, 28.0, 32.0, 32.0),
    slot("F11", 437.0, 28.0, 32.0, 32.0),
    slot("F12", 471.0, 28.0, 32.0, 32.0),
    slot("PrintScreen", 513.0, 28.0, 32.0, 32.0),
    slot("ScrollLock", 547.0, 28.0, 32.0, 32.0),
    slot("Pause", 581.0, 28.0, 32.0, 32.0),
    slot("Backquote", 13.0, 66.0, 32.0, 32.0),
    slot("Digit1", 47.0, 66.0, 32.0, 32.0),
    slot("Digit2", 81.0, 66.0, 32.0, 32.0),
    slot("Digit3", 115.0, 66.0, 32.0, 32.0),
    slot("Digit4", 149.0, 66.0, 32.0, 32.0),
    slot("Digit5", 183.0, 66.0, 32.0, 32.0),
    slot("Digit6", 217.0, 66.0, 32.0, 32.0),
    slot("Digit7", 251.0, 66.0, 32.0, 32.0),
    slot("Digit8", 285.0, 66.0, 32.0, 32.0),
    slot("Digit9", 319.0, 66.0, 32.0, 32.0),
    slot("Digit0", 353.0, 66.0, 32.0, 32.0),
    slot("Minus", 387.0, 66.0, 32.0, 32.0),
    slot("Equal", 421.0, 66.0, 32.0, 32.0),
    slot("Backspace", 455.0, 66.0, 48.0, 32.0),
    slot("Insert", 513.0, 66.0, 32.0, 32.0),
    slot("Home", 547.0, 66.0, 32.0, 32.0),
    slot("PageUp", 581.0, 66.0, 32.0, 32.0),
    slot("Tab", 13.0, 100.0, 49.0, 32.0),
    slot("KeyQ", 64.0, 100.0, 32.0, 32.0),
    slot("KeyW", 98.0, 100.0, 32.0, 32.0),
    slot("KeyE", 132.0, 100.0, 32.0, 32.0),
    slot("KeyR", 166.0, 100.0, 32.0, 32.0),
    slot("KeyT", 200.0, 100.0, 32.0, 32.0),
    slot("KeyY", 234.0, 100.0, 32.0, 32.0),
    slot("KeyU", 268.0, 100.0, 32.0, 32.0),
    slot("KeyI", 302.0, 100.0, 32.0, 32.0),
    slot("KeyO", 336.0, 100.0, 32.0, 32.0),
    slot("KeyP", 370.0, 100.0, 32.0, 32.0),
    slot("BracketLeft", 404.0, 100.0, 32.0, 32.0),
    slot("BracketRight", 438.0, 100.0, 32.0, 32.0),
    slot("Backslash", 472.0, 100.0, 32.0, 32.0),
    slot("Delete", 513.0, 100.0, 32.0, 32.0),
    slot("End", 547.0, 100.0, 32.0, 32.0),
    slot("PageDown", 581.0, 100.0, 32.0, 32.0),
    slot("CapsLock", 13.0, 133.0, 66.0, 32.0),
    slot("KeyA", 81.0, 133.0, 32.0, 32.0),
    slot("KeyS", 115.0, 133.0, 32.0, 32.0),
    slot("KeyD", 149.0, 133.0, 32.0, 32.0),
    slot("KeyF", 183.0, 133.0, 32.0, 32.0),
    slot("KeyG", 217.0, 133.0, 32.0, 32.0),
    slot("KeyH", 251.0, 133.0, 32.0, 32.0),
    slot("KeyJ", 285.0, 133.0, 32.0, 32.0),
    slot("KeyK", 319.0, 133.0, 32.0, 32.0),
    slot("KeyL", 353.0, 133.0, 32.0, 32.0),
    slot("Semicolon", 387.0, 133.0, 32.0, 32.0),
    slot("Quote", 421.0, 133.0, 32.0, 32.0),
    slot("Enter", 455.0, 133.0, 48.0, 32.0),
    slot("ShiftLeft", 13.0, 166.0, 82.0, 32.0),
    slot("KeyZ", 98.0, 166.0, 32.0, 32.0),
    slot("KeyX", 132.0, 166.0, 32.0, 32.0),
    slot("KeyC", 166.0, 166.0, 32.0, 32.0),
    slot("KeyV", 200.0, 166.0, 32.0, 32.0),
    slot("KeyB", 234.0, 166.0, 32.0, 32.0),
    slot("KeyN", 268.0, 166.0, 32.0, 32.0),
    slot("KeyM", 302.0, 166.0, 32.0, 32.0),
    slot("Comma", 336.0, 166.0, 32.0, 32.0),
    slot("Period", 370.0, 166.0, 32.0, 32.0),
    slot("Slash", 404.0, 166.0, 32.0, 32.0),
    slot("ShiftRight", 438.0, 166.0, 65.0, 32.0),
    slot("ControlLeft", 13.0, 199.0, 48.0, 32.0),
    slot("MetaLeft", 63.0, 199.0, 47.0, 32.0),
    slot("AltLeft", 112.0, 199.0, 51.0, 32.0),
    slot("Space", 165.0, 199.0, 167.0, 32.0),
    slot("AltRight", 334.0, 199.0, 53.0, 32.0),
    slot("ContextMenu", 390.0, 199.0, 55.0, 32.0),
    slot("ControlRight", 448.0, 199.0, 55.0, 32.0),
];

const fn slot(
    code: &'static str,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
) -> KeySlotSpec {
    KeySlotSpec {
        code,
        x,
        y,
        width,
        height,
    }
}

pub fn default_bindings() -> Vec<KeyBinding> {
    [
        ("AltLeft", KeyAction::Jump),
        ("KeyZ", KeyAction::PickUp),
        ("KeyC", KeyAction::OpenCharacter),
        ("KeyE", KeyAction::OpenEquipment),
        ("KeyI", KeyAction::OpenInventory),
        ("KeyK", KeyAction::OpenKeyConfig),
        ("KeyS", KeyAction::OpenSkills),
        ("ControlLeft", KeyAction::BasicAttack),
    ]
    .into_iter()
    .map(|(code, action)| KeyBinding {
        code: code.to_owned(),
        action: action as i32,
        skill_id: 0,
    })
    .collect()
}

pub fn validate_bindings(bindings: &[KeyBinding]) -> Result<(), KeyBindingError> {
    let mut codes = HashSet::new();
    let mut actions = HashSet::new();
    let mut skills = HashSet::new();
    for binding in bindings {
        if !SLOTS.iter().any(|slot| slot.code == binding.code) {
            return Err(KeyBindingError::UnsupportedCode {
                code: binding.code.clone(),
            });
        }
        let has_action = binding.action != KeyAction::Unspecified as i32;
        let has_skill = binding.skill_id != 0;
        if has_action == has_skill {
            return Err(KeyBindingError::InvalidTarget {
                code: binding.code.clone(),
            });
        }
        if !codes.insert(binding.code.clone()) {
            return Err(KeyBindingError::DuplicateCode {
                code: binding.code.clone(),
            });
        }
        if has_action {
            let action = KeyAction::try_from(binding.action).map_err(|_| {
                KeyBindingError::UnsupportedAction {
                    action: binding.action,
                }
            })?;
            if !ACTIONS.iter().any(|spec| spec.action == action) {
                return Err(KeyBindingError::UnsupportedAction {
                    action: binding.action,
                });
            }
            if !actions.insert(action) {
                return Err(KeyBindingError::DuplicateAction { action });
            }
        } else if !skills.insert(binding.skill_id) {
            return Err(KeyBindingError::DuplicateSkill {
                skill_id: binding.skill_id,
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use oozems_proto::v1::KeyAction;
    use oozems_proto::v1::KeyBinding;

    use super::KeyBindingError;
    use super::default_bindings;
    use super::validate_bindings;

    #[test]
    fn defaults_are_valid_and_cover_every_supported_action() {
        let bindings = default_bindings();

        validate_bindings(&bindings).expect("valid default bindings");
        assert_eq!(bindings.len(), super::ACTIONS.len());
        assert!(bindings.iter().any(|binding| {
            binding.code == "ControlLeft" && binding.action == KeyAction::BasicAttack as i32
        }));
        assert!(bindings.iter().any(|binding| {
            binding.code == "AltLeft" && binding.action == KeyAction::Jump as i32
        }));
    }

    #[test]
    fn duplicate_actions_are_rejected() {
        let bindings = vec![
            KeyBinding {
                code: "KeyA".to_owned(),
                action: KeyAction::Jump as i32,
                skill_id: 0,
            },
            KeyBinding {
                code: "KeyB".to_owned(),
                action: KeyAction::Jump as i32,
                skill_id: 0,
            },
        ];

        assert_eq!(
            validate_bindings(&bindings),
            Err(KeyBindingError::DuplicateAction {
                action: KeyAction::Jump
            })
        );
    }

    #[test]
    fn keymaps_saved_before_default_changes_remain_valid() {
        let mut bindings = default_bindings()
            .into_iter()
            .filter(|binding| binding.action != KeyAction::BasicAttack as i32)
            .collect::<Vec<_>>();
        bindings
            .iter_mut()
            .find(|binding| binding.action == KeyAction::Jump as i32)
            .expect("Jump binding")
            .code = "Space".to_owned();

        validate_bindings(&bindings).expect("legacy keymap");
    }

    #[test]
    fn skill_targets_are_validated_and_unique() {
        let duplicate = vec![
            KeyBinding {
                code: "KeyA".to_owned(),
                action: KeyAction::Unspecified as i32,
                skill_id: 1_000,
            },
            KeyBinding {
                code: "KeyB".to_owned(),
                action: KeyAction::Unspecified as i32,
                skill_id: 1_000,
            },
        ];
        assert_eq!(
            validate_bindings(&duplicate),
            Err(KeyBindingError::DuplicateSkill { skill_id: 1_000 })
        );

        let ambiguous = [KeyBinding {
            code: "KeyA".to_owned(),
            action: KeyAction::Jump as i32,
            skill_id: 1_000,
        }];
        assert_eq!(
            validate_bindings(&ambiguous),
            Err(KeyBindingError::InvalidTarget {
                code: "KeyA".to_owned(),
            })
        );
    }

    #[test]
    fn qwerty_slots_follow_the_wide_tab_key() {
        let tab = super::SLOTS
            .iter()
            .find(|slot| slot.code == "Tab")
            .expect("Tab slot");
        let q = super::SLOTS
            .iter()
            .find(|slot| slot.code == "KeyQ")
            .expect("Q slot");
        let backslash = super::SLOTS
            .iter()
            .find(|slot| slot.code == "Backslash")
            .expect("Backslash slot");

        assert_eq!((tab.x, tab.width), (13.0, 49.0));
        assert_eq!((q.x, q.width), (64.0, 32.0));
        assert_eq!((backslash.x, backslash.width), (472.0, 32.0));
    }
}
