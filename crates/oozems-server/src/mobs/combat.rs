use oozems_proto::v1::CombatEvent;
use oozems_proto::v1::CombatEventKind;
use shipyard::EntitiesViewMut;
use shipyard::IntoIter;
use shipyard::UniqueView;
use shipyard::UniqueViewMut;
use shipyard::View;
use shipyard::ViewMut;

use super::ai::next_random;
use super::ai::reset_mob;
use super::components::CombatFormulas;
use super::components::CombatRules;
use super::components::MobCombat;
use super::components::MobIdentity;
use super::components::MobMotion;
use super::components::PendingEvents;
use super::components::PlayerPresence;
use super::components::PlayerTarget;
use super::components::Position;
use super::components::Projectile;
use super::components::ProjectileSpawn;
use super::components::ProjectileSpawns;
use super::components::SimulationErrors;
use super::components::TargetCache;
use super::components::Tick;
use crate::skill_formula::FormulaCatalog;
use crate::skill_formula::FormulaEvaluationError;
use crate::skill_formula::evaluate_damage_profile;

const ATTACK_ANIMATION_MS: u64 = 500;
const PROJECTILE_LIFETIME_MS: u64 = 5_000;

#[derive(Clone, Copy)]
enum MobDamageKind {
    Physical,
    Magical,
}

impl MobDamageKind {
    fn profile(self) -> &'static str {
        match self {
            Self::Physical => "physical",
            Self::Magical => "magical",
        }
    }
}

pub(super) fn collect_player_targets(
    positions: View<Position>,
    players: View<PlayerPresence>,
    mut targets: UniqueViewMut<TargetCache>,
) {
    targets.0 = (&positions, &players)
        .iter()
        .map(|(position, player)| PlayerTarget {
            id: player.id.clone(),
            position: *position,
            level: player.level,
            current_hp: player.current_hp,
        })
        .collect();
}

pub(super) fn retain_aggro(
    positions: View<Position>,
    mut combats: ViewMut<MobCombat>,
    targets: UniqueView<TargetCache>,
    rules: UniqueView<CombatRules>,
) {
    for (position, combat) in (&positions, &mut combats).iter() {
        if combat.current_hp == 0 {
            combat.aggro_target = None;
            continue;
        }
        let retained = combat.aggro_target.as_ref().is_some_and(|target_id| {
            targets.0.iter().any(|target| {
                target.id == *target_id
                    && target.current_hp > 0
                    && target.position.layer == position.layer
                    && distance(*position, target.position) <= rules.0.disengage_range
            })
        });
        if !retained {
            combat.aggro_target = None;
        }
    }
}

pub(super) fn respawn_mobs(
    mut positions: ViewMut<Position>,
    mut motions: ViewMut<MobMotion>,
    mut combats: ViewMut<MobCombat>,
    identities: View<MobIdentity>,
    targets: UniqueView<TargetCache>,
    tick: UniqueView<Tick>,
    mut events: UniqueViewMut<PendingEvents>,
) {
    for (mut position, motion, combat, identity) in
        (&mut positions, &mut motions, &mut combats, &identities).iter()
    {
        if combat.current_hp != 0
            || combat
                .dead_until_ms
                .is_none_or(|deadline| tick.now_ms < deadline)
        {
            continue;
        }
        combat.current_hp = combat.maximum_hp;
        combat.dead_until_ms = None;
        combat.aggro_target = None;
        combat.next_attack_ms = tick.now_ms;
        // Movement runs later in this workload. Hold the mob for this tick so
        // clients receive its exact spawn position before normal AI resumes.
        combat.movement_resume_ms = tick.now_ms.saturating_add(1);
        reset_mob(&mut position, motion);
        for target in &targets.0 {
            queue_event(
                &mut events,
                &target.id,
                CombatEventKind::MobRespawned,
                &identity.public_id,
                &identity.public_id,
                0,
                *position,
            );
        }
    }
}

// Shipyard uses the signature as the system's explicit borrow schedule.
#[allow(clippy::too_many_arguments)]
pub(super) fn apply_touch_damage(
    positions: View<Position>,
    identities: View<MobIdentity>,
    combats: View<MobCombat>,
    motions: View<MobMotion>,
    mut players: ViewMut<PlayerPresence>,
    tick: UniqueView<Tick>,
    rules: UniqueView<CombatRules>,
    formulas: UniqueView<CombatFormulas>,
    mut events: UniqueViewMut<PendingEvents>,
    mut errors: UniqueViewMut<SimulationErrors>,
) {
    let touching_mobs = (&positions, &identities, &combats, &motions)
        .iter()
        .filter(|(_, _, combat, _)| combat.current_hp > 0 && combat.body_attack)
        .map(|(position, identity, combat, motion)| {
            (
                *position,
                identity.clone(),
                combat.clone(),
                motion.random_state,
            )
        })
        .collect::<Vec<_>>();

    for (player_position, player) in (&positions, &mut players).iter() {
        if player.current_hp == 0 || player.invulnerable_until_ms > tick.now_ms {
            continue;
        }
        let Some((_mob_position, identity, combat, random_state)) =
            touching_mobs.iter().find(|(mob_position, _, _, _)| {
                mob_position.layer == player_position.layer
                    && (mob_position.x - player_position.x).abs() <= rules.0.touch_horizontal_reach
                    && (mob_position.y - player_position.y).abs() <= rules.0.touch_vertical_reach
            })
        else {
            continue;
        };
        let mut random_state = *random_state ^ tick.now_ms;
        let damage = match calculate_mob_damage(
            &formulas.0,
            MobDamageKind::Physical,
            combat.physical_attack,
            combat.level,
            player.level,
            &mut random_state,
        ) {
            Ok(damage) => damage,
            Err(error) => {
                errors.0.push(format!(
                    "touch damage from mob {} failed: {error}",
                    identity.public_id
                ));
                continue;
            }
        };
        player.current_hp = player
            .current_hp
            .saturating_sub(u32::try_from(damage).unwrap_or(u32::MAX));
        player.invulnerable_until_ms = tick
            .now_ms
            .saturating_add(duration_millis(rules.0.player_invulnerability));
        queue_event(
            &mut events,
            &player.id,
            CombatEventKind::MobTouchedPlayer,
            &identity.public_id,
            &player.id,
            damage,
            *player_position,
        );
    }
}

// Shipyard uses the signature as the system's explicit borrow schedule.
#[allow(clippy::too_many_arguments)]
pub(super) fn queue_projectile_attacks(
    positions: View<Position>,
    identities: View<MobIdentity>,
    mut combats: ViewMut<MobCombat>,
    mut motions: ViewMut<MobMotion>,
    targets: UniqueView<TargetCache>,
    tick: UniqueView<Tick>,
    rules: UniqueView<CombatRules>,
    formulas: UniqueView<CombatFormulas>,
    mut spawns: UniqueViewMut<ProjectileSpawns>,
    mut errors: UniqueViewMut<SimulationErrors>,
) {
    for (position, identity, combat, motion) in
        (&positions, &identities, &mut combats, &mut motions).iter()
    {
        if combat.current_hp == 0 || combat.magic_attack <= 0 || tick.now_ms < combat.next_attack_ms
        {
            continue;
        }
        let Some(target) = combat
            .aggro_target
            .as_deref()
            .and_then(|target_id| targets.0.iter().find(|target| target.id == target_id))
            .filter(|target| {
                target.current_hp > 0
                    && target.position.layer == position.layer
                    && distance(*position, target.position) <= rules.0.projectile_range
            })
        else {
            continue;
        };
        let damage = match calculate_mob_damage(
            &formulas.0,
            MobDamageKind::Magical,
            combat.magic_attack,
            combat.level,
            target.level,
            &mut motion.random_state,
        ) {
            Ok(damage) => damage,
            Err(error) => {
                errors.0.push(format!(
                    "projectile damage from mob {} failed: {error}",
                    identity.public_id
                ));
                continue;
            }
        };
        spawns.0.push(ProjectileSpawn {
            source_mob_id: identity.public_id.clone(),
            target_player_id: target.id.clone(),
            position: *position,
            damage,
        });
        combat.next_attack_ms = tick
            .now_ms
            .saturating_add(duration_millis(rules.0.mob_attack_interval));
        combat.attack_until_ms = tick.now_ms.saturating_add(ATTACK_ANIMATION_MS);
        motion.mode = oozems_proto::v1::MobMovementMode::Attacking;
    }
}

pub(super) fn spawn_projectiles(
    mut entities: EntitiesViewMut,
    mut positions: ViewMut<Position>,
    mut projectiles: ViewMut<Projectile>,
    tick: UniqueView<Tick>,
    rules: UniqueView<CombatRules>,
    mut spawns: UniqueViewMut<ProjectileSpawns>,
    mut events: UniqueViewMut<PendingEvents>,
) {
    for spawn in spawns.0.drain(..) {
        let id = next_id(&mut events, "projectile");
        entities.add_entity(
            (&mut positions, &mut projectiles),
            (
                spawn.position,
                Projectile {
                    public_id: id,
                    source_mob_id: spawn.source_mob_id,
                    target_player_id: spawn.target_player_id,
                    speed: rules.0.projectile_speed,
                    damage: spawn.damage,
                    expires_at_ms: tick.now_ms.saturating_add(PROJECTILE_LIFETIME_MS),
                    impacted: false,
                },
            ),
        );
    }
}

pub(super) fn advance_projectiles(
    mut positions: ViewMut<Position>,
    mut projectiles: ViewMut<Projectile>,
    mut players: ViewMut<PlayerPresence>,
    targets: UniqueView<TargetCache>,
    tick: UniqueView<Tick>,
    rules: UniqueView<CombatRules>,
    mut events: UniqueViewMut<PendingEvents>,
) {
    for (mut position, projectile) in (&mut positions, &mut projectiles).iter() {
        if projectile.impacted || tick.now_ms >= projectile.expires_at_ms {
            projectile.impacted = true;
            continue;
        }
        let Some(target) = targets
            .0
            .iter()
            .find(|target| target.id == projectile.target_player_id && target.current_hp > 0)
        else {
            projectile.impacted = true;
            continue;
        };
        let delta_x = target.position.x - position.x;
        let delta_y = target.position.y - position.y;
        let distance = delta_x.hypot(delta_y);
        if distance > 0.0 {
            let travel = (projectile.speed * tick.elapsed_seconds).min(distance);
            position.x += delta_x / distance * travel;
            position.y += delta_y / distance * travel;
        }
        position.layer = target.position.layer;
        if distance > rules.0.projectile_hit_reach
            && distance - projectile.speed * tick.elapsed_seconds > rules.0.projectile_hit_reach
        {
            continue;
        }
        projectile.impacted = true;
        let Some(player) = (&mut players)
            .iter()
            .find(|player| player.id == projectile.target_player_id)
        else {
            continue;
        };
        if player.invulnerable_until_ms > tick.now_ms || player.current_hp == 0 {
            continue;
        }
        player.current_hp = player
            .current_hp
            .saturating_sub(u32::try_from(projectile.damage).unwrap_or(u32::MAX));
        player.invulnerable_until_ms = tick
            .now_ms
            .saturating_add(duration_millis(rules.0.player_invulnerability));
        queue_event(
            &mut events,
            &player.id,
            CombatEventKind::MobProjectileHitPlayer,
            &projectile.source_mob_id,
            &player.id,
            projectile.damage,
            *position,
        );
    }
}

fn calculate_mob_damage(
    formulas: &FormulaCatalog,
    kind: MobDamageKind,
    attack: i32,
    monster_level: u32,
    player_level: u32,
    random_state: &mut u64,
) -> Result<u64, FormulaEvaluationError> {
    let Some(profile) = formulas.defense_profile(kind.profile()) else {
        return Ok(u64::try_from(attack.max(1)).unwrap_or(1));
    };
    let range = evaluate_damage_profile(
        profile,
        &[
            ("DamageBeforeDefense", f64::from(attack.max(1))),
            ("MonsterLevel", f64::from(monster_level)),
            ("PlayerLevel", f64::from(player_level)),
            ("WeaponDefense", 0.0),
            ("MagicDefense", 0.0),
        ],
    )?;
    let minimum = final_damage(range.minimum);
    let maximum = final_damage(range.maximum).max(minimum);
    let width = maximum.saturating_sub(minimum).saturating_add(1);
    Ok(minimum.saturating_add(next_random(random_state) % width))
}

#[allow(clippy::too_many_arguments)]
pub(super) fn calculate_player_damage(
    formulas: &FormulaCatalog,
    physical_defense: i32,
    monster_level: u32,
    player_level: u32,
    minimum_damage: u32,
    maximum_damage: u32,
    fixed_damage: bool,
    random_state: &mut u64,
) -> Result<u64, FormulaEvaluationError> {
    let (minimum, maximum) = if fixed_damage {
        (u64::from(minimum_damage), u64::from(maximum_damage))
    } else if let Some(profile) = formulas.defense_profile("physical") {
        let variables = |damage| {
            [
                ("DamageBeforeDefense", f64::from(damage)),
                ("MonsterLevel", f64::from(monster_level)),
                ("PlayerLevel", f64::from(player_level)),
                ("WeaponDefense", f64::from(physical_defense.max(0))),
                ("MagicDefense", 0.0),
            ]
        };
        let minimum = evaluate_damage_profile(profile, &variables(minimum_damage))?.minimum;
        let maximum = evaluate_damage_profile(profile, &variables(maximum_damage))?.maximum;
        (final_damage(minimum), final_damage(maximum))
    } else {
        (u64::from(minimum_damage), u64::from(maximum_damage))
    };
    let maximum = maximum.max(minimum);
    let width = maximum.saturating_sub(minimum).saturating_add(1);
    Ok(minimum.saturating_add(next_random(random_state) % width))
}

fn final_damage(value: f64) -> u64 {
    if !value.is_finite() {
        return 1;
    }
    value.trunc().max(1.0).min(u64::MAX as f64) as u64
}

fn distance(
    left: Position,
    right: Position,
) -> f32 {
    (left.x - right.x).hypot(left.y - right.y)
}

pub(super) fn next_id(
    events: &mut PendingEvents,
    prefix: &str,
) -> String {
    events.next_sequence = events.next_sequence.saturating_add(1);
    format!("{prefix}:{}", events.next_sequence)
}

pub(super) fn queue_event(
    events: &mut PendingEvents,
    recipient: &str,
    kind: CombatEventKind,
    source_id: &str,
    target_id: &str,
    damage: u64,
    position: Position,
) {
    let event = CombatEvent {
        id: next_id(events, "combat"),
        kind: kind as i32,
        source_id: source_id.to_owned(),
        target_id: target_id.to_owned(),
        damage,
        position: Some(position.vector()),
    };
    events
        .by_player
        .entry(recipient.to_owned())
        .or_default()
        .push(event);
}

fn duration_millis(duration: std::time::Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::MobDamageKind;
    use super::calculate_mob_damage;
    use super::calculate_player_damage;
    use crate::skill_formula::FormulaCatalog;

    #[test]
    fn configured_defense_formula_controls_mob_damage() {
        let formulas = FormulaCatalog::load(Path::new("../../config/skill-formulas.toml"))
            .expect("formula catalog");
        let mut random_state = 1;

        let damage = calculate_mob_damage(
            &formulas,
            MobDamageKind::Physical,
            20,
            10,
            10,
            &mut random_state,
        )
        .expect("mob damage");

        assert!((20..=20).contains(&damage));
    }

    #[test]
    fn mob_defense_reduces_nonfixed_player_damage() {
        let formulas = FormulaCatalog::load(Path::new("../../config/skill-formulas.toml"))
            .expect("formula catalog");
        let mut random_state = 1;

        let damage =
            calculate_player_damage(&formulas, 10, 10, 10, 20, 20, false, &mut random_state)
                .expect("player damage");

        assert!((14..=15).contains(&damage));
    }

    #[test]
    fn fixed_damage_ignores_mob_defense() {
        let formulas = FormulaCatalog::load(Path::new("../../config/skill-formulas.toml"))
            .expect("formula catalog");
        let mut random_state = 1;

        let damage =
            calculate_player_damage(&formulas, 10_000, 100, 1, 10, 10, true, &mut random_state)
                .expect("fixed damage");

        assert_eq!(damage, 10);
    }
}
