use std::collections::HashSet;

use oozems_proto::v1::KeyAction;
use oozems_proto::v1::KeyBinding;

use crate::movement::PlayerInput;

#[derive(Default)]
pub struct KeyboardState {
    pressed: HashSet<String>,
    just_pressed: HashSet<String>,
}

#[derive(Debug, PartialEq)]
pub struct FrameInput {
    pub player: PlayerInput,
    pub actions: Vec<KeyAction>,
    pub skills: Vec<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BindingTarget {
    Action(KeyAction),
    Skill(u32),
}

pub fn set_key(
    state: &mut KeyboardState,
    bindings: &[KeyBinding],
    code: &str,
    pressed: bool,
) -> bool {
    let handled = is_direction_code(code) || bindings.iter().any(|binding| binding.code == code);
    if !handled {
        if !pressed {
            state.pressed.remove(code);
        }
        return false;
    }
    if pressed {
        if state.pressed.insert(code.to_owned()) {
            state.just_pressed.insert(code.to_owned());
        }
    } else {
        state.pressed.remove(code);
    }
    true
}

pub fn drain_frame_input(
    state: &mut KeyboardState,
    bindings: &[KeyBinding],
) -> FrameInput {
    let actions = bindings
        .iter()
        .filter(|binding| state.just_pressed.contains(&binding.code))
        .filter(|binding| binding.skill_id == 0)
        .filter_map(|binding| KeyAction::try_from(binding.action).ok())
        .collect::<Vec<_>>();
    let skills = bindings
        .iter()
        .filter(|binding| state.just_pressed.contains(&binding.code))
        .filter_map(|binding| (binding.skill_id != 0).then_some(binding.skill_id))
        .collect::<Vec<_>>();
    let player = PlayerInput {
        horizontal: axis(&state.pressed, "ArrowRight", "ArrowLeft"),
        vertical: axis(&state.pressed, "ArrowDown", "ArrowUp"),
        jump_pressed: action_is_just_pressed(state, bindings, KeyAction::Jump),
        portal_pressed: state.just_pressed.contains("ArrowUp"),
        ..PlayerInput::default()
    };
    state.just_pressed.clear();
    FrameInput {
        player,
        actions,
        skills,
    }
}

pub fn assign_target(
    bindings: &[KeyBinding],
    code: &str,
    target: BindingTarget,
) -> Vec<KeyBinding> {
    let mut updated = bindings
        .iter()
        .filter(|binding| binding.code != code && binding_target(binding) != Some(target))
        .cloned()
        .collect::<Vec<_>>();
    let (action, skill_id) = match target {
        BindingTarget::Action(action) => (action as i32, 0),
        BindingTarget::Skill(skill_id) => (KeyAction::Unspecified as i32, skill_id),
    };
    updated.push(KeyBinding {
        code: code.to_owned(),
        action,
        skill_id,
    });
    updated.sort_by(|left, right| left.code.cmp(&right.code));
    updated
}

pub fn target_for_code(
    bindings: &[KeyBinding],
    code: &str,
) -> Option<BindingTarget> {
    bindings
        .iter()
        .find(|binding| binding.code == code)
        .and_then(binding_target)
}

fn binding_target(binding: &KeyBinding) -> Option<BindingTarget> {
    if binding.skill_id != 0 && binding.action == KeyAction::Unspecified as i32 {
        return Some(BindingTarget::Skill(binding.skill_id));
    }
    (binding.skill_id == 0)
        .then(|| KeyAction::try_from(binding.action).ok())
        .flatten()
        .filter(|action| *action != KeyAction::Unspecified)
        .map(BindingTarget::Action)
}

fn action_is_just_pressed(
    state: &KeyboardState,
    bindings: &[KeyBinding],
    action: KeyAction,
) -> bool {
    bindings.iter().any(|binding| {
        binding.action == action as i32 && state.just_pressed.contains(&binding.code)
    })
}

fn axis(
    pressed: &HashSet<String>,
    positive: &str,
    negative: &str,
) -> f32 {
    f32::from(pressed.contains(positive) as u8) - f32::from(pressed.contains(negative) as u8)
}

fn is_direction_code(code: &str) -> bool {
    matches!(code, "ArrowLeft" | "ArrowRight" | "ArrowUp" | "ArrowDown")
}

#[cfg(test)]
mod tests {
    use oozems_proto::v1::KeyAction;
    use oozems_proto::v1::KeyBinding;

    use super::BindingTarget;
    use super::KeyboardState;
    use super::assign_target;
    use super::drain_frame_input;
    use super::set_key;

    #[test]
    fn configured_actions_are_edge_triggered() {
        let bindings = vec![binding("Space", KeyAction::Jump)];
        let mut state = KeyboardState::default();

        assert!(set_key(&mut state, &bindings, "Space", true));
        let first = drain_frame_input(&mut state, &bindings);
        assert!(first.player.jump_pressed);
        assert_eq!(first.actions, vec![KeyAction::Jump]);

        let held = drain_frame_input(&mut state, &bindings);
        assert!(!held.player.jump_pressed);
        assert!(held.actions.is_empty());
        assert!(held.skills.is_empty());
    }

    #[test]
    fn arrows_drive_movement_and_up_interaction() {
        let mut state = KeyboardState::default();

        assert!(set_key(&mut state, &[], "ArrowLeft", true));
        assert!(set_key(&mut state, &[], "ArrowUp", true));
        let input = drain_frame_input(&mut state, &[]).player;

        assert_eq!(input.horizontal, -1.0);
        assert_eq!(input.vertical, -1.0);
        assert!(input.portal_pressed);
        assert!(!input.jump_pressed);
    }

    #[test]
    fn assigning_moves_an_action_and_replaces_the_target() {
        let bindings = vec![
            binding("Space", KeyAction::Jump),
            binding("KeyZ", KeyAction::PickUp),
        ];

        let updated = assign_target(&bindings, "KeyZ", BindingTarget::Action(KeyAction::Jump));

        assert_eq!(updated, vec![binding("KeyZ", KeyAction::Jump)]);
    }

    #[test]
    fn configured_skills_are_edge_triggered() {
        let bindings = vec![KeyBinding {
            code: "KeyA".to_owned(),
            action: KeyAction::Unspecified as i32,
            skill_id: 1_000,
        }];
        let mut state = KeyboardState::default();

        assert!(set_key(&mut state, &bindings, "KeyA", true));
        let first = drain_frame_input(&mut state, &bindings);
        assert_eq!(first.skills, vec![1_000]);
        assert!(first.actions.is_empty());
        assert!(drain_frame_input(&mut state, &bindings).skills.is_empty());
    }

    fn binding(
        code: &str,
        action: KeyAction,
    ) -> KeyBinding {
        KeyBinding {
            code: code.to_owned(),
            action: action as i32,
            skill_id: 0,
        }
    }
}
