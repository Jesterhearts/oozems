use std::collections::HashMap;
use std::sync::Arc;

use oozems_proto::v1::CombatEvent;
use oozems_proto::v1::MobMovementMode;
use oozems_proto::v1::Platform;
use oozems_proto::v1::Vec2;
use shipyard::Component;
use shipyard::Unique;

use super::MobDeath;
use crate::gameplay::CombatConfig;
use crate::skill_formula::FormulaCatalog;

#[derive(Component, Clone, Debug)]
pub(super) struct MobIdentity {
    pub public_id: String,
    pub definition_id: u32,
    pub spawn_id: u32,
}

#[derive(Component, Clone, Copy, Debug)]
#[track(Modification)]
pub(super) struct Position {
    pub x: f32,
    pub y: f32,
    pub layer: i32,
}

impl Position {
    pub fn vector(self) -> Vec2 {
        Vec2 {
            x: self.x,
            y: self.y,
        }
    }
}

#[derive(Component, Clone, Debug)]
pub(super) struct MobMotion {
    pub spawn_position: Position,
    pub spawn_support: Option<usize>,
    pub support: Option<usize>,
    pub roam_left: f32,
    pub roam_right: f32,
    pub move_speed: f32,
    pub can_move: bool,
    pub can_jump: bool,
    pub flies: bool,
    pub flip_x: bool,
    pub direction: i8,
    pub velocity_y: f32,
    pub decision_seconds: f32,
    pub random_state: u64,
    pub mode: MobMovementMode,
}

#[derive(Component, Clone, Debug)]
pub(super) struct MobCombat {
    pub level: u32,
    pub maximum_hp: u64,
    pub current_hp: u64,
    pub physical_attack: i32,
    pub physical_defense: i32,
    pub magic_attack: i32,
    #[allow(dead_code)]
    pub magic_defense: i32,
    pub accuracy: i32,
    pub avoidability: i32,
    pub body_attack: bool,
    pub aggro_target: Option<String>,
    pub next_attack_ms: u64,
    pub attack_until_ms: u64,
    pub movement_resume_ms: u64,
    pub dead_until_ms: Option<u64>,
    pub respawn_delay_ms: u64,
    pub player_attack_transaction: Option<u64>,
}

#[derive(Component, Clone, Debug)]
pub(super) struct PlayerPresence {
    pub id: String,
    pub level: u32,
    pub current_hp: u32,
    pub weapon_defense: i32,
    pub magic_defense: i32,
    pub accuracy: i32,
    pub accuracy_bonus: i32,
    pub intelligence: u32,
    pub luck: u32,
    pub avoidability: i32,
    pub last_seen_ms: u64,
    pub invulnerable_until_ms: u64,
    pub contact_attempt_after_ms: u64,
}

#[derive(Component, Clone, Debug)]
pub(super) struct Projectile {
    pub public_id: String,
    pub source_mob_id: String,
    pub target_player_id: String,
    pub speed: f32,
    pub damage: u64,
    pub expires_at_ms: u64,
    pub impacted: bool,
}

#[derive(Clone, Debug)]
pub(super) struct PlayerTarget {
    pub id: String,
    pub position: Position,
    pub level: u32,
    pub current_hp: u32,
    pub magic_defense: i32,
    pub avoidability: i32,
}

#[derive(Clone, Debug)]
pub(super) struct ProjectileSpawn {
    pub source_mob_id: String,
    pub target_player_id: String,
    pub position: Position,
    pub damage: u64,
    pub missed: bool,
}

#[derive(Unique)]
pub(super) struct Terrain {
    pub platforms: Vec<Platform>,
    pub height: f32,
}

#[derive(Unique, Clone, Copy)]
pub(super) struct Tick {
    pub elapsed_seconds: f32,
    pub now_ms: u64,
}

#[derive(Unique)]
pub(super) struct CombatRules(pub CombatConfig);

#[derive(Unique)]
pub(super) struct CombatFormulas(pub Arc<FormulaCatalog>);

#[derive(Unique, Default)]
pub(super) struct TargetCache(pub Vec<PlayerTarget>);

#[derive(Unique, Default)]
pub(super) struct ProjectileSpawns(pub Vec<ProjectileSpawn>);

#[derive(Unique, Default)]
pub(super) struct PendingEvents {
    pub by_player: HashMap<String, Vec<CombatEvent>>,
    pub mob_deaths_by_player: HashMap<String, Vec<MobDeath>>,
    pub staged_drops_by_player: HashMap<String, Vec<crate::items::StagedDropGrant>>,
    pub next_sequence: u64,
}

#[derive(Unique, Default)]
pub(super) struct SimulationErrors(pub Vec<String>);
