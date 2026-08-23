use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;
use thiserror::Error;

#[derive(Clone, Copy, Debug)]
pub struct GameplayConfig {
    pub item_drop_despawn: Duration,
    pub initial_skill_points: u32,
    pub combat: CombatConfig,
    pub movement: MovementConfig,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CombatConfig {
    pub disengage_range: f32,
    pub player_attack_range: f32,
    pub attack_vertical_reach: f32,
    pub touch_horizontal_reach: f32,
    pub touch_vertical_reach: f32,
    pub projectile_range: f32,
    pub projectile_speed: f32,
    pub projectile_hit_reach: f32,
    pub mob_attack_interval: Duration,
    pub player_invulnerability: Duration,
    pub default_respawn: Duration,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MovementConfig {
    pub walk_speed: f32,
    pub climb_speed: f32,
    pub gravity: f32,
    pub jump_speed: f32,
    pub speed_cap: u32,
    pub jump_cap: u32,
    pub snapshot_interval: Duration,
    pub maximum_snapshot_gap: Duration,
    pub persistence_interval: Duration,
    pub position_tolerance: f32,
    pub ground_tolerance: f32,
    pub platform_edge_tolerance: f32,
    pub ladder_reach: f32,
    pub ladder_end_reach: f32,
    pub portal_horizontal_reach: f32,
    pub portal_vertical_reach: f32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GameplayFile {
    items: ItemRulesFile,
    skills: SkillRulesFile,
    #[serde(default)]
    combat: CombatRulesFile,
    #[serde(default)]
    movement: MovementRulesFile,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ItemRulesFile {
    drop_despawn: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SkillRulesFile {
    initial_points: u32,
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct CombatRulesFile {
    disengage_range: f32,
    player_attack_range: f32,
    attack_vertical_reach: f32,
    touch_horizontal_reach: f32,
    touch_vertical_reach: f32,
    projectile_range: f32,
    projectile_speed: f32,
    projectile_hit_reach: f32,
    mob_attack_interval: String,
    player_invulnerability: String,
    default_respawn: String,
}

impl Default for CombatRulesFile {
    fn default() -> Self {
        Self {
            disengage_range: 520.0,
            player_attack_range: 220.0,
            attack_vertical_reach: 90.0,
            touch_horizontal_reach: 28.0,
            touch_vertical_reach: 48.0,
            projectile_range: 420.0,
            projectile_speed: 240.0,
            projectile_hit_reach: 18.0,
            mob_attack_interval: "1500ms".to_owned(),
            player_invulnerability: "1s".to_owned(),
            default_respawn: "7s".to_owned(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct MovementRulesFile {
    walk_speed: f32,
    climb_speed: f32,
    gravity: f32,
    jump_speed: f32,
    speed_cap: u32,
    jump_cap: u32,
    snapshot_interval: String,
    maximum_snapshot_gap: String,
    persistence_interval: String,
    position_tolerance: f32,
    ground_tolerance: f32,
    platform_edge_tolerance: f32,
    ladder_reach: f32,
    ladder_end_reach: f32,
    portal_horizontal_reach: f32,
    portal_vertical_reach: f32,
}

impl Default for MovementRulesFile {
    fn default() -> Self {
        Self {
            walk_speed: 220.0,
            climb_speed: 135.0,
            gravity: 1_150.0,
            jump_speed: 480.0,
            speed_cap: 200,
            jump_cap: 200,
            snapshot_interval: "200ms".to_owned(),
            maximum_snapshot_gap: "1s".to_owned(),
            persistence_interval: "2s".to_owned(),
            position_tolerance: 24.0,
            ground_tolerance: 8.0,
            platform_edge_tolerance: 20.0,
            ladder_reach: 32.0,
            ladder_end_reach: 20.0,
            portal_horizontal_reach: 48.0,
            portal_vertical_reach: 64.0,
        }
    }
}

#[derive(Debug, Error)]
pub enum GameplayConfigError {
    #[error("failed to read gameplay configuration {path}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse gameplay configuration {path}")]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("items.drop_despawn in {path} is invalid")]
    InvalidDropDespawn {
        path: PathBuf,
        #[source]
        source: humantime::DurationError,
    },
    #[error("items.drop_despawn in {path} must be greater than zero")]
    EmptyDropDespawn { path: PathBuf },
    #[error("movement.{field} in {path} is invalid")]
    InvalidMovementDuration {
        path: PathBuf,
        field: &'static str,
        #[source]
        source: humantime::DurationError,
    },
    #[error("movement.{field} in {path} must be a finite value greater than zero")]
    InvalidMovementValue { path: PathBuf, field: &'static str },
    #[error("combat.{field} in {path} is invalid")]
    InvalidCombatDuration {
        path: PathBuf,
        field: &'static str,
        #[source]
        source: humantime::DurationError,
    },
    #[error("combat.{field} in {path} must be a finite value greater than zero")]
    InvalidCombatValue { path: PathBuf, field: &'static str },
    #[error(
        "movement.maximum_snapshot_gap in {path} must not be shorter than \
         movement.snapshot_interval"
    )]
    ShortMovementGap { path: PathBuf },
}

impl GameplayConfig {
    pub fn load(path: &Path) -> Result<Self, GameplayConfigError> {
        let source = fs::read_to_string(path).map_err(|source| GameplayConfigError::Read {
            path: path.to_owned(),
            source,
        })?;
        let file = toml::from_str::<GameplayFile>(&source).map_err(|source| {
            GameplayConfigError::Parse {
                path: path.to_owned(),
                source,
            }
        })?;
        let item_drop_despawn =
            humantime::parse_duration(&file.items.drop_despawn).map_err(|source| {
                GameplayConfigError::InvalidDropDespawn {
                    path: path.to_owned(),
                    source,
                }
            })?;
        if item_drop_despawn.is_zero() {
            return Err(GameplayConfigError::EmptyDropDespawn {
                path: path.to_owned(),
            });
        }

        let combat = parse_combat_config(file.combat, path)?;
        let movement = parse_movement_config(file.movement, path)?;
        Ok(Self {
            item_drop_despawn,
            initial_skill_points: file.skills.initial_points,
            combat,
            movement,
        })
    }
}

fn parse_combat_config(
    file: CombatRulesFile,
    path: &Path,
) -> Result<CombatConfig, GameplayConfigError> {
    for (field, value) in [
        ("disengage_range", file.disengage_range),
        ("player_attack_range", file.player_attack_range),
        ("attack_vertical_reach", file.attack_vertical_reach),
        ("touch_horizontal_reach", file.touch_horizontal_reach),
        ("touch_vertical_reach", file.touch_vertical_reach),
        ("projectile_range", file.projectile_range),
        ("projectile_speed", file.projectile_speed),
        ("projectile_hit_reach", file.projectile_hit_reach),
    ] {
        if !value.is_finite() || value <= 0.0 {
            return Err(GameplayConfigError::InvalidCombatValue {
                path: path.to_owned(),
                field,
            });
        }
    }
    let parse_duration = |field, value: &str| {
        humantime::parse_duration(value).map_err(|source| {
            GameplayConfigError::InvalidCombatDuration {
                path: path.to_owned(),
                field,
                source,
            }
        })
    };
    let mob_attack_interval = parse_duration("mob_attack_interval", &file.mob_attack_interval)?;
    let player_invulnerability =
        parse_duration("player_invulnerability", &file.player_invulnerability)?;
    let default_respawn = parse_duration("default_respawn", &file.default_respawn)?;
    for (field, duration) in [
        ("mob_attack_interval", mob_attack_interval),
        ("player_invulnerability", player_invulnerability),
        ("default_respawn", default_respawn),
    ] {
        if duration.is_zero() {
            return Err(GameplayConfigError::InvalidCombatValue {
                path: path.to_owned(),
                field,
            });
        }
    }
    Ok(CombatConfig {
        disengage_range: file.disengage_range,
        player_attack_range: file.player_attack_range,
        attack_vertical_reach: file.attack_vertical_reach,
        touch_horizontal_reach: file.touch_horizontal_reach,
        touch_vertical_reach: file.touch_vertical_reach,
        projectile_range: file.projectile_range,
        projectile_speed: file.projectile_speed,
        projectile_hit_reach: file.projectile_hit_reach,
        mob_attack_interval,
        player_invulnerability,
        default_respawn,
    })
}

fn parse_movement_config(
    file: MovementRulesFile,
    path: &Path,
) -> Result<MovementConfig, GameplayConfigError> {
    for (field, value) in [
        ("walk_speed", file.walk_speed),
        ("climb_speed", file.climb_speed),
        ("gravity", file.gravity),
        ("jump_speed", file.jump_speed),
        ("position_tolerance", file.position_tolerance),
        ("ground_tolerance", file.ground_tolerance),
        ("platform_edge_tolerance", file.platform_edge_tolerance),
        ("ladder_reach", file.ladder_reach),
        ("ladder_end_reach", file.ladder_end_reach),
        ("portal_horizontal_reach", file.portal_horizontal_reach),
        ("portal_vertical_reach", file.portal_vertical_reach),
    ] {
        if !value.is_finite() || value <= 0.0 {
            return Err(GameplayConfigError::InvalidMovementValue {
                path: path.to_owned(),
                field,
            });
        }
    }
    if file.speed_cap == 0 {
        return Err(GameplayConfigError::InvalidMovementValue {
            path: path.to_owned(),
            field: "speed_cap",
        });
    }
    if file.jump_cap == 0 {
        return Err(GameplayConfigError::InvalidMovementValue {
            path: path.to_owned(),
            field: "jump_cap",
        });
    }
    let parse_duration = |field, value: &str| {
        humantime::parse_duration(value).map_err(|source| {
            GameplayConfigError::InvalidMovementDuration {
                path: path.to_owned(),
                field,
                source,
            }
        })
    };
    let snapshot_interval = parse_duration("snapshot_interval", &file.snapshot_interval)?;
    let maximum_snapshot_gap = parse_duration("maximum_snapshot_gap", &file.maximum_snapshot_gap)?;
    let persistence_interval = parse_duration("persistence_interval", &file.persistence_interval)?;
    if snapshot_interval.is_zero() {
        return Err(GameplayConfigError::InvalidMovementValue {
            path: path.to_owned(),
            field: "snapshot_interval",
        });
    }
    if maximum_snapshot_gap < snapshot_interval {
        return Err(GameplayConfigError::ShortMovementGap {
            path: path.to_owned(),
        });
    }
    if persistence_interval.is_zero() {
        return Err(GameplayConfigError::InvalidMovementValue {
            path: path.to_owned(),
            field: "persistence_interval",
        });
    }
    Ok(MovementConfig {
        walk_speed: file.walk_speed,
        climb_speed: file.climb_speed,
        gravity: file.gravity,
        jump_speed: file.jump_speed,
        speed_cap: file.speed_cap,
        jump_cap: file.jump_cap,
        snapshot_interval,
        maximum_snapshot_gap,
        persistence_interval,
        position_tolerance: file.position_tolerance,
        ground_tolerance: file.ground_tolerance,
        platform_edge_tolerance: file.platform_edge_tolerance,
        ladder_reach: file.ladder_reach,
        ladder_end_reach: file.ladder_end_reach,
        portal_horizontal_reach: file.portal_horizontal_reach,
        portal_vertical_reach: file.portal_vertical_reach,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::Duration;

    use super::GameplayConfig;

    #[test]
    fn loads_human_readable_drop_duration() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("gameplay.toml");
        fs::write(
            &path,
            concat!(
                "# See README.md for configuration reference.\n",
                "[items]\n",
                "drop_despawn = \"10m\"\n",
                "[skills]\n",
                "initial_points = 3\n",
            ),
        )
        .expect("write configuration");

        let config = GameplayConfig::load(&path).expect("valid configuration");

        assert_eq!(config.item_drop_despawn, Duration::from_secs(600));
        assert_eq!(config.initial_skill_points, 3);
        assert_eq!(config.combat.disengage_range, 520.0);
        assert_eq!(config.combat.default_respawn, Duration::from_secs(7));
        assert_eq!(config.movement.speed_cap, 200);
        assert_eq!(config.movement.jump_cap, 200);
        assert_eq!(
            config.movement.snapshot_interval,
            Duration::from_millis(200)
        );
    }

    #[test]
    fn rejects_zero_drop_duration() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("gameplay.toml");
        fs::write(
            &path,
            "[items]\ndrop_despawn = \"0s\"\n[skills]\ninitial_points = 3\n",
        )
        .expect("write configuration");

        assert!(GameplayConfig::load(&path).is_err());
    }

    #[test]
    fn rejects_invalid_movement_limits() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("gameplay.toml");
        fs::write(
            &path,
            concat!(
                "[items]\n",
                "drop_despawn = \"10m\"\n",
                "[skills]\n",
                "initial_points = 3\n",
                "[movement]\n",
                "speed_cap = 0\n",
            ),
        )
        .expect("write configuration");

        let error = GameplayConfig::load(&path)
            .expect_err("zero speed cap must fail")
            .to_string();

        assert!(error.contains("movement.speed_cap"));
    }
}
