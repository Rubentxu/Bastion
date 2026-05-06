//! CEL-lite expression lexer.
//!
//! Tokenizes input strings into tokens for the parser.

/// A single token from the lexer.
#[derive(Clone, Debug, PartialEq)]
pub struct Token {
    /// The kind of token.
    pub kind: TokenKind,
    /// The start position in the input string (byte index).
    pub position: usize,
    /// The literal text of the token (for identifiers and literals).
    pub lexeme: String,
}

/// Token kinds for the CEL-lite expression language.
#[derive(Clone, Debug, PartialEq)]
pub enum TokenKind {
    /// End of input.
    Eof,
    /// Integer literal.
    Int(i32),
    /// Boolean literal: `true` or `false`.
    Bool(bool),
    /// String literal (single-quoted).
    Str(String),
    /// Identifier: [a-zA-Z_]\w*.
    Ident(String),
    /// `==` equality.
    Eq,
    /// `!=` inequality.
    Ne,
    /// `>` greater-than.
    Gt,
    /// `<` less-than.
    Lt,
    /// `>=` greater-than-or-equal.
    Ge,
    /// `<=` less-than-or-equal.
    Le,
    /// `and` boolean conjunction.
    And,
    /// `or` boolean disjunction.
    Or,
    /// `not` boolean negation.
    Not,
    /// `(` left parenthesis.
    LParen,
    /// `)` right parenthesis.
    RParen,
    /// `,` comma (for function arguments).
    Comma,
    /// Unrecognized character.
    Illegal,
}

/// CEL-lite expression lexer.
#[derive(Debug)]
pub struct Lexer<'a> {
    /// Remaining input to tokenize.
    input: &'a str,
    /// Current byte position in input.
    pos: usize,
}

impl<'a> Lexer<'a> {
    /// Create a new lexer for the given input.
    pub fn new(input: &'a str) -> Self {
        Self { input, pos: 0 }
    }

    /// Return the next token from the input.
    pub fn next_token(&mut self) -> Token {
        self.skip_whitespace();
        let start = self.pos;

        if self.pos >= self.input.len() {
            return Token {
                kind: TokenKind::Eof,
                position: start,
                lexeme: String::new(),
            };
        }

        let ch = self.input[self.pos..].chars().next().unwrap();
        let kind = match ch {
            '(' => {
                self.advance();
                TokenKind::LParen
            }
            ')' => {
                self.advance();
                TokenKind::RParen
            }
            ',' => {
                self.advance();
                TokenKind::Comma
            }
            '=' if self.peek_next() == Some('=') => {
                self.advance();
                self.advance();
                TokenKind::Eq
            }
            '!' if self.peek_next() == Some('=') => {
                self.advance();
                self.advance();
                TokenKind::Ne
            }
            '>' if self.peek_next() == Some('=') => {
                self.advance();
                self.advance();
                TokenKind::Ge
            }
            '<' if self.peek_next() == Some('=') => {
                self.advance();
                self.advance();
                TokenKind::Le
            }
            '>' => {
                self.advance();
                TokenKind::Gt
            }
            '<' => {
                self.advance();
                TokenKind::Lt
            }
            '\'' => self.string_literal(),
            '0'..='9' => self.integer(),
            'a'..='z' | 'A'..='Z' | '_' => self.identifier_or_keyword(),
            _ => {
                self.advance();
                TokenKind::Illegal
            }
        };

        Token {
            kind,
            position: start,
            lexeme: self.input[start..self.pos].to_string(),
        }
    }

    /// Return all tokens from the input.
    pub fn tokenize(&mut self) -> Vec<Token> {
        let mut tokens = Vec::new();
        loop {
            let token = self.next_token();
            tokens.push(token.clone());
            if token.kind == TokenKind::Eof {
                break;
            }
        }
        tokens
    }

    fn advance(&mut self) {
        let ch = self.input[self.pos..].chars().next();
        if let Some(c) = ch {
            self.pos += c.len_utf8();
        }
    }

    fn peek_next(&self) -> Option<char> {
        self.input[self.pos..].chars().nth(1)
    }

    fn skip_whitespace(&mut self) {
        while self.pos < self.input.len() {
            let ch = self.input[self.pos..].chars().next().unwrap();
            if ch.is_whitespace() {
                self.advance();
            } else {
                break;
            }
        }
    }

    fn string_literal(&mut self) -> TokenKind {
        // Expect opening single quote
        self.advance(); // consume '
        let start = self.pos;
        let mut escaped = false;

        while self.pos < self.input.len() {
            let ch = self.input[self.pos..].chars().next().unwrap();
            if escaped {
                escaped = false;
                self.advance();
                continue;
            }
            if ch == '\\' {
                escaped = true;
                self.advance();
                continue;
            }
            if ch == '\'' {
                // End of string
                let literal = &self.input[start..self.pos];
                self.advance(); // consume closing '
                return TokenKind::Str(literal.to_string());
            }
            self.advance();
        }

        // Unterminated string — return what we have
        TokenKind::Str(self.input[start..self.pos].to_string())
    }

    fn integer(&mut self) -> TokenKind {
        let start = self.pos;
        while self.pos < self.input.len() {
            let ch = self.input[self.pos..].chars().next().unwrap();
            if ch.is_ascii_digit() {
                self.advance();
            } else {
                break;
            }
        }
        let text = &self.input[start..self.pos];
        match text.parse::<i32>() {
            Ok(n) => TokenKind::Int(n),
            Err(_) => TokenKind::Illegal,
        }
    }

    fn identifier_or_keyword(&mut self) -> TokenKind {
        let start = self.pos;
        while self.pos < self.input.len() {
            let ch = self.input[self.pos..].chars().next().unwrap();
            if ch.is_alphanumeric() || ch == '_' {
                self.advance();
            } else {
                break;
            }
        }
        let text = &self.input[start..self.pos];
        match text {
            "true" => TokenKind::Bool(true),
            "false" => TokenKind::Bool(false),
            "and" => TokenKind::And,
            "or" => TokenKind::Or,
            "not" => TokenKind::Not,
            _ => TokenKind::Ident(text.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── Valid token sequences ─────────────────────────────────────────────────

    #[test]
    fn lexer_exit_code_equals_zero() {
        let mut lex = Lexer::new("exit_code == 0");
        let tokens: Vec<_> = lex.tokenize();
        assert_eq!(tokens.len(), 4); // ident, eq, int, eof
        assert!(matches!(&tokens[0].kind, TokenKind::Ident(s) if s == "exit_code"));
        assert!(matches!(&tokens[1].kind, TokenKind::Eq));
        assert!(matches!(&tokens[2].kind, TokenKind::Int(0)));
        assert!(matches!(&tokens[3].kind, TokenKind::Eof));
    }

    #[test]
    fn lexer_contains_fact_string_literal() {
        let mut lex = Lexer::new("contains_fact('build_status')");
        let tokens: Vec<_> = lex.tokenize();
        // contains_fact ( ident ) 'build_status' ( EOF
        assert!(matches!(&tokens[0].kind, TokenKind::Ident(s) if s == "contains_fact"));
        assert!(matches!(&tokens[1].kind, TokenKind::LParen));
        assert!(matches!(&tokens[2].kind, TokenKind::Str(s) if s == "build_status"));
        assert!(matches!(&tokens[3].kind, TokenKind::RParen));
        assert!(matches!(&tokens[4].kind, TokenKind::Eof));
    }

    #[test]
    fn lexer_fact_comparison() {
        let mut lex = Lexer::new("fact('tests_failed') > '0'");
        let tokens: Vec<_> = lex.tokenize();
        assert!(matches!(&tokens[0].kind, TokenKind::Ident(s) if s == "fact"));
        assert!(matches!(&tokens[1].kind, TokenKind::LParen));
        assert!(matches!(&tokens[2].kind, TokenKind::Str(s) if s == "tests_failed"));
        assert!(matches!(&tokens[3].kind, TokenKind::RParen));
        assert!(matches!(&tokens[4].kind, TokenKind::Gt));
        assert!(matches!(&tokens[5].kind, TokenKind::Str(s) if s == "0"));
    }

    #[test]
    fn lexer_timed_out() {
        let mut lex = Lexer::new("timed_out");
        let tokens: Vec<_> = lex.tokenize();
        assert!(matches!(&tokens[0].kind, TokenKind::Ident(s) if s == "timed_out"));
        assert!(matches!(&tokens[1].kind, TokenKind::Eof));
    }

    #[test]
    fn lexer_boolean_combinators() {
        let mut lex = Lexer::new("a and b or not c");
        let tokens: Vec<_> = lex.tokenize();
        assert!(matches!(&tokens[0].kind, TokenKind::Ident(s) if s == "a"));
        assert!(matches!(&tokens[1].kind, TokenKind::And));
        assert!(matches!(&tokens[2].kind, TokenKind::Ident(s) if s == "b"));
        assert!(matches!(&tokens[3].kind, TokenKind::Or));
        assert!(matches!(&tokens[4].kind, TokenKind::Not));
        assert!(matches!(&tokens[5].kind, TokenKind::Ident(s) if s == "c"));
    }

    #[test]
    fn lexer_true_false_booleans() {
        let mut lex = Lexer::new("true and false");
        let tokens: Vec<_> = lex.tokenize();
        assert!(matches!(&tokens[0].kind, TokenKind::Bool(true)));
        assert!(matches!(&tokens[1].kind, TokenKind::And));
        assert!(matches!(&tokens[2].kind, TokenKind::Bool(false)));
    }

    #[test]
    fn lexer_all_operators() {
        let mut lex = Lexer::new("a == b != c > d < e >= f <= g");
        let tokens: Vec<_> = lex.tokenize();
        assert!(matches!(&tokens[1].kind, TokenKind::Eq));
        assert!(matches!(&tokens[3].kind, TokenKind::Ne));
        assert!(matches!(&tokens[5].kind, TokenKind::Gt));
        assert!(matches!(&tokens[7].kind, TokenKind::Lt));
        assert!(matches!(&tokens[9].kind, TokenKind::Ge));
        assert!(matches!(&tokens[11].kind, TokenKind::Le));
    }

    // ─── Invalid input ────────────────────────────────────────────────────────

    #[test]
    fn lexer_unknown_char_rejected() {
        let mut lex = Lexer::new("exit_code @ 0");
        let tokens: Vec<_> = lex.tokenize();
        // Second token should be Illegal (@)
        assert!(matches!(&tokens[1].kind, TokenKind::Illegal));
    }

    #[test]
    fn lexer_unterminated_string() {
        let mut lex = Lexer::new("'unclosed");
        let tokens: Vec<_> = lex.tokenize();
        assert!(matches!(&tokens[0].kind, TokenKind::Str(s) if s == "unclosed"));
        assert!(matches!(&tokens[1].kind, TokenKind::Eof));
    }

    #[test]
    fn lexer_position_tracked() {
        let mut lex = Lexer::new("  exit_code");
        let token = lex.next_token();
        assert!(matches!(&token.kind, TokenKind::Ident(s) if s == "exit_code"));
        assert_eq!(token.position, 2); // after two spaces
    }
}
