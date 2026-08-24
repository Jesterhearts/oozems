use oozems_proto::v1::CombatEvent;
use oozems_proto::v1::CombatEventKind;
use shipyard::EntitiesViewMut;
use shipyard::IntoIter;
use shipyard::UniqueView;
use shipyard::UniqueViewMut;
use shipyard::View;
use shipyard::ViewMut;

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
use crate::jobs::SkillAttackType;
use crate::random::next_u64;
use crate::skill_formula::FormulaCatalog;
use crate::skill_formula::FormulaEvaluationError;
use crate::skill_formula::evaluate_damage_profile;
use crate::skill_formula::evaluate_profile_property;

const ATTACK_ANIMATION_MS: u64 = 500;
const PROJECTILE_LIFETIME_MS: u64 = 5_000;

#[derive(Clone, Copy, PartialEq, Eq)]
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
            magic_defense: player.magic_defense,
            avoidability: player.avoidability,
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
            || combat.player_attack_transaction.is_some()
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
        if player.current_hp == 0
            || player.invulnerable_until_ms > tick.now_ms
            || player.contact_attempt_after_ms > tick.now_ms
        {
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
        player.contact_attempt_after_ms = tick
            .now_ms
            .saturating_add(duration_millis(rules.0.player_invulnerability));
        let mut random_state = *random_state ^ tick.now_ms;
        let hit = match physical_attack_hits(
            &formulas.0,
            combat.accuracy,
            player.avoidability,
            combat.level,
            player.level,
            &mut random_state,
        ) {
            Ok(hit) => hit,
            Err(error) => {
                errors.0.push(format!(
                    "touch accuracy from mob {} failed: {error}",
                    identity.public_id
                ));
                continue;
            }
        };
        if !hit {
            queue_event(
                &mut events,
                &player.id,
                CombatEventKind::MobMissedPlayer,
                &identity.public_id,
                &player.id,
                0,
                *player_position,
            );
            continue;
        }
        let damage = match calculate_mob_damage(
            &formulas.0,
            MobDamageKind::Physical,
            combat.physical_attack,
            combat.level,
            player.level,
            player.weapon_defense,
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
        combat.next_attack_ms = tick
            .now_ms
            .saturating_add(duration_millis(rules.0.mob_attack_interval));
        combat.attack_until_ms = tick.now_ms.saturating_add(ATTACK_ANIMATION_MS);
        motion.mode = oozems_proto::v1::MobMovementMode::Attacking;
        let hit = match physical_attack_hits(
            &formulas.0,
            combat.accuracy,
            target.avoidability,
            combat.level,
            target.level,
            &mut motion.random_state,
        ) {
            Ok(hit) => hit,
            Err(error) => {
                errors.0.push(format!(
                    "projectile accuracy from mob {} failed: {error}",
                    identity.public_id
                ));
                continue;
            }
        };
        if !hit {
            spawns.0.push(ProjectileSpawn {
                source_mob_id: identity.public_id.clone(),
                target_player_id: target.id.clone(),
                position: target.position,
                damage: 0,
                missed: true,
            });
            continue;
        }
        let damage = match calculate_mob_damage(
            &formulas.0,
            MobDamageKind::Magical,
            combat.magic_attack,
            combat.level,
            target.level,
            target.magic_defense,
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
            missed: false,
        });
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
        if spawn.missed {
            queue_event(
                &mut events,
                &spawn.target_player_id,
                CombatEventKind::MobMissedPlayer,
                &spawn.source_mob_id,
                &spawn.target_player_id,
                0,
                spawn.position,
            );
            continue;
        }
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
    defense: i32,
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
            (
                "WeaponDefense",
                f64::from(
                    (kind == MobDamageKind::Physical)
                        .then_some(defense)
                        .unwrap_or_default()
                        .max(0),
                ),
            ),
            (
                "MagicDefense",
                f64::from(
                    (kind == MobDamageKind::Magical)
                        .then_some(defense)
                        .unwrap_or_default()
                        .max(0),
                ),
            ),
        ],
    )?;
    let minimum = final_damage(range.minimum);
    let maximum = final_damage(range.maximum).max(minimum);
    let width = maximum.saturating_sub(minimum).saturating_add(1);
    Ok(minimum.saturating_add(next_u64(random_state) % width))
}

pub(super) fn physical_attack_hits(
    formulas: &FormulaCatalog,
    accuracy: i32,
    avoidability: i32,
    monster_level: u32,
    player_level: u32,
    random_state: &mut u64,
) -> Result<bool, FormulaEvaluationError> {
    if avoidability <= 0 {
        return Ok(true);
    }
    if accuracy <= 0 {
        return Ok(false);
    }
    let Some(profile) = formulas.accuracy_profile("physical") else {
        return Ok(true);
    };
    let chance = evaluate_profile_property(
        profile,
        "hit_chance",
        &[
            ("Accuracy", f64::from(accuracy)),
            ("Avoidability", f64::from(avoidability)),
            ("MonsterLevel", f64::from(monster_level)),
            ("PlayerLevel", f64::from(player_level)),
        ],
    )?
    .clamp(0.0, 1.0);
    let threshold = (chance * 1_000_000.0).trunc() as u64;
    Ok(next_u64(random_state) % 1_000_000 < threshold)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn player_attack_hits(
    formulas: &FormulaCatalog,
    attack_type: SkillAttackType,
    physical_accuracy: i32,
    accuracy_bonus: i32,
    intelligence: u32,
    luck: u32,
    avoidability: i32,
    monster_level: u32,
    player_level: u32,
    random_state: &mut u64,
) -> Result<bool, FormulaEvaluationError> {
    match attack_type {
        SkillAttackType::Physical => physical_attack_hits(
            formulas,
            physical_accuracy,
            avoidability,
            monster_level,
            player_level,
            random_state,
        ),
        SkillAttackType::Magical => magical_attack_hits(
            formulas,
            accuracy_bonus,
            intelligence,
            luck,
            avoidability,
            monster_level,
            player_level,
            random_state,
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn magical_attack_hits(
    formulas: &FormulaCatalog,
    accuracy_bonus: i32,
    intelligence: u32,
    luck: u32,
    avoidability: i32,
    monster_level: u32,
    player_level: u32,
    random_state: &mut u64,
) -> Result<bool, FormulaEvaluationError> {
    let Some(profile) = formulas.accuracy_profile("magical") else {
        return Ok(true);
    };
    let stats = [
        ("Intelligence", f64::from(intelligence)),
        ("Luck", f64::from(luck)),
    ];
    let accuracy =
        evaluate_profile_property(profile, "accuracy", &stats)? + f64::from(accuracy_bonus);
    let ratio = evaluate_profile_property(
        profile,
        "ratio",
        &[
            ("Accuracy", accuracy.max(0.0)),
            ("Intelligence", f64::from(intelligence)),
            ("Luck", f64::from(luck)),
            ("Avoidability", f64::from(avoidability.max(0))),
            ("MonsterLevel", f64::from(monster_level)),
            ("PlayerLevel", f64::from(player_level)),
        ],
    )?;
    let chance = evaluate_profile_property(
        profile,
        "hit_rate",
        &[("AccuracyRatio", ratio.clamp(0.0, 1.0))],
    )?
    .clamp(0.0, 1.0);
    let threshold = (chance * 1_000_000.0).trunc() as u64;
    Ok(next_u64(random_state) % 1_000_000 < threshold)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn calculate_player_damage(
    formulas: &FormulaCatalog,
    attack_type: SkillAttackType,
    defense: i32,
    monster_level: u32,
    player_level: u32,
    minimum_damage: u32,
    maximum_damage: u32,
    fixed_damage: bool,
    random_state: &mut u64,
) -> Result<u64, FormulaEvaluationError> {
    let (minimum, maximum) = if fixed_damage {
        (u64::from(minimum_damage), u64::from(maximum_damage))
    } else if let Some(profile) = formulas.defense_profile(match attack_type {
        SkillAttackType::Physical => "physical",
        SkillAttackType::Magical => "magical",
    }) {
        let variables = |damage| {
            [
                ("DamageBeforeDefense", f64::from(damage)),
                ("MonsterLevel", f64::from(monster_level)),
                ("PlayerLevel", f64::from(player_level)),
                (
                    "WeaponDefense",
                    f64::from(
                        (attack_type == SkillAttackType::Physical)
                            .then_some(defense)
                            .unwrap_or_default()
                            .max(0),
                    ),
                ),
                (
                    "MagicDefense",
                    f64::from(
                        (attack_type == SkillAttackType::Magical)
                            .then_some(defense)
                            .unwrap_or_default()
                            .max(0),
                    ),
                ),
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
    Ok(minimum.saturating_add(next_u64(random_state) % width))
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
    use super::physical_attack_hits;
    use super::player_attack_hits;
    use crate::jobs::SkillAttackType;
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
            0,
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

        let damage = calculate_player_damage(
            &formulas,
            SkillAttackType::Physical,
            10,
            10,
            10,
            20,
            20,
            false,
            &mut random_state,
        )
        .expect("player damage");

        assert!((14..=15).contains(&damage));
    }

    #[test]
    fn fixed_damage_ignores_mob_defense() {
        let formulas = FormulaCatalog::load(Path::new("../../config/skill-formulas.toml"))
            .expect("formula catalog");
        let mut random_state = 1;

        let damage = calculate_player_damage(
            &formulas,
            SkillAttackType::Physical,
            10_000,
            100,
            1,
            10,
            10,
            true,
            &mut random_state,
        )
        .expect("fixed damage");

        assert_eq!(damage, 10);
    }

    #[test]
    fn physical_accuracy_formula_has_deterministic_hit_and_miss_boundaries() {
        let formulas = FormulaCatalog::load(Path::new("../../config/skill-formulas.toml"))
            .expect("formula catalog");

        assert!(
            physical_attack_hits(&formulas, 100, 10, 10, 10, &mut 1).expect("high-accuracy hit")
        );
        assert!(
            !physical_attack_hits(&formulas, 1, 10, 10, 10, &mut 1).expect("low-accuracy miss")
        );
    }

    #[test]
    fn magical_accuracy_uses_intelligence_and_luck() {
        let formulas = FormulaCatalog::load(Path::new("../../config/skill-formulas.toml"))
            .expect("formula catalog");

        assert!(
            player_attack_hits(
                &formulas,
                SkillAttackType::Magical,
                0,
                0,
                100,
                10,
                10,
                10,
                10,
                &mut 1,
            )
            .expect("high magical accuracy hit")
        );
        assert!(
            !player_attack_hits(
                &formulas,
                SkillAttackType::Magical,
                100,
                0,
                10,
                10,
                10,
                10,
                10,
                &mut 1,
            )
            .expect("low magical accuracy miss")
        );
    }

    #[test]
    fn projected_player_defenses_reduce_matching_mob_damage() {
        let formulas = FormulaCatalog::load(Path::new("../../config/skill-formulas.toml"))
            .expect("formula catalog");

        for kind in [MobDamageKind::Physical, MobDamageKind::Magical] {
            let damage = calculate_mob_damage(&formulas, kind, 20, 10, 10, 10, &mut 1)
                .expect("defended mob damage");
            assert!((14..=15).contains(&damage));
        }
    }
}
