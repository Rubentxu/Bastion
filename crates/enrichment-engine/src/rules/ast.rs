//! CEL-lite expression AST and evaluation.
//!
//! Defines the typed `Expr` AST enum, `ParseError`, and `EvalContext` for evaluation.

use std::collections::HashMap;

use crate::models::{Fact, OperationInvocation, OperationResult};

/// A typed expression AST for CEL-lite.
#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    // ─── Literals ─────────────────────────────────────────────────────────────
    Bool(bool),
    Int(i32),
    Str(String),

    // ─── Field access ─────────────────────────────────────────────────────────
    /// Access to a built-in field: `exit_code`, `timed_out`, `stdout`.
    Field(&'static str),

    // ─── Comparisons ──────────────────────────────────────────────────────────
    Eq(Box<Expr>, Box<Expr>),
    Ne(Box<Expr>, Box<Expr>),
    Gt(Box<Expr>, Box<Expr>),
    Lt(Box<Expr>, Box<Expr>),
    Ge(Box<Expr>, Box<Expr>),
    Le(Box<Expr>, Box<Expr>),

    // ─── Boolean combinators ──────────────────────────────────────────────────
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Not(Box<Expr>),

    // ─── Predicate functions ───────────────────────────────────────────────────
    /// `contains_fact('key')` — true if any fact has the given key.
    ContainsFact(String),
    /// `fact('key')` — returns the fact's value (used in comparisons).
    Fact(String),
    /// `stdout_contains('str')` — true if stdout contains substring.
    StdoutContains(String),
}

/// Comparison operator for `fact(...)` predicates.
#[derive(Clone, Debug, PartialEq)]
pub enum CompOp {
    Eq,
    Ne,
    Gt,
    Lt,
    Ge,
    Le,
}

/// Parse error with position information.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum ParseError {
    #[error("Unexpected token at position {position}: expected {expected}, found {found}")]
    UnexpectedToken {
        position: usize,
        expected: String,
        found: String,
    },
    #[error("Unterminated string at position {position}")]
    UnterminatedString { position: usize },
    #[error("Invalid expression: {0}")]
    Invalid(String),
}

/// Context for evaluating an expression: the invocation, result, and facts.
#[derive(Debug)]
pub struct EvalContext<'a> {
    /// The original invocation.
    pub invocation: &'a OperationInvocation,
    /// The operation result.
    pub result: &'a OperationResult,
    /// All extracted facts.
    pub facts: &'a [Fact],
    /// Lazily-built map from fact key -> fact for O(1) lookup.
    fact_map: HashMap<&'a str, &'a Fact>,
}

impl<'a> EvalContext<'a> {
    /// Create a new evaluation context.
    pub fn new(invocation: &'a OperationInvocation, result: &'a OperationResult, facts: &'a [Fact]) -> Self {
        let mut ctx = Self {
            invocation,
            result,
            facts,
            fact_map: HashMap::new(),
        };
        ctx.build_fact_map();
        ctx
    }

    fn build_fact_map(&mut self) {
        for fact in self.facts {
            self.fact_map.entry(&fact.key).or_insert(fact);
        }
    }

    /// Look up a fact by key, returning None if absent.
    pub fn get_fact(&self, key: &str) -> Option<&'a Fact> {
        self.fact_map.get(key).copied()
    }

    /// Evaluate an expression to a boolean result.
    ///
    /// Returns `false` for any evaluation error (missing fact, type mismatch).
    /// This makes evaluation deterministic and panic-free.
    pub fn evaluate(&self, expr: &Expr) -> bool {
        match expr {
            Expr::Bool(b) => *b,
            Expr::Int(n) => *n != 0,
            Expr::Str(s) => !s.is_empty(),

            Expr::Field("exit_code") => self.result.exit_code != 0,
            Expr::Field("timed_out") => self.result.timed_out,
            Expr::Field("stdout") => !self.result.stdout.is_empty(),
            Expr::Field(other) => {
                tracing::warn!(field = other, "Unknown field access");
                false
            }

            Expr::Eq(l, r) => self.compare_eq(l, r),
            Expr::Ne(l, r) => !self.compare_eq(l, r),
            Expr::Gt(l, r) => self.compare_gt(l, r),
            Expr::Lt(l, r) => self.compare_lt(l, r),
            Expr::Ge(l, r) => self.compare_ge(l, r),
            Expr::Le(l, r) => self.compare_le(l, r),

            Expr::And(l, r) => self.evaluate(l) && self.evaluate(r),
            Expr::Or(l, r) => self.evaluate(l) || self.evaluate(r),
            Expr::Not(e) => !self.evaluate(e),

            Expr::ContainsFact(key) => self.fact_map.contains_key(key.as_str()),

            Expr::Fact(key) => {
                // Fact by itself is used in comparisons; this case shouldn't normally be reached
                // in isolation since Fact is always wrapped in a comparison by parse_comparison
                tracing::warn!(key = key, "Fact expression not in a comparison");
                false
            }

            Expr::StdoutContains(sub) => self.result.stdout.contains(sub.as_str()),
        }
    }

    fn compare_eq(&self, l: &Expr, r: &Expr) -> bool {
        // Special case: if left is Fact, evaluate as predicate
        if let Expr::Fact(key) = l {
            return self.compare_fact_eq(key, r);
        }
        // String comparison for the CEL-lite DSL
        let l_val = self.extract_str(l);
        let r_val = self.extract_str(r);
        l_val == r_val
    }

    fn compare_fact_eq(&self, key: &str, right: &Expr) -> bool {
        let fact = match self.get_fact(key) {
            Some(f) => f,
            None => return false,
        };
        let expected = self.extract_str(right);
        fact.value == expected
    }

    fn compare_gt(&self, l: &Expr, r: &Expr) -> bool {
        if let Expr::Fact(key) = l {
            return self.compare_fact_cmp(key, r, |a, b| a > b);
        }
        let l_val = self.extract_i32(l);
        let r_val = self.extract_i32(r);
        l_val > r_val
    }

    fn compare_lt(&self, l: &Expr, r: &Expr) -> bool {
        if let Expr::Fact(key) = l {
            return self.compare_fact_cmp(key, r, |a, b| a < b);
        }
        let l_val = self.extract_i32(l);
        let r_val = self.extract_i32(r);
        l_val < r_val
    }

    fn compare_ge(&self, l: &Expr, r: &Expr) -> bool {
        if let Expr::Fact(key) = l {
            return self.compare_fact_cmp(key, r, |a, b| a >= b);
        }
        let l_val = self.extract_i32(l);
        let r_val = self.extract_i32(r);
        l_val >= r_val
    }

    fn compare_le(&self, l: &Expr, r: &Expr) -> bool {
        if let Expr::Fact(key) = l {
            return self.compare_fact_cmp(key, r, |a, b| a <= b);
        }
        let l_val = self.extract_i32(l);
        let r_val = self.extract_i32(r);
        l_val <= r_val
    }

    fn compare_fact_cmp<F>(&self, key: &str, right: &Expr, cmp: F) -> bool
    where
        F: Fn(i32, i32) -> bool,
    {
        let fact = match self.get_fact(key) {
            Some(f) => f,
            None => return false,
        };
        let fact_i32: i32 = fact.value.parse().unwrap_or(0);
        let right_i32 = self.extract_i32(right);
        cmp(fact_i32, right_i32)
    }

    fn extract_str(&self, expr: &Expr) -> String {
        match expr {
            Expr::Str(s) => s.clone(),
            Expr::Int(n) => n.to_string(),
            Expr::Bool(b) => b.to_string(),
            Expr::Field("exit_code") => self.result.exit_code.to_string(),
            Expr::Field("timed_out") => self.result.timed_out.to_string(),
            Expr::Field("stdout") => self.result.stdout.clone(),
            _ => String::new(),
        }
    }

    fn extract_i32(&self, expr: &Expr) -> i32 {
        match expr {
            Expr::Int(n) => *n,
            Expr::Str(s) => s.parse().unwrap_or(0),
            Expr::Field("exit_code") => self.result.exit_code,
            _ => 0,
        }
    }
}

// ─── Recursive-descent parser ─────────────────────────────────────────────────

use super::lexer::{Lexer, Token, TokenKind};

/// CEL-lite expression parser.
/// Parses a token stream into an `Expr` AST.
#[derive(Debug)]
pub struct Parser<'a> {
    lexer: Lexer<'a>,
    current: Token,
}

impl<'a> Parser<'a> {
    /// Parse an expression string into an AST.
    pub fn parse(input: &'a str) -> Result<Expr, ParseError> {
        let mut parser = Self {
            lexer: Lexer::new(input),
            current: Token { kind: TokenKind::Eof, position: 0, lexeme: String::new() },
        };
        parser.advance();
        let expr = parser.parse_expression()?;
        if parser.current.kind != TokenKind::Eof {
            Err(ParseError::UnexpectedToken {
                position: parser.current.position,
                expected: "end of input".to_string(),
                found: format!("{:?}", parser.current.kind),
            })
        } else {
            Ok(expr)
        }
    }

    fn advance(&mut self) {
        self.current = self.lexer.next_token();
    }

    fn parse_expression(&mut self) -> Result<Expr, ParseError> {
        self.parse_or()
    }

    fn parse_or(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_and()?;
        while self.current.kind == TokenKind::Or {
            self.advance();
            let right = self.parse_and()?;
            left = Expr::Or(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_not()?;
        while self.current.kind == TokenKind::And {
            self.advance();
            let right = self.parse_not()?;
            left = Expr::And(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_not(&mut self) -> Result<Expr, ParseError> {
        if self.current.kind == TokenKind::Not {
            self.advance();
            let inner = self.parse_not()?;
            return Ok(Expr::Not(Box::new(inner)));
        }
        self.parse_comparison()
    }

    fn parse_comparison(&mut self) -> Result<Expr, ParseError> {
        let left = self.parse_primary()?;

        match &self.current.kind {
            TokenKind::Eq => {
                self.advance();
                let right = self.parse_primary()?;
                Ok(Expr::Eq(Box::new(left), Box::new(right)))
            }
            TokenKind::Ne => {
                self.advance();
                let right = self.parse_primary()?;
                Ok(Expr::Ne(Box::new(left), Box::new(right)))
            }
            TokenKind::Gt => {
                self.advance();
                let right = self.parse_primary()?;
                Ok(Expr::Gt(Box::new(left), Box::new(right)))
            }
            TokenKind::Lt => {
                self.advance();
                let right = self.parse_primary()?;
                Ok(Expr::Lt(Box::new(left), Box::new(right)))
            }
            TokenKind::Ge => {
                self.advance();
                let right = self.parse_primary()?;
                Ok(Expr::Ge(Box::new(left), Box::new(right)))
            }
            TokenKind::Le => {
                self.advance();
                let right = self.parse_primary()?;
                Ok(Expr::Le(Box::new(left), Box::new(right)))
            }
            _ => Ok(left),
        }
    }

    fn parse_primary(&mut self) -> Result<Expr, ParseError> {
        match &self.current.kind {
            TokenKind::Bool(b) => {
                let expr = Expr::Bool(*b);
                self.advance();
                Ok(expr)
            }
            TokenKind::Int(n) => {
                let expr = Expr::Int(*n);
                self.advance();
                Ok(expr)
            }
            TokenKind::Str(s) => {
                let expr = Expr::Str(s.clone());
                self.advance();
                Ok(expr)
            }
            TokenKind::Ident(name) => {
                let name = name.clone();
                self.advance();
                self.parse_ident_or_call(name)
            }
            TokenKind::LParen => {
                self.advance();
                let expr = self.parse_expression()?;
                if self.current.kind != TokenKind::RParen {
                    return Err(ParseError::UnexpectedToken {
                        position: self.current.position,
                        expected: ")".to_string(),
                        found: format!("{:?}", self.current.kind),
                    });
                }
                self.advance();
                Ok(expr)
            }
            TokenKind::Eof => Err(ParseError::UnexpectedToken {
                position: self.current.position,
                expected: "expression".to_string(),
                found: "end of input".to_string(),
            }),
            _ => Err(ParseError::UnexpectedToken {
                position: self.current.position,
                expected: "expression".to_string(),
                found: format!("{:?}", self.current.kind),
            }),
        }
    }

    fn parse_ident_or_call(&mut self, name: String) -> Result<Expr, ParseError> {
        // Check if it's a function call
        if self.current.kind == TokenKind::LParen {
            self.advance();
            let args = self.parse_call_args()?;
            if self.current.kind != TokenKind::RParen {
                return Err(ParseError::UnexpectedToken {
                    position: self.current.position,
                    expected: ")".to_string(),
                    found: format!("{:?}", self.current.kind),
                });
            }
            self.advance();

            match name.as_str() {
                "contains_fact" => {
                    if args.len() != 1 {
                        return Err(ParseError::Invalid(format!(
                            "contains_fact takes 1 argument, got {}",
                            args.len()
                        )));
                    }
                    if let Expr::Str(key) = &args[0] {
                        Ok(Expr::ContainsFact(key.clone()))
                    } else {
                        Err(ParseError::Invalid(
                            "contains_fact argument must be a string literal".to_string(),
                        ))
                    }
                }
                "fact" => {
                    // fact('key') — returns Expr::Fact(key) which parse_comparison will wrap
                    if args.len() != 1 {
                        return Err(ParseError::Invalid(format!(
                            "fact takes 1 argument (key), got {}",
                            args.len()
                        )));
                    }
                    let key = if let Expr::Str(k) = &args[0] {
                        k.clone()
                    } else {
                        return Err(ParseError::Invalid(
                            "fact key must be a string literal".to_string(),
                        ));
                    };
                    Ok(Expr::Fact(key))
                }
                "stdout_contains" => {
                    if args.len() != 1 {
                        return Err(ParseError::Invalid(format!(
                            "stdout_contains takes 1 argument, got {}",
                            args.len()
                        )));
                    }
                    if let Expr::Str(s) = &args[0] {
                        Ok(Expr::StdoutContains(s.clone()))
                    } else {
                        Err(ParseError::Invalid(
                            "stdout_contains argument must be a string literal".to_string(),
                        ))
                    }
                }
                "timed_out" => {
                    if args.is_empty() {
                        Ok(Expr::Field("timed_out"))
                    } else {
                        Err(ParseError::Invalid(
                            "timed_out takes no arguments".to_string(),
                        ))
                    }
                }
                "exit_code" => {
                    if args.is_empty() {
                        Ok(Expr::Field("exit_code"))
                    } else {
                        Err(ParseError::Invalid(
                            "exit_code takes no arguments".to_string(),
                        ))
                    }
                }
                _ => Err(ParseError::Invalid(format!("Unknown function: {}", name))),
            }
        } else {
            // Plain identifier — could be a field
            match name.as_str() {
                "exit_code" => Ok(Expr::Field("exit_code")),
                "timed_out" => Ok(Expr::Field("timed_out")),
                "stdout" => Ok(Expr::Field("stdout")),
                _ => Ok(Expr::Str(name)),
            }
        }
    }

    fn parse_call_args(&mut self) -> Result<Vec<Expr>, ParseError> {
        let mut args = Vec::new();
        if self.current.kind == TokenKind::RParen {
            return Ok(args);
        }
        args.push(self.parse_expression()?);
        while self.current.kind == TokenKind::Comma {
            self.advance();
            args.push(self.parse_expression()?);
        }
        Ok(args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── Parser tests ─────────────────────────────────────────────────────────

    #[test]
    fn parse_exit_code_equals_zero() {
        let expr = Parser::parse("exit_code == 0").unwrap();
        assert!(matches!(expr, Expr::Eq(l, r)
            if matches!(*l, Expr::Field("exit_code"))
            && matches!(*r, Expr::Int(0))));
    }

    #[test]
    fn parse_compound_and() {
        let expr = Parser::parse("exit_code == 0 and contains_fact('build_status')").unwrap();
        assert!(matches!(expr, Expr::And(_, _)));
    }

    #[test]
    fn parse_malformed_returns_error() {
        let result = Parser::parse("exit_code ==");
        assert!(result.is_err());
        if let Err(ParseError::UnexpectedToken { position, .. }) = result {
            assert!(position > 0);
        }
    }

    // ─── Evaluator tests ──────────────────────────────────────────────────────

    fn fact(key: &str, value: &str) -> Fact {
        Fact {
            key: key.to_string(),
            value: value.to_string(),
            tags: vec![],
            source_extractor: "test".to_string(),
            confidence: 1.0,
        }
    }

    #[test]
    fn eval_exit_code_eq_true_when_zero() {
        let invocation = OperationInvocation::from_command("mvn package");
        let result = OperationResult { exit_code: 0, stdout: String::new(), stderr: String::new(), duration_ms: 0, timed_out: false };
        let facts = vec![];
        let ctx = EvalContext::new(&invocation, &result, &facts);
        let expr = Parser::parse("exit_code == 0").unwrap();
        assert!(ctx.evaluate(&expr));
    }

    #[test]
    fn eval_exit_code_eq_false_when_nonzero() {
        let invocation = OperationInvocation::from_command("mvn package");
        let result = OperationResult { exit_code: 1, stdout: String::new(), stderr: String::new(), duration_ms: 0, timed_out: false };
        let facts = vec![];
        let ctx = EvalContext::new(&invocation, &result, &facts);
        let expr = Parser::parse("exit_code == 0").unwrap();
        assert!(!ctx.evaluate(&expr));
    }

    #[test]
    fn eval_contains_fact_true_when_present() {
        let invocation = OperationInvocation::from_command("mvn package");
        let result = OperationResult { exit_code: 0, stdout: String::new(), stderr: String::new(), duration_ms: 0, timed_out: false };
        let facts = vec![fact("build_status", "BUILD SUCCESS")];
        let ctx = EvalContext::new(&invocation, &result, &facts);
        let expr = Parser::parse("contains_fact('build_status')").unwrap();
        assert!(ctx.evaluate(&expr));
    }

    #[test]
    fn eval_contains_fact_false_when_absent() {
        let invocation = OperationInvocation::from_command("mvn package");
        let result = OperationResult { exit_code: 0, stdout: String::new(), stderr: String::new(), duration_ms: 0, timed_out: false };
        let facts = vec![];
        let ctx = EvalContext::new(&invocation, &result, &facts);
        let expr = Parser::parse("contains_fact('build_status')").unwrap();
        assert!(!ctx.evaluate(&expr));
    }

    #[test]
    fn eval_fact_missing_returns_false() {
        let invocation = OperationInvocation::from_command("mvn package");
        let result = OperationResult { exit_code: 0, stdout: String::new(), stderr: String::new(), duration_ms: 0, timed_out: false };
        let facts = vec![];
        let ctx = EvalContext::new(&invocation, &result, &facts);
        let expr = Parser::parse("fact('tests_failed') > '0'").unwrap();
        assert!(!ctx.evaluate(&expr));
    }

    #[test]
    fn eval_stdout_contains() {
        let invocation = OperationInvocation::from_command("mvn package");
        let result = OperationResult { exit_code: 0, stdout: "BUILD SUCCESS".to_string(), stderr: String::new(), duration_ms: 0, timed_out: false };
        let facts = vec![];
        let ctx = EvalContext::new(&invocation, &result, &facts);
        let expr = Parser::parse("stdout_contains('BUILD SUCCESS')").unwrap();
        assert!(ctx.evaluate(&expr));
    }

    #[test]
    fn eval_timed_out_true() {
        let invocation = OperationInvocation::from_command("mvn package");
        let result = OperationResult { exit_code: 0, stdout: String::new(), stderr: String::new(), duration_ms: 0, timed_out: true };
        let facts = vec![];
        let ctx = EvalContext::new(&invocation, &result, &facts);
        let expr = Parser::parse("timed_out").unwrap();
        assert!(ctx.evaluate(&expr));
    }

    #[test]
    fn eval_timed_out_false() {
        let invocation = OperationInvocation::from_command("mvn package");
        let result = OperationResult { exit_code: 0, stdout: String::new(), stderr: String::new(), duration_ms: 0, timed_out: false };
        let facts = vec![];
        let ctx = EvalContext::new(&invocation, &result, &facts);
        let expr = Parser::parse("timed_out").unwrap();
        assert!(!ctx.evaluate(&expr));
    }

    #[test]
    fn eval_and_short_circuit() {
        // If left is false, right is not evaluated (but our evaluator is pure, so both sides run)
        let invocation = OperationInvocation::from_command("mvn package");
        let result = OperationResult { exit_code: 1, stdout: String::new(), stderr: String::new(), duration_ms: 0, timed_out: false };
        let facts = vec![];
        let ctx = EvalContext::new(&invocation, &result, &facts);
        let expr = Parser::parse("exit_code == 0 and contains_fact('build_status')").unwrap();
        // exit_code == 0 is false, so overall should be false
        assert!(!ctx.evaluate(&expr));
    }

    #[test]
    fn eval_or_short_circuit() {
        let invocation = OperationInvocation::from_command("mvn package");
        let result = OperationResult { exit_code: 0, stdout: String::new(), stderr: String::new(), duration_ms: 0, timed_out: false };
        let facts = vec![];
        let ctx = EvalContext::new(&invocation, &result, &facts);
        let expr = Parser::parse("exit_code == 0 or contains_fact('build_status')").unwrap();
        // exit_code == 0 is true, so overall should be true
        assert!(ctx.evaluate(&expr));
    }

    #[test]
    fn eval_not() {
        let invocation = OperationInvocation::from_command("mvn package");
        let result = OperationResult { exit_code: 1, stdout: String::new(), stderr: String::new(), duration_ms: 0, timed_out: false };
        let facts = vec![];
        let ctx = EvalContext::new(&invocation, &result, &facts);
        let expr = Parser::parse("not exit_code == 0").unwrap();
        assert!(ctx.evaluate(&expr));
    }

    #[test]
    fn eval_type_mismatch_returns_false() {
        // fact('tests_run') is "10", comparing with integer string "not_a_number"
        let invocation = OperationInvocation::from_command("mvn package");
        let result = OperationResult { exit_code: 0, stdout: String::new(), stderr: String::new(), duration_ms: 0, timed_out: false };
        let facts = vec![fact("tests_run", "10")];
        let ctx = EvalContext::new(&invocation, &result, &facts);
        let expr = Parser::parse("fact('tests_run') == 'not_a_number'").unwrap();
        assert!(!ctx.evaluate(&expr)); // No panic, returns false
    }

    #[test]
    fn eval_complex_expression() {
        let invocation = OperationInvocation::from_command("mvn package");
        let result = OperationResult { exit_code: 0, stdout: String::new(), stderr: String::new(), duration_ms: 0, timed_out: false };
        let facts = vec![
            fact("build_status", "BUILD SUCCESS"),
            fact("tests_failed", "2"),
        ];
        let ctx = EvalContext::new(&invocation, &result, &facts);
        let expr = Parser::parse(
            "exit_code == 0 and contains_fact('build_status') and fact('tests_failed') > '0'",
        )
        .unwrap();
        assert!(ctx.evaluate(&expr));
    }
}
