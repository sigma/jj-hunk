use crate::diff::Hunk;
use crate::glob::glob_match;
use crate::spec::{DefaultAction, FileSpec, HunkSpec, Spec};
use regex::Regex;
use std::collections::{HashMap, HashSet};
use thiserror::Error;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum HunksetError {
    #[error("{message}")]
    Parse {
        message: String,
        input: String,
        position: usize,
    },
    #[error("unknown function '{name}'")]
    UnknownFunction { name: String },
    #[error("invalid regex '{pattern}': {source}")]
    InvalidRegex {
        pattern: String,
        source: regex::Error,
    },
}

impl HunksetError {
    /// Format the error with a caret pointing at the position in the input.
    pub fn display_with_context(&self) -> String {
        match self {
            HunksetError::Parse { message, input, position } => {
                let caret = format!("{}^", " ".repeat(*position));
                format!("{}\n{}\n{}", input, caret, message)
            }
            other => format!("{}", other),
        }
    }
}

// ---------------------------------------------------------------------------
// AST
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    All,
    None,
    Function(String, Vec<Arg>),
    Union(Box<Expr>, Box<Expr>),
    Intersection(Box<Expr>, Box<Expr>),
    Difference(Box<Expr>, Box<Expr>),
    Negation(Box<Expr>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Arg {
    Pattern(StringPattern),
    Range(usize, usize),
}

#[derive(Debug, Clone, PartialEq)]
pub struct StringPattern {
    pub kind: PatternKind,
    pub value: String,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PatternKind {
    Exact,
    Substring,
    Glob,
    Regex,
}

/// A pattern with a pre-compiled regex (if applicable).
struct CompiledPattern {
    kind: PatternKind,
    value: String,
    compiled_regex: Option<Regex>,
}

impl CompiledPattern {
    fn compile(pattern: &StringPattern) -> Result<Self, HunksetError> {
        let compiled_regex = if pattern.kind == PatternKind::Regex {
            Some(Regex::new(&pattern.value).map_err(|e| HunksetError::InvalidRegex {
                pattern: pattern.value.clone(),
                source: e,
            })?)
        } else {
            None
        };
        Ok(Self {
            kind: pattern.kind,
            value: pattern.value.clone(),
            compiled_regex,
        })
    }

    fn matches(&self, haystack: &str) -> bool {
        match self.kind {
            PatternKind::Exact => haystack == self.value,
            PatternKind::Substring => haystack.contains(&self.value),
            PatternKind::Glob => glob_match(&self.value, haystack),
            PatternKind::Regex => {
                self.compiled_regex.as_ref().map_or(false, |re| re.is_match(haystack))
            }
        }
    }
}

fn compile_patterns(patterns: Vec<StringPattern>) -> Result<Vec<CompiledPattern>, HunksetError> {
    patterns.iter().map(CompiledPattern::compile).collect()
}

// ---------------------------------------------------------------------------
// Tokenizer
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum TokenKind {
    LParen,
    RParen,
    Pipe,
    Ampersand,
    Tilde,
    Comma,
    DotDot,
    Ident(String),
    Str(String),
    Number(usize),
    Colon,
}

#[derive(Debug, Clone)]
struct Token {
    kind: TokenKind,
    pos: usize, // byte offset in the input
}

struct Tokenizer {
    chars: Vec<char>,
    pos: usize,
}

impl Tokenizer {
    fn new(input: &str) -> Self {
        Self {
            chars: input.chars().collect(),
            pos: 0,
        }
    }

    fn peek_char(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn next_char(&mut self) -> Option<char> {
        let ch = self.chars.get(self.pos).copied();
        if ch.is_some() {
            self.pos += 1;
        }
        ch
    }

    fn skip_whitespace(&mut self) {
        while let Some(ch) = self.peek_char() {
            if ch.is_whitespace() {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn emit(&self, kind: TokenKind, pos: usize) -> Token {
        Token { kind, pos }
    }

    fn tokenize(&mut self) -> Result<Vec<Token>, String> {
        let mut tokens = Vec::new();

        loop {
            self.skip_whitespace();
            let Some(ch) = self.peek_char() else {
                break;
            };

            let start = self.pos;
            match ch {
                '(' => {
                    self.next_char();
                    tokens.push(self.emit(TokenKind::LParen, start));
                }
                ')' => {
                    self.next_char();
                    tokens.push(self.emit(TokenKind::RParen, start));
                }
                '|' => {
                    self.next_char();
                    tokens.push(self.emit(TokenKind::Pipe, start));
                }
                '&' => {
                    self.next_char();
                    tokens.push(self.emit(TokenKind::Ampersand, start));
                }
                '~' => {
                    self.next_char();
                    tokens.push(self.emit(TokenKind::Tilde, start));
                }
                ',' => {
                    self.next_char();
                    tokens.push(self.emit(TokenKind::Comma, start));
                }
                ':' => {
                    self.next_char();
                    tokens.push(self.emit(TokenKind::Colon, start));
                }
                '.' => {
                    if self.chars.get(self.pos + 1) == Some(&'.') {
                        self.pos += 2;
                        tokens.push(self.emit(TokenKind::DotDot, start));
                    } else {
                        return Err(format!("unexpected '.' at position {}", self.pos));
                    }
                }
                '"' => {
                    let tok = self.read_string(start)?;
                    tokens.push(tok);
                }
                _ if ch.is_ascii_digit() => {
                    tokens.push(self.read_number(start)?);
                }
                _ if is_ident_start(ch) => {
                    tokens.push(self.read_ident(start));
                }
                _ => {
                    return Err(format!("unexpected character '{}' at position {}", ch, self.pos));
                }
            }
        }

        Ok(tokens)
    }

    fn read_string(&mut self, start: usize) -> Result<Token, String> {
        self.next_char(); // consume opening quote
        let mut value = String::new();
        loop {
            match self.next_char() {
                Some('"') => return Ok(Token { kind: TokenKind::Str(value), pos: start }),
                Some('\\') => match self.next_char() {
                    Some('n') => value.push('\n'),
                    Some('t') => value.push('\t'),
                    Some('\\') => value.push('\\'),
                    Some('"') => value.push('"'),
                    Some(c) => {
                        value.push('\\');
                        value.push(c);
                    }
                    None => return Err("unterminated string escape".to_string()),
                },
                Some(c) => value.push(c),
                None => return Err("unterminated string literal".to_string()),
            }
        }
    }

    fn read_number(&mut self, start: usize) -> Result<Token, String> {
        let mut digits = String::new();
        while let Some(ch) = self.peek_char() {
            if ch.is_ascii_digit() {
                digits.push(ch);
                self.next_char();
            } else {
                break;
            }
        }
        let n: usize = digits.parse().map_err(|_| {
            format!("number too large at position {}", start)
        })?;
        Ok(Token { kind: TokenKind::Number(n), pos: start })
    }

    fn read_ident(&mut self, start: usize) -> Token {
        let mut name = String::new();
        while let Some(ch) = self.peek_char() {
            if is_ident_char(ch) {
                name.push(ch);
                self.next_char();
            } else {
                break;
            }
        }
        Token { kind: TokenKind::Ident(name), pos: start }
    }
}

fn is_ident_start(ch: char) -> bool {
    ch.is_ascii_alphabetic() || ch == '_'
}

fn is_ident_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_' || ch == '-'
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

struct Parser {
    input: String,
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn new(input: &str, tokens: Vec<Token>) -> Self {
        Self { input: input.to_string(), tokens, pos: 0 }
    }

    fn peek(&self) -> Option<&TokenKind> {
        self.tokens.get(self.pos).map(|t| &t.kind)
    }

    fn next_token(&mut self) -> Option<Token> {
        let tok = self.tokens.get(self.pos).cloned();
        if tok.is_some() {
            self.pos += 1;
        }
        tok
    }

    fn next_kind(&mut self) -> Option<TokenKind> {
        self.next_token().map(|t| t.kind)
    }

    fn expect(&mut self, expected: &TokenKind) -> Result<(), HunksetError> {
        match self.next_token() {
            Some(tok) if tok.kind == *expected => Ok(()),
            Some(tok) => Err(self.error_at(tok.pos, &format!("expected {:?}, got {:?}", expected, tok.kind))),
            None => Err(self.error_at(self.input.len(), &format!("expected {:?}, got end of input", expected))),
        }
    }

    fn error_at(&self, pos: usize, msg: &str) -> HunksetError {
        HunksetError::Parse {
            message: msg.to_string(),
            input: self.input.clone(),
            position: pos,
        }
    }

    fn current_pos(&self) -> usize {
        self.tokens.get(self.pos).map(|t| t.pos).unwrap_or(self.input.len())
    }

    /// hunkset = union
    fn parse(&mut self) -> Result<Expr, HunksetError> {
        let expr = self.parse_union()?;
        if self.pos < self.tokens.len() {
            let tok = &self.tokens[self.pos];
            return Err(self.error_at(tok.pos, &format!("unexpected {:?}", tok.kind)));
        }
        Ok(expr)
    }

    /// union = intersection ("|" intersection)*
    fn parse_union(&mut self) -> Result<Expr, HunksetError> {
        let mut left = self.parse_intersection()?;
        while self.peek() == Some(&TokenKind::Pipe) {
            self.next_kind();
            let right = self.parse_intersection()?;
            left = Expr::Union(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    /// intersection = difference ("&" difference)*
    fn parse_intersection(&mut self) -> Result<Expr, HunksetError> {
        let mut left = self.parse_difference()?;
        while self.peek() == Some(&TokenKind::Ampersand) {
            self.next_kind();
            let right = self.parse_difference()?;
            left = Expr::Intersection(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    /// difference = negation ("~" negation)?
    fn parse_difference(&mut self) -> Result<Expr, HunksetError> {
        let left = self.parse_negation()?;
        if self.peek() == Some(&TokenKind::Tilde) {
            self.next_kind();
            let right = self.parse_negation()?;
            Ok(Expr::Difference(Box::new(left), Box::new(right)))
        } else {
            Ok(left)
        }
    }

    /// negation = "~" atom | atom
    fn parse_negation(&mut self) -> Result<Expr, HunksetError> {
        if self.peek() == Some(&TokenKind::Tilde) {
            self.next_kind();
            let atom = self.parse_atom()?;
            Ok(Expr::Negation(Box::new(atom)))
        } else {
            self.parse_atom()
        }
    }

    /// atom = function_call | "(" hunkset ")" | "all()" | "none()"
    fn parse_atom(&mut self) -> Result<Expr, HunksetError> {
        match self.peek() {
            Some(TokenKind::LParen) => {
                self.next_kind();
                let expr = self.parse_union()?;
                self.expect(&TokenKind::RParen)?;
                Ok(expr)
            }
            Some(TokenKind::Ident(_)) => self.parse_function_call(),
            _ => Err(self.error_at(self.current_pos(), "expected function or '('")),
        }
    }

    /// function_call = IDENT "(" args? ")"
    fn parse_function_call(&mut self) -> Result<Expr, HunksetError> {
        let name = match self.next_kind() {
            Some(TokenKind::Ident(name)) => name,
            _ => return Err(self.error_at(self.current_pos(), "expected function name")),
        };

        // all and none with no parens
        if self.peek() != Some(&TokenKind::LParen) {
            return match name.as_str() {
                "all" => Ok(Expr::All),
                "none" => Ok(Expr::None),
                _ => Err(self.error_at(self.current_pos(), &format!("expected '(' after '{}'", name))),
            };
        }

        self.expect(&TokenKind::LParen)?;

        if name == "all" || name == "none" {
            self.expect(&TokenKind::RParen)?;
            return Ok(if name == "all" { Expr::All } else { Expr::None });
        }

        let mut args = Vec::new();
        if self.peek() != Some(&TokenKind::RParen) {
            args.push(self.parse_arg()?);
            while self.peek() == Some(&TokenKind::Comma) {
                self.next_kind();
                args.push(self.parse_arg()?);
            }
        }

        self.expect(&TokenKind::RParen)?;
        Ok(Expr::Function(name, args))
    }

    /// arg = pattern | number_range | number
    /// pattern = (IDENT ":")? (STRING | IDENT)
    /// number_range = NUMBER ".." NUMBER
    fn parse_arg(&mut self) -> Result<Arg, HunksetError> {
        match self.peek().cloned() {
            Some(TokenKind::Number(_)) => {
                let n = match self.next_kind() {
                    Some(TokenKind::Number(n)) => n,
                    _ => unreachable!(),
                };
                if self.peek() == Some(&TokenKind::DotDot) {
                    self.next_kind();
                    match self.next_kind() {
                        Some(TokenKind::Number(m)) => Ok(Arg::Range(n, m)),
                        _ => Err(self.error_at(self.current_pos(), "expected number after '..'")),
                    }
                } else {
                    Ok(Arg::Pattern(StringPattern {
                        kind: PatternKind::Exact,
                        value: n.to_string(),
                    }))
                }
            }
            Some(TokenKind::Str(_)) => {
                let value = match self.next_kind() {
                    Some(TokenKind::Str(s)) => s,
                    _ => unreachable!(),
                };
                Ok(Arg::Pattern(StringPattern {
                    kind: PatternKind::Substring,
                    value,
                }))
            }
            Some(TokenKind::Ident(_)) => {
                let ident = match self.next_kind() {
                    Some(TokenKind::Ident(s)) => s,
                    _ => unreachable!(),
                };
                // Check for pattern prefix: exact:, substring:, glob:, regex:
                if self.peek() == Some(&TokenKind::Colon) {
                    let kind = match ident.as_str() {
                        "exact" => PatternKind::Exact,
                        "substring" => PatternKind::Substring,
                        "glob" => PatternKind::Glob,
                        "regex" => PatternKind::Regex,
                        _ => return Err(self.error_at(self.current_pos(), &format!("unknown pattern kind '{}'", ident))),
                    };
                    self.next_kind(); // consume colon
                    let value = match self.next_kind() {
                        Some(TokenKind::Str(s)) => s,
                        Some(TokenKind::Ident(s)) => s,
                        _ => {
                            return Err(self.error_at(self.current_pos(), &format!("expected string after '{}:'", ident)))
                        }
                    };
                    Ok(Arg::Pattern(StringPattern { kind, value }))
                } else {
                    // Bare identifier — treat as exact match
                    Ok(Arg::Pattern(StringPattern {
                        kind: PatternKind::Exact,
                        value: ident,
                    }))
                }
            }
            _ => Err(self.error_at(self.current_pos(), "expected argument")),
        }
    }
}

/// Parse a hunkset expression string into an AST.
pub fn parse(input: &str) -> Result<Expr, HunksetError> {
    let mut tokenizer = Tokenizer::new(input);
    let tokens = tokenizer.tokenize().map_err(|e| {
        // Extract position from tokenizer error messages like "... at position N"
        let pos = e
            .rfind("position ")
            .and_then(|idx| e[idx + 9..].trim().parse::<usize>().ok())
            .unwrap_or(0);
        HunksetError::Parse {
            message: e,
            input: input.to_string(),
            position: pos,
        }
    })?;
    if tokens.is_empty() {
        return Err(HunksetError::Parse {
            message: "empty expression".to_string(),
            input: input.to_string(),
            position: 0,
        });
    }
    let mut parser = Parser::new(input, tokens);
    parser.parse()
}

// ---------------------------------------------------------------------------
// Evaluator
// ---------------------------------------------------------------------------

/// A hunk with its file-level context, used during evaluation.
#[derive(Debug)]
pub struct EnrichedHunk<'a> {
    pub file_path: &'a str,
    pub file_status: &'a str,
    pub hunk: &'a Hunk,
}

/// Evaluate a hunkset expression against a list of enriched hunks.
/// Returns a set of indices into the input slice that match.
pub fn evaluate(expr: &Expr, hunks: &[EnrichedHunk]) -> Result<HashSet<usize>, HunksetError> {
    match expr {
        Expr::All => Ok((0..hunks.len()).collect()),
        Expr::None => Ok(HashSet::new()),
        Expr::Negation(inner) => {
            let inner_set = evaluate(inner, hunks)?;
            Ok((0..hunks.len())
                .filter(|i| !inner_set.contains(i))
                .collect())
        }
        Expr::Union(left, right) => {
            let mut result = evaluate(left, hunks)?;
            result.extend(evaluate(right, hunks)?);
            Ok(result)
        }
        Expr::Intersection(left, right) => {
            let left_set = evaluate(left, hunks)?;
            let right_set = evaluate(right, hunks)?;
            Ok(left_set.intersection(&right_set).copied().collect())
        }
        Expr::Difference(left, right) => {
            let left_set = evaluate(left, hunks)?;
            let right_set = evaluate(right, hunks)?;
            Ok(left_set.difference(&right_set).copied().collect())
        }
        Expr::Function(name, args) => evaluate_function(name, args, hunks),
    }
}

fn evaluate_function(name: &str, args: &[Arg], hunks: &[EnrichedHunk]) -> Result<HashSet<usize>, HunksetError> {
    // Pre-compile patterns (validates regex upfront)
    let compiled = compile_patterns(extract_patterns(args))?;

    match name.as_ref() {
        "file" => Ok(eval_file(args, hunks)),
        "glob" => Ok(eval_glob(args, hunks)),
        "extension" => Ok(eval_extension(&compiled, hunks)),
        "status" => Ok(eval_status(&compiled, hunks)),
        "type" => Ok(eval_type(&compiled, hunks)),
        "lines" => Ok(eval_lines(args, hunks, LineRangeMode::Either)),
        "before_line" => Ok(eval_lines(args, hunks, LineRangeMode::Before)),
        "after_line" => Ok(eval_lines(args, hunks, LineRangeMode::After)),
        "content" => Ok(eval_content(&compiled, hunks, ContentMode::Either)),
        "added" => Ok(eval_content(&compiled, hunks, ContentMode::Added)),
        "removed" => Ok(eval_content(&compiled, hunks, ContentMode::Removed)),
        "id" => Ok(eval_id(&compiled, hunks)),
        "function" => Ok(eval_semantic(&compiled, hunks, SemanticField::Function)),
        "scope" => Ok(eval_semantic(&compiled, hunks, SemanticField::Scope)),
        "annotation" | "decorator" => Ok(eval_annotation(&compiled, hunks)),
        "doc" => Ok(eval_doc(hunks)),
        "import" => Ok(eval_import(hunks)),
        "toplevel" => Ok(eval_toplevel(hunks)),
        "depth" => Ok(eval_depth(args, hunks)),
        _ => Err(HunksetError::UnknownFunction { name: name.to_string() }),
    }
}

// --- helpers ---

fn filter_by_field<'a, F>(
    patterns: &[CompiledPattern],
    hunks: &[EnrichedHunk<'a>],
    field: F,
) -> HashSet<usize>
where
    F: Fn(&EnrichedHunk<'a>) -> &'a str,
{
    hunks
        .iter()
        .enumerate()
        .filter(|(_, h)| patterns.iter().any(|p| p.matches(field(h))))
        .map(|(i, _)| i)
        .collect()
}

// --- file predicates ---

/// file() defaults to exact matching for paths (not substring).
/// Users can explicitly use `glob:` or `substring:` prefixes.
fn eval_file(args: &[Arg], hunks: &[EnrichedHunk]) -> HashSet<usize> {
    let patterns: Vec<CompiledPattern> = extract_patterns(args)
        .into_iter()
        .map(|p| {
            let kind = if p.kind == PatternKind::Substring { PatternKind::Exact } else { p.kind };
            // unwrap is safe: if it were a regex, it was already validated in evaluate_function
            CompiledPattern::compile(&StringPattern { kind, value: p.value }).unwrap()
        })
        .collect();
    hunks
        .iter()
        .enumerate()
        .filter(|(_, h)| patterns.iter().any(|p| p.matches(h.file_path)))
        .map(|(i, _)| i)
        .collect()
}

fn eval_glob(args: &[Arg], hunks: &[EnrichedHunk]) -> HashSet<usize> {
    // For glob(), force the pattern kind to Glob regardless of how it was specified
    let patterns: Vec<CompiledPattern> = extract_patterns(args)
        .into_iter()
        .map(|p| CompiledPattern::compile(&StringPattern { kind: PatternKind::Glob, value: p.value }).unwrap())
        .collect();
    hunks
        .iter()
        .enumerate()
        .filter(|(_, h)| patterns.iter().any(|p| p.matches(h.file_path)))
        .map(|(i, _)| i)
        .collect()
}

fn eval_extension(patterns: &[CompiledPattern], hunks: &[EnrichedHunk]) -> HashSet<usize> {
    hunks
        .iter()
        .enumerate()
        .filter(|(_, h)| {
            let ext = std::path::Path::new(h.file_path)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("");
            patterns.iter().any(|p| p.matches(ext))
        })
        .map(|(i, _)| i)
        .collect()
}

fn eval_status(patterns: &[CompiledPattern], hunks: &[EnrichedHunk]) -> HashSet<usize> {
    filter_by_field(patterns, hunks, |h| h.file_status)
}

// --- hunk type ---

fn eval_type(patterns: &[CompiledPattern], hunks: &[EnrichedHunk]) -> HashSet<usize> {
    filter_by_field(patterns, hunks, |h| &h.hunk.hunk_type)
}

// --- line ranges ---

#[derive(Clone, Copy)]
enum LineRangeMode {
    Before,
    After,
    Either,
}

fn eval_lines(args: &[Arg], hunks: &[EnrichedHunk], mode: LineRangeMode) -> HashSet<usize> {
    let ranges = extract_ranges(args);
    if ranges.is_empty() {
        return HashSet::new();
    }

    hunks
        .iter()
        .enumerate()
        .filter(|(_, h)| {
            ranges.iter().any(|&(start, end)| {
                let before_touches = hunk_touches_range(
                    h.hunk.before_range.start,
                    h.hunk.before_range.length,
                    start,
                    end,
                );
                let after_touches = hunk_touches_range(
                    h.hunk.after_range.start,
                    h.hunk.after_range.length,
                    start,
                    end,
                );
                match mode {
                    LineRangeMode::Before => before_touches,
                    LineRangeMode::After => after_touches,
                    LineRangeMode::Either => before_touches || after_touches,
                }
            })
        })
        .map(|(i, _)| i)
        .collect()
}

fn hunk_touches_range(hunk_start: usize, hunk_len: usize, range_start: usize, range_end: usize) -> bool {
    if hunk_len == 0 {
        // Pure insert/delete at a point — check if the point is in range
        return hunk_start >= range_start && hunk_start <= range_end;
    }
    let hunk_end = hunk_start + hunk_len - 1;
    hunk_start <= range_end && hunk_end >= range_start
}

// --- content matching ---

#[derive(Clone, Copy)]
enum ContentMode {
    Added,
    Removed,
    Either,
}

fn eval_content(patterns: &[CompiledPattern], hunks: &[EnrichedHunk], mode: ContentMode) -> HashSet<usize> {
    hunks
        .iter()
        .enumerate()
        .filter(|(_, h)| {
            patterns.iter().any(|p| match mode {
                ContentMode::Added => p.matches(&h.hunk.added),
                ContentMode::Removed => p.matches(&h.hunk.removed),
                ContentMode::Either => p.matches(&h.hunk.added) || p.matches(&h.hunk.removed),
            })
        })
        .map(|(i, _)| i)
        .collect()
}

// --- stable ID ---

fn eval_id(patterns: &[CompiledPattern], hunks: &[EnrichedHunk]) -> HashSet<usize> {
    let ids: HashSet<String> = patterns
        .iter()
        .filter_map(|p| crate::diff::normalize_hunk_id(&p.value))
        .collect();

    hunks
        .iter()
        .enumerate()
        .filter(|(_, h)| ids.contains(&h.hunk.id))
        .map(|(i, _)| i)
        .collect()
}

// --- semantic ---

#[derive(Clone, Copy)]
enum SemanticField {
    Function,
    Scope,
}

fn eval_semantic(patterns: &[CompiledPattern], hunks: &[EnrichedHunk], field: SemanticField) -> HashSet<usize> {
    let result: HashSet<usize> = hunks
        .iter()
        .enumerate()
        .filter(|(_, h)| {
            let value = match field {
                SemanticField::Function => h.hunk.semantic.enclosing_function.as_deref(),
                SemanticField::Scope => h.hunk.semantic.enclosing_scope.as_deref(),
            };
            match value {
                Some(v) => patterns.iter().any(|p| p.matches(v)),
                None => false,
            }
        })
        .map(|(i, _)| i)
        .collect();

    result
}

// --- annotation/decorator ---

fn eval_annotation(patterns: &[CompiledPattern], hunks: &[EnrichedHunk]) -> HashSet<usize> {
    hunks
        .iter()
        .enumerate()
        .filter(|(_, h)| {
            if patterns.is_empty() {
                !h.hunk.semantic.annotations.is_empty()
            } else {
                h.hunk.semantic.annotations.iter().any(|ann| {
                    patterns.iter().any(|p| p.matches(ann))
                })
            }
        })
        .map(|(i, _)| i)
        .collect()
}

// --- doc comments ---

fn eval_doc(hunks: &[EnrichedHunk]) -> HashSet<usize> {
    hunks
        .iter()
        .enumerate()
        .filter(|(_, h)| h.hunk.semantic.is_doc_comment)
        .map(|(i, _)| i)
        .collect()
}

// --- imports ---

fn eval_import(hunks: &[EnrichedHunk]) -> HashSet<usize> {
    hunks
        .iter()
        .enumerate()
        .filter(|(_, h)| h.hunk.semantic.is_import)
        .map(|(i, _)| i)
        .collect()
}

// --- toplevel ---

fn eval_toplevel(hunks: &[EnrichedHunk]) -> HashSet<usize> {
    hunks
        .iter()
        .enumerate()
        .filter(|(_, h)| h.hunk.semantic.is_toplevel)
        .map(|(i, _)| i)
        .collect()
}

// --- depth ---

fn eval_depth(args: &[Arg], hunks: &[EnrichedHunk]) -> HashSet<usize> {
    // Collect exact values and ranges, check via min/max bounds
    let mut exact: Vec<usize> = Vec::new();
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for arg in args {
        match arg {
            Arg::Pattern(p) => {
                if let Ok(n) = p.value.parse::<usize>() {
                    exact.push(n);
                }
            }
            Arg::Range(start, end) => {
                ranges.push((*start, *end));
            }
        }
    }

    hunks
        .iter()
        .enumerate()
        .filter(|(_, h)| {
            let d = h.hunk.semantic.nesting_depth;
            exact.contains(&d) || ranges.iter().any(|&(lo, hi)| d >= lo && d <= hi)
        })
        .map(|(i, _)| i)
        .collect()
}

// --- helpers ---

fn extract_patterns(args: &[Arg]) -> Vec<StringPattern> {
    args.iter()
        .filter_map(|a| match a {
            Arg::Pattern(p) => Some(p.clone()),
            _ => None,
        })
        .collect()
}

fn extract_ranges(args: &[Arg]) -> Vec<(usize, usize)> {
    args.iter()
        .filter_map(|a| match a {
            Arg::Range(start, end) => Some((*start, *end)),
            _ => None,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Hunkset → Spec conversion
// ---------------------------------------------------------------------------

/// Convert a set of matched enriched hunks into a Spec suitable for
/// split/commit/squash operations.
pub fn to_spec(selected: &HashSet<usize>, hunks: &[EnrichedHunk]) -> Spec {
    let mut files: HashMap<String, Vec<String>> = HashMap::new();

    for &idx in selected {
        let h = &hunks[idx];
        files
            .entry(h.file_path.to_string())
            .or_default()
            .push(h.hunk.id.clone());
    }

    let spec_files: HashMap<String, FileSpec> = files
        .into_iter()
        .map(|(path, ids)| {
            let hunk_spec = HunkSpec {
                hunks: Vec::new(),
                ids,
            };
            (path, FileSpec::Selection(hunk_spec))
        })
        .collect();

    Spec {
        files: spec_files,
        default: DefaultAction::Reset,
    }
}

// ---------------------------------------------------------------------------
// Detection: is this a hunkset expression or JSON/YAML?
// ---------------------------------------------------------------------------

/// Returns true if the input looks like a hunkset expression rather than
/// JSON or YAML. Uses a trial parse: if it parses as a hunkset, it is one.
pub fn is_hunkset(input: &str) -> bool {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return false;
    }
    // Quick reject: JSON/YAML object/array literals
    let first = trimmed.as_bytes()[0];
    if first == b'{' || first == b'[' {
        return false;
    }
    // Trial parse: if it tokenizes and parses as a valid hunkset, treat it as one.
    parse(trimmed).is_ok()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- tokenizer tests --

    fn kinds(tokens: &[Token]) -> Vec<&TokenKind> {
        tokens.iter().map(|t| &t.kind).collect()
    }

    #[test]
    fn tokenize_simple_expression() {
        let mut t = Tokenizer::new("type(insert) & file(\"src/lib.rs\")");
        let tokens = t.tokenize().unwrap();
        assert_eq!(
            kinds(&tokens),
            vec![
                &TokenKind::Ident("type".into()),
                &TokenKind::LParen,
                &TokenKind::Ident("insert".into()),
                &TokenKind::RParen,
                &TokenKind::Ampersand,
                &TokenKind::Ident("file".into()),
                &TokenKind::LParen,
                &TokenKind::Str("src/lib.rs".into()),
                &TokenKind::RParen,
            ]
        );
    }

    #[test]
    fn tokenize_range() {
        let mut t = Tokenizer::new("lines(10..20)");
        let tokens = t.tokenize().unwrap();
        assert_eq!(
            kinds(&tokens),
            vec![
                &TokenKind::Ident("lines".into()),
                &TokenKind::LParen,
                &TokenKind::Number(10),
                &TokenKind::DotDot,
                &TokenKind::Number(20),
                &TokenKind::RParen,
            ]
        );
    }

    #[test]
    fn tokenize_pattern_prefix() {
        let mut t = Tokenizer::new(r#"added(regex:"fn\s+")"#);
        let tokens = t.tokenize().unwrap();
        assert_eq!(
            kinds(&tokens),
            vec![
                &TokenKind::Ident("added".into()),
                &TokenKind::LParen,
                &TokenKind::Ident("regex".into()),
                &TokenKind::Colon,
                &TokenKind::Str(r"fn\s+".into()),
                &TokenKind::RParen,
            ]
        );
    }

    // -- parser tests --

    #[test]
    fn parse_all_none() {
        assert_eq!(parse("all()").unwrap(), Expr::All);
        assert_eq!(parse("none()").unwrap(), Expr::None);
    }

    #[test]
    fn parse_simple_function() {
        let expr = parse("type(insert)").unwrap();
        assert_eq!(
            expr,
            Expr::Function(
                "type".into(),
                vec![Arg::Pattern(StringPattern {
                    kind: PatternKind::Exact,
                    value: "insert".into(),
                })]
            )
        );
    }

    #[test]
    fn parse_union() {
        let expr = parse("type(insert) | type(delete)").unwrap();
        match expr {
            Expr::Union(_, _) => {}
            _ => panic!("expected union, got {:?}", expr),
        }
    }

    #[test]
    fn parse_intersection() {
        let expr = parse("type(insert) & file(\"src/lib.rs\")").unwrap();
        match expr {
            Expr::Intersection(_, _) => {}
            _ => panic!("expected intersection, got {:?}", expr),
        }
    }

    #[test]
    fn parse_negation() {
        let expr = parse("~type(delete)").unwrap();
        match expr {
            Expr::Negation(_) => {}
            _ => panic!("expected negation, got {:?}", expr),
        }
    }

    #[test]
    fn parse_difference() {
        let expr = parse("all() ~ type(delete)").unwrap();
        match expr {
            Expr::Difference(_, _) => {}
            _ => panic!("expected difference, got {:?}", expr),
        }
    }

    #[test]
    fn parse_precedence() {
        // a | b & c should parse as a | (b & c)
        let expr = parse("type(insert) | type(replace) & file(\"x\")").unwrap();
        match expr {
            Expr::Union(_, right) => match *right {
                Expr::Intersection(_, _) => {}
                _ => panic!("expected intersection on right of union"),
            },
            _ => panic!("expected union at top level"),
        }
    }

    #[test]
    fn parse_parenthesized() {
        let expr = parse("(type(insert) | type(replace)) & file(\"x\")").unwrap();
        match expr {
            Expr::Intersection(left, _) => match *left {
                Expr::Union(_, _) => {}
                _ => panic!("expected union inside parens"),
            },
            _ => panic!("expected intersection at top level"),
        }
    }

    #[test]
    fn parse_range_arg() {
        let expr = parse("lines(10..20)").unwrap();
        assert_eq!(
            expr,
            Expr::Function("lines".into(), vec![Arg::Range(10, 20)])
        );
    }

    #[test]
    fn parse_pattern_prefix() {
        let expr = parse(r#"added(regex:"TODO")"#).unwrap();
        assert_eq!(
            expr,
            Expr::Function(
                "added".into(),
                vec![Arg::Pattern(StringPattern {
                    kind: PatternKind::Regex,
                    value: "TODO".into(),
                })]
            )
        );
    }

    #[test]
    fn parse_multiple_args() {
        let expr = parse(r#"id("hunk-aabb", "hunk-ccdd")"#).unwrap();
        match expr {
            Expr::Function(name, args) => {
                assert_eq!(name, "id");
                assert_eq!(args.len(), 2);
            }
            _ => panic!("expected function"),
        }
    }

    // -- evaluator tests --

    fn make_hunk(index: usize, hunk_type: &str, removed: &str, added: &str) -> Hunk {
        use crate::diff::LineRange;
        Hunk {
            index,
            id: format!("hunk-{:064x}", index),
            hunk_type: hunk_type.to_string(),
            removed: removed.to_string(),
            added: added.to_string(),
            before_range: LineRange {
                start: index * 10 + 1,
                length: if removed.is_empty() { 0 } else { removed.lines().count() },
            },
            after_range: LineRange {
                start: index * 10 + 1,
                length: if added.is_empty() { 0 } else { added.lines().count() },
            },
            context: None,
            semantic: crate::diff::SemanticInfo::default(),
        }
    }

    #[test]
    fn eval_type_filter() {
        let hunks_data = vec![
            make_hunk(0, "insert", "", "new line\n"),
            make_hunk(1, "delete", "old line\n", ""),
            make_hunk(2, "replace", "before\n", "after\n"),
        ];
        let enriched: Vec<EnrichedHunk> = hunks_data
            .iter()
            .map(|h| EnrichedHunk {
                file_path: "src/lib.rs",
                file_status: "modified",
                hunk: h,
            })
            .collect();

        let expr = parse("type(insert)").unwrap();
        let result = evaluate(&expr, &enriched).unwrap();
        assert_eq!(result, HashSet::from([0]));

        let expr = parse("type(insert) | type(delete)").unwrap();
        let result = evaluate(&expr, &enriched).unwrap();
        assert_eq!(result, HashSet::from([0, 1]));
    }

    #[test]
    fn eval_file_filter() {
        let h1 = make_hunk(0, "insert", "", "x\n");
        let h2 = make_hunk(1, "insert", "", "y\n");
        let enriched = vec![
            EnrichedHunk {
                file_path: "src/lib.rs",
                file_status: "modified",
                hunk: &h1,
            },
            EnrichedHunk {
                file_path: "tests/test.rs",
                file_status: "added",
                hunk: &h2,
            },
        ];

        let expr = parse(r#"file("src/lib.rs")"#).unwrap();
        let result = evaluate(&expr, &enriched).unwrap();
        assert_eq!(result, HashSet::from([0]));
    }

    #[test]
    fn eval_glob_filter() {
        let h1 = make_hunk(0, "insert", "", "x\n");
        let h2 = make_hunk(1, "insert", "", "y\n");
        let enriched = vec![
            EnrichedHunk {
                file_path: "src/lib.rs",
                file_status: "modified",
                hunk: &h1,
            },
            EnrichedHunk {
                file_path: "tests/test.rs",
                file_status: "added",
                hunk: &h2,
            },
        ];

        let expr = parse(r#"glob("src/**/*.rs")"#).unwrap();
        let result = evaluate(&expr, &enriched).unwrap();
        assert_eq!(result, HashSet::from([0]));
    }

    #[test]
    fn eval_content_filter() {
        let h1 = make_hunk(0, "insert", "", "TODO: fix this\n");
        let h2 = make_hunk(1, "replace", "old code\n", "new code\n");
        let enriched: Vec<EnrichedHunk> = vec![
            EnrichedHunk {
                file_path: "a.rs",
                file_status: "modified",
                hunk: &h1,
            },
            EnrichedHunk {
                file_path: "b.rs",
                file_status: "modified",
                hunk: &h2,
            },
        ];

        let expr = parse(r#"added("TODO")"#).unwrap();
        let result = evaluate(&expr, &enriched).unwrap();
        assert_eq!(result, HashSet::from([0]));

        let expr = parse(r#"removed("old")"#).unwrap();
        let result = evaluate(&expr, &enriched).unwrap();
        assert_eq!(result, HashSet::from([1]));
    }

    #[test]
    fn eval_intersection_and_difference() {
        let h1 = make_hunk(0, "insert", "", "new\n");
        let h2 = make_hunk(1, "insert", "", "also new\n");
        let h3 = make_hunk(2, "delete", "gone\n", "");
        let enriched = vec![
            EnrichedHunk {
                file_path: "src/a.rs",
                file_status: "modified",
                hunk: &h1,
            },
            EnrichedHunk {
                file_path: "src/b.rs",
                file_status: "modified",
                hunk: &h2,
            },
            EnrichedHunk {
                file_path: "tests/c.rs",
                file_status: "modified",
                hunk: &h3,
            },
        ];

        // insertions in src/
        let expr = parse(r#"type(insert) & glob("src/**")"#).unwrap();
        let result = evaluate(&expr, &enriched).unwrap();
        assert_eq!(result, HashSet::from([0, 1]));

        // everything except deletions
        let expr = parse("all() ~ type(delete)").unwrap();
        let result = evaluate(&expr, &enriched).unwrap();
        assert_eq!(result, HashSet::from([0, 1]));
    }

    #[test]
    fn eval_lines_filter() {
        let mut h1 = make_hunk(0, "replace", "old\n", "new\n");
        h1.before_range.start = 5;
        h1.before_range.length = 1;
        let mut h2 = make_hunk(1, "insert", "", "added\n");
        h2.before_range.start = 25;
        h2.before_range.length = 0;
        let enriched = vec![
            EnrichedHunk {
                file_path: "a.rs",
                file_status: "modified",
                hunk: &h1,
            },
            EnrichedHunk {
                file_path: "a.rs",
                file_status: "modified",
                hunk: &h2,
            },
        ];

        let expr = parse("lines(1..10)").unwrap();
        let result = evaluate(&expr, &enriched).unwrap();
        assert_eq!(result, HashSet::from([0]));

        let expr = parse("lines(20..30)").unwrap();
        let result = evaluate(&expr, &enriched).unwrap();
        assert_eq!(result, HashSet::from([1]));
    }

    #[test]
    fn eval_to_spec() {
        let h1 = make_hunk(0, "insert", "", "x\n");
        let h2 = make_hunk(1, "delete", "y\n", "");
        let enriched = vec![
            EnrichedHunk {
                file_path: "src/a.rs",
                file_status: "modified",
                hunk: &h1,
            },
            EnrichedHunk {
                file_path: "src/b.rs",
                file_status: "modified",
                hunk: &h2,
            },
        ];

        let selected = HashSet::from([0]);
        let spec = to_spec(&selected, &enriched);

        assert_eq!(spec.default, DefaultAction::Reset);
        assert!(spec.files.contains_key("src/a.rs"));
        assert!(!spec.files.contains_key("src/b.rs"));
    }

    // -- is_hunkset tests --

    #[test]
    fn detect_hunkset_vs_json() {
        assert!(is_hunkset("type(insert)"));
        assert!(is_hunkset("all()"));
        assert!(is_hunkset("~type(delete)"));
        assert!(is_hunkset("(type(insert) | type(delete))"));
        assert!(!is_hunkset(r#"{"files": {}}"#));
        assert!(!is_hunkset("[1, 2, 3]"));
    }

    // -- error reporting tests --

    #[test]
    fn parse_error_shows_caret() {
        // Parser-level error: missing closing paren
        let err = parse("type(insert").unwrap_err();
        let display = err.display_with_context();
        assert!(display.contains("type(insert"));
        assert!(display.contains("^"));
    }

    #[test]
    fn unknown_function_is_error() {
        let h = make_hunk(0, "insert", "", "x\n");
        let enriched = vec![EnrichedHunk {
            file_path: "a.rs",
            file_status: "modified",
            hunk: &h,
        }];
        let expr = parse("functon(\"foo\")").unwrap();
        let err = evaluate(&expr, &enriched).unwrap_err();
        assert!(matches!(err, HunksetError::UnknownFunction { .. }));
    }
}
