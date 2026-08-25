#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Expression<Atom> {
    Atom(Atom),
    Negate(Box<Expression<Atom>>),
    Binary {
        operator: BinaryOperator,
        left: Box<Expression<Atom>>,
        right: Box<Expression<Atom>>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BinaryOperator {
    Add,
    Subtract,
    Multiply,
    Divide,
    Exponentiate,
}

type ParseAtom<Atom> = for<'input> fn(&mut Parser<'input, Atom>) -> Result<Atom, String>;

pub(crate) struct Parser<'input, Atom> {
    input: &'input [u8],
    position: usize,
    parse_atom: ParseAtom<Atom>,
}

pub(crate) fn parse<Atom>(
    source: &str,
    parse_atom: ParseAtom<Atom>,
) -> Result<Expression<Atom>, String> {
    debug_assert!(source.is_ascii());
    let mut parser = Parser {
        input: source.as_bytes(),
        position: 0,
        parse_atom,
    };
    let expression = parser.parse_expression()?;
    parser.skip_whitespace();
    if parser.position != parser.input.len() {
        return parser.error("unexpected trailing input");
    }
    Ok(expression)
}

impl<Atom> Parser<'_, Atom> {
    pub(crate) fn parse_expression(&mut self) -> Result<Expression<Atom>, String> {
        self.parse_precedence(0)
    }

    fn parse_precedence(
        &mut self,
        minimum_binding_power: u8,
    ) -> Result<Expression<Atom>, String> {
        let mut expression = if self.consume(b'+') {
            self.parse_precedence(6)?
        } else if self.consume(b'-') {
            Expression::Negate(Box::new(self.parse_precedence(6)?))
        } else if self.consume(b'(') {
            let expression = self.parse_expression()?;
            self.expect(b')')?;
            expression
        } else {
            Expression::Atom((self.parse_atom)(self)?)
        };

        loop {
            let Some((operator, left_binding_power, right_binding_power)) = self.binary_operator()
            else {
                return Ok(expression);
            };
            if left_binding_power < minimum_binding_power {
                return Ok(expression);
            }
            self.position += 1;
            let right = self.parse_precedence(right_binding_power)?;
            expression = Expression::Binary {
                operator,
                left: Box::new(expression),
                right: Box::new(right),
            };
        }
    }

    fn binary_operator(&mut self) -> Option<(BinaryOperator, u8, u8)> {
        self.skip_whitespace();
        match self.peek()? {
            b'+' => Some((BinaryOperator::Add, 1, 2)),
            b'-' => Some((BinaryOperator::Subtract, 1, 2)),
            b'*' => Some((BinaryOperator::Multiply, 3, 4)),
            b'/' => Some((BinaryOperator::Divide, 3, 4)),
            b'^' => Some((BinaryOperator::Exponentiate, 7, 6)),
            _ => None,
        }
    }

    pub(crate) fn identifier(&mut self) -> Result<String, String> {
        self.skip_whitespace();
        let start = self.position;
        while self
            .peek()
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        {
            self.position += 1;
        }
        if start == self.position {
            return self.error("expected an identifier");
        }
        Ok(std::str::from_utf8(&self.input[start..self.position])
            .expect("formula is validated as ASCII")
            .to_owned())
    }

    pub(crate) fn integer(&mut self) -> Result<(usize, &str), String> {
        self.skip_whitespace();
        let start = self.position;
        while self.peek().is_some_and(|byte| byte.is_ascii_digit()) {
            self.position += 1;
        }
        if start == self.position {
            return self.error("expected an integer");
        }
        Ok((
            start,
            std::str::from_utf8(&self.input[start..self.position])
                .expect("formula is validated as ASCII"),
        ))
    }

    pub(crate) fn number(&mut self) -> Result<(usize, &str), String> {
        self.skip_whitespace();
        let start = self.position;
        let mut decimal_points = 0;
        while self.peek().is_some_and(|byte| {
            if byte == b'.' {
                decimal_points += 1;
                true
            } else {
                byte.is_ascii_digit()
            }
        }) {
            self.position += 1;
        }
        if decimal_points > 1 || start == self.position {
            return self.error("invalid number");
        }
        Ok((
            start,
            std::str::from_utf8(&self.input[start..self.position])
                .expect("formula is validated as ASCII"),
        ))
    }

    pub(crate) fn expect(
        &mut self,
        expected: u8,
    ) -> Result<(), String> {
        if self.consume(expected) {
            return Ok(());
        }
        self.error(&format!("expected {:?}", char::from(expected)))
    }

    pub(crate) fn consume(
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

    pub(crate) fn skip_whitespace(&mut self) {
        while self.peek().is_some_and(|byte| byte.is_ascii_whitespace()) {
            self.position += 1;
        }
    }

    pub(crate) fn peek(&self) -> Option<u8> {
        self.input.get(self.position).copied()
    }

    pub(crate) fn position(&self) -> usize {
        self.position
    }

    pub(crate) fn error<T>(
        &self,
        message: &str,
    ) -> Result<T, String> {
        Err(format!("{message} at byte {}", self.position))
    }
}

#[cfg(test)]
mod tests {
    use super::BinaryOperator;
    use super::Expression;
    use super::Parser;
    use super::parse;

    #[test]
    fn operators_apply_shared_precedence_and_associativity() {
        let expression =
            parse("2 + 3 * 2 ^ 3 ^ 2 / 256 - 1", parse_integer).expect("valid shared expression");
        assert_eq!(evaluate(&expression), 7);

        let signed = parse("-2 ^ 2 + 5", parse_integer).expect("valid signed expression");
        assert_eq!(evaluate(&signed), 1);
    }

    fn parse_integer(parser: &mut Parser<'_, i128>) -> Result<i128, String> {
        parser.skip_whitespace();
        if !parser.peek().is_some_and(|byte| byte.is_ascii_digit()) {
            return parser.error("expected an integer or parenthesized expression");
        }
        let (start, source) = parser.integer()?;
        source
            .parse()
            .map_err(|_| format!("integer is too large at byte {start}"))
    }

    fn evaluate(expression: &Expression<i128>) -> i128 {
        match expression {
            Expression::Atom(value) => *value,
            Expression::Negate(value) => -evaluate(value),
            Expression::Binary {
                operator,
                left,
                right,
            } => {
                let left = evaluate(left);
                let right = evaluate(right);
                match operator {
                    BinaryOperator::Add => left + right,
                    BinaryOperator::Subtract => left - right,
                    BinaryOperator::Multiply => left * right,
                    BinaryOperator::Divide => left / right,
                    BinaryOperator::Exponentiate => left.pow(
                        u32::try_from(right).expect("test expression uses non-negative powers"),
                    ),
                }
            }
        }
    }
}
