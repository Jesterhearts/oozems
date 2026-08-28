use std::collections::HashMap;

use oozems_proto::v1::EquipmentSlot;
use wz_reader::WzNodeArc;
use wz_reader::property::Vector2D;

use super::CharacterContentError;
use super::equipment_specs;
use super::required_style;
use crate::attacks::AttackReach;
use crate::content::wz::child;
use crate::content::wz::int_value;
use crate::content::wz::parse;
use crate::content::wz::string_value;
use crate::content::wz::vector_value;

pub(super) struct WeaponAttackSource {
    pub(super) action: WzNodeArc,
    pub(super) reach: AttackReach,
}

pub(super) struct WeaponAttacks {
    pub(super) bare_hands: AttackReach,
    pub(super) equipped: HashMap<u32, WeaponAttackSource>,
}

pub(super) fn load(
    root: &WzNodeArc,
    equipment: &HashMap<u32, WzNodeArc>,
) -> Result<WeaponAttacks, CharacterContentError> {
    let afterimages = required_node(root, "Afterimage", "Character.wz")?;
    parse(&afterimages, "Character.wz Afterimage".to_owned())?;
    let bare_hands_profile = load_profile(&afterimages, "barehands")?;
    let bare_hands_action = required_action(&bare_hands_profile, "0", "stabO1", "barehands")?;
    let bare_hands = attack_reach(&bare_hands_action, "barehands", "stabO1")?;
    let equipped = equipment_specs()
        .into_iter()
        .filter(|(_, slot)| *slot == EquipmentSlot::Weapon)
        .map(|(item_id, _)| {
            let weapon = required_style(equipment, item_id, "equipment weapon")?;
            parse(&weapon, format!("Character.wz weapon {item_id:08}"))?;
            let info = required_node(&weapon, "info", &format!("weapon {item_id:08}"))?;
            let afterimage_name = string_value(&info, "afterImage")?.ok_or_else(|| {
                CharacterContentError::Invalid {
                    message: format!("weapon {item_id:08} has no afterImage profile"),
                }
            })?;
            let attack_speed =
                int_value(&info, "attackSpeed")?.ok_or_else(|| CharacterContentError::Invalid {
                    message: format!("weapon {item_id:08} has no attackSpeed"),
                })?;
            let profile = load_profile(&afterimages, &afterimage_name)?;
            let action = required_action(
                &profile,
                &attack_speed.to_string(),
                "swingO1",
                &afterimage_name,
            )?;
            let reach = attack_reach(&action, &afterimage_name, "swingO1")?;
            Ok((item_id, WeaponAttackSource { action, reach }))
        })
        .collect::<Result<HashMap<_, _>, CharacterContentError>>()?;
    Ok(WeaponAttacks {
        bare_hands,
        equipped,
    })
}

fn load_profile(
    afterimages: &WzNodeArc,
    name: &str,
) -> Result<WzNodeArc, CharacterContentError> {
    let profile_name = format!("{name}.img");
    let profile = required_node(afterimages, &profile_name, "Character.wz Afterimage")?;
    parse(&profile, format!("Character.wz Afterimage {profile_name}"))?;
    Ok(profile)
}

fn required_action(
    profile: &WzNodeArc,
    speed: &str,
    action: &str,
    profile_name: &str,
) -> Result<WzNodeArc, CharacterContentError> {
    let speed = required_node(
        profile,
        speed,
        &format!("afterimage profile {profile_name}"),
    )?;
    required_node(
        &speed,
        action,
        &format!("afterimage profile {profile_name}"),
    )
}

fn attack_reach(
    action: &WzNodeArc,
    profile_name: &str,
    action_name: &str,
) -> Result<AttackReach, CharacterContentError> {
    let left_top = required_vector(action, "lt", profile_name, action_name)?;
    let right_bottom = required_vector(action, "rb", profile_name, action_name)?;
    attack_reach_from_bounds(left_top, right_bottom).ok_or_else(|| CharacterContentError::Invalid {
        message: format!("afterimage profile {profile_name} has invalid attack bounds"),
    })
}

fn required_node(
    parent: &WzNodeArc,
    name: &str,
    context: &str,
) -> Result<WzNodeArc, CharacterContentError> {
    child(parent, name)?.ok_or_else(|| CharacterContentError::Invalid {
        message: format!("{context} has no {name}"),
    })
}

fn required_vector(
    parent: &WzNodeArc,
    name: &str,
    profile_name: &str,
    action_name: &str,
) -> Result<Vector2D, CharacterContentError> {
    let node = required_node(
        parent,
        name,
        &format!("afterimage profile {profile_name} {action_name}"),
    )?;
    vector_value(&node)?.ok_or_else(|| CharacterContentError::Invalid {
        message: format!("afterimage profile {profile_name} {action_name} {name} is not a vector"),
    })
}

pub(super) fn attack_reach_from_bounds(
    Vector2D(left, top): Vector2D,
    Vector2D(right, bottom): Vector2D,
) -> Option<AttackReach> {
    if left > right || top > bottom {
        return None;
    }
    let horizontal = left.unsigned_abs().max(right.unsigned_abs()) as f32;
    (horizontal > 0.0).then_some(AttackReach {
        horizontal,
        top: top as f32,
        bottom: bottom as f32,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use wz_reader::WzNode;
    use wz_reader::WzNodeArc;
    use wz_reader::property::Vector2D;
    use wz_reader::property::WzString;

    use super::attack_reach_from_bounds;
    use super::load;
    use crate::attacks::AttackReach;

    const WEAPON_IDS: [u32; 3] = [1_302_000, 1_312_004, 1_322_005];

    #[test]
    fn fake_weapon_content_loads_the_authored_swing_bounds() {
        let root = branch("root", None);
        let afterimages = branch("Afterimage", Some(&root));
        let bare_hands_profile = branch("barehands.img", Some(&afterimages));
        let bare_hands_speed = branch("0", Some(&bare_hands_profile));
        let bare_hands_action = branch("stabO1", Some(&bare_hands_speed));
        add(
            &bare_hands_action,
            WzNode::from_str("lt", Vector2D(-64, -33), Some(&bare_hands_action)).into_lock(),
        );
        add(
            &bare_hands_action,
            WzNode::from_str("rb", Vector2D(-27, -18), Some(&bare_hands_action)).into_lock(),
        );
        let profile = branch("swordOL.img", Some(&afterimages));
        let speed = branch("5", Some(&profile));
        let action = branch("swingO1", Some(&speed));
        add(
            &action,
            WzNode::from_str("lt", Vector2D(-88, -62), Some(&action)).into_lock(),
        );
        add(
            &action,
            WzNode::from_str("rb", Vector2D(-18, -6), Some(&action)).into_lock(),
        );
        let equipment = WEAPON_IDS
            .into_iter()
            .map(|item_id| {
                let weapon = branch(&format!("{item_id:08}.img"), None);
                let info = branch("info", Some(&weapon));
                add(
                    &info,
                    WzNode::from_str(
                        "afterImage",
                        WzString::from_str("swordOL", [0; 4]),
                        Some(&info),
                    )
                    .into_lock(),
                );
                add(
                    &info,
                    WzNode::from_str("attackSpeed", 5, Some(&info)).into_lock(),
                );
                (item_id, weapon)
            })
            .collect();

        let attacks = load(&root, &equipment).expect("fake weapon attacks");

        assert_eq!(
            attacks.bare_hands,
            AttackReach {
                horizontal: 64.0,
                top: -33.0,
                bottom: -18.0,
            }
        );

        for item_id in WEAPON_IDS {
            assert_eq!(
                attacks.equipped.get(&item_id).map(|attack| attack.reach),
                Some(AttackReach {
                    horizontal: 88.0,
                    top: -62.0,
                    bottom: -6.0,
                })
            );
        }
    }

    #[test]
    fn inverted_attack_bounds_are_rejected() {
        assert_eq!(
            attack_reach_from_bounds(Vector2D(1, -10), Vector2D(-1, 10)),
            None
        );
    }

    fn branch(
        name: &str,
        parent: Option<&WzNodeArc>,
    ) -> WzNodeArc {
        let node = WzNode::from_str(name, 0, parent).into_lock();
        if let Some(parent) = parent {
            add(parent, Arc::clone(&node));
        }
        node
    }

    fn add(
        parent: &WzNodeArc,
        child: WzNodeArc,
    ) {
        parent.write().expect("parent lock").add(&child);
    }
}
