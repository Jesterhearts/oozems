use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::fs;
use std::path::Path;
use std::path::PathBuf;

use oozems_proto::v1::PlayerState;
use serde::Deserialize;
use sha2::Digest;
use thiserror::Error;

use crate::formula_parser::BinaryOperator;
use crate::formula_parser::Expression;
use crate::formula_parser::Parser;

const MAX_CONFIGURED_LEVEL: u32 = 10_000;
const ABILITY_POINTS_PER_LEVEL: u32 = 5;
const SKILL_POINTS_PER_LEVEL: u32 = 3;
const MAX_RESOURCE: u32 = 30_000;

#[derive(Clone, Debug)]
pub struct ExperienceCurves {
    default_curve: String,
    curves: BTreeMap<String, ExperienceCurve>,
}

#[derive(Clone, Debug)]
pub struct ExperienceCurve {
    name: String,
    requirements: Vec<u64>,
}

#[derive(Debug, Error)]
pub enum ExperienceConfigError {
    #[error("failed to read XP curve configuration {path}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse XP curve configuration {path}")]
    Toml {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("XP curve configuration {path} is invalid: {message}")]
    Invalid { path: PathBuf, message: String },
}

#[derive(Debug, Error)]
pub enum ExperienceRuleError {
    #[error("XP curve {curve:?} does not define player level {level}")]
    LevelNotConfigured { curve: String, level: u32 },
    #[error("player {player_id:?} does not contain character stats")]
    MissingStats { player_id: String },
    #[error("player {player_id:?} experience exceeds the supported range")]
    Overflow { player_id: String },
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CurveFile {
    default_curve: String,
    curves: Vec<RawCurve>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCurve {
    name: String,
    ranges: Vec<RawRange>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRange {
    start: u32,
    end: u32,
    formula: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ExperienceAtom {
    Literal(i128),
    Level,
    AtLevel(u32),
}

#[derive(Debug)]
struct CompiledRange {
    start: u32,
    end: u32,
    expression: Expression<ExperienceAtom>,
}

impl ExperienceCurves {
    pub fn load(path: &Path) -> Result<Self, ExperienceConfigError> {
        let source = fs::read_to_string(path).map_err(|source| ExperienceConfigError::Read {
            path: path.to_owned(),
            source,
        })?;
        let raw = toml::from_str(&source).map_err(|source| ExperienceConfigError::Toml {
            path: path.to_owned(),
            source,
        })?;
        compile_curves(raw).map_err(|message| ExperienceConfigError::Invalid {
            path: path.to_owned(),
            message,
        })
    }

    pub fn default_curve(&self) -> &ExperienceCurve {
        self.curves
            .get(&self.default_curve)
            .expect("validated default XP curve")
    }
}

impl ExperienceCurve {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn required_for_level(
        &self,
        level: u32,
    ) -> Option<u64> {
        let index = usize::try_from(level.checked_sub(1)?).ok()?;
        self.requirements.get(index).copied()
    }

    pub fn max_level(&self) -> u32 {
        u32::try_from(self.requirements.len()).expect("configured level limit fits u32")
    }
}

pub fn apply_curve(
    mut player: PlayerState,
    curve: &ExperienceCurve,
) -> Result<PlayerState, ExperienceRuleError> {
    let required = required_for_level(curve, player.level)?;
    let stats = player
        .stats
        .as_mut()
        .ok_or_else(|| ExperienceRuleError::MissingStats {
            player_id: player.id.clone(),
        })?;
    stats.experience_required = required;
    Ok(player)
}

pub fn required_for_level(
    curve: &ExperienceCurve,
    level: u32,
) -> Result<u64, ExperienceRuleError> {
    curve
        .required_for_level(level)
        .ok_or_else(|| ExperienceRuleError::LevelNotConfigured {
            curve: curve.name.clone(),
            level,
        })
}

pub fn grant_experience(
    mut player: PlayerState,
    amount: u64,
    curve: &ExperienceCurve,
    learned: crate::skills::LearnedSkillModifiers,
) -> Result<PlayerState, ExperienceRuleError> {
    let player_id = player.id.clone();
    let mut level = player.level;
    let mut skill_points = player.skill_points;
    let stats = player
        .stats
        .as_mut()
        .ok_or_else(|| ExperienceRuleError::MissingStats {
            player_id: player_id.clone(),
        })?;
    stats.experience =
        stats
            .experience
            .checked_add(amount)
            .ok_or_else(|| ExperienceRuleError::Overflow {
                player_id: player_id.clone(),
            })?;
    stats.experience_required = required_for_level(curve, level)?;
    while level < curve.max_level() && stats.experience >= stats.experience_required {
        stats.experience -= stats.experience_required;
        level += 1;
        stats.ability_points = stats
            .ability_points
            .checked_add(ABILITY_POINTS_PER_LEVEL)
            .ok_or_else(|| ExperienceRuleError::Overflow {
                player_id: player_id.clone(),
            })?;
        if !crate::jobs::is_beginner_job(stats.job_id) {
            skill_points = skill_points
                .checked_add(SKILL_POINTS_PER_LEVEL)
                .ok_or_else(|| ExperienceRuleError::Overflow {
                    player_id: player_id.clone(),
                })?;
        }
        let (base_hp, base_mp) = level_resource_growth(&player_id, level, stats.job_id);
        let hp_growth = base_hp.saturating_add(learned.max_hp_per_level);
        let mp_growth = base_mp
            .saturating_add(stats.intelligence / 10)
            .saturating_add(learned.max_mp_per_level);
        stats.max_hp = stats.max_hp.saturating_add(hp_growth).min(MAX_RESOURCE);
        stats.max_mp = stats.max_mp.saturating_add(mp_growth).min(MAX_RESOURCE);
        stats.hp = stats.max_hp;
        stats.mp = stats.max_mp;
        stats.experience_required = required_for_level(curve, level)?;
    }
    player.level = level;
    player.skill_points = skill_points;
    Ok(player)
}

fn level_resource_growth(
    player_id: &str,
    target_level: u32,
    job_id: u32,
) -> (u32, u32) {
    use crate::jobs::GrowthFamily;

    let (hp, mp) = match crate::jobs::growth_family(job_id) {
        GrowthFamily::Beginner => ((12, 16), (10, 12)),
        GrowthFamily::Warrior => ((24, 28), (4, 6)),
        GrowthFamily::Magician => ((10, 14), (22, 24)),
        GrowthFamily::Bowman | GrowthFamily::Thief => ((20, 24), (14, 16)),
        GrowthFamily::Pirate => ((22, 28), (18, 23)),
        GrowthFamily::Aran => ((44, 48), (4, 8)),
    };
    (
        stable_growth_roll(player_id, target_level, b"level-hp", hp.0, hp.1),
        stable_growth_roll(player_id, target_level, b"level-mp", mp.0, mp.1),
    )
}

pub(crate) fn stable_growth_roll(
    player_id: &str,
    sequence: u32,
    domain: &[u8],
    minimum: u32,
    maximum: u32,
) -> u32 {
    let mut digest = sha2::Sha256::new();
    digest.update(b"oozems-v83-growth\0");
    digest.update(domain);
    digest.update(b"\0");
    digest.update(player_id.as_bytes());
    digest.update(sequence.to_le_bytes());
    let bytes = digest.finalize();
    let sample = u64::from_le_bytes(bytes[..8].try_into().expect("SHA-256 prefix"));
    let width = u64::from(maximum - minimum + 1);
    minimum + u32::try_from(sample % width).expect("growth range fits u32")
}

fn compile_curves(raw: CurveFile) -> Result<ExperienceCurves, String> {
    validate_name("default curve", &raw.default_curve)?;
    if raw.curves.is_empty() {
        return Err("at least one XP curve is required".to_owned());
    }

    let mut curves = BTreeMap::new();
    for raw_curve in raw.curves {
        validate_name("curve", &raw_curve.name)?;
        let curve = compile_curve(raw_curve)?;
        let name = curve.name.clone();
        if curves.insert(name.clone(), curve).is_some() {
            return Err(format!("XP curve {name:?} is defined more than once"));
        }
    }
    if !curves.contains_key(&raw.default_curve) {
        return Err(format!(
            "default XP curve {:?} is not defined",
            raw.default_curve
        ));
    }

    Ok(ExperienceCurves {
        default_curve: raw.default_curve,
        curves,
    })
}

fn validate_name(
    context: &str,
    name: &str,
) -> Result<(), String> {
    if name.is_empty()
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(format!(
            "{context} name {name:?} must contain only ASCII letters, digits, hyphens, or \
             underscores"
        ));
    }
    Ok(())
}

fn compile_curve(raw: RawCurve) -> Result<ExperienceCurve, String> {
    if raw.ranges.is_empty() {
        return Err(format!("XP curve {:?} has no level ranges", raw.name));
    }

    let mut raw_ranges = raw.ranges;
    raw_ranges.sort_by_key(|range| range.start);
    let mut ranges = Vec::with_capacity(raw_ranges.len());
    let mut expected_start = 1;
    for range in raw_ranges {
        validate_range(&raw.name, &range, expected_start)?;
        let expression = parse_formula(&range.formula).map_err(|message| {
            format!(
                "XP curve {:?}, levels {} through {}, has an invalid formula: {message}",
                raw.name, range.start, range.end
            )
        })?;
        expected_start = range.end + 1;
        ranges.push(CompiledRange {
            start: range.start,
            end: range.end,
            expression,
        });
    }

    let max_level = ranges.last().expect("non-empty ranges").end;
    let dependencies = collect_curve_dependencies(&raw.name, &ranges, max_level)?;
    let order = evaluation_order(&raw.name, &dependencies)?;
    let mut resolved = vec![None; max_level as usize + 1];
    for level in order {
        let range = range_for_level(&ranges, level).expect("validated contiguous XP ranges");
        let value = evaluate_expression(&raw.name, &range.expression, level, &resolved)?;
        resolved[level as usize] = Some(validate_experience(&raw.name, level, value)?);
    }
    let requirements = resolved
        .into_iter()
        .skip(1)
        .map(|value| value.expect("topologically resolved XP level"))
        .collect();

    Ok(ExperienceCurve {
        name: raw.name,
        requirements,
    })
}

fn validate_range(
    curve_name: &str,
    range: &RawRange,
    expected_start: u32,
) -> Result<(), String> {
    if range.start == 0 || range.end == 0 {
        return Err(format!(
            "XP curve {curve_name:?} uses level 0; levels start at 1"
        ));
    }
    if range.start > range.end {
        return Err(format!(
            "XP curve {curve_name:?} has a range starting at {} after its end at {}",
            range.start, range.end
        ));
    }
    if range.end > MAX_CONFIGURED_LEVEL {
        return Err(format!(
            "XP curve {curve_name:?} ends at level {}, above the supported limit of \
             {MAX_CONFIGURED_LEVEL}",
            range.end
        ));
    }
    if range.start != expected_start {
        let problem = if range.start < expected_start {
            "overlaps an earlier range"
        } else {
            "leaves a gap after the earlier range"
        };
        return Err(format!(
            "XP curve {curve_name:?} range {} through {} {problem}; the next range must start at \
             {expected_start}",
            range.start, range.end
        ));
    }
    Ok(())
}

fn collect_curve_dependencies(
    curve_name: &str,
    ranges: &[CompiledRange],
    max_level: u32,
) -> Result<Vec<Vec<u32>>, String> {
    let mut dependencies = vec![Vec::new(); max_level as usize + 1];
    for level in 1..=max_level {
        let range = range_for_level(ranges, level).expect("validated contiguous XP ranges");
        let mut referenced_levels = BTreeSet::new();
        collect_expression_dependencies(&range.expression, &mut referenced_levels);
        for referenced_level in &referenced_levels {
            if *referenced_level > max_level {
                return Err(format!(
                    "XP curve {curve_name:?} does not define referenced level {referenced_level}"
                ));
            }
        }
        dependencies[level as usize] = referenced_levels.into_iter().collect();
    }
    Ok(dependencies)
}

fn collect_expression_dependencies(
    expression: &Expression<ExperienceAtom>,
    dependencies: &mut BTreeSet<u32>,
) {
    match expression {
        Expression::Atom(ExperienceAtom::AtLevel(level)) => {
            dependencies.insert(*level);
        }
        Expression::Negate(value) => collect_expression_dependencies(value, dependencies),
        Expression::Binary { left, right, .. } => {
            collect_expression_dependencies(left, dependencies);
            collect_expression_dependencies(right, dependencies);
        }
        Expression::Atom(ExperienceAtom::Literal(_) | ExperienceAtom::Level) => {}
    }
}

fn evaluation_order(
    curve_name: &str,
    dependencies: &[Vec<u32>],
) -> Result<Vec<u32>, String> {
    let max_level = dependencies.len() - 1;
    let mut unresolved = vec![0_usize; dependencies.len()];
    let mut dependents = vec![Vec::new(); dependencies.len()];
    for level in 1..=max_level {
        unresolved[level] = dependencies[level].len();
        for dependency in &dependencies[level] {
            dependents[*dependency as usize].push(level as u32);
        }
    }

    let mut ready = (1..=max_level)
        .filter(|level| unresolved[*level] == 0)
        .map(|level| level as u32)
        .collect::<VecDeque<_>>();
    let mut order = Vec::with_capacity(max_level);
    while let Some(level) = ready.pop_front() {
        order.push(level);
        for dependent in &dependents[level as usize] {
            let remaining = &mut unresolved[*dependent as usize];
            *remaining -= 1;
            if *remaining == 0 {
                ready.push_back(*dependent);
            }
        }
    }
    if order.len() != max_level {
        let cycle = dependency_cycle(dependencies, &unresolved)
            .into_iter()
            .map(|level| level.to_string())
            .collect::<Vec<_>>()
            .join(" -> ");
        return Err(format!(
            "XP curve {curve_name:?} contains an atLevel dependency cycle: {cycle}"
        ));
    }
    Ok(order)
}

fn dependency_cycle(
    dependencies: &[Vec<u32>],
    unresolved: &[usize],
) -> Vec<u32> {
    let mut level = (1..unresolved.len())
        .find(|level| unresolved[*level] > 0)
        .expect("cycle contains an unresolved level") as u32;
    let mut path = Vec::new();
    let mut positions = HashMap::new();
    loop {
        if let Some(start) = positions.insert(level, path.len()) {
            let mut cycle = path[start..].to_vec();
            cycle.push(level);
            return cycle;
        }
        path.push(level);
        level = dependencies[level as usize]
            .iter()
            .copied()
            .find(|dependency| unresolved[*dependency as usize] > 0)
            .expect("unresolved dependency graph contains a cycle");
    }
}

fn range_for_level(
    ranges: &[CompiledRange],
    level: u32,
) -> Option<&CompiledRange> {
    ranges
        .iter()
        .find(|range| (range.start..=range.end).contains(&level))
}

fn validate_experience(
    curve_name: &str,
    level: u32,
    value: i128,
) -> Result<u64, String> {
    let value = u64::try_from(value).map_err(|_| {
        format!(
            "XP curve {curve_name:?} produces non-positive or excessive XP {value} at level \
             {level}"
        )
    })?;
    if value == 0 {
        return Err(format!(
            "XP curve {curve_name:?} produces zero XP at level {level}"
        ));
    }
    Ok(value)
}

fn evaluate_expression(
    curve_name: &str,
    expression: &Expression<ExperienceAtom>,
    level: u32,
    resolved: &[Option<u64>],
) -> Result<i128, String> {
    match expression {
        Expression::Atom(atom) => evaluate_atom(curve_name, atom, level, resolved),
        Expression::Negate(value) => evaluate_expression(curve_name, value, level, resolved)?
            .checked_neg()
            .ok_or_else(|| arithmetic_error(curve_name, level, "negation overflow")),
        Expression::Binary {
            operator,
            left,
            right,
        } => {
            let left = evaluate_expression(curve_name, left, level, resolved)?;
            let right = evaluate_expression(curve_name, right, level, resolved)?;
            evaluate_binary(curve_name, level, *operator, left, right)
        }
    }
}

fn evaluate_atom(
    curve_name: &str,
    atom: &ExperienceAtom,
    level: u32,
    resolved: &[Option<u64>],
) -> Result<i128, String> {
    match atom {
        ExperienceAtom::Literal(value) => Ok(*value),
        ExperienceAtom::Level => Ok(i128::from(level)),
        ExperienceAtom::AtLevel(referenced_level) => resolved
            .get(*referenced_level as usize)
            .copied()
            .flatten()
            .map(i128::from)
            .ok_or_else(|| {
                format!(
                    "XP curve {curve_name:?} could not resolve referenced level {referenced_level}"
                )
            }),
    }
}

fn evaluate_binary(
    curve_name: &str,
    level: u32,
    operator: BinaryOperator,
    left: i128,
    right: i128,
) -> Result<i128, String> {
    let value = match operator {
        BinaryOperator::Add => left.checked_add(right),
        BinaryOperator::Subtract => left.checked_sub(right),
        BinaryOperator::Multiply => left.checked_mul(right),
        BinaryOperator::Divide => {
            if right == 0 {
                return Err(arithmetic_error(curve_name, level, "division by zero"));
            }
            left.checked_div(right)
        }
        BinaryOperator::Exponentiate => {
            let exponent = u32::try_from(right).map_err(|_| {
                arithmetic_error(
                    curve_name,
                    level,
                    "an exponent must be a non-negative 32-bit integer",
                )
            })?;
            left.checked_pow(exponent)
        }
    };
    value.ok_or_else(|| arithmetic_error(curve_name, level, "integer overflow"))
}

fn arithmetic_error(
    curve_name: &str,
    level: u32,
    message: &str,
) -> String {
    format!("XP curve {curve_name:?} has {message} while evaluating level {level}")
}

fn parse_formula(source: &str) -> Result<Expression<ExperienceAtom>, String> {
    if !source.is_ascii() {
        return Err("formulas must contain only ASCII characters".to_owned());
    }
    crate::formula_parser::parse(source, parse_atom)
}

fn parse_atom(parser: &mut Parser<'_, ExperienceAtom>) -> Result<ExperienceAtom, String> {
    parser.skip_whitespace();
    if parser.peek().is_some_and(|byte| byte.is_ascii_digit()) {
        let (start, source) = parser.integer()?;
        let value = source
            .parse()
            .map_err(|_| format!("integer is too large at byte {start}"))?;
        return Ok(ExperienceAtom::Literal(value));
    }
    if parser.peek().is_some_and(|byte| byte.is_ascii_alphabetic()) {
        return parse_identifier(parser);
    }
    parser.error("expected a number, Level, atLevel(...), or parenthesized expression")
}

fn parse_identifier(parser: &mut Parser<'_, ExperienceAtom>) -> Result<ExperienceAtom, String> {
    let name = parser.identifier()?;
    match name.as_str() {
        "Level" => Ok(ExperienceAtom::Level),
        "atLevel" => {
            parser.expect(b'(')?;
            let (start, source) = parser.integer()?;
            let level = source
                .parse::<i128>()
                .map_err(|_| format!("integer is too large at byte {start}"))?;
            let level = u32::try_from(level)
                .ok()
                .filter(|level| *level > 0)
                .ok_or_else(|| {
                    format!(
                        "atLevel requires a positive 32-bit level number at byte {}",
                        parser.position()
                    )
                })?;
            parser.expect(b')')?;
            Ok(ExperienceAtom::AtLevel(level))
        }
        _ => parser.error(&format!("unknown identifier {name:?}")),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use oozems_proto::v1::CharacterStats;
    use oozems_proto::v1::PlayerState;

    use super::ExperienceCurves;
    use super::apply_curve;
    use super::compile_curves;
    use super::grant_experience;
    use super::parse_formula;

    #[test]
    fn at_level_resolves_acyclic_cross_range_dependencies() {
        let config = curves(
            r#"
default_curve = "linked"

[[curves]]
name = "linked"

[[curves.ranges]]
start = 1
end = 1
formula = "15"

[[curves.ranges]]
start = 2
end = 3
formula = "atLevel(1) + Level * 10"
"#,
        )
        .expect("valid linked configuration");
        let curve = config.default_curve();

        assert_eq!(curve.required_for_level(1), Some(15));
        assert_eq!(curve.required_for_level(2), Some(35));
        assert_eq!(curve.required_for_level(3), Some(45));
        assert_eq!(curve.max_level(), 3);

        let player = PlayerState {
            id: "configured-player".to_owned(),
            level: 2,
            stats: Some(CharacterStats {
                experience: 100,
                experience_required: 1,
                ..CharacterStats::default()
            }),
            ..PlayerState::default()
        };
        let configured = apply_curve(player, curve).expect("apply linked curve");
        let stats = configured.stats.expect("configured stats");
        assert_eq!(stats.experience, 100);
        assert_eq!(stats.experience_required, 35);
    }

    #[test]
    fn experience_rewards_carry_across_multiple_levels() {
        let config = curves(
            r#"
default_curve = "rewards"

[[curves]]
name = "rewards"

[[curves.ranges]]
start = 1
end = 3
formula = "10"
"#,
        )
        .expect("valid reward curve");
        let player = PlayerState {
            id: "rewarded-player".to_owned(),
            level: 1,
            stats: Some(CharacterStats::default()),
            ..PlayerState::default()
        };

        let rewarded = grant_experience(
            player,
            25,
            config.default_curve(),
            crate::skills::LearnedSkillModifiers::default(),
        )
        .expect("reward XP");
        let stats = rewarded.stats.expect("rewarded stats");

        assert_eq!(rewarded.level, 3);
        assert_eq!(stats.experience, 5);
        assert_eq!(stats.experience_required, 10);
        assert_eq!(stats.ability_points, 10);
    }

    #[test]
    fn level_up_applies_job_growth_passives_caps_and_full_recovery() {
        let config = curves(
            r#"
default_curve = "growth"

[[curves]]
name = "growth"

[[curves.ranges]]
start = 1
end = 2
formula = "10"
"#,
        )
        .expect("growth curve");
        let player = PlayerState {
            id: "warrior-growth".to_owned(),
            level: 1,
            skill_points: 1,
            stats: Some(CharacterStats {
                job_id: 100,
                hp: 1,
                max_hp: 100,
                mp: 1,
                max_mp: 50,
                intelligence: 20,
                ..CharacterStats::default()
            }),
            ..PlayerState::default()
        };
        let learned = crate::skills::LearnedSkillModifiers {
            max_hp_per_level: 40,
            ..crate::skills::LearnedSkillModifiers::default()
        };

        let player = grant_experience(player, 10, config.default_curve(), learned)
            .expect("grant level-up experience");
        let stats = player.stats.expect("stats");

        assert_eq!(player.level, 2);
        assert_eq!(player.skill_points, 4);
        assert!((164..=168).contains(&stats.max_hp));
        assert!((56..=58).contains(&stats.max_mp));
        assert_eq!(stats.hp, stats.max_hp);
        assert_eq!(stats.mp, stats.max_mp);
    }

    #[test]
    fn default_curve_is_selected_and_unselected_curves_are_validated() {
        let config = curves(
            r#"
default_curve = "hard"

[[curves]]
name = "easy"

[[curves.ranges]]
start = 1
end = 1
formula = "10"

[[curves]]
name = "hard"

[[curves.ranges]]
start = 1
end = 1
formula = "20"
"#,
        )
        .expect("multiple valid curves");
        assert_eq!(config.default_curve().name(), "hard");
        assert_eq!(config.default_curve().required_for_level(1), Some(20));

        let error = curves(
            r#"
default_curve = "valid"

[[curves]]
name = "valid"

[[curves.ranges]]
start = 1
end = 1
formula = "15"

[[curves]]
name = "unused-cycle"

[[curves.ranges]]
start = 1
end = 1
formula = "atLevel(1)"
"#,
        )
        .expect_err("an unselected cyclic curve must fail");
        assert!(error.contains("unused-cycle"));
        assert!(error.contains("dependency cycle"));
    }

    #[test]
    fn at_level_cycles_are_rejected() {
        let error = curves(
            r#"
default_curve = "cycle"

[[curves]]
name = "cycle"

[[curves.ranges]]
start = 1
end = 1
formula = "atLevel(2)"

[[curves.ranges]]
start = 2
end = 2
formula = "atLevel(1)"
"#,
        )
        .expect_err("cyclic configuration must fail");

        assert!(error.contains("1 -> 2 -> 1"));
    }

    #[test]
    fn range_gaps_and_missing_references_are_rejected() {
        let gap = curves(
            r#"
default_curve = "gap"

[[curves]]
name = "gap"

[[curves.ranges]]
start = 2
end = 3
formula = "15"
"#,
        )
        .expect_err("gap must fail");
        assert!(gap.contains("must start at 1"));

        let missing = curves(
            r#"
default_curve = "missing"

[[curves]]
name = "missing"

[[curves.ranges]]
start = 1
end = 1
formula = "atLevel(2)"
"#,
        )
        .expect_err("missing reference must fail");
        assert!(missing.contains("does not define referenced level 2"));
    }

    #[test]
    fn invalid_arithmetic_is_rejected_during_startup_evaluation() {
        let error = curves(
            r#"
default_curve = "broken"

[[curves]]
name = "broken"

[[curves.ranges]]
start = 1
end = 1
formula = "Level / (Level - 1)"
"#,
        )
        .expect_err("division by zero must fail");

        assert!(error.contains("division by zero"));
    }

    #[test]
    fn bundled_configuration_is_valid() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/xp-curves.toml");
        let source = std::fs::read_to_string(&path).expect("bundled XP curve file");
        assert!(source.starts_with("# See README.md for configuration reference.\n"));
        let config = ExperienceCurves::load(&path).expect("bundled XP curve configuration");

        assert_eq!(config.default_curve().name(), "default");
        assert_eq!(config.default_curve().required_for_level(1), Some(15));
        assert_eq!(config.default_curve().required_for_level(10), Some(1_500));
        assert_eq!(config.default_curve().required_for_level(11), Some(2_000));
    }

    fn curves(source: &str) -> Result<ExperienceCurves, String> {
        let raw = toml::from_str(source).map_err(|error| error.to_string())?;
        compile_curves(raw)
    }

    #[test]
    fn parser_rejects_unknown_identifiers() {
        assert!(parse_formula("level + 1").is_err());
    }
}
