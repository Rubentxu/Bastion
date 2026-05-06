//! Rules module — CEL-lite expression lexer, AST, and evaluator.
//!
//! # Architecture
//!
//! - [`lexer`] — Tokenization of CEL-lite expressions
//! - [`ast`] — Typed AST enum and evaluation over `EvalContext`
//! - [`evaluator`] — `RuleEvaluator` trait and `DefaultRuleEvaluator`

pub mod ast;
pub mod evaluator;
pub mod lexer;

pub use ast::{Expr, ParseError};
pub use evaluator::{DefaultRuleEvaluator, RuleEvaluator};
pub use lexer::{Lexer, Token, TokenKind};
