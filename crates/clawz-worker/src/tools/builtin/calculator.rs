use crate::tools::tool_trait::{Tool, ToolContext};
use async_trait::async_trait;
use clawz_core::error::ClawzError;
use clawz_core::types::{ToolResult, ToolSchema};
use serde_json::Value;
use std::fmt;

pub struct CalculatorTool;

impl CalculatorTool {
    pub fn new() -> Self {
        Self
    }

    /// Evaluate a mathematical expression string safely.
    pub fn evaluate(expr: &str) -> Result<f64, CalcError> {
        let tokens = tokenize(expr)?;
        let mut parser = Parser::new(tokens);
        let result = parser.parse_expr()?;
        if !parser.is_done() {
            return Err(CalcError::Parse(format!(
                "unexpected token at position {}",
                parser.pos
            )));
        }
        Ok(result)
    }
}

impl Default for CalculatorTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for CalculatorTool {
    fn name(&self) -> &str {
        "calculator"
    }

    fn description(&self) -> &str {
        "Evaluate mathematical expressions. Supports arithmetic, trig (sin, cos, tan, asin, acos, atan, atan2), \
         log/exp (log, log2, log10, ln, exp), powers (pow, sqrt, cbrt), rounding (floor, ceil, round, abs), \
         and constants (pi, e, tau)."
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "calculator".into(),
            description: self.description().into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "expression": {
                        "type": "string",
                        "description": "Mathematical expression to evaluate (e.g., '2 + 3 * sqrt(16)')"
                    }
                },
                "required": ["expression"]
            }),
        }
    }

    async fn execute(
        &self,
        _ctx: &ToolContext,
        args: Value,
    ) -> Result<ToolResult, ClawzError> {
        let expr = args["expression"]
            .as_str()
            .ok_or_else(|| ClawzError::Validation("expression required".into()))?;

        let result = Self::evaluate(expr).map_err(|e| ClawzError::Validation(e.to_string()))?;

        // Check for NaN / Infinity
        let output = if result.is_nan() {
            serde_json::json!({
                "expression": expr,
                "result": null,
                "error": "result is NaN"
            })
        } else if result.is_infinite() {
            serde_json::json!({
                "expression": expr,
                "result": null,
                "error": if result.is_sign_positive() { "overflow: +Infinity" } else { "overflow: -Infinity" }
            })
        } else {
            serde_json::json!({
                "expression": expr,
                "result": result,
                "result_str": format_number(result)
            })
        };

        Ok(ToolResult {
            tool_call_id: String::new(),
            output: output.to_string(),
            is_error: false,
        })
    }
}

fn format_number(n: f64) -> String {
    if n == n.floor() && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        format!("{}", n)
    }
}

// ─── Tokenizer ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Number(f64),
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Caret,
    LParen,
    RParen,
    Comma,
    Ident(String),
}

#[derive(Debug)]
pub struct CalcError(String);

impl CalcError {
    fn parse(msg: impl Into<String>) -> Self {
        CalcError(msg.into())
    }
}

impl fmt::Display for CalcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "calculator error: {}", self.0)
    }
}

impl std::error::Error for CalcError {}

impl From<String> for CalcError {
    fn from(s: String) -> Self {
        CalcError(s)
    }
}

impl CalcError {
    fn lex(msg: impl Into<String>) -> Self {
        CalcError(format!("lex error: {}", msg.into()))
    }
    #[allow(non_snake_case)]
    fn Parse(msg: impl Into<String>) -> Self {
        CalcError(format!("parse error: {}", msg.into()))
    }
    fn eval(msg: impl Into<String>) -> Self {
        CalcError(format!("eval error: {}", msg.into()))
    }
}

fn tokenize(input: &str) -> Result<Vec<Token>, CalcError> {
    let mut tokens = Vec::new();
    let mut chars = input.chars().peekable();

    while let Some(&c) = chars.peek() {
        match c {
            ' ' | '\t' | '\n' | '\r' => {
                chars.next();
            }
            '0'..='9' | '.' => {
                let mut num_str = String::new();
                while let Some(&d) = chars.peek() {
                    if d.is_ascii_digit() || d == '.' || d == 'e' || d == 'E' {
                        num_str.push(d);
                        chars.next();
                        // Handle exponent sign
                        if d == 'e' || d == 'E' {
                            if let Some(&sign) = chars.peek() {
                                if sign == '+' || sign == '-' {
                                    num_str.push(sign);
                                    chars.next();
                                }
                            }
                        }
                    } else {
                        break;
                    }
                }
                let n: f64 = num_str
                    .parse()
                    .map_err(|_| CalcError::lex(format!("invalid number: {}", num_str)))?;
                tokens.push(Token::Number(n));
            }
            'a'..='z' | 'A'..='Z' | '_' => {
                let mut name = String::new();
                while let Some(&d) = chars.peek() {
                    if d.is_alphanumeric() || d == '_' {
                        name.push(d);
                        chars.next();
                    } else {
                        break;
                    }
                }
                tokens.push(Token::Ident(name));
            }
            '+' => { tokens.push(Token::Plus); chars.next(); }
            '-' => { tokens.push(Token::Minus); chars.next(); }
            '*' => { tokens.push(Token::Star); chars.next(); }
            '/' => { tokens.push(Token::Slash); chars.next(); }
            '%' => { tokens.push(Token::Percent); chars.next(); }
            '^' => { tokens.push(Token::Caret); chars.next(); }
            '(' => { tokens.push(Token::LParen); chars.next(); }
            ')' => { tokens.push(Token::RParen); chars.next(); }
            ',' => { tokens.push(Token::Comma); chars.next(); }
            other => {
                return Err(CalcError::lex(format!("unexpected character: '{}'", other)));
            }
        }
    }

    Ok(tokens)
}

// ─── Recursive Descent Parser ─────────────────────────────────────────────────

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn new(tokens: Vec<Token>) -> Self {
        Self { tokens, pos: 0 }
    }

    fn is_done(&self) -> bool {
        self.pos >= self.tokens.len()
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn consume(&mut self) -> Option<Token> {
        if self.pos < self.tokens.len() {
            let tok = self.tokens[self.pos].clone();
            self.pos += 1;
            Some(tok)
        } else {
            None
        }
    }

    fn expect(&mut self, expected: &Token) -> Result<(), CalcError> {
        match self.peek() {
            Some(t) if t == expected => {
                self.consume();
                Ok(())
            }
            other => Err(CalcError::Parse(format!(
                "expected {:?}, got {:?}",
                expected, other
            ))),
        }
    }

    /// expr = additive
    fn parse_expr(&mut self) -> Result<f64, CalcError> {
        self.parse_additive()
    }

    /// additive = multiplicative (('+' | '-') multiplicative)*
    fn parse_additive(&mut self) -> Result<f64, CalcError> {
        let mut left = self.parse_multiplicative()?;

        loop {
            match self.peek() {
                Some(Token::Plus) => {
                    self.consume();
                    left += self.parse_multiplicative()?;
                }
                Some(Token::Minus) => {
                    self.consume();
                    left -= self.parse_multiplicative()?;
                }
                _ => break,
            }
        }

        Ok(left)
    }

    /// multiplicative = power (('*' | '/' | '%') power)*
    fn parse_multiplicative(&mut self) -> Result<f64, CalcError> {
        let mut left = self.parse_power()?;

        loop {
            match self.peek() {
                Some(Token::Star) => {
                    self.consume();
                    left *= self.parse_power()?;
                }
                Some(Token::Slash) => {
                    self.consume();
                    let right = self.parse_power()?;
                    if right == 0.0 {
                        return Err(CalcError::eval("division by zero"));
                    }
                    left /= right;
                }
                Some(Token::Percent) => {
                    self.consume();
                    let right = self.parse_power()?;
                    if right == 0.0 {
                        return Err(CalcError::eval("modulo by zero"));
                    }
                    left %= right;
                }
                _ => break,
            }
        }

        Ok(left)
    }

    /// power = unary ('^' unary)*  (right-associative)
    fn parse_power(&mut self) -> Result<f64, CalcError> {
        let base = self.parse_unary()?;

        if matches!(self.peek(), Some(Token::Caret)) {
            self.consume();
            let exp = self.parse_power()?; // right-associative
            Ok(base.powf(exp))
        } else {
            Ok(base)
        }
    }

    /// unary = ('-' | '+') unary | primary
    fn parse_unary(&mut self) -> Result<f64, CalcError> {
        match self.peek() {
            Some(Token::Minus) => {
                self.consume();
                Ok(-self.parse_unary()?)
            }
            Some(Token::Plus) => {
                self.consume();
                self.parse_unary()
            }
            _ => self.parse_primary(),
        }
    }

    /// primary = number | ident | ident '(' args ')' | '(' expr ')'
    fn parse_primary(&mut self) -> Result<f64, CalcError> {
        match self.peek().cloned() {
            Some(Token::Number(n)) => {
                self.consume();
                Ok(n)
            }

            Some(Token::LParen) => {
                self.consume();
                let val = self.parse_expr()?;
                self.expect(&Token::RParen)?;
                Ok(val)
            }

            Some(Token::Ident(name)) => {
                self.consume();

                // Check if it's a function call
                if matches!(self.peek(), Some(Token::LParen)) {
                    self.consume(); // '('
                    let mut args = Vec::new();

                    if !matches!(self.peek(), Some(Token::RParen)) {
                        args.push(self.parse_expr()?);
                        while matches!(self.peek(), Some(Token::Comma)) {
                            self.consume();
                            args.push(self.parse_expr()?);
                        }
                    }
                    self.expect(&Token::RParen)?;
                    call_function(&name, &args)
                } else {
                    // Constant
                    resolve_constant(&name)
                }
            }

            other => Err(CalcError::Parse(format!("unexpected token: {:?}", other))),
        }
    }
}

fn resolve_constant(name: &str) -> Result<f64, CalcError> {
    match name {
        "pi" | "PI" => Ok(std::f64::consts::PI),
        "e" | "E" => Ok(std::f64::consts::E),
        "tau" | "TAU" => Ok(std::f64::consts::TAU),
        "inf" | "infinity" | "Infinity" => Ok(f64::INFINITY),
        "nan" | "NaN" => Ok(f64::NAN),
        other => Err(CalcError::eval(format!("unknown constant: '{}'", other))),
    }
}

fn call_function(name: &str, args: &[f64]) -> Result<f64, CalcError> {
    let one = || -> Result<f64, CalcError> {
        if args.len() == 1 {
            Ok(args[0])
        } else {
            Err(CalcError::eval(format!("{}() takes 1 argument, got {}", name, args.len())))
        }
    };

    match name {
        // Trig (radians)
        "sin" => Ok(one()?.sin()),
        "cos" => Ok(one()?.cos()),
        "tan" => Ok(one()?.tan()),
        "asin" => Ok(one()?.asin()),
        "acos" => Ok(one()?.acos()),
        "atan" => Ok(one()?.atan()),
        "atan2" => {
            if args.len() == 2 {
                Ok(args[0].atan2(args[1]))
            } else {
                Err(CalcError::eval("atan2() takes 2 arguments"))
            }
        }
        "sinh" => Ok(one()?.sinh()),
        "cosh" => Ok(one()?.cosh()),
        "tanh" => Ok(one()?.tanh()),

        // Trig (degrees helpers)
        "sind" => Ok(one()?.to_radians().sin()),
        "cosd" => Ok(one()?.to_radians().cos()),
        "tand" => Ok(one()?.to_radians().tan()),

        // Logarithm / exponential
        "log" | "log10" => Ok(one()?.log10()),
        "log2" => Ok(one()?.log2()),
        "ln" => Ok(one()?.ln()),
        "exp" => Ok(one()?.exp()),
        "exp2" => Ok(one()?.exp2()),

        // Power / root
        "pow" => {
            if args.len() == 2 {
                Ok(args[0].powf(args[1]))
            } else {
                Err(CalcError::eval("pow() takes 2 arguments"))
            }
        }
        "sqrt" => {
            let x = one()?;
            if x < 0.0 {
                Err(CalcError::eval("sqrt of negative number"))
            } else {
                Ok(x.sqrt())
            }
        }
        "cbrt" => Ok(one()?.cbrt()),
        "hypot" => {
            if args.len() == 2 {
                Ok(args[0].hypot(args[1]))
            } else {
                Err(CalcError::eval("hypot() takes 2 arguments"))
            }
        }

        // Rounding
        "floor" => Ok(one()?.floor()),
        "ceil" | "ceiling" => Ok(one()?.ceil()),
        "round" => Ok(one()?.round()),
        "trunc" => Ok(one()?.trunc()),
        "fract" => Ok(one()?.fract()),
        "abs" => Ok(one()?.abs()),
        "sign" | "signum" => Ok(one()?.signum()),

        // Min/max
        "min" => {
            if args.len() == 2 {
                Ok(args[0].min(args[1]))
            } else {
                Err(CalcError::eval("min() takes 2 arguments"))
            }
        }
        "max" => {
            if args.len() == 2 {
                Ok(args[0].max(args[1]))
            } else {
                Err(CalcError::eval("max() takes 2 arguments"))
            }
        }
        "clamp" => {
            if args.len() == 3 {
                Ok(args[0].clamp(args[1], args[2]))
            } else {
                Err(CalcError::eval("clamp() takes 3 arguments"))
            }
        }

        other => Err(CalcError::eval(format!("unknown function: '{}'", other))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eval(s: &str) -> f64 {
        CalculatorTool::evaluate(s).expect(s)
    }

    #[test]
    fn test_basic_arithmetic() {
        assert_eq!(eval("2 + 3"), 5.0);
        assert_eq!(eval("10 - 4"), 6.0);
        assert_eq!(eval("3 * 4"), 12.0);
        assert_eq!(eval("10 / 4"), 2.5);
        assert_eq!(eval("10 % 3"), 1.0);
    }

    #[test]
    fn test_operator_precedence() {
        assert_eq!(eval("2 + 3 * 4"), 14.0);
        assert_eq!(eval("(2 + 3) * 4"), 20.0);
        assert_eq!(eval("2 ^ 3 ^ 2"), 512.0); // right-associative: 2^(3^2) = 2^9
    }

    #[test]
    fn test_unary_minus() {
        assert_eq!(eval("-5"), -5.0);
        assert_eq!(eval("-(3 + 2)"), -5.0);
        assert_eq!(eval("3 * -2"), -6.0);
    }

    #[test]
    fn test_functions() {
        assert!((eval("sqrt(16)") - 4.0).abs() < 1e-10);
        assert!((eval("sin(0)")).abs() < 1e-10);
        assert!((eval("cos(0)") - 1.0).abs() < 1e-10);
        assert!((eval("ln(e)") - 1.0).abs() < 1e-10);
        assert!((eval("log(100)") - 2.0).abs() < 1e-10);
        assert!((eval("abs(-5)") - 5.0).abs() < 1e-10);
        assert_eq!(eval("floor(3.7)"), 3.0);
        assert_eq!(eval("ceil(3.2)"), 4.0);
        assert_eq!(eval("round(3.5)"), 4.0);
    }

    #[test]
    fn test_constants() {
        assert!((eval("pi") - std::f64::consts::PI).abs() < 1e-10);
        assert!((eval("e") - std::f64::consts::E).abs() < 1e-10);
    }

    #[test]
    fn test_complex_expression() {
        // sqrt(3^2 + 4^2) = 5
        assert!((eval("sqrt(3^2 + 4^2)") - 5.0).abs() < 1e-10);
        // 2 * pi * 5 = circumference of circle r=5
        let expected = 2.0 * std::f64::consts::PI * 5.0;
        assert!((eval("2 * pi * 5") - expected).abs() < 1e-10);
    }

    #[test]
    fn test_division_by_zero() {
        let result = CalculatorTool::evaluate("1 / 0");
        assert!(result.is_err());
    }

    #[test]
    fn test_unknown_function() {
        let result = CalculatorTool::evaluate("foobar(1)");
        assert!(result.is_err());
    }

    #[test]
    fn test_multiarg_functions() {
        assert_eq!(eval("pow(2, 10)"), 1024.0);
        assert_eq!(eval("min(3, 5)"), 3.0);
        assert_eq!(eval("max(3, 5)"), 5.0);
        assert!((eval("atan2(1, 1)") - std::f64::consts::FRAC_PI_4).abs() < 1e-10);
    }

    #[tokio::test]
    async fn test_tool_execute() {
        let tool = CalculatorTool::new();
        let ctx = ToolContext {
            agent_id: "a".into(),
            conversation_id: "c".into(),
            user_id: None,
            config: crate::tools::ToolConfig::default(),
        };
        let result = tool
            .execute(&ctx, serde_json::json!({"expression": "2 + 2"}))
            .await
            .unwrap();
        let parsed: Value = serde_json::from_str(&result.output).unwrap();
        assert_eq!(parsed["result"], 4.0);
    }
}
