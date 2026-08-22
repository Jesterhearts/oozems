use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::fs;
use std::path::Path;
use std::path::PathBuf;

use oozems_proto::v1::PlayerState;
use serde::Deserialize;
use thiserror::Error;

const MAX_CONFIGURED_LEVEL: u32 = 10_000;

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
enum Expression {
    Literal(i128),
    Level,
    AtLevel(u32),
    Negate(Box<Expression>),
    Binary {
        operator: BinaryOperator,
        left: Box<Expression>,
        right: Box<Expression>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BinaryOperator {
    Add,
    Subtract,
    Multiply,
    Divide,
    Exponentiate,
}

#[derive(Debug)]
struct CompiledRange {
    start: u32,
    end: u32,
    expression: Expression,
}

struct FormulaParser<'a> {
    input: &'a [u8],
    position: usize,
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
    expression: &Expression,
    dependencies: &mut BTreeSet<u32>,
) {
    match expression {
        Expression::AtLevel(level) => {
            dependencies.insert(*level);
        }
        Expression::Negate(value) => collect_expression_dependencies(value, dependencies),
        Expression::Binary { left, right, .. } => {
            collect_expression_dependencies(left, dependencies);
            collect_expression_dependencies(right, dependencies);
        }
        Expression::Literal(_) | Expression::Level => {}
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
    expression: &Expression,
    level: u32,
    resolved: &[Option<u64>],
) -> Result<i128, String> {
    match expression {
        Expression::Literal(value) => Ok(*value),
        Expression::Level => Ok(i128::from(level)),
        Expression::AtLevel(referenced_level) => resolved
            .get(*referenced_level as usize)
            .copied()
            .flatten()
            .map(i128::from)
            .ok_or_else(|| {
                format!(
                    "XP curve {curve_name:?} could not resolve referenced level {referenced_level}"
                )
            }),
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

fn parse_formula(source: &str) -> Result<Expression, String> {
    if !source.is_ascii() {
        return Err("formulas must contain only ASCII characters".to_owned());
    }
    let mut parser = FormulaParser {
        input: source.as_bytes(),
        position: 0,
    };
    let expression = parser.parse_addition()?;
    parser.skip_whitespace();
    if parser.position != parser.input.len() {
        return parser.error("unexpected trailing input");
    }
    Ok(expression)
}

impl FormulaParser<'_> {
    fn parse_addition(&mut self) -> Result<Expression, String> {
        let mut expression = self.parse_multiplication()?;
        loop {
            let operator = if self.consume(b'+') {
                BinaryOperator::Add
            } else if self.consume(b'-') {
                BinaryOperator::Subtract
            } else {
                return Ok(expression);
            };
            expression = binary(operator, expression, self.parse_multiplication()?);
        }
    }

    fn parse_multiplication(&mut self) -> Result<Expression, String> {
        let mut expression = self.parse_unary()?;
        loop {
            let operator = if self.consume(b'*') {
                BinaryOperator::Multiply
            } else if self.consume(b'/') {
                BinaryOperator::Divide
            } else {
                return Ok(expression);
            };
            expression = binary(operator, expression, self.parse_unary()?);
        }
    }

    fn parse_unary(&mut self) -> Result<Expression, String> {
        if self.consume(b'+') {
            return self.parse_unary();
        }
        if self.consume(b'-') {
            return Ok(Expression::Negate(Box::new(self.parse_unary()?)));
        }
        self.parse_power()
    }

    fn parse_power(&mut self) -> Result<Expression, String> {
        let base = self.parse_primary()?;
        if self.consume(b'^') {
            return Ok(binary(
                BinaryOperator::Exponentiate,
                base,
                self.parse_unary()?,
            ));
        }
        Ok(base)
    }

    fn parse_primary(&mut self) -> Result<Expression, String> {
        self.skip_whitespace();
        if self.consume(b'(') {
            let expression = self.parse_addition()?;
            self.expect(b')')?;
            return Ok(expression);
        }
        if self.peek().is_some_and(|byte| byte.is_ascii_digit()) {
            return self.parse_integer().map(Expression::Literal);
        }
        if self.peek().is_some_and(|byte| byte.is_ascii_alphabetic()) {
            return self.parse_identifier();
        }
        self.error("expected a number, Level, atLevel(...), or parenthesized expression")
    }

    fn parse_identifier(&mut self) -> Result<Expression, String> {
        self.skip_whitespace();
        let start = self.position;
        while self
            .peek()
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        {
            self.position += 1;
        }
        let name = std::str::from_utf8(&self.input[start..self.position])
            .expect("formula is validated as ASCII");
        match name {
            "Level" => Ok(Expression::Level),
            "atLevel" => {
                self.expect(b'(')?;
                let level = self.parse_integer()?;
                let level = u32::try_from(level)
                    .ok()
                    .filter(|level| *level > 0)
                    .ok_or_else(|| {
                        format!(
                            "atLevel requires a positive 32-bit level number at byte {}",
                            self.position
                        )
                    })?;
                self.expect(b')')?;
                Ok(Expression::AtLevel(level))
            }
            _ => self.error(&format!("unknown identifier {name:?}")),
        }
    }

    fn parse_integer(&mut self) -> Result<i128, String> {
        self.skip_whitespace();
        let start = self.position;
        while self.peek().is_some_and(|byte| byte.is_ascii_digit()) {
            self.position += 1;
        }
        if start == self.position {
            return self.error("expected an integer");
        }
        let value = std::str::from_utf8(&self.input[start..self.position])
            .expect("formula is validated as ASCII");
        value
            .parse()
            .map_err(|_| format!("integer is too large at byte {start}"))
    }

    fn expect(
        &mut self,
        expected: u8,
    ) -> Result<(), String> {
        if self.consume(expected) {
            return Ok(());
        }
        self.error(&format!("expected {:?}", char::from(expected)))
    }

    fn consume(
        &mut self,
        expected: u8,
    ) -> bool {
        self.skip_whitespace();
        if self.peek() == Some(expected) {
            self.position += 1;
            true
        } else {
            false
        }
    }

    fn skip_whitespace(&mut self) {
        while self.peek().is_some_and(|byte| byte.is_ascii_whitespace()) {
            self.position += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.input.get(self.position).copied()
    }

    fn error<T>(
        &self,
        message: &str,
    ) -> Result<T, String> {
        Err(format!("{message} at byte {}", self.position))
    }
}

fn binary(
    operator: BinaryOperator,
    left: Expression,
    right: Expression,
) -> Expression {
    Expression::Binary {
        operator,
        left: Box::new(left),
        right: Box::new(right),
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
    use super::parse_formula;

    #[test]
    fn formulas_apply_precedence_and_right_associative_exponents() {
        let config = curves(
            r#"
default_curve = "math"

[[curves]]
name = "math"

[[curves.ranges]]
start = 1
end = 3
formula = "2 + Level * 2 ^ 3 ^ 2 / 256 - 1"
"#,
        )
        .expect("valid formula configuration");
        let curve = config.default_curve();

        assert_eq!(curve.required_for_level(1), Some(3));
        assert_eq!(curve.required_for_level(2), Some(5));
        assert_eq!(curve.required_for_level(3), Some(7));

        let signed = curves(
            r#"
default_curve = "signed"

[[curves]]
name = "signed"

[[curves.ranges]]
start = 1
end = 1
formula = "-2 ^ 2 + 5"
"#,
        )
        .expect("valid signed formula configuration");
        assert_eq!(signed.default_curve().required_for_level(1), Some(1));
    }

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
