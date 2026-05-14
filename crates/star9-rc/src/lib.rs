//! Reusable Plan 9 rc language core.
//!
//! This crate deliberately has no dependency on the Star 9 runtime. Host effects
//! are routed through [`RcHost`], so the same parser, expansion engine, and
//! evaluator can run against Star 9, tests, or another embedding.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

pub type RcResult<T> = std::result::Result<T, RcError>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RcError {
    message: String,
    span: Option<Span>,
}

impl RcError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            span: None,
        }
    }

    fn at(message: impl Into<String>, span: Span) -> Self {
        Self {
            message: message.into(),
            span: Some(span),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for RcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.span {
            Some(span) => write!(f, "{} at {}..{}", self.message, span.start, span.end),
            None => f.write_str(&self.message),
        }
    }
}

impl std::error::Error for RcError {}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RcOutput {
    pub status: RcStatus,
    pub stdout: String,
    pub stderr: String,
    pub exited: bool,
}

impl RcOutput {
    pub fn success(stdout: impl Into<String>) -> Self {
        Self {
            status: RcStatus::success(),
            stdout: stdout.into(),
            stderr: String::new(),
            exited: false,
        }
    }

    pub fn failure(status: impl Into<String>, stderr: impl Into<String>) -> Self {
        Self {
            status: RcStatus(status.into()),
            stdout: String::new(),
            stderr: stderr.into(),
            exited: false,
        }
    }

    fn append(&mut self, next: RcOutput) {
        self.status = next.status;
        self.stdout.push_str(&next.stdout);
        self.stderr.push_str(&next.stderr);
        self.exited = next.exited;
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RcStatus(String);

impl RcStatus {
    pub fn success() -> Self {
        Self(String::new())
    }

    pub fn failure() -> Self {
        Self("false".into())
    }

    pub fn from_code(code: i32) -> Self {
        if code == 0 {
            Self::success()
        } else {
            Self(code.to_string())
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_success(&self) -> bool {
        self.0.is_empty()
            || self.0 == "0"
            || (self.0.contains('|')
                && self.0.split('|').all(|part| part.is_empty() || part == "0"))
    }

    fn invert(&self) -> Self {
        if self.is_success() {
            Self::failure()
        } else {
            Self::success()
        }
    }

    pub fn from_status(status: impl Into<String>) -> Self {
        let status = status.into();
        if status == "0" {
            Self::success()
        } else {
            Self(status)
        }
    }

    pub fn pipeline(left: &Self, right: &Self) -> Self {
        let left = if left.0.is_empty() { "0" } else { &left.0 };
        let right = if right.0.is_empty() { "0" } else { &right.0 };
        Self(format!("{left}|{right}"))
    }
}

impl fmt::Display for RcStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.is_empty() {
            f.write_str("0")
        } else {
            f.write_str(&self.0)
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RcStat {
    pub is_dir: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RcCommandResult {
    pub status: RcStatus,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RcCommandInvocation {
    pub name: String,
    pub args: Vec<String>,
    pub stdin: String,
    pub env: BTreeMap<String, Vec<String>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RcProcessGraphKind {
    Pipeline,
    Background,
    ProcessSubstitutionRead,
    ProcessSubstitutionWrite,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RcFdBindingSpec {
    pub fd: u32,
    pub path: String,
    pub readable: bool,
    pub writable: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RcProcessStageSpec {
    pub command: String,
    pub cwd: String,
    pub env: BTreeMap<String, Vec<String>>,
    pub stdin: Option<String>,
    pub stdout: Option<String>,
    pub stderr: Option<String>,
    pub fd_bindings: Vec<RcFdBindingSpec>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RcProcessGraphSpec {
    pub kind: RcProcessGraphKind,
    pub job_id: Option<u32>,
    pub stages: Vec<RcProcessStageSpec>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RcProcessStageRecord {
    pub command: String,
    pub task_id: Option<String>,
    pub fd_bindings: Vec<RcFdBindingSpec>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RcProcessGraphRecord {
    pub graph_id: String,
    pub kind: RcProcessGraphKind,
    pub job_id: Option<u32>,
    pub stages: Vec<RcProcessStageRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RcProcessStageOutcome {
    pub status: RcStatus,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RcProcessJobResult {
    pub id: u32,
    pub status: RcStatus,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RcExecutableStageSpec {
    pub argv: Vec<String>,
    pub stdin: String,
    pub cwd: String,
    pub env: BTreeMap<String, Vec<String>>,
    pub fd_bindings: Vec<RcFdBindingSpec>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RcExecutableGraphSpec {
    pub kind: RcProcessGraphKind,
    pub job_id: Option<u32>,
    pub stages: Vec<RcExecutableStageSpec>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RcStartedProcessJob {
    pub id: u32,
}

pub trait RcHost {
    fn current_dir(&self) -> String;
    fn set_current_dir(&mut self, path: &str) -> RcResult<()>;
    fn read_file(&mut self, path: &str) -> RcResult<Vec<u8>>;
    fn write_file(&mut self, path: &str, data: &[u8]) -> RcResult<()>;
    fn append_file(&mut self, path: &str, data: &[u8]) -> RcResult<()>;
    fn read_dir(&mut self, path: &str) -> RcResult<Vec<String>>;
    fn stat(&mut self, path: &str) -> RcResult<RcStat>;
    fn run_command(&mut self, invocation: RcCommandInvocation) -> RcResult<RcCommandResult>;
    fn load_environment(&mut self) -> RcResult<Option<BTreeMap<String, Vec<u8>>>> {
        Ok(None)
    }
    fn store_environment(&mut self, _env: &BTreeMap<String, Vec<u8>>) -> RcResult<()> {
        Ok(())
    }
    fn prepare_process_graph(
        &mut self,
        _spec: &RcProcessGraphSpec,
    ) -> RcResult<Option<RcProcessGraphRecord>> {
        Ok(None)
    }
    fn finish_process_graph(
        &mut self,
        _record: &RcProcessGraphRecord,
        _outcomes: &[RcProcessStageOutcome],
    ) -> RcResult<()> {
        Ok(())
    }
    fn wait_process_job(
        &mut self,
        _job_id: Option<u32>,
    ) -> RcResult<Option<Vec<RcProcessJobResult>>> {
        Ok(None)
    }
    fn send_note_to_processes(&mut self, _note: &str) -> RcResult<()> {
        Ok(())
    }
    fn execute_process_graph(
        &mut self,
        _spec: RcExecutableGraphSpec,
    ) -> RcResult<Option<RcOutput>> {
        Ok(None)
    }
    fn start_process_graph_job(
        &mut self,
        _spec: RcExecutableGraphSpec,
    ) -> RcResult<Option<RcStartedProcessJob>> {
        Ok(None)
    }
    fn rfork(&mut self, flags: &str) -> RcResult<()> {
        if flags.chars().all(|flag| flag == 'e') {
            Ok(())
        } else {
            Err(RcError::new(format!("unsupported rfork flags {flags}")))
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Script {
    pub commands: Vec<Node>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Node {
    Empty,
    Simple(SimpleCommand),
    Block(Vec<Node>),
    Sequence(Vec<Node>),
    And(Box<Node>, Box<Node>),
    Or(Box<Node>, Box<Node>),
    Pipe(Box<Node>, Box<Node>, Option<PipeSpec>),
    Not(Box<Node>),
    Background(Box<Node>),
    If {
        condition: Box<Node>,
        then_branch: Box<Node>,
        else_branch: Option<Box<Node>>,
    },
    For {
        var: String,
        values: Vec<Word>,
        body: Box<Node>,
    },
    While {
        condition: Box<Node>,
        body: Box<Node>,
    },
    Switch {
        value: Word,
        cases: Vec<SwitchCase>,
    },
    Function {
        name: String,
        body: Option<Box<Node>>,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SimpleCommand {
    pub assignments: Vec<Assignment>,
    pub words: Vec<Word>,
    pub redirects: Vec<Redirect>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Assignment {
    pub name: String,
    pub values: Vec<Word>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Word {
    raw: String,
    span: Span,
}

impl Word {
    pub fn new(raw: impl Into<String>) -> Self {
        Self {
            raw: raw.into(),
            span: Span::default(),
        }
    }

    pub fn raw(&self) -> &str {
        &self.raw
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SwitchCase {
    pub patterns: Vec<Word>,
    pub body: Vec<Node>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PipeSpec {
    pub from_fd: u32,
    pub to_fd: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RedirectMode {
    Read,
    Write,
    Append,
    Here,
    Dup { from: Option<u32> },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RedirectTarget {
    Word(Word),
    Process(Box<Node>),
    HereDoc(HereDoc),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HereDoc {
    pub body: String,
    pub expand: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Redirect {
    pub fd: u32,
    pub mode: RedirectMode,
    pub target: Option<RedirectTarget>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Token {
    kind: TokenKind,
    span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum TokenKind {
    Word(String),
    Newline,
    Semi,
    AndAnd,
    OrOr,
    Amp,
    Pipe(Option<PipeSpec>),
    Bang,
    LBrace,
    RBrace,
    LParen,
    RParen,
    Redirect(RedirectOp),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RedirectOp {
    fd: u32,
    mode: RedirectMode,
}

pub fn parse(source: &str) -> RcResult<Script> {
    let prepared = prepare_here_docs(source)?;
    Parser::new(lex(&prepared.source)?, prepared.here_docs).parse_script()
}

#[derive(Clone, Debug)]
struct PreparedSource {
    source: String,
    here_docs: BTreeMap<String, HereDoc>,
}

#[derive(Clone, Debug)]
struct HereMarker {
    start: usize,
    end: usize,
    delimiter: String,
    quoted: bool,
    marker: String,
}

fn prepare_here_docs(source: &str) -> RcResult<PreparedSource> {
    let lines = source.split_inclusive('\n').collect::<Vec<_>>();
    let mut out = String::new();
    let mut here_docs = BTreeMap::new();
    let mut i = 0;
    let mut next_id = 0;
    while i < lines.len() {
        let line = lines[i];
        let markers = find_here_markers(line, next_id)?;
        if markers.is_empty() {
            out.push_str(line);
            i += 1;
            continue;
        }

        next_id += markers.len();
        i += 1;
        for marker in &markers {
            let mut body = String::new();
            let mut found = false;
            while i < lines.len() {
                let candidate = lines[i];
                let trimmed = candidate.trim_end_matches('\n').trim_end_matches('\r');
                i += 1;
                if trimmed == marker.delimiter {
                    found = true;
                    break;
                }
                body.push_str(candidate);
            }
            if !found {
                return Err(RcError::new(format!(
                    "unterminated here document {}",
                    marker.delimiter
                )));
            }
            here_docs.insert(
                marker.marker.clone(),
                HereDoc {
                    body,
                    expand: !marker.quoted,
                },
            );
        }

        let mut replaced = line.to_string();
        for marker in markers.iter().rev() {
            replaced.replace_range(marker.start..marker.end, &marker.marker);
        }
        out.push_str(&replaced);
    }
    Ok(PreparedSource {
        source: out,
        here_docs,
    })
}

fn find_here_markers(line: &str, next_id: usize) -> RcResult<Vec<HereMarker>> {
    let chars = line.char_indices().collect::<Vec<_>>();
    let mut markers = Vec::new();
    let mut quoted = false;
    let mut i = 0;
    while i < chars.len() {
        let (byte, ch) = chars[i];
        if ch == '\'' {
            quoted = !quoted;
            i += 1;
            continue;
        }
        if !quoted && ch == '<' && chars.get(i + 1).map(|(_, c)| *c) == Some('<') {
            let mut j = i + 2;
            while j < chars.len() && chars[j].1.is_whitespace() && chars[j].1 != '\n' {
                j += 1;
            }
            if j >= chars.len() {
                break;
            }
            let start = chars[j].0;
            let mut delimiter = String::new();
            let mut delim_quoted = false;
            let end;
            if chars[j].1 == '\'' {
                delim_quoted = true;
                j += 1;
                while j < chars.len() {
                    if chars[j].1 == '\'' {
                        j += 1;
                        break;
                    }
                    delimiter.push(chars[j].1);
                    j += 1;
                }
                end = chars
                    .get(j)
                    .map(|(idx, _)| *idx)
                    .unwrap_or_else(|| line.len());
            } else {
                while j < chars.len()
                    && !chars[j].1.is_whitespace()
                    && !matches!(chars[j].1, ';' | '&' | '|')
                {
                    delimiter.push(chars[j].1);
                    j += 1;
                }
                end = chars
                    .get(j)
                    .map(|(idx, _)| *idx)
                    .unwrap_or_else(|| line.len());
            }
            if delimiter.is_empty() {
                return Err(RcError::at(
                    "empty here document delimiter",
                    Span { start: byte, end },
                ));
            }
            markers.push(HereMarker {
                start,
                end,
                delimiter,
                quoted: delim_quoted,
                marker: format!("__star9_heredoc_{}__", next_id + markers.len()),
            });
            i = j;
            continue;
        }
        i += 1;
    }
    Ok(markers)
}

fn lex(source: &str) -> RcResult<Vec<Token>> {
    let chars: Vec<char> = source.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    let mut at_line_start = true;
    while i < chars.len() {
        let ch = chars[i];
        match ch {
            ' ' | '\t' | '\r' => {
                i += 1;
            }
            '\n' => {
                out.push(token(TokenKind::Newline, i, i + 1));
                i += 1;
                at_line_start = true;
            }
            '#' if at_line_start => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            ';' => {
                out.push(token(TokenKind::Semi, i, i + 1));
                i += 1;
                at_line_start = false;
            }
            '{' => {
                out.push(token(TokenKind::LBrace, i, i + 1));
                i += 1;
                at_line_start = false;
            }
            '}' => {
                out.push(token(TokenKind::RBrace, i, i + 1));
                i += 1;
                at_line_start = false;
            }
            '(' => {
                out.push(token(TokenKind::LParen, i, i + 1));
                i += 1;
                at_line_start = false;
            }
            ')' => {
                out.push(token(TokenKind::RParen, i, i + 1));
                i += 1;
                at_line_start = false;
            }
            '&' => {
                if chars.get(i + 1) == Some(&'&') {
                    out.push(token(TokenKind::AndAnd, i, i + 2));
                    i += 2;
                } else {
                    out.push(token(TokenKind::Amp, i, i + 1));
                    i += 1;
                }
                at_line_start = false;
            }
            '|' => {
                if chars.get(i + 1) == Some(&'|') {
                    out.push(token(TokenKind::OrOr, i, i + 2));
                    i += 2;
                } else {
                    let (spec, next) = parse_pipe_spec(&chars, i + 1)?;
                    out.push(token(TokenKind::Pipe(spec), i, next));
                    i = next;
                }
                at_line_start = false;
            }
            '!' => {
                out.push(token(TokenKind::Bang, i, i + 1));
                i += 1;
                at_line_start = false;
            }
            '<' | '>' => {
                let (op, next) = parse_redirect(&chars, i)?;
                out.push(token(TokenKind::Redirect(op), i, next));
                i = next;
                at_line_start = false;
            }
            _ => {
                let (word, next) = read_word(&chars, i)?;
                out.push(token(TokenKind::Word(word), i, next));
                i = next;
                at_line_start = false;
            }
        }
    }
    Ok(out)
}

fn token(kind: TokenKind, start: usize, end: usize) -> Token {
    Token {
        kind,
        span: Span { start, end },
    }
}

fn read_word(chars: &[char], mut i: usize) -> RcResult<(String, usize)> {
    let start = i;
    let mut out = String::new();
    while i < chars.len() {
        let ch = chars[i];
        if ch == '(' && out.contains('$') {
            out.push(ch);
            i += 1;
            while i < chars.len() {
                out.push(chars[i]);
                if chars[i] == ')' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            continue;
        }
        if ch.is_whitespace() || matches!(ch, ';' | '{' | '}' | '(' | ')' | '&' | '|' | '<' | '>') {
            break;
        }
        if ch == '\'' {
            out.push(ch);
            i += 1;
            while i < chars.len() {
                out.push(chars[i]);
                if chars[i] == '\'' {
                    if chars.get(i + 1) == Some(&'\'') {
                        out.push('\'');
                        i += 2;
                        continue;
                    }
                    i += 1;
                    break;
                }
                i += 1;
            }
            continue;
        }
        if ch == '`' && chars.get(i + 1) == Some(&'{') {
            let (sub, next) = read_braced_substitution(chars, i)?;
            out.push_str(&sub);
            i = next;
            continue;
        }
        out.push(ch);
        i += 1;
    }
    if out.is_empty() {
        Err(RcError::at("expected word", Span { start, end: i }))
    } else {
        Ok((out, i))
    }
}

fn read_braced_substitution(chars: &[char], start: usize) -> RcResult<(String, usize)> {
    let mut i = start;
    let mut depth = 0_i32;
    let mut out = String::new();
    while i < chars.len() {
        let ch = chars[i];
        out.push(ch);
        if ch == '\'' {
            i += 1;
            while i < chars.len() {
                out.push(chars[i]);
                if chars[i] == '\'' {
                    if chars.get(i + 1) == Some(&'\'') {
                        out.push('\'');
                        i += 2;
                        continue;
                    }
                    i += 1;
                    break;
                }
                i += 1;
            }
            continue;
        }
        if ch == '{' {
            depth += 1;
        } else if ch == '}' {
            depth -= 1;
            if depth == 0 {
                return Ok((out, i + 1));
            }
        }
        i += 1;
    }
    Err(RcError::at(
        "unterminated command substitution",
        Span {
            start,
            end: chars.len(),
        },
    ))
}

fn parse_pipe_spec(chars: &[char], start: usize) -> RcResult<(Option<PipeSpec>, usize)> {
    if chars.get(start) != Some(&'[') {
        return Ok((None, start));
    }
    let end = find_closing_bracket(chars, start)?;
    let text: String = chars[start + 1..end].iter().collect();
    let spec = if let Some((left, right)) = text.split_once('=') {
        PipeSpec {
            from_fd: left.parse().unwrap_or(1),
            to_fd: right.parse().unwrap_or(0),
        }
    } else {
        PipeSpec {
            from_fd: text.parse().unwrap_or(1),
            to_fd: 0,
        }
    };
    Ok((Some(spec), end + 1))
}

fn parse_redirect(chars: &[char], start: usize) -> RcResult<(RedirectOp, usize)> {
    let op = chars[start];
    let mut i = start + 1;
    let mut mode = if op == '<' {
        RedirectMode::Read
    } else {
        RedirectMode::Write
    };
    if op == '>' && chars.get(i) == Some(&'>') {
        mode = RedirectMode::Append;
        i += 1;
    } else if op == '<' && chars.get(i) == Some(&'<') {
        mode = RedirectMode::Here;
        i += 1;
    }
    let mut fd = if op == '<' { 0 } else { 1 };
    if chars.get(i) == Some(&'[') {
        let end = find_closing_bracket(chars, i)?;
        let text: String = chars[i + 1..end].iter().collect();
        if let Some((left, right)) = text.split_once('=') {
            fd = left.parse().unwrap_or(fd);
            let from = if right.is_empty() {
                None
            } else {
                Some(right.parse().unwrap_or(fd))
            };
            mode = RedirectMode::Dup { from };
        } else if !text.is_empty() {
            fd = text.parse().unwrap_or(fd);
        }
        i = end + 1;
    }
    Ok((RedirectOp { fd, mode }, i))
}

fn find_closing_bracket(chars: &[char], start: usize) -> RcResult<usize> {
    let mut i = start + 1;
    while i < chars.len() {
        if chars[i] == ']' {
            return Ok(i);
        }
        i += 1;
    }
    Err(RcError::at(
        "unterminated fd specifier",
        Span {
            start,
            end: chars.len(),
        },
    ))
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    here_docs: BTreeMap<String, HereDoc>,
}

impl Parser {
    fn new(tokens: Vec<Token>, here_docs: BTreeMap<String, HereDoc>) -> Self {
        Self {
            tokens,
            pos: 0,
            here_docs,
        }
    }

    fn parse_script(&mut self) -> RcResult<Script> {
        let commands = self
            .parse_sequence_until(|kind| matches!(kind, TokenKind::RBrace | TokenKind::RParen))?;
        Ok(Script { commands })
    }

    fn parse_sequence_until(
        &mut self,
        stop: impl Fn(&TokenKind) -> bool + Copy,
    ) -> RcResult<Vec<Node>> {
        let mut nodes = Vec::new();
        self.skip_separators();
        while let Some(token) = self.peek() {
            if stop(&token.kind) {
                break;
            }
            let before = self.pos;
            let span = token.span;
            let kind = token.kind.clone();
            nodes.push(self.parse_and_or()?);
            if self.pos == before {
                return Err(RcError::at(
                    format!("parser made no progress at token {kind:?}"),
                    Span {
                        start: span.start,
                        end: span.end,
                    },
                ));
            }
            if matches!(self.peek_kind(), Some(TokenKind::Amp)) {
                self.pos += 1;
                let node = nodes.pop().unwrap_or(Node::Empty);
                nodes.push(Node::Background(Box::new(node)));
            }
            if !self.consume_separator() {
                if self.peek().is_some_and(|token| !stop(&token.kind)) {
                    continue;
                }
                break;
            }
            self.skip_separators();
        }
        Ok(nodes)
    }

    fn parse_and_or(&mut self) -> RcResult<Node> {
        let mut left = self.parse_pipeline()?;
        loop {
            match self.peek_kind() {
                Some(TokenKind::AndAnd) => {
                    self.pos += 1;
                    let right = self.parse_pipeline()?;
                    left = Node::And(Box::new(left), Box::new(right));
                }
                Some(TokenKind::OrOr) => {
                    self.pos += 1;
                    let right = self.parse_pipeline()?;
                    left = Node::Or(Box::new(left), Box::new(right));
                }
                _ => break,
            }
        }
        Ok(left)
    }

    fn parse_pipeline(&mut self) -> RcResult<Node> {
        let mut left = self.parse_unary()?;
        while let Some(TokenKind::Pipe(spec)) = self.peek_kind().cloned() {
            self.pos += 1;
            let right = self.parse_unary()?;
            left = Node::Pipe(Box::new(left), Box::new(right), spec);
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> RcResult<Node> {
        if matches!(self.peek_kind(), Some(TokenKind::Bang)) {
            self.pos += 1;
            Ok(Node::Not(Box::new(self.parse_unary()?)))
        } else {
            self.parse_command()
        }
    }

    fn parse_command(&mut self) -> RcResult<Node> {
        match self.peek_kind() {
            Some(TokenKind::LBrace) => self.parse_block(),
            Some(TokenKind::LParen) => self.parse_paren_group(),
            Some(TokenKind::Word(word)) if word == "if" => self.parse_if(),
            Some(TokenKind::Word(word)) if word == "while" => self.parse_while(),
            Some(TokenKind::Word(word)) if word == "for" => self.parse_for(),
            Some(TokenKind::Word(word)) if word == "switch" => self.parse_switch(),
            Some(TokenKind::Word(word)) if word == "fn" => self.parse_function(),
            _ => self.parse_simple(),
        }
    }

    fn parse_block(&mut self) -> RcResult<Node> {
        self.expect_lbrace()?;
        let body = self.parse_sequence_until(|kind| matches!(kind, TokenKind::RBrace))?;
        self.expect_rbrace()?;
        Ok(Node::Block(body))
    }

    fn parse_paren_group(&mut self) -> RcResult<Node> {
        self.expect_lparen()?;
        let body = self.parse_sequence_until(|kind| matches!(kind, TokenKind::RParen))?;
        self.expect_rparen()?;
        Ok(Node::Block(body))
    }

    fn parse_if(&mut self) -> RcResult<Node> {
        self.expect_word("if")?;
        self.expect_lparen()?;
        let condition =
            Node::Sequence(self.parse_sequence_until(|kind| matches!(kind, TokenKind::RParen))?);
        self.expect_rparen()?;
        self.skip_separators();
        let then_branch = self.parse_command()?;
        let before_else = self.pos;
        self.skip_separators();
        let else_branch = if self.match_word("if") && self.match_word("not") {
            self.skip_separators();
            Some(Box::new(self.parse_command()?))
        } else {
            self.pos = before_else;
            None
        };
        Ok(Node::If {
            condition: Box::new(condition),
            then_branch: Box::new(then_branch),
            else_branch,
        })
    }

    fn parse_while(&mut self) -> RcResult<Node> {
        self.expect_word("while")?;
        self.expect_lparen()?;
        let condition =
            Node::Sequence(self.parse_sequence_until(|kind| matches!(kind, TokenKind::RParen))?);
        self.expect_rparen()?;
        self.skip_separators();
        Ok(Node::While {
            condition: Box::new(condition),
            body: Box::new(self.parse_command()?),
        })
    }

    fn parse_for(&mut self) -> RcResult<Node> {
        self.expect_word("for")?;
        self.expect_lparen()?;
        let var = self.expect_any_word()?;
        let mut values = Vec::new();
        if self.match_word("in") {
            while !matches!(self.peek_kind(), Some(TokenKind::RParen)) {
                values.push(Word::new(self.expect_any_word()?));
            }
        }
        self.expect_rparen()?;
        self.skip_separators();
        Ok(Node::For {
            var,
            values,
            body: Box::new(self.parse_command()?),
        })
    }

    fn parse_switch(&mut self) -> RcResult<Node> {
        self.expect_word("switch")?;
        self.expect_lparen()?;
        let value = Word::new(self.expect_any_word()?);
        self.expect_rparen()?;
        self.skip_separators();
        self.expect_lbrace()?;
        let mut cases = Vec::new();
        self.skip_separators();
        while !matches!(self.peek_kind(), Some(TokenKind::RBrace) | None) {
            self.expect_word("case")?;
            let mut patterns = Vec::new();
            while !matches!(
                self.peek_kind(),
                Some(TokenKind::Semi | TokenKind::Newline | TokenKind::RBrace) | None
            ) {
                patterns.push(Word::new(self.expect_any_word()?));
            }
            self.consume_separator();
            let body = self.parse_sequence_until(|kind| {
                matches!(kind, TokenKind::RBrace)
                    || matches!(kind, TokenKind::Word(word) if word == "case")
            })?;
            cases.push(SwitchCase { patterns, body });
            self.skip_separators();
        }
        self.expect_rbrace()?;
        Ok(Node::Switch { value, cases })
    }

    fn parse_function(&mut self) -> RcResult<Node> {
        self.expect_word("fn")?;
        let name = self.expect_any_word()?;
        let body = if matches!(self.peek_kind(), Some(TokenKind::LBrace)) {
            Some(Box::new(self.parse_block()?))
        } else {
            None
        };
        Ok(Node::Function { name, body })
    }

    fn parse_simple(&mut self) -> RcResult<Node> {
        let mut simple = SimpleCommand::default();
        let mut seen_word = false;
        while let Some(kind) = self.peek_kind().cloned() {
            match kind {
                TokenKind::Word(raw) => {
                    self.pos += 1;
                    if !seen_word {
                        if let Some((name, suffix)) = parse_assignment_head(&raw) {
                            let values = if suffix.is_empty()
                                && matches!(self.peek_kind(), Some(TokenKind::LParen))
                            {
                                self.pos += 1;
                                let mut values = Vec::new();
                                while !matches!(self.peek_kind(), Some(TokenKind::RParen) | None) {
                                    values.push(Word::new(self.expect_any_word()?));
                                }
                                self.expect_rparen()?;
                                values
                            } else if suffix.is_empty() {
                                Vec::new()
                            } else {
                                vec![Word::new(suffix)]
                            };
                            simple.assignments.push(Assignment { name, values });
                            continue;
                        }
                    }
                    seen_word = true;
                    simple.words.push(Word::new(raw));
                }
                TokenKind::Redirect(op) => {
                    self.pos += 1;
                    let target = if matches!(op.mode, RedirectMode::Dup { .. }) {
                        None
                    } else if matches!(
                        op.mode,
                        RedirectMode::Read | RedirectMode::Write | RedirectMode::Append
                    ) && matches!(self.peek_kind(), Some(TokenKind::LBrace))
                    {
                        Some(RedirectTarget::Process(Box::new(self.parse_block()?)))
                    } else {
                        let raw = self.expect_any_word()?;
                        if matches!(op.mode, RedirectMode::Here) {
                            self.here_docs
                                .remove(&raw)
                                .map(RedirectTarget::HereDoc)
                                .or_else(|| Some(RedirectTarget::Word(Word::new(raw))))
                        } else {
                            Some(RedirectTarget::Word(Word::new(raw)))
                        }
                    };
                    simple.redirects.push(Redirect {
                        fd: op.fd,
                        mode: op.mode,
                        target,
                    });
                }
                _ if is_command_terminator(&kind) => break,
                _ => break,
            }
        }
        Ok(
            if simple.assignments.is_empty()
                && simple.words.is_empty()
                && simple.redirects.is_empty()
            {
                Node::Empty
            } else {
                Node::Simple(simple)
            },
        )
    }

    fn skip_separators(&mut self) {
        while matches!(self.peek_kind(), Some(TokenKind::Semi | TokenKind::Newline)) {
            self.pos += 1;
        }
    }

    fn consume_separator(&mut self) -> bool {
        if matches!(self.peek_kind(), Some(TokenKind::Semi | TokenKind::Newline)) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect_lbrace(&mut self) -> RcResult<()> {
        self.expect_symbol(|kind| matches!(kind, TokenKind::LBrace), "{")
    }

    fn expect_rbrace(&mut self) -> RcResult<()> {
        self.expect_symbol(|kind| matches!(kind, TokenKind::RBrace), "}")
    }

    fn expect_lparen(&mut self) -> RcResult<()> {
        self.expect_symbol(|kind| matches!(kind, TokenKind::LParen), "(")
    }

    fn expect_rparen(&mut self) -> RcResult<()> {
        self.expect_symbol(|kind| matches!(kind, TokenKind::RParen), ")")
    }

    fn expect_symbol(
        &mut self,
        predicate: impl Fn(&TokenKind) -> bool,
        label: &str,
    ) -> RcResult<()> {
        match self.peek() {
            Some(token) if predicate(&token.kind) => {
                self.pos += 1;
                Ok(())
            }
            Some(token) => Err(RcError::at(format!("expected {label}"), token.span)),
            None => Err(RcError::new(format!("expected {label}"))),
        }
    }

    fn expect_word(&mut self, expected: &str) -> RcResult<()> {
        match self.peek_kind() {
            Some(TokenKind::Word(word)) if word == expected => {
                self.pos += 1;
                Ok(())
            }
            _ => Err(RcError::new(format!("expected {expected}"))),
        }
    }

    fn expect_any_word(&mut self) -> RcResult<String> {
        match self.peek_kind().cloned() {
            Some(TokenKind::Word(word)) => {
                self.pos += 1;
                Ok(word)
            }
            Some(_) => Err(RcError::new("expected word")),
            None => Err(RcError::new("expected word")),
        }
    }

    fn match_word(&mut self, expected: &str) -> bool {
        if matches!(self.peek_kind(), Some(TokenKind::Word(word)) if word == expected) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn peek_kind(&self) -> Option<&TokenKind> {
        self.peek().map(|token| &token.kind)
    }
}

fn is_command_terminator(kind: &TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Semi
            | TokenKind::Newline
            | TokenKind::AndAnd
            | TokenKind::OrOr
            | TokenKind::Amp
            | TokenKind::Pipe(_)
            | TokenKind::RBrace
            | TokenKind::RParen
    )
}

fn parse_assignment_head(raw: &str) -> Option<(String, String)> {
    let (name, suffix) = raw.split_once('=')?;
    if is_name(name) {
        Some((name.to_string(), suffix.to_string()))
    } else {
        None
    }
}

fn is_name(value: &str) -> bool {
    let mut chars = value.chars();
    matches!(chars.next(), Some(ch) if ch.is_ascii_alphabetic() || ch == '_' || ch == '*')
        && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

#[derive(Clone)]
pub struct RcSession<H: RcHost> {
    host: H,
    vars: BTreeMap<String, Vec<String>>,
    functions: BTreeMap<String, Node>,
    last_status: RcStatus,
    argv0: String,
    argv: Vec<String>,
    jobs: Vec<RcJob>,
    next_job_id: u32,
    notes_in_progress: BTreeSet<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RcJob {
    id: u32,
    status: RcStatus,
    stdout: String,
    stderr: String,
    graph: Option<RcProcessGraphRecord>,
}

impl<H: RcHost> RcSession<H> {
    pub fn new(host: H) -> Self {
        let mut vars = BTreeMap::new();
        vars.insert("path".into(), vec![".".into(), "bin".into()]);
        vars.insert("ifs".into(), vec![" \t\n".into()]);
        let mut session = Self {
            host,
            vars,
            functions: BTreeMap::new(),
            last_status: RcStatus::success(),
            argv0: "rc".into(),
            argv: Vec::new(),
            jobs: Vec::new(),
            next_job_id: 1,
            notes_in_progress: BTreeSet::new(),
        };
        session.import_host_environment().ok();
        session.publish_environment().ok();
        session
    }

    pub fn host(&self) -> &H {
        &self.host
    }

    pub fn host_mut(&mut self) -> &mut H {
        &mut self.host
    }

    pub fn set_args(&mut self, args: Vec<String>) {
        self.argv = args;
        self.refresh_arg_vars();
        self.publish_environment().ok();
    }

    pub fn set_argv0(&mut self, argv0: impl Into<String>) {
        self.argv0 = argv0.into();
    }

    pub fn set_var(&mut self, name: impl Into<String>, values: Vec<String>) {
        self.vars.insert(name.into(), values);
        self.publish_environment().ok();
    }

    pub fn get_var(&self, name: &str) -> Vec<String> {
        self.lookup_var(name)
    }

    pub fn last_status(&self) -> &RcStatus {
        &self.last_status
    }

    pub fn export_environment(&self) -> BTreeMap<String, Vec<u8>> {
        let mut env = BTreeMap::new();
        for (name, values) in &self.vars {
            env.insert(name.clone(), values.join("\0").into_bytes());
        }
        for (name, body) in &self.functions {
            env.insert(format!("fn#{name}"), render_node(body).into_bytes());
        }
        env
    }

    pub fn import_environment(&mut self, env: BTreeMap<String, Vec<u8>>) {
        for (name, data) in env {
            if let Some(function) = name.strip_prefix("fn#") {
                if let Ok(source) = String::from_utf8(data) {
                    if let Ok(script) = parse(&format!("fn {function} {source}")) {
                        if let Some(Node::Function {
                            body: Some(body), ..
                        }) = script.commands.into_iter().next()
                        {
                            self.functions.insert(function.to_string(), *body);
                        }
                    }
                }
                continue;
            }
            let values = data
                .split(|byte| *byte == 0)
                .map(|part| String::from_utf8_lossy(part).into_owned())
                .collect::<Vec<_>>();
            self.vars.insert(name, values);
        }
        self.publish_environment().ok();
    }

    pub fn deliver_note(&mut self, note: &str) -> RcOutput {
        self.host.send_note_to_processes(note).ok();
        let name = format!("sig{note}");
        self.run_note_function(&name)
            .unwrap_or_else(|| RcOutput::failure(note, format!("rc: unhandled note {note}\n")))
    }

    pub fn prompt(&self) -> String {
        format!("rc:{}% ", self.host.current_dir())
    }

    pub fn eval_source(&mut self, source: &str) -> RcOutput {
        self.import_host_environment().ok();
        match parse(source) {
            Ok(script) => self.eval_script(&script, String::new()),
            Err(err) => {
                self.last_status = RcStatus::failure();
                RcOutput::failure("parse", format!("rc: {err}\n"))
            }
        }
    }

    pub fn eval_script(&mut self, script: &Script, input: String) -> RcOutput {
        let output = self.eval_nodes(&script.commands, input);
        self.last_status = output.status.clone();
        self.vars
            .insert("status".into(), vec![self.last_status.to_string()]);
        self.publish_environment().ok();
        output
    }

    fn import_host_environment(&mut self) -> RcResult<()> {
        if let Some(env) = self.host.load_environment()? {
            self.import_environment_without_publish(env);
        }
        Ok(())
    }

    fn publish_environment(&mut self) -> RcResult<()> {
        self.host.store_environment(&self.export_environment())
    }

    fn import_environment_without_publish(&mut self, env: BTreeMap<String, Vec<u8>>) {
        for (name, data) in env {
            if let Some(function) = name.strip_prefix("fn#") {
                if let Ok(source) = String::from_utf8(data) {
                    if let Ok(script) = parse(&format!("fn {function} {source}")) {
                        if let Some(Node::Function {
                            body: Some(body), ..
                        }) = script.commands.into_iter().next()
                        {
                            self.functions.insert(function.to_string(), *body);
                        }
                    }
                }
                continue;
            }
            let values = data
                .split(|byte| *byte == 0)
                .map(|part| String::from_utf8_lossy(part).into_owned())
                .collect::<Vec<_>>();
            self.vars.insert(name, values);
        }
        self.refresh_arg_vars();
    }

    fn eval_nodes(&mut self, nodes: &[Node], input: String) -> RcOutput {
        let mut out = RcOutput::default();
        let mut current_input = input;
        for node in nodes {
            let next = self.eval_node(node, std::mem::take(&mut current_input));
            let exited = next.exited;
            out.append(next);
            if exited {
                break;
            }
        }
        out
    }

    fn prepare_pipeline_graph(
        &mut self,
        left: &Node,
        right: &Node,
        from_fd: u32,
        to_fd: u32,
    ) -> Option<RcProcessGraphRecord> {
        let mut left_stage = self.stage_spec(left);
        left_stage.fd_bindings.push(RcFdBindingSpec {
            fd: from_fd,
            path: "pipe:0/data".into(),
            readable: false,
            writable: true,
        });
        let mut right_stage = self.stage_spec(right);
        right_stage.fd_bindings.push(RcFdBindingSpec {
            fd: to_fd,
            path: "pipe:0/data1".into(),
            readable: true,
            writable: false,
        });
        self.host
            .prepare_process_graph(&RcProcessGraphSpec {
                kind: RcProcessGraphKind::Pipeline,
                job_id: None,
                stages: vec![left_stage, right_stage],
            })
            .ok()
            .flatten()
    }

    fn prepare_background_graph(&mut self, id: u32, node: &Node) -> Option<RcProcessGraphRecord> {
        let mut stage = self.stage_spec(node);
        stage.stdout = Some(format!("job:{id}/stdout"));
        stage.stderr = Some(format!("job:{id}/stderr"));
        self.host
            .prepare_process_graph(&RcProcessGraphSpec {
                kind: RcProcessGraphKind::Background,
                job_id: Some(id),
                stages: vec![stage],
            })
            .ok()
            .flatten()
    }

    fn prepare_process_substitution_graph(
        &mut self,
        kind: RcProcessGraphKind,
        node: &Node,
        fd: u32,
    ) -> Option<RcProcessGraphRecord> {
        let mut stage = self.stage_spec(node);
        let readable = matches!(&kind, RcProcessGraphKind::ProcessSubstitutionRead);
        stage.fd_bindings.push(RcFdBindingSpec {
            fd,
            path: if readable {
                "pipe:0/data".into()
            } else {
                "pipe:0/data1".into()
            },
            readable,
            writable: !readable,
        });
        self.host
            .prepare_process_graph(&RcProcessGraphSpec {
                kind,
                job_id: None,
                stages: vec![stage],
            })
            .ok()
            .flatten()
    }

    fn stage_spec(&self, node: &Node) -> RcProcessStageSpec {
        RcProcessStageSpec {
            command: render_node(node),
            cwd: self.host.current_dir(),
            env: self.vars.clone(),
            stdin: None,
            stdout: None,
            stderr: None,
            fd_bindings: Vec::new(),
        }
    }

    fn finish_process_graph(
        &mut self,
        record: Option<&RcProcessGraphRecord>,
        statuses: &[RcStatus],
    ) {
        let Some(record) = record else {
            return;
        };
        let outcomes = statuses
            .iter()
            .cloned()
            .map(|status| RcProcessStageOutcome { status })
            .collect::<Vec<_>>();
        self.host.finish_process_graph(record, &outcomes).ok();
    }

    fn try_execute_pipeline_graph(
        &mut self,
        left: &Node,
        right: &Node,
        from_fd: u32,
        to_fd: u32,
        input: &str,
    ) -> Option<RcOutput> {
        let mut nodes = Vec::new();
        let mut edges = Vec::new();
        Self::append_pipeline_nodes(left, &mut nodes, &mut edges)?;
        edges.push((from_fd, to_fd));
        Self::append_pipeline_nodes(right, &mut nodes, &mut edges)?;
        if nodes.len() < 2 || edges.len() + 1 != nodes.len() {
            return None;
        }

        let mut bindings = vec![Vec::new(); nodes.len()];
        for (index, (from_fd, to_fd)) in edges.into_iter().enumerate() {
            bindings[index].push(RcFdBindingSpec {
                fd: from_fd,
                path: format!("pipe:{index}/data"),
                readable: false,
                writable: true,
            });
            bindings[index + 1].push(RcFdBindingSpec {
                fd: to_fd,
                path: format!("pipe:{index}/data1"),
                readable: true,
                writable: false,
            });
        }

        let mut stages = Vec::new();
        for (index, node) in nodes.into_iter().enumerate() {
            let stdin = if index == 0 {
                input.to_string()
            } else {
                String::new()
            };
            stages.push(self.executable_stage(node, stdin, bindings[index].clone())?);
        }
        self.host
            .execute_process_graph(RcExecutableGraphSpec {
                kind: RcProcessGraphKind::Pipeline,
                job_id: None,
                stages,
            })
            .ok()
            .flatten()
    }

    fn append_pipeline_nodes<'a>(
        node: &'a Node,
        nodes: &mut Vec<&'a Node>,
        edges: &mut Vec<(u32, u32)>,
    ) -> Option<()> {
        match node {
            Node::Simple(_) => {
                nodes.push(node);
                Some(())
            }
            Node::Pipe(left, right, spec) => {
                Self::append_pipeline_nodes(left, nodes, edges)?;
                edges.push((
                    spec.as_ref().map(|spec| spec.from_fd).unwrap_or(1),
                    spec.as_ref().map(|spec| spec.to_fd).unwrap_or(0),
                ));
                Self::append_pipeline_nodes(right, nodes, edges)
            }
            _ => None,
        }
    }

    fn try_start_background_graph(&mut self, id: u32, node: &Node, input: &str) -> bool {
        let Some(stage) = self.executable_stage(node, input.to_string(), Vec::new()) else {
            return false;
        };
        self.host
            .start_process_graph_job(RcExecutableGraphSpec {
                kind: RcProcessGraphKind::Background,
                job_id: Some(id),
                stages: vec![stage],
            })
            .ok()
            .flatten()
            .is_some()
    }

    fn executable_stage(
        &mut self,
        node: &Node,
        stdin: String,
        fd_bindings: Vec<RcFdBindingSpec>,
    ) -> Option<RcExecutableStageSpec> {
        let Node::Simple(simple) = node else {
            return None;
        };
        if !simple.redirects.is_empty() {
            return None;
        }

        let mut env = self.vars.clone();
        for assignment in &simple.assignments {
            env.insert(
                assignment.name.clone(),
                if assignment.values.is_empty() {
                    Vec::new()
                } else {
                    self.expand_words(&assignment.values)
                },
            );
        }

        let words = self.expand_words(&simple.words);
        if words.is_empty()
            || self.functions.contains_key(&words[0])
            || BUILTINS.contains(&words[0].as_str())
        {
            return None;
        }
        let argv = self.resolve_external_argv(&words)?;
        Some(RcExecutableStageSpec {
            argv,
            stdin,
            cwd: self.host.current_dir(),
            env,
            fd_bindings,
        })
    }

    fn resolve_external_argv(&mut self, words: &[String]) -> Option<Vec<String>> {
        let command = words.first()?;
        if matches!(command.as_str(), "wasi" | "worker" | "native") && words.len() > 1 {
            return Some(words.to_vec());
        }
        if is_host_executable_path(command) {
            return Some(words.to_vec());
        }
        if command.contains('/') {
            return self.host_executable_argv(command, &words[1..]);
        }
        for dir in self.lookup_var("path") {
            let candidate = join_path(&dir, command);
            if let Some(argv) = self.host_executable_argv(&candidate, &words[1..]) {
                return Some(argv);
            }
        }
        None
    }

    fn host_executable_argv(&mut self, path: &str, args: &[String]) -> Option<Vec<String>> {
        let bytes = self.host.read_file(path).ok()?;
        if !is_host_executable(path, &bytes) {
            return None;
        }
        let mut argv = vec![path.to_string()];
        argv.extend(args.iter().cloned());
        Some(argv)
    }

    fn eval_node(&mut self, node: &Node, input: String) -> RcOutput {
        match node {
            Node::Empty => RcOutput::default(),
            Node::Sequence(nodes) | Node::Block(nodes) => self.eval_nodes(nodes, input),
            Node::Simple(simple) => self.eval_simple(simple, input),
            Node::And(left, right) => {
                let left = self.eval_node(left, input);
                if left.status.is_success() {
                    let mut right = self.eval_node(right, String::new());
                    right.stderr = left.stderr + &right.stderr;
                    right
                } else {
                    left
                }
            }
            Node::Or(left, right) => {
                let left = self.eval_node(left, input);
                if left.status.is_success() {
                    left
                } else {
                    let mut right = self.eval_node(right, String::new());
                    right.stderr = left.stderr + &right.stderr;
                    right
                }
            }
            Node::Pipe(left, right, spec) => {
                let from_fd = spec.as_ref().map(|spec| spec.from_fd).unwrap_or(1);
                let to_fd = spec.as_ref().map(|spec| spec.to_fd).unwrap_or(0);
                if let Some(out) =
                    self.try_execute_pipeline_graph(left, right, from_fd, to_fd, &input)
                {
                    return out;
                }
                let graph = self.prepare_pipeline_graph(left, right, from_fd, to_fd);
                let mut left = self.eval_node(left, input);
                let left_status = left.status.clone();
                let piped = match from_fd {
                    2 => std::mem::take(&mut left.stderr),
                    _ => std::mem::take(&mut left.stdout),
                };
                let right_input = if to_fd == 0 { piped } else { String::new() };
                let mut right = self.eval_node(right, right_input);
                let right_status = right.status.clone();
                right.status = RcStatus::pipeline(&left_status, &right_status);
                right.stdout = left.stdout + &right.stdout;
                right.stderr = left.stderr + &right.stderr;
                self.finish_process_graph(graph.as_ref(), &[left_status, right_status]);
                right
            }
            Node::Not(inner) => {
                let mut out = self.eval_node(inner, input);
                out.status = out.status.invert();
                out
            }
            Node::Background(inner) => {
                let id = self.next_job_id;
                self.next_job_id += 1;
                if self.try_start_background_graph(id, inner, &input) {
                    return RcOutput::success(format!("[{id}]\n"));
                }
                let graph = self.prepare_background_graph(id, inner);
                let out = self.eval_node(inner, input);
                self.finish_process_graph(graph.as_ref(), std::slice::from_ref(&out.status));
                self.jobs.push(RcJob {
                    id,
                    status: out.status,
                    stdout: out.stdout,
                    stderr: out.stderr,
                    graph,
                });
                RcOutput::success(format!("[{id}]\n"))
            }
            Node::If {
                condition,
                then_branch,
                else_branch,
            } => {
                let cond = self.eval_node(condition, input);
                if cond.status.is_success() {
                    self.eval_node(then_branch, String::new())
                } else if let Some(else_branch) = else_branch {
                    self.eval_node(else_branch, String::new())
                } else {
                    RcOutput {
                        status: RcStatus::success(),
                        stdout: String::new(),
                        stderr: cond.stderr,
                        exited: false,
                    }
                }
            }
            Node::For { var, values, body } => {
                let values = if values.is_empty() {
                    self.argv.clone()
                } else {
                    self.expand_words(values)
                };
                let old = self.vars.get(var).cloned();
                let mut out = RcOutput::default();
                for value in values {
                    self.vars.insert(var.clone(), vec![value]);
                    out.append(self.eval_node(body, String::new()));
                }
                restore_var(&mut self.vars, var, old);
                out
            }
            Node::While { condition, body } => {
                let mut out = RcOutput::default();
                let mut guard = 0_usize;
                loop {
                    guard += 1;
                    if guard > 100_000 {
                        return RcOutput::failure(
                            "loop",
                            "rc: while loop iteration limit exceeded\n",
                        );
                    }
                    let cond = self.eval_node(condition, String::new());
                    if !cond.status.is_success() {
                        break;
                    }
                    out.append(self.eval_node(body, String::new()));
                }
                out
            }
            Node::Switch { value, cases } => {
                let value = self
                    .expand_word(value)
                    .into_iter()
                    .next()
                    .unwrap_or_default();
                for case in cases {
                    let patterns = self.expand_words(&case.patterns);
                    if patterns
                        .iter()
                        .any(|pattern| pattern_match(pattern, &value))
                    {
                        return self.eval_nodes(&case.body, input);
                    }
                }
                RcOutput::default()
            }
            Node::Function { name, body } => {
                if let Some(body) = body {
                    self.functions.insert(name.clone(), *body.clone());
                } else {
                    self.functions.remove(name);
                }
                RcOutput::default()
            }
        }
    }

    fn eval_simple(&mut self, simple: &SimpleCommand, input: String) -> RcOutput {
        let mut restore = Vec::new();
        let assignment_values = simple
            .assignments
            .iter()
            .map(|assignment| {
                (
                    assignment.name.clone(),
                    if assignment.values.is_empty() {
                        Vec::new()
                    } else {
                        self.expand_words(&assignment.values)
                    },
                )
            })
            .collect::<Vec<_>>();

        if simple.words.is_empty() {
            for (name, values) in assignment_values {
                self.vars.insert(name, values);
            }
            self.publish_environment().ok();
            return RcOutput::default();
        }

        for (name, values) in assignment_values {
            restore.push((name.clone(), self.vars.get(&name).cloned()));
            self.vars.insert(name, values);
        }

        let mut input = input;
        let mut sinks = BTreeMap::from([(1, FdSink::Stdout), (2, FdSink::Stderr)]);
        for redirect in &simple.redirects {
            if let Err(err) = self.prepare_redirect(redirect, &mut input, &mut sinks) {
                return RcOutput::failure("redirect", format!("rc: {err}\n"));
            }
        }

        let words = self.expand_words(&simple.words);
        if words.is_empty() {
            for (name, old) in restore {
                restore_var(&mut self.vars, &name, old);
            }
            return RcOutput::default();
        }
        let mut out = if let Some(function) = self.functions.get(&words[0]).cloned() {
            let old_argv = self.argv.clone();
            self.set_args(words[1..].to_vec());
            let out = self.eval_node(&function, input);
            self.set_args(old_argv);
            out
        } else if let Some(out) = self.run_builtin(&words, &input) {
            out
        } else if let Some(out) = self.run_path_command(&words, input.clone()) {
            out
        } else {
            match self.host.run_command(RcCommandInvocation {
                name: words[0].clone(),
                args: words[1..].to_vec(),
                stdin: input.clone(),
                env: self.vars.clone(),
            }) {
                Ok(result) => RcOutput {
                    status: result.status,
                    stdout: result.stdout,
                    stderr: result.stderr,
                    exited: false,
                },
                Err(err) => RcOutput::failure("notfound", format!("{}: {err}\n", words[0])),
            }
        };
        self.import_host_environment().ok();

        if let Err(err) = self.apply_output_sinks(&mut out, sinks) {
            out.status = RcStatus::failure();
            out.stderr.push_str(&format!("rc: {err}\n"));
        }
        self.last_status = out.status.clone();
        self.vars
            .insert("status".into(), vec![self.last_status.to_string()]);
        for (name, old) in restore {
            restore_var(&mut self.vars, &name, old);
        }
        self.publish_environment().ok();
        out
    }

    fn run_path_command(&mut self, words: &[String], input: String) -> Option<RcOutput> {
        let command = words.first()?;
        if command.contains('/') {
            return self.run_rc_script(command, &words[1..], input).ok();
        }
        for dir in self.lookup_var("path") {
            let candidate = join_path(&dir, command);
            if let Ok(out) = self.run_rc_script(&candidate, &words[1..], input.clone()) {
                return Some(out);
            }
        }
        None
    }

    fn run_rc_script(&mut self, path: &str, args: &[String], input: String) -> RcResult<RcOutput> {
        let bytes = self.host.read_file(path)?;
        if is_host_executable(path, &bytes) {
            let result = self.host.run_command(RcCommandInvocation {
                name: path.to_string(),
                args: args.to_vec(),
                stdin: input,
                env: self.vars.clone(),
            })?;
            return Ok(RcOutput {
                status: result.status,
                stdout: result.stdout,
                stderr: result.stderr,
                exited: false,
            });
        }
        let source = String::from_utf8_lossy(&bytes).into_owned();
        let old_argv0 = self.argv0.clone();
        let old_argv = self.argv.clone();
        self.argv0 = path.to_string();
        self.set_args(args.to_vec());
        let script = parse(&source)?;
        let out = self.eval_script(&script, input);
        self.argv0 = old_argv0;
        self.set_args(old_argv);
        Ok(out)
    }

    fn run_note_function(&mut self, name: &str) -> Option<RcOutput> {
        if !self.notes_in_progress.insert(name.to_string()) {
            return None;
        }
        let function = self.functions.get(name).cloned();
        let out = function.map(|function| {
            let old_argv = self.argv.clone();
            self.set_args(Vec::new());
            let out = self.eval_node(&function, String::new());
            self.set_args(old_argv);
            out
        });
        self.notes_in_progress.remove(name);
        out
    }

    fn run_builtin(&mut self, words: &[String], input: &str) -> Option<RcOutput> {
        let name = words.first()?.as_str();
        let args = &words[1..];
        let out = match name {
            "echo" => RcOutput::success(args.join(" ") + "\n"),
            "cd" => {
                let path = args.first().cloned().unwrap_or_else(|| ".".into());
                match self.host.set_current_dir(&path) {
                    Ok(()) => RcOutput::default(),
                    Err(err) => RcOutput::failure("cd", format!("cd: {path}: {err}\n")),
                }
            }
            "pwd" => RcOutput::success(self.host.current_dir() + "\n"),
            "status" => RcOutput::success(self.last_status.to_string() + "\n"),
            "true" => RcOutput::default(),
            "false" => RcOutput {
                status: RcStatus::failure(),
                stdout: String::new(),
                stderr: String::new(),
                exited: false,
            },
            "~" => {
                if args.len() < 2 {
                    RcOutput::failure("usage", "usage: ~ subject pattern ...\n")
                } else if args[1..]
                    .iter()
                    .any(|pattern| pattern_match(pattern, &args[0]))
                {
                    RcOutput::default()
                } else {
                    RcOutput {
                        status: RcStatus::failure(),
                        stdout: String::new(),
                        stderr: String::new(),
                        exited: false,
                    }
                }
            }
            "eval" => self.eval_source(&args.join(" ")),
            "exec" => {
                if args.is_empty() {
                    RcOutput::failure("usage", "usage: exec command [args ...]\n")
                } else if let Some(mut out) = self.run_path_command(args, input.to_string()) {
                    out.exited = true;
                    out
                } else {
                    let mut out = match self.host.run_command(RcCommandInvocation {
                        name: args[0].clone(),
                        args: args[1..].to_vec(),
                        stdin: input.to_string(),
                        env: self.vars.clone(),
                    }) {
                        Ok(result) => RcOutput {
                            status: result.status,
                            stdout: result.stdout,
                            stderr: result.stderr,
                            exited: false,
                        },
                        Err(err) => RcOutput::failure("notfound", format!("{}: {err}\n", args[0])),
                    };
                    out.exited = true;
                    out
                }
            }
            "." => match args.first() {
                Some(path) => match self.host.read_file(path) {
                    Ok(bytes) => self.eval_source(&String::from_utf8_lossy(&bytes)),
                    Err(err) => RcOutput::failure("source", format!(".: {path}: {err}\n")),
                },
                None => RcOutput::failure("usage", "usage: . file\n"),
            },
            "shift" => {
                let n = args
                    .first()
                    .and_then(|value| value.parse::<usize>().ok())
                    .unwrap_or(1);
                self.argv = self.argv.iter().skip(n).cloned().collect();
                self.refresh_arg_vars();
                RcOutput::default()
            }
            "whatis" => {
                let mut stdout = String::new();
                for arg in args {
                    if let Some(values) = self.vars.get(arg) {
                        stdout.push_str(&format!("{arg}=({})\n", values.join(" ")));
                    } else if let Some(function) = self.functions.get(arg) {
                        stdout.push_str(&format!("fn {arg} {}\n", render_node(function)));
                    } else if BUILTINS.contains(&arg.as_str()) {
                        stdout.push_str(&format!("builtin {arg}\n"));
                    } else {
                        stdout.push_str(&format!("{arg}: not found\n"));
                    }
                }
                RcOutput::success(stdout)
            }
            "test" => self.run_test(args),
            "basename" => self.run_basename(args),
            "exit" => {
                let status = args
                    .first()
                    .map(|value| RcStatus(value.clone()))
                    .unwrap_or_else(|| self.last_status.clone());
                let mut out = self.run_note_function("sigexit").unwrap_or_default();
                out.status = status;
                out.exited = true;
                out
            }
            "wait" => self.run_wait(args),
            "rfork" => self.run_rfork(args),
            "flag" => self.run_flag(args),
            "builtin" => self.run_builtin_command(args, input),
            "cat" if args.is_empty() => RcOutput::success(input.to_string()),
            _ => return None,
        };
        Some(out)
    }

    fn run_wait(&mut self, args: &[String]) -> RcOutput {
        if args.len() > 1 {
            return RcOutput::failure("usage", "usage: wait [job]\n");
        }
        let requested = match args.first() {
            Some(arg) => match arg.parse::<u32>() {
                Ok(id) => Some(id),
                Err(_) => {
                    return RcOutput::failure("usage", format!("wait: {arg}: bad job id\n"));
                }
            },
            None => None,
        };

        if let Ok(Some(results)) = self.host.wait_process_job(requested) {
            return self.render_wait_results(results);
        }

        let jobs = if let Some(id) = requested {
            let Some(index) = self.jobs.iter().position(|job| job.id == id) else {
                return RcOutput::failure("wait", format!("wait: {id}: no such job\n"));
            };
            vec![self.jobs.remove(index)]
        } else {
            std::mem::take(&mut self.jobs)
        };
        let mut out = String::new();
        let mut status = RcStatus::success();
        for job in jobs {
            if !job.stdout.is_empty() {
                out.push_str(&job.stdout);
            }
            if !job.stderr.is_empty() {
                out.push_str(&job.stderr);
            }
            out.push_str(&format!("[{}] {}\n", job.id, job.status));
            if !job.status.is_success() {
                status = job.status.clone();
            }
            if let Some(graph) = &job.graph {
                self.finish_process_graph(Some(graph), std::slice::from_ref(&job.status));
            }
        }
        RcOutput {
            status,
            stdout: out,
            stderr: String::new(),
            exited: false,
        }
    }

    fn render_wait_results(&self, results: Vec<RcProcessJobResult>) -> RcOutput {
        let mut out = String::new();
        let mut status = RcStatus::success();
        for job in results {
            out.push_str(&job.stdout);
            out.push_str(&job.stderr);
            out.push_str(&format!("[{}] {}\n", job.id, job.status));
            if !job.status.is_success() {
                status = job.status.clone();
            }
        }
        RcOutput {
            status,
            stdout: out,
            stderr: String::new(),
            exited: false,
        }
    }

    fn run_rfork(&mut self, args: &[String]) -> RcOutput {
        let flags = args.join("");
        match self.host.rfork(&flags) {
            Ok(()) => RcOutput::default(),
            Err(err) => RcOutput::failure("rfork", format!("rfork: {err}\n")),
        }
    }

    fn run_flag(&self, args: &[String]) -> RcOutput {
        match args {
            [] => RcOutput::success("\n"),
            [flag] if flag == "x" || flag == "e" || flag == "r" || flag == "i" => {
                RcOutput::default()
            }
            [flag] => RcOutput::failure("flag", format!("flag: unknown flag {flag}\n")),
            _ => RcOutput::failure("usage", "usage: flag [name]\n"),
        }
    }

    fn run_builtin_command(&mut self, args: &[String], input: &str) -> RcOutput {
        if args.is_empty() {
            return RcOutput::failure("usage", "usage: builtin command [args ...]\n");
        }
        match args[0].as_str() {
            "builtin" => RcOutput::default(),
            name if BUILTINS.contains(&name) && name != "builtin" => {
                self.run_builtin(args, input).unwrap_or_default()
            }
            name => RcOutput::failure("builtin", format!("builtin: {name}: not a builtin\n")),
        }
    }

    fn run_test(&mut self, args: &[String]) -> RcOutput {
        let success = match args {
            [flag, path] if flag == "-e" || flag == "-f" => self
                .host
                .stat(path)
                .map(|stat| flag == "-e" || !stat.is_dir)
                .unwrap_or(false),
            [flag, path] if flag == "-d" => self
                .host
                .stat(path)
                .map(|stat| stat.is_dir)
                .unwrap_or(false),
            [left, op, right] if op == "=" => left == right,
            [left, op, right] if op == "!=" => left != right,
            [value] => !value.is_empty(),
            _ => false,
        };
        if success {
            RcOutput::default()
        } else {
            RcOutput {
                status: RcStatus::failure(),
                stdout: String::new(),
                stderr: String::new(),
                exited: false,
            }
        }
    }

    fn run_basename(&self, args: &[String]) -> RcOutput {
        let Some(path) = args.first() else {
            return RcOutput::failure("usage", "usage: basename path [suffix]\n");
        };
        let mut leaf = path
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or(path)
            .to_string();
        if let Some(suffix) = args.get(1) {
            if leaf.ends_with(suffix) {
                leaf.truncate(leaf.len() - suffix.len());
            }
        }
        RcOutput::success(leaf + "\n")
    }

    fn prepare_redirect(
        &mut self,
        redirect: &Redirect,
        input: &mut String,
        sinks: &mut BTreeMap<u32, FdSink>,
    ) -> RcResult<()> {
        match &redirect.mode {
            RedirectMode::Read => {
                match redirect.target.as_ref() {
                    Some(RedirectTarget::Process(node)) => {
                        let graph = self.prepare_process_substitution_graph(
                            RcProcessGraphKind::ProcessSubstitutionRead,
                            node,
                            1,
                        );
                        let out = self.eval_node(node, String::new());
                        self.finish_process_graph(
                            graph.as_ref(),
                            std::slice::from_ref(&out.status),
                        );
                        *input = out.stdout;
                    }
                    _ => {
                        let path = self.redirect_path(redirect)?;
                        *input = if is_null_path(&path) {
                            String::new()
                        } else {
                            String::from_utf8_lossy(&self.host.read_file(&path)?).into_owned()
                        };
                    }
                }
                Ok(())
            }
            RedirectMode::Here => {
                *input = self.redirect_here_text(redirect)?;
                Ok(())
            }
            RedirectMode::Write | RedirectMode::Append => match redirect.target.as_ref() {
                Some(RedirectTarget::Process(node)) => {
                    let graph = self.prepare_process_substitution_graph(
                        RcProcessGraphKind::ProcessSubstitutionWrite,
                        node,
                        redirect.fd,
                    );
                    sinks.insert(
                        redirect.fd,
                        FdSink::Process {
                            node: node.clone(),
                            graph,
                        },
                    );
                    Ok(())
                }
                _ => {
                    let path = self.redirect_path(redirect)?;
                    if is_null_path(&path) {
                        sinks.insert(redirect.fd, FdSink::Closed);
                    } else {
                        sinks.insert(
                            redirect.fd,
                            FdSink::File {
                                append: matches!(redirect.mode, RedirectMode::Append),
                                path,
                            },
                        );
                    }
                    Ok(())
                }
            },
            RedirectMode::Dup { from } => {
                if let Some(from) = from {
                    let sink = sinks.get(from).cloned().unwrap_or(match from {
                        1 => FdSink::Stdout,
                        2 => FdSink::Stderr,
                        _ => FdSink::Closed,
                    });
                    sinks.insert(redirect.fd, sink);
                } else {
                    sinks.insert(redirect.fd, FdSink::Closed);
                }
                Ok(())
            }
        }
    }

    fn redirect_path(&mut self, redirect: &Redirect) -> RcResult<String> {
        match redirect.target.as_ref() {
            Some(RedirectTarget::Word(word)) => self
                .expand_word(word)
                .into_iter()
                .next()
                .ok_or_else(|| RcError::new("redirect target expanded to empty list")),
            Some(RedirectTarget::HereDoc(_)) => Err(RcError::new("here document is not a path")),
            Some(RedirectTarget::Process(_)) => {
                Err(RcError::new("process substitution is not a path"))
            }
            None => Err(RcError::new("redirect target expanded to empty list")),
        }
    }

    fn redirect_here_text(&mut self, redirect: &Redirect) -> RcResult<String> {
        match redirect.target.as_ref() {
            Some(RedirectTarget::HereDoc(doc)) if doc.expand => {
                Ok(self.expand_raw_word(&doc.body)?.join(" "))
            }
            Some(RedirectTarget::HereDoc(doc)) => Ok(doc.body.clone()),
            Some(RedirectTarget::Word(word)) => Ok(word.raw().to_string()),
            Some(RedirectTarget::Process(_)) => Err(RcError::new(
                "process substitution cannot be a here document",
            )),
            None => Ok(String::new()),
        }
    }

    fn apply_output_sinks(
        &mut self,
        output: &mut RcOutput,
        sinks: BTreeMap<u32, FdSink>,
    ) -> RcResult<()> {
        let stdout = std::mem::take(&mut output.stdout);
        let stderr = std::mem::take(&mut output.stderr);
        self.apply_fd_sink(1, stdout, &sinks, output)?;
        self.apply_fd_sink(2, stderr, &sinks, output)
    }

    fn apply_fd_sink(
        &mut self,
        fd: u32,
        data: String,
        sinks: &BTreeMap<u32, FdSink>,
        output: &mut RcOutput,
    ) -> RcResult<()> {
        if data.is_empty() {
            return Ok(());
        }
        match sinks.get(&fd).cloned().unwrap_or(match fd {
            2 => FdSink::Stderr,
            _ => FdSink::Stdout,
        }) {
            FdSink::Stdout => output.stdout.push_str(&data),
            FdSink::Stderr => output.stderr.push_str(&data),
            FdSink::Closed => {}
            FdSink::File { path, append } => {
                if append {
                    self.host.append_file(&path, data.as_bytes())?;
                } else {
                    self.host.write_file(&path, data.as_bytes())?;
                }
            }
            FdSink::Process { node, graph } => {
                let next = self.eval_node(&node, data);
                self.finish_process_graph(graph.as_ref(), std::slice::from_ref(&next.status));
                output.stdout.push_str(&next.stdout);
                output.stderr.push_str(&next.stderr);
                output.status = next.status;
                output.exited |= next.exited;
            }
        }
        Ok(())
    }

    fn expand_words(&mut self, words: &[Word]) -> Vec<String> {
        let mut out = Vec::new();
        for word in words {
            let expanded = self.expand_word(word);
            if expanded.is_empty() {
                continue;
            }
            for value in expanded {
                out.extend(self.expand_glob_or_literal(value));
            }
        }
        out
    }

    fn expand_word(&mut self, word: &Word) -> Vec<String> {
        self.expand_raw_word(word.raw())
            .unwrap_or_else(|err| vec![format!("rc-expansion-error:{err}")])
    }

    fn expand_raw_word(&mut self, raw: &str) -> RcResult<Vec<String>> {
        let parts = split_caret_parts(raw);
        let mut acc = vec![String::new()];
        for part in parts {
            let values = self.expand_part(&part)?;
            acc = concat_values(acc, values);
        }
        Ok(acc)
    }

    fn expand_part(&mut self, raw: &str) -> RcResult<Vec<String>> {
        let chars: Vec<char> = raw.chars().collect();
        let mut i = 0;
        let mut acc = vec![String::new()];
        while i < chars.len() {
            match chars[i] {
                '\'' => {
                    let (literal, next) = read_quoted_literal(&chars, i)?;
                    acc = concat_values(acc, vec![literal]);
                    i = next;
                }
                '$' => {
                    let (values, next) = self.read_variable_expansion(&chars, i)?;
                    acc = concat_values(acc, values);
                    i = next;
                }
                '`' if chars.get(i + 1) == Some(&'{') => {
                    let (source, next) = extract_command_substitution(&chars, i)?;
                    let out = self.eval_source(&source);
                    let values = split_ifs(&out.stdout, &self.lookup_var("ifs"));
                    acc = concat_values(acc, values);
                    i = next;
                }
                ch => {
                    acc = concat_values(acc, vec![ch.to_string()]);
                    i += 1;
                }
            }
        }
        Ok(acc)
    }

    fn read_variable_expansion(
        &self,
        chars: &[char],
        start: usize,
    ) -> RcResult<(Vec<String>, usize)> {
        let mut i = start + 1;
        let mut count = false;
        let mut quote = false;
        if chars.get(i) == Some(&'#') {
            count = true;
            i += 1;
        } else if chars.get(i) == Some(&'"') {
            quote = true;
            i += 1;
        }
        let name_start = i;
        if chars.get(i) == Some(&'*') {
            i += 1;
        } else {
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
        }
        if i == name_start {
            return Ok((vec!["$".into()], start + 1));
        }
        let name: String = chars[name_start..i].iter().collect();
        let mut values = self.lookup_var(&name);
        if chars.get(i) == Some(&'(') {
            let end = chars[i + 1..]
                .iter()
                .position(|ch| *ch == ')')
                .map(|offset| i + 1 + offset)
                .ok_or_else(|| RcError::new("unterminated variable subscript"))?;
            let subs: String = chars[i + 1..end].iter().collect();
            let mut selected = Vec::new();
            for sub in subs.split_whitespace() {
                if let Ok(index) = sub.parse::<usize>() {
                    if index > 0 {
                        if let Some(value) = values.get(index - 1) {
                            selected.push(value.clone());
                        }
                    }
                }
            }
            values = selected;
            i = end + 1;
        }
        if count {
            values = vec![values.len().to_string()];
        } else if quote {
            values = vec![values.join(" ")];
        }
        Ok((values, i))
    }

    fn lookup_var(&self, name: &str) -> Vec<String> {
        match name {
            "*" => self.argv.clone(),
            "0" => vec![self.argv0.clone()],
            value if value.chars().all(|ch| ch.is_ascii_digit()) => value
                .parse::<usize>()
                .ok()
                .and_then(|index| {
                    if index == 0 {
                        Some(self.argv0.clone())
                    } else {
                        self.argv.get(index - 1).cloned()
                    }
                })
                .into_iter()
                .collect(),
            "status" => vec![self.last_status.to_string()],
            _ => self.vars.get(name).cloned().unwrap_or_default(),
        }
    }

    fn refresh_arg_vars(&mut self) {
        self.vars.insert("*".into(), self.argv.clone());
    }

    fn expand_glob_or_literal(&mut self, value: String) -> Vec<String> {
        if !has_glob_chars(&value) {
            return vec![value];
        }
        let matches = self.glob(&value);
        if matches.is_empty() {
            vec![value]
        } else {
            matches
        }
    }

    fn glob(&mut self, pattern: &str) -> Vec<String> {
        let absolute = pattern.starts_with('/');
        let trimmed = pattern.trim_start_matches('/');
        let parts = trimmed
            .split('/')
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>();
        let base = if absolute {
            "."
        } else {
            &self.host.current_dir()
        };
        let mut out = Vec::new();
        self.glob_inner(base, &parts, String::new(), &mut out);
        out.sort();
        out.dedup();
        out
    }

    fn glob_inner(&mut self, base: &str, parts: &[&str], prefix: String, out: &mut Vec<String>) {
        if parts.is_empty() {
            out.push(if prefix.is_empty() {
                ".".into()
            } else {
                prefix
            });
            return;
        }
        let part = parts[0];
        if !has_glob_chars(part) {
            let next_prefix = join_path(&prefix, part);
            let next_base = join_path(base, part);
            self.glob_inner(&next_base, &parts[1..], next_prefix, out);
            return;
        }
        let entries = match self.host.read_dir(base) {
            Ok(entries) => entries,
            Err(_) => return,
        };
        for entry in entries {
            let clean = entry.trim_end_matches('/');
            if clean.starts_with('.') && !part.starts_with('.') {
                continue;
            }
            if pattern_match(part, clean) {
                let next_prefix = join_path(&prefix, clean);
                let next_base = join_path(base, clean);
                self.glob_inner(&next_base, &parts[1..], next_prefix, out);
            }
        }
    }
}

#[derive(Clone, Debug)]
enum FdSink {
    Stdout,
    Stderr,
    File {
        path: String,
        append: bool,
    },
    Process {
        node: Box<Node>,
        graph: Option<RcProcessGraphRecord>,
    },
    Closed,
}

const BUILTINS: &[&str] = &[
    ".", "basename", "builtin", "cd", "echo", "eval", "exec", "exit", "false", "flag", "pwd",
    "rfork", "shift", "status", "test", "true", "wait", "whatis", "~",
];

fn restore_var(vars: &mut BTreeMap<String, Vec<String>>, name: &str, old: Option<Vec<String>>) {
    if let Some(old) = old {
        vars.insert(name.to_string(), old);
    } else {
        vars.remove(name);
    }
}

fn split_caret_parts(raw: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut chars = raw.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\'' {
            current.push(ch);
            while let Some(inner) = chars.next() {
                current.push(inner);
                if inner == '\'' {
                    if chars.peek() == Some(&'\'') {
                        current.push(chars.next().unwrap());
                        continue;
                    }
                    break;
                }
            }
        } else if ch == '^' {
            parts.push(std::mem::take(&mut current));
        } else {
            current.push(ch);
        }
    }
    parts.push(current);
    parts
}

fn concat_values(left: Vec<String>, right: Vec<String>) -> Vec<String> {
    if left.is_empty() || right.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for l in &left {
        for r in &right {
            out.push(format!("{l}{r}"));
        }
    }
    out
}

fn read_quoted_literal(chars: &[char], start: usize) -> RcResult<(String, usize)> {
    let mut out = String::new();
    let mut i = start + 1;
    while i < chars.len() {
        if chars[i] == '\'' {
            if chars.get(i + 1) == Some(&'\'') {
                out.push('\'');
                i += 2;
                continue;
            }
            return Ok((out, i + 1));
        }
        out.push(chars[i]);
        i += 1;
    }
    Err(RcError::new("unterminated quote"))
}

fn extract_command_substitution(chars: &[char], start: usize) -> RcResult<(String, usize)> {
    let (raw, next) = read_braced_substitution(chars, start)?;
    Ok((raw[2..raw.len() - 1].to_string(), next))
}

fn split_ifs(value: &str, ifs: &[String]) -> Vec<String> {
    let chars = ifs
        .first()
        .cloned()
        .unwrap_or_else(|| " \t\n".into())
        .chars()
        .collect::<BTreeSet<_>>();
    value
        .split(|ch| chars.contains(&ch))
        .filter(|part| !part.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn render_node(node: &Node) -> String {
    match node {
        Node::Empty => String::new(),
        Node::Simple(simple) => {
            let mut parts = simple
                .assignments
                .iter()
                .map(|assignment| {
                    if assignment.values.is_empty() {
                        format!("{}=()", assignment.name)
                    } else {
                        format!(
                            "{}=({})",
                            assignment.name,
                            assignment
                                .values
                                .iter()
                                .map(render_word)
                                .collect::<Vec<_>>()
                                .join(" ")
                        )
                    }
                })
                .collect::<Vec<_>>();
            parts.extend(simple.words.iter().map(render_word));
            parts.extend(simple.redirects.iter().map(render_redirect));
            parts.join(" ")
        }
        Node::Block(nodes) | Node::Sequence(nodes) => format!(
            "{{ {} }}",
            nodes.iter().map(render_node).collect::<Vec<_>>().join("; ")
        ),
        Node::And(left, right) => format!("{} && {}", render_node(left), render_node(right)),
        Node::Or(left, right) => format!("{} || {}", render_node(left), render_node(right)),
        Node::Pipe(left, right, spec) => {
            let pipe = spec
                .as_ref()
                .map(|spec| {
                    if spec.to_fd == 0 {
                        format!("|[{}]", spec.from_fd)
                    } else {
                        format!("|[{}={}]", spec.from_fd, spec.to_fd)
                    }
                })
                .unwrap_or_else(|| "|".into());
            format!("{} {} {}", render_node(left), pipe, render_node(right))
        }
        Node::Not(inner) => format!("! {}", render_node(inner)),
        Node::Background(inner) => format!("{} &", render_node(inner)),
        Node::If {
            condition,
            then_branch,
            else_branch,
        } => {
            let mut out = format!(
                "if({}) {}",
                render_node(condition),
                render_node(then_branch)
            );
            if let Some(else_branch) = else_branch {
                out.push_str(" if not ");
                out.push_str(&render_node(else_branch));
            }
            out
        }
        Node::For { var, values, body } => {
            let values = values.iter().map(render_word).collect::<Vec<_>>().join(" ");
            if values.is_empty() {
                format!("for({var}) {}", render_node(body))
            } else {
                format!("for({var} in {values}) {}", render_node(body))
            }
        }
        Node::While { condition, body } => {
            format!("while({}) {}", render_node(condition), render_node(body))
        }
        Node::Switch { value, cases } => {
            let cases = cases
                .iter()
                .map(|case| {
                    format!(
                        "case {}\n{}",
                        case.patterns
                            .iter()
                            .map(render_word)
                            .collect::<Vec<_>>()
                            .join(" "),
                        case.body
                            .iter()
                            .map(render_node)
                            .collect::<Vec<_>>()
                            .join("; ")
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            format!("switch({}){{\n{}\n}}", render_word(value), cases)
        }
        Node::Function { name, body } => body
            .as_ref()
            .map(|body| format!("fn {name} {}", render_node(body)))
            .unwrap_or_else(|| format!("fn {name}")),
    }
}

fn render_word(word: &Word) -> String {
    word.raw().to_string()
}

fn render_redirect(redirect: &Redirect) -> String {
    let op = match redirect.mode {
        RedirectMode::Read => "<".to_string(),
        RedirectMode::Write => ">".to_string(),
        RedirectMode::Append => ">>".to_string(),
        RedirectMode::Here => "<<".to_string(),
        RedirectMode::Dup { from: Some(from) } => format!(">[{}={}]", redirect.fd, from),
        RedirectMode::Dup { from: None } => format!(">[{}=]", redirect.fd),
    };
    let target = match redirect.target.as_ref() {
        Some(RedirectTarget::Word(word)) => render_word(word),
        Some(RedirectTarget::Process(node)) => render_node(node),
        Some(RedirectTarget::HereDoc(_)) => "<here-doc>".into(),
        None => String::new(),
    };
    if target.is_empty() {
        op
    } else {
        format!("{op}{target}")
    }
}

fn has_glob_chars(value: &str) -> bool {
    value.chars().any(|ch| matches!(ch, '*' | '?' | '['))
}

pub fn pattern_match(pattern: &str, value: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let v: Vec<char> = value.chars().collect();
    pattern_match_inner(&p, &v)
}

fn pattern_match_inner(pattern: &[char], value: &[char]) -> bool {
    if pattern.is_empty() {
        return value.is_empty();
    }
    match pattern[0] {
        '*' => {
            pattern_match_inner(&pattern[1..], value)
                || (!value.is_empty() && pattern_match_inner(pattern, &value[1..]))
        }
        '?' => !value.is_empty() && pattern_match_inner(&pattern[1..], &value[1..]),
        '[' => {
            if value.is_empty() {
                return false;
            }
            if let Some(end) = pattern.iter().position(|ch| *ch == ']') {
                let class = &pattern[1..end];
                char_class_match(class, value[0])
                    && pattern_match_inner(&pattern[end + 1..], &value[1..])
            } else {
                pattern[0] == value[0] && pattern_match_inner(&pattern[1..], &value[1..])
            }
        }
        ch => {
            !value.is_empty() && ch == value[0] && pattern_match_inner(&pattern[1..], &value[1..])
        }
    }
}

fn char_class_match(class: &[char], value: char) -> bool {
    let mut negate = false;
    let mut i = 0;
    if class.first() == Some(&'~') {
        negate = true;
        i = 1;
    }
    let mut matched = false;
    while i < class.len() {
        if i + 2 < class.len() && class[i + 1] == '-' {
            if class[i] <= value && value <= class[i + 2] {
                matched = true;
            }
            i += 3;
        } else {
            if class[i] == value {
                matched = true;
            }
            i += 1;
        }
    }
    if negate {
        !matched
    } else {
        matched
    }
}

fn join_path(base: &str, leaf: &str) -> String {
    if base.is_empty() || base == "." {
        leaf.to_string()
    } else if leaf.is_empty() {
        base.to_string()
    } else {
        format!("{}/{}", base.trim_end_matches('/'), leaf)
    }
}

fn is_null_path(path: &str) -> bool {
    matches!(path, "/dev/null" | "dev/null" | "#null" | "#null/null")
}

fn is_host_executable(path: &str, bytes: &[u8]) -> bool {
    bytes.starts_with(b"\0asm") || is_host_executable_path(path)
}

fn is_host_executable_path(path: &str) -> bool {
    path.ends_with(".wasm")
        || path.ends_with(".wat")
        || path.ends_with(".js")
        || path.ends_with(".mjs")
}

#[cfg(test)]
pub mod fake {
    use super::*;

    #[derive(Clone, Debug)]
    pub struct FakeHost {
        cwd: String,
        files: BTreeMap<String, Vec<u8>>,
    }

    impl Default for FakeHost {
        fn default() -> Self {
            Self {
                cwd: ".".into(),
                files: BTreeMap::new(),
            }
        }
    }

    impl FakeHost {
        pub fn with_file(mut self, path: &str, data: &str) -> Self {
            self.files.insert(clean(path), data.as_bytes().to_vec());
            self
        }

        fn resolve(&self, path: &str) -> String {
            if path == "." || path.starts_with('#') {
                clean(path)
            } else if path.starts_with('/') {
                clean(path.trim_start_matches('/'))
            } else if self.cwd == "." {
                clean(path)
            } else {
                clean(&format!("{}/{}", self.cwd, path))
            }
        }
    }

    impl RcHost for FakeHost {
        fn current_dir(&self) -> String {
            self.cwd.clone()
        }

        fn set_current_dir(&mut self, path: &str) -> RcResult<()> {
            let path = self.resolve(path);
            if path == "."
                || self
                    .files
                    .keys()
                    .any(|file| file.starts_with(&(path.clone() + "/")))
            {
                self.cwd = path;
                Ok(())
            } else {
                Err(RcError::new("not a directory"))
            }
        }

        fn read_file(&mut self, path: &str) -> RcResult<Vec<u8>> {
            self.files
                .get(&self.resolve(path))
                .cloned()
                .ok_or_else(|| RcError::new("not found"))
        }

        fn write_file(&mut self, path: &str, data: &[u8]) -> RcResult<()> {
            let path = self.resolve(path);
            self.files.insert(path, data.to_vec());
            Ok(())
        }

        fn append_file(&mut self, path: &str, data: &[u8]) -> RcResult<()> {
            self.files
                .entry(self.resolve(path))
                .or_default()
                .extend_from_slice(data);
            Ok(())
        }

        fn read_dir(&mut self, path: &str) -> RcResult<Vec<String>> {
            let path = self.resolve(path);
            let prefix = if path == "." {
                String::new()
            } else {
                format!("{path}/")
            };
            let mut entries = BTreeSet::new();
            for file in self.files.keys() {
                if let Some(rest) = file.strip_prefix(&prefix) {
                    if let Some((dir, _)) = rest.split_once('/') {
                        entries.insert(format!("{dir}/"));
                    } else {
                        entries.insert(rest.to_string());
                    }
                }
            }
            Ok(entries.into_iter().collect())
        }

        fn stat(&mut self, path: &str) -> RcResult<RcStat> {
            let path = self.resolve(path);
            Ok(RcStat {
                is_dir: path == "."
                    || self
                        .files
                        .keys()
                        .any(|file| file.starts_with(&(path.clone() + "/"))),
            })
        }

        fn run_command(&mut self, invocation: RcCommandInvocation) -> RcResult<RcCommandResult> {
            match invocation.name.as_str() {
                "upper" => Ok(RcCommandResult {
                    status: RcStatus::success(),
                    stdout: invocation.stdin.to_uppercase(),
                    stderr: String::new(),
                }),
                "argv" => Ok(RcCommandResult {
                    status: RcStatus::success(),
                    stdout: invocation.args.join(",") + "\n",
                    stderr: String::new(),
                }),
                _ => Err(RcError::new("command not found")),
            }
        }
    }

    fn clean(path: &str) -> String {
        let mut out = Vec::new();
        for part in path.split('/') {
            match part {
                "" | "." => {}
                ".." => {
                    out.pop();
                }
                part => out.push(part),
            }
        }
        if out.is_empty() {
            ".".into()
        } else {
            out.join("/")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::FakeHost;
    use super::*;

    #[test]
    fn parser_handles_control_flow_and_functions() {
        let script = parse("fn greet { echo hi $1 }\nfor(i in a b) { greet $i }\n").unwrap();
        assert_eq!(script.commands.len(), 2);
        assert!(matches!(script.commands[0], Node::Function { .. }));
        assert!(matches!(script.commands[1], Node::For { .. }));
    }

    #[test]
    fn lexer_keeps_bang_inside_service_address_words() {
        let script = parse("srv -nqC tcp!9p.io sources /n/sources").unwrap();
        let Node::Simple(command) = &script.commands[0] else {
            panic!("expected simple command");
        };
        assert_eq!(command.words[2].raw(), "tcp!9p.io");
    }

    #[test]
    fn variables_lists_quotes_and_concatenation_expand_like_rc() {
        let mut rc = RcSession::new(FakeHost::default());
        let out = rc.eval_source("x=(a b c); echo $#x $x(2) pre^$x 'don''t'");
        assert_eq!(out.status, RcStatus::success());
        assert_eq!(out.stdout, "3 b prea preb prec don't\n");
    }

    #[test]
    fn command_substitution_and_ifs_split_output() {
        let mut rc = RcSession::new(FakeHost::default());
        let out = rc.eval_source("echo `{echo a b}");
        assert_eq!(out.stdout, "a b\n");
    }

    #[test]
    fn functions_for_if_switch_and_status_work() {
        let mut rc = RcSession::new(FakeHost::default());
        let out = rc.eval_source(
            "fn twice { echo $1 $1 }\nfor(i in one two) twice $i\nif(~ two two) echo ok\nswitch(two){case one\n echo bad\ncase t*\n echo switch}\n",
        );
        assert_eq!(out.status, RcStatus::success());
        assert_eq!(out.stdout, "one one\ntwo two\nok\nswitch\n");
    }

    #[test]
    fn globbing_uses_host_filesystem_and_preserves_misses() {
        let host = FakeHost::default()
            .with_file("tmp/a.txt", "a")
            .with_file("tmp/b.txt", "b")
            .with_file("tmp/c.bin", "c");
        let mut rc = RcSession::new(host);
        let out = rc.eval_source("echo tmp/*.txt nope*");
        assert_eq!(out.stdout, "tmp/a.txt tmp/b.txt nope*\n");
    }

    #[test]
    fn redirection_pipeline_and_source_work() {
        let host = FakeHost::default().with_file("script.rc", "echo sourced");
        let mut rc = RcSession::new(host);
        let out = rc.eval_source("echo hello | upper >out; . script.rc; cat <out");
        assert_eq!(out.status, RcStatus::success());
        assert_eq!(out.stdout, "sourced\nHELLO\n");
    }

    #[test]
    fn fd_dup_process_substitution_here_docs_and_exit_work() {
        let mut rc = RcSession::new(FakeHost::default());
        rc.set_var("name", vec!["world".into()]);
        let out = rc.eval_source(
            "echo err >[1=2]\n\
             echo hidden >/dev/null\n\
             cat <{echo proc}\n\
             cat <<EOF\nhello $name\nEOF\n\
             echo before\n\
             exit done\n\
             echo after\n",
        );
        assert_eq!(out.status, RcStatus("done".into()));
        assert_eq!(out.stdout, "proc\nhello world\nbefore\n");
        assert_eq!(out.stderr, "err\n");
        assert!(out.exited);
    }

    #[test]
    fn path_search_runs_rc_scripts_with_argv0_and_args() {
        let host = FakeHost::default().with_file("bin/hello", "echo script-$1-$0");
        let mut rc = RcSession::new(host);
        let out = rc.eval_source("hello world");
        assert_eq!(out.status, RcStatus::success());
        assert_eq!(out.stdout, "script-world-bin/hello\n");
    }

    #[test]
    fn parses_9front_style_if_not_and_process_substitution_script() {
        let mut rc = RcSession::new(FakeHost::default());
        rc.set_argv0("9fs");
        let out = rc.eval_source(
            "rfork e\n\
             switch($1){\n\
             case ''\n\
                 echo usage: $0 >[1=2]\n\
                 exit usage\n\
             case vac:*\n\
                 cat <{echo $1}\n\
             }\n",
        );
        assert_eq!(out.status, RcStatus("usage".into()));
        assert_eq!(out.stderr, "usage: 9fs\n");

        let mut rc = RcSession::new(FakeHost::default());
        rc.set_args(vec!["vac:abc".into()]);
        let out = rc.eval_source(
            "switch($1){\n\
             case vac:*\n\
                 cat <{echo $1}\n\
             }\n",
        );
        assert_eq!(out.status, RcStatus::success());
        assert_eq!(out.stdout, "vac:abc\n");
    }

    #[test]
    fn environment_export_import_and_note_hooks_work() {
        let mut rc = RcSession::new(FakeHost::default());
        let out = rc.eval_source("x=(one two); fn sigexit { echo bye }; fn hello { echo hi-$1 }");
        assert_eq!(out.status, RcStatus::success());
        let env = rc.export_environment();
        assert_eq!(env.get("x").map(Vec::as_slice), Some(&b"one\0two"[..]));
        assert!(env.contains_key("fn#hello"));

        let mut restored = RcSession::new(FakeHost::default());
        restored.import_environment(env);
        let out = restored.eval_source("hello there; exit done; echo after");
        assert_eq!(out.status, RcStatus("done".into()));
        assert_eq!(out.stdout, "hi-there\nbye\n");
        assert!(out.exited);

        let out = restored.deliver_note("exit");
        assert_eq!(out.stdout, "bye\n");
    }

    #[test]
    fn background_wait_builtin_and_rfork_have_precise_behavior() {
        let mut rc = RcSession::new(FakeHost::default());
        let out = rc.eval_source("echo background & wait");
        assert_eq!(out.status, RcStatus::success());
        assert_eq!(out.stdout, "[1]\nbackground\n[1] 0\n");

        let mut rc = RcSession::new(FakeHost::default());
        let out = rc.eval_source("echo one & echo two & wait 1; wait");
        assert_eq!(out.status, RcStatus::success());
        assert_eq!(out.stdout, "[1]\n[2]\none\n[1] 0\ntwo\n[2] 0\n");

        let out = rc.eval_source("builtin echo ok; rfork e; rfork z");
        assert!(!out.status.is_success());
        assert_eq!(out.stdout, "ok\n");
        assert!(out.stderr.contains("unsupported rfork flags z"));
    }

    #[test]
    fn license_clean_rc_corpus_runs() {
        struct Fixture {
            name: &'static str,
            source: &'static str,
            stdout: &'static str,
            stderr: &'static str,
            success: bool,
        }
        let fixtures = [
            Fixture {
                name: "functions-control",
                source: include_str!("../tests/fixtures/functions-control.rc"),
                stdout: "one one\ntwo two\nmatched\n",
                stderr: "",
                success: true,
            },
            Fixture {
                name: "redirection-pipeline",
                source: include_str!("../tests/fixtures/redirection-pipeline.rc"),
                stdout: "HELLO\nproc\n",
                stderr: "",
                success: true,
            },
            Fixture {
                name: "env-jobs",
                source: include_str!("../tests/fixtures/env-jobs.rc"),
                stdout: "two\n[1]\njob\n[1] 0\n",
                stderr: "",
                success: true,
            },
        ];
        for fixture in fixtures {
            let mut rc = RcSession::new(FakeHost::default());
            let out = rc.eval_source(fixture.source);
            assert_eq!(
                out.status.is_success(),
                fixture.success,
                "{} status {:?}",
                fixture.name,
                out.status
            );
            assert_eq!(out.stdout, fixture.stdout, "{} stdout", fixture.name);
            assert_eq!(out.stderr, fixture.stderr, "{} stderr", fixture.name);
        }
    }

    #[test]
    fn optional_oracle_smoke_compares_portable_rc_cases() {
        let Ok(oracle) = std::env::var("STAR9_RC_ORACLE") else {
            return;
        };
        let fixtures = [
            "echo oracle",
            "x=(one two); echo $#x $x(2)",
            "if(~ two t*) echo match",
        ];
        for source in fixtures {
            let mut rc = RcSession::new(FakeHost::default());
            let actual = rc.eval_source(source);
            let oracle_output = std::process::Command::new(&oracle)
                .arg("-c")
                .arg(source)
                .output()
                .unwrap_or_else(|err| panic!("failed to run STAR9_RC_ORACLE={oracle}: {err}"));
            assert_eq!(
                actual.stdout,
                String::from_utf8_lossy(&oracle_output.stdout),
                "stdout mismatch for {source:?}"
            );
            assert_eq!(
                actual.stderr,
                String::from_utf8_lossy(&oracle_output.stderr),
                "stderr mismatch for {source:?}"
            );
            assert_eq!(
                actual.status.is_success(),
                oracle_output.status.success(),
                "status mismatch for {source:?}: actual {:?}, oracle {:?}",
                actual.status,
                oracle_output.status
            );
        }
    }
}
