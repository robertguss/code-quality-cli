//! A small shell parser: enough structure to find every command a Bash tool
//! call would run, with its assignments, arguments and redirections. It does
//! not evaluate anything; anything it cannot parse is an error so the caller
//! can fail closed.

/// One piece of a word, before expansion.
#[derive(Clone, Debug, PartialEq)]
pub enum Part {
    /// Unquoted literal text.
    Lit(String),
    /// Quoted or escaped literal text.
    Quoted(String),
    /// `$NAME` or `${NAME}`.
    Var(String),
    /// A leading `~` that expands to the home directory.
    Tilde,
    /// `$(...)`, backticks or `<(...)`: the commands inside also run.
    Subst { raw: String, body: List },
    /// An expansion whose value cannot be known (`$?`, `${X:-y}`, `$((1+2))`).
    Dyn(String),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Word {
    pub parts: Vec<Part>,
    /// The source text of the word.
    pub raw: String,
}

impl Word {
    fn lit(text: &str) -> Word {
        Word {
            parts: vec![Part::Lit(text.to_string())],
            raw: text.to_string(),
        }
    }

    /// The word's text when it is a single unquoted literal (reserved words).
    pub fn plain(&self) -> Option<&str> {
        match self.parts.as_slice() {
            [Part::Lit(s)] => Some(s),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RedirOp {
    Read,
    /// `>`, `>>`, `>|`, `<>`, `&>`, `&>>` and `>&file`: the target is written.
    Write,
    /// `>&N` or `<&N`: duplicates a descriptor, no file involved.
    Dup,
    /// `<<` and `<<<`: the target holds the text fed to stdin.
    Here,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Redirect {
    pub op: RedirOp,
    pub target: Word,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SimpleCmd {
    pub assigns: Vec<(String, Word)>,
    pub words: Vec<Word>,
    pub redirects: Vec<Redirect>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Simple(SimpleCmd),
    /// `( ... )` when `subshell`, otherwise `{ ...; }`, a function body or a case.
    Group {
        subshell: bool,
        body: List,
        redirects: Vec<Redirect>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Pipeline {
    pub stages: Vec<Node>,
    pub background: bool,
}

pub type List = Vec<Pipeline>;

/// Parse a whole script.
pub fn parse(src: &str) -> Result<List, String> {
    let mut p = Parser::new(src, 0);
    let list = p.list(End::Eof)?;
    match p.next()? {
        Tok::Eof => Ok(list),
        t => Err(format!("unexpected {}", t.describe())),
    }
}

const MAX_DEPTH: usize = 48;

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Word(Word),
    Op(&'static str),
    Redir(RedirOp),
    /// A heredoc operator together with its (already extracted) body.
    Here(Word),
    Newline,
    Eof,
}

impl Tok {
    fn describe(&self) -> String {
        match self {
            Tok::Word(w) => format!("`{}`", w.raw),
            Tok::Op(op) => format!("`{op}`"),
            Tok::Redir(_) | Tok::Here(_) => "redirection".into(),
            Tok::Newline => "newline".into(),
            Tok::Eof => "end of input".into(),
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum End {
    Eof,
    Paren,
    Brace,
    Case,
}

struct Parser {
    s: Vec<char>,
    i: usize,
    peeked: Option<Tok>,
    depth: usize,
    /// Inside `[[ ... ]]`, where `<`, `&&` and parentheses are plain words.
    dbracket: bool,
}

const OPS: &[&str] = &[";;&", ";;", ";&", "&&", "||", "|&", ";", "&", "|", "(", ")"];

fn is_meta(c: char) -> bool {
    matches!(
        c,
        ' ' | '\t' | '\r' | '\n' | ';' | '&' | '|' | '(' | ')' | '<' | '>'
    )
}

pub(crate) fn is_name(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c == '_' || c.is_ascii_alphabetic())
        && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

/// `NAME=` or `NAME+=` at the start of an unquoted first part.
fn assignment_name(w: &Word) -> Option<(String, usize)> {
    let Some(Part::Lit(first)) = w.parts.first() else {
        return None;
    };
    let eq = first.find('=')?;
    let name = first[..eq].strip_suffix('+').unwrap_or(&first[..eq]);
    is_name(name).then(|| (name.to_string(), eq + 1))
}

fn split_assignment(w: Word) -> Option<(String, Word)> {
    let (name, at) = assignment_name(&w)?;
    let mut parts = w.parts;
    let Part::Lit(first) = &parts[0] else {
        return None;
    };
    let rest = first[at..].to_string();
    if rest.is_empty() {
        parts.remove(0);
    } else {
        parts[0] = Part::Lit(rest);
    }
    let raw = w.raw.split_once('=').map(|(_, v)| v).unwrap_or("");
    Some((
        name,
        Word {
            parts,
            raw: raw.to_string(),
        },
    ))
}

impl Parser {
    fn new(src: &str, depth: usize) -> Parser {
        Parser {
            s: src.chars().collect(),
            i: 0,
            peeked: None,
            depth,
            dbracket: false,
        }
    }

    fn at(&self, off: usize) -> Option<char> {
        self.s.get(self.i + off).copied()
    }

    fn starts(&self, text: &str) -> bool {
        text.chars().enumerate().all(|(k, c)| self.at(k) == Some(c))
    }

    fn slice(&self, from: usize, to: usize) -> String {
        self.s[from..to].iter().collect()
    }

    fn sub_parser(&self, src: &str) -> Result<Parser, String> {
        if self.depth >= MAX_DEPTH {
            return Err("nested too deeply".into());
        }
        Ok(Parser::new(src, self.depth + 1))
    }

    fn peek(&mut self) -> Result<&Tok, String> {
        if self.peeked.is_none() {
            let t = self.lex()?;
            self.peeked = Some(t);
        }
        Ok(self.peeked.as_ref().unwrap())
    }

    fn next(&mut self) -> Result<Tok, String> {
        match self.peeked.take() {
            Some(t) => Ok(t),
            None => self.lex(),
        }
    }

    fn peek_plain(&mut self, word: &str) -> Result<bool, String> {
        Ok(matches!(self.peek()?, Tok::Word(w) if w.plain() == Some(word)))
    }

    fn expect_op(&mut self, op: &str) -> Result<(), String> {
        match self.next()? {
            Tok::Op(o) if o == op => Ok(()),
            t => Err(format!("expected `{op}`, found {}", t.describe())),
        }
    }

    fn skip_newlines(&mut self) -> Result<(), String> {
        while matches!(self.peek()?, Tok::Newline) {
            self.next()?;
        }
        Ok(())
    }

    fn skip_blanks(&mut self) {
        loop {
            match self.at(0) {
                Some(' ' | '\t' | '\r') => self.i += 1,
                Some('\\') if self.at(1) == Some('\n') => self.i += 2,
                Some('#') => {
                    while self.at(0).is_some_and(|c| c != '\n') {
                        self.i += 1;
                    }
                }
                _ => return,
            }
        }
    }

    fn lex(&mut self) -> Result<Tok, String> {
        self.skip_blanks();
        let Some(c) = self.at(0) else {
            return Ok(Tok::Eof);
        };
        if c == '\n' {
            self.i += 1;
            return Ok(Tok::Newline);
        }
        if self.dbracket && matches!(c, '&' | '|' | '<' | '>' | '(' | ')') {
            let start = self.i;
            while self
                .at(0)
                .is_some_and(|c| matches!(c, '&' | '|' | '<' | '>' | '(' | ')'))
            {
                self.i += 1;
            }
            return Ok(Tok::Word(Word::lit(&self.slice(start, self.i))));
        }
        if (c == '<' || c == '>') && self.at(1) == Some('(') {
            return self.word().map(Tok::Word);
        }
        // An optional descriptor number glued to a redirection operator.
        let mut k = 0;
        while self.at(k).is_some_and(|c| c.is_ascii_digit()) {
            k += 1;
        }
        if matches!(self.at(k), Some('<' | '>')) {
            self.i += k;
            return self.redir();
        }
        if self.starts("&>") {
            return self.redir();
        }
        for op in OPS {
            if self.starts(op) {
                self.i += op.len();
                return Ok(Tok::Op(op));
            }
        }
        self.word().map(Tok::Word)
    }

    fn redir(&mut self) -> Result<Tok, String> {
        const TABLE: &[(&str, RedirOp)] = &[
            ("&>>", RedirOp::Write),
            ("&>", RedirOp::Write),
            (">>", RedirOp::Write),
            (">|", RedirOp::Write),
            (">&", RedirOp::Dup),
            (">", RedirOp::Write),
            ("<<<", RedirOp::Here),
            ("<<-", RedirOp::Here),
            ("<<", RedirOp::Here),
            ("<&", RedirOp::Dup),
            ("<>", RedirOp::Write),
            ("<", RedirOp::Read),
        ];
        for (text, op) in TABLE {
            if self.starts(text) {
                self.i += text.len();
                if text.starts_with("<<") && *text != "<<<" {
                    return self.heredoc(*text == "<<-");
                }
                return Ok(Tok::Redir(*op));
            }
        }
        Err("bad redirection".into())
    }

    /// Read the delimiter, then cut the body out of the source so later
    /// parsing never sees it.
    fn heredoc(&mut self, strip_tabs: bool) -> Result<Tok, String> {
        self.skip_blanks();
        let delim = self.word()?;
        let expand = delim.parts.iter().all(|p| matches!(p, Part::Lit(_)));
        let delim_text: String = delim
            .parts
            .iter()
            .map(|p| match p {
                Part::Lit(s) | Part::Quoted(s) => s.as_str(),
                _ => "",
            })
            .collect();
        let mut body = String::new();
        if let Some(nl) = (self.i..self.s.len()).find(|&j| self.s[j] == '\n') {
            let start = nl + 1;
            let mut j = start;
            let mut end = self.s.len();
            while j < self.s.len() {
                let line_end = (j..self.s.len())
                    .find(|&k| self.s[k] == '\n')
                    .unwrap_or(self.s.len());
                let line = self.slice(j, line_end);
                let cmp = if strip_tabs {
                    line.trim_start_matches('\t')
                } else {
                    &line
                };
                if cmp == delim_text {
                    end = (line_end + 1).min(self.s.len());
                    break;
                }
                body.push_str(cmp);
                body.push('\n');
                j = line_end + 1;
            }
            self.s.drain(start..end);
        }
        let word = if expand {
            let mut p = self.sub_parser(&body)?;
            let mut parts = Vec::new();
            p.dq(&mut parts, None)?;
            Word { parts, raw: body }
        } else {
            Word {
                parts: vec![Part::Quoted(body.clone())],
                raw: body,
            }
        };
        Ok(Tok::Here(word))
    }

    fn word(&mut self) -> Result<Word, String> {
        let start = self.i;
        let mut parts: Vec<Part> = Vec::new();
        while let Some(c) = self.at(0) {
            match c {
                '<' | '>' if self.at(1) == Some('(') && parts.is_empty() => {
                    self.i += 2;
                    let sub = self.subst_list(start)?;
                    parts.push(sub);
                    break;
                }
                c if is_meta(c) => {
                    // `name=(a b)` array assignments keep their parentheses.
                    if c == '(' && !parts.is_empty() && {
                        let w = Word {
                            parts: parts.clone(),
                            raw: String::new(),
                        };
                        assignment_name(&w).is_some_and(
                            |(_, at)| matches!(&parts[..], [Part::Lit(s)] if s.len() == at),
                        )
                    } {
                        let from = self.i;
                        self.skip_balanced('(', ')')?;
                        parts.push(Part::Dyn(self.slice(from, self.i)));
                        continue;
                    }
                    break;
                }
                '\'' => {
                    self.i += 1;
                    let from = self.i;
                    while self.at(0).is_some_and(|c| c != '\'') {
                        self.i += 1;
                    }
                    if self.at(0).is_none() {
                        return Err("unterminated single quote".into());
                    }
                    parts.push(Part::Quoted(self.slice(from, self.i)));
                    self.i += 1;
                }
                '"' => {
                    self.i += 1;
                    self.dq(&mut parts, Some('"'))?;
                }
                '\\' => {
                    self.i += 1;
                    match self.at(0) {
                        Some('\n') => self.i += 1,
                        Some(c) => {
                            parts.push(Part::Quoted(c.to_string()));
                            self.i += 1;
                        }
                        None => parts.push(Part::Lit("\\".into())),
                    }
                }
                '$' => self.dollar(&mut parts, false)?,
                '`' => {
                    let p = self.backtick()?;
                    parts.push(p);
                }
                '~' if parts.is_empty() && self.at(1).is_none_or(|c| c == '/' || is_meta(c)) => {
                    self.i += 1;
                    parts.push(Part::Tilde);
                }
                c => {
                    push_lit(&mut parts, c, false);
                    self.i += 1;
                }
            }
        }
        if parts.is_empty() && self.i == start {
            return Err(format!("unexpected `{}`", self.at(0).unwrap_or(' ')));
        }
        Ok(Word {
            parts,
            raw: self.slice(start, self.i),
        })
    }

    /// Double-quoted contents up to `term` (or the end, for heredoc bodies).
    fn dq(&mut self, parts: &mut Vec<Part>, term: Option<char>) -> Result<(), String> {
        // An empty "" still makes a (quoted, empty) word.
        parts.push(Part::Quoted(String::new()));
        loop {
            let Some(c) = self.at(0) else {
                if term.is_some() {
                    return Err("unterminated double quote".into());
                }
                return Ok(());
            };
            if Some(c) == term {
                self.i += 1;
                return Ok(());
            }
            match c {
                '\\' => {
                    let next = self.at(1);
                    self.i += 1;
                    match next {
                        Some('\n') => self.i += 1,
                        Some(n @ ('$' | '`' | '\\')) => {
                            push_lit(parts, n, true);
                            self.i += 1;
                        }
                        Some('"') if term.is_some() => {
                            push_lit(parts, '"', true);
                            self.i += 1;
                        }
                        _ => push_lit(parts, '\\', true),
                    }
                }
                '$' => self.dollar(parts, true)?,
                '`' => {
                    let p = self.backtick()?;
                    parts.push(p);
                }
                c => {
                    push_lit(parts, c, true);
                    self.i += 1;
                }
            }
        }
    }

    fn dollar(&mut self, parts: &mut Vec<Part>, in_dq: bool) -> Result<(), String> {
        let start = self.i;
        self.i += 1;
        match self.at(0) {
            Some('\'') if !in_dq => {
                self.i += 1;
                let mut text = String::new();
                loop {
                    match self.at(0) {
                        None => return Err("unterminated $'' string".into()),
                        Some('\'') => break,
                        Some('\\') => {
                            self.i += 1;
                            let c = self.at(0).ok_or("unterminated $'' string")?;
                            text.push(match c {
                                'n' => '\n',
                                't' => '\t',
                                'r' => '\r',
                                'e' | 'E' => '\x1b',
                                'a' => '\x07',
                                '0' => '\0',
                                c => c,
                            });
                        }
                        Some(c) => text.push(c),
                    }
                    self.i += 1;
                }
                self.i += 1;
                parts.push(Part::Quoted(text));
            }
            Some('"') if !in_dq => {
                self.i += 1;
                self.dq(parts, Some('"'))?;
            }
            Some('(') if self.at(1) == Some('(') => {
                self.skip_balanced('(', ')')?;
                parts.push(Part::Dyn(self.slice(start, self.i)));
            }
            Some('(') => {
                self.i += 1;
                let sub = self.subst_list(start)?;
                parts.push(sub);
            }
            Some('{') => {
                self.skip_balanced('{', '}')?;
                let inner = self.slice(start + 2, self.i - 1);
                if is_name(&inner) {
                    parts.push(Part::Var(inner));
                } else {
                    parts.push(Part::Dyn(self.slice(start, self.i)));
                }
            }
            Some(c) if c == '_' || c.is_ascii_alphabetic() => {
                let from = self.i;
                while self
                    .at(0)
                    .is_some_and(|c| c == '_' || c.is_ascii_alphanumeric())
                {
                    self.i += 1;
                }
                parts.push(Part::Var(self.slice(from, self.i)));
            }
            Some(c) if c.is_ascii_digit() || "?$#@*!-".contains(c) => {
                self.i += 1;
                parts.push(Part::Dyn(self.slice(start, self.i)));
            }
            _ => push_lit(parts, '$', in_dq),
        }
        Ok(())
    }

    /// The commands of `$( ... )` or `<( ... )`; the opening is already consumed.
    fn subst_list(&mut self, start: usize) -> Result<Part, String> {
        if self.depth >= MAX_DEPTH {
            return Err("nested too deeply".into());
        }
        self.depth += 1;
        let saved = self.dbracket;
        self.dbracket = false;
        let body = self.list(End::Paren)?;
        self.expect_op(")")?;
        self.dbracket = saved;
        self.depth -= 1;
        Ok(Part::Subst {
            raw: self.slice(start, self.i),
            body,
        })
    }

    fn backtick(&mut self) -> Result<Part, String> {
        let start = self.i;
        self.i += 1;
        let mut inner = String::new();
        loop {
            match self.at(0) {
                None => return Err("unterminated backtick".into()),
                Some('`') => break,
                Some('\\') if matches!(self.at(1), Some('`' | '\\' | '$')) => {
                    inner.push(self.at(1).unwrap());
                    self.i += 2;
                    continue;
                }
                Some(c) => inner.push(c),
            }
            self.i += 1;
        }
        self.i += 1;
        let mut p = self.sub_parser(&inner)?;
        let body = p.list(End::Eof)?;
        if !matches!(p.next()?, Tok::Eof) {
            return Err("unbalanced backtick command".into());
        }
        Ok(Part::Subst {
            raw: self.slice(start, self.i),
            body,
        })
    }

    /// Skip from an opening bracket to its match, honouring quotes.
    fn skip_balanced(&mut self, open: char, close: char) -> Result<(), String> {
        let mut depth = 0usize;
        loop {
            match self.at(0) {
                None => return Err(format!("unbalanced `{open}`")),
                Some('\\') => self.i += 1,
                Some('\'') => {
                    self.i += 1;
                    while self.at(0).is_some_and(|c| c != '\'') {
                        self.i += 1;
                    }
                }
                Some('"') => {
                    self.i += 1;
                    while self.at(0).is_some_and(|c| c != '"') {
                        if self.at(0) == Some('\\') {
                            self.i += 1;
                        }
                        self.i += 1;
                    }
                }
                Some(c) if c == open => depth += 1,
                Some(c) if c == close => {
                    depth -= 1;
                    if depth == 0 {
                        self.i += 1;
                        return Ok(());
                    }
                }
                _ => {}
            }
            if self.at(0).is_none() {
                return Err(format!("unbalanced `{open}`"));
            }
            self.i += 1;
        }
    }

    fn list(&mut self, end: End) -> Result<List, String> {
        let mut list = Vec::new();
        loop {
            while matches!(self.peek()?, Tok::Newline | Tok::Op(";")) {
                self.next()?;
            }
            match self.peek()? {
                Tok::Eof => {
                    if end == End::Eof {
                        return Ok(list);
                    }
                    return Err("unexpected end of input".into());
                }
                Tok::Op(")") if end == End::Paren => return Ok(list),
                Tok::Op(";;" | ";&" | ";;&") if end == End::Case => return Ok(list),
                _ => {}
            }
            if (end == End::Brace && self.peek_plain("}")?)
                || (end == End::Case && self.peek_plain("esac")?)
            {
                return Ok(list);
            }
            let mut pipeline = self.pipeline()?;
            match self.peek()? {
                Tok::Op("&&" | "||" | ";") | Tok::Newline => {
                    self.next()?;
                }
                Tok::Op("&") => {
                    self.next()?;
                    pipeline.background = true;
                }
                _ => {}
            }
            list.push(pipeline);
        }
    }

    fn pipeline(&mut self) -> Result<Pipeline, String> {
        let mut stages = vec![self.command()?];
        while matches!(self.peek()?, Tok::Op("|" | "|&")) {
            self.next()?;
            self.skip_newlines()?;
            stages.push(self.command()?);
        }
        Ok(Pipeline {
            stages,
            background: false,
        })
    }

    fn command(&mut self) -> Result<Node, String> {
        loop {
            let reserved = match self.peek()? {
                Tok::Word(w) => w.plain().map(str::to_string),
                _ => None,
            };
            match reserved.as_deref() {
                Some("!" | "if" | "then" | "else" | "elif" | "do" | "while" | "until") => {
                    self.next()?;
                }
                Some("fi" | "done" | "esac") => {
                    self.next()?;
                    return Ok(Node::Simple(SimpleCmd::default()));
                }
                Some("for" | "select") => {
                    while !matches!(self.peek()?, Tok::Op(_) | Tok::Newline | Tok::Eof) {
                        self.next()?;
                    }
                    return Ok(Node::Simple(SimpleCmd::default()));
                }
                Some("case") => return self.case(),
                Some("function") => {
                    self.next()?;
                    self.next()?;
                    if matches!(self.peek()?, Tok::Op("(")) {
                        self.next()?;
                        self.expect_op(")")?;
                    }
                    return self.function_body();
                }
                Some("{") => {
                    self.next()?;
                    let body = self.list(End::Brace)?;
                    self.next()?;
                    let redirects = self.trailing_redirects()?;
                    return Ok(Node::Group {
                        subshell: false,
                        body,
                        redirects,
                    });
                }
                _ => break,
            }
        }
        if matches!(self.peek()?, Tok::Op("(")) {
            self.next()?;
            let body = self.list(End::Paren)?;
            self.expect_op(")")?;
            let redirects = self.trailing_redirects()?;
            return Ok(Node::Group {
                subshell: true,
                body,
                redirects,
            });
        }
        self.simple()
    }

    fn function_body(&mut self) -> Result<Node, String> {
        self.skip_newlines()?;
        let body = self.command()?;
        Ok(Node::Group {
            subshell: true,
            body: vec![Pipeline {
                stages: vec![body],
                background: false,
            }],
            redirects: Vec::new(),
        })
    }

    fn case(&mut self) -> Result<Node, String> {
        self.next()?;
        let subject = match self.next()? {
            Tok::Word(w) => w,
            t => {
                return Err(format!(
                    "expected a word after case, found {}",
                    t.describe()
                ));
            }
        };
        self.skip_newlines()?;
        if !self.peek_plain("in")? {
            return Err("expected `in` after case".into());
        }
        self.next()?;
        let mut body = vec![Pipeline {
            stages: vec![Node::Simple(SimpleCmd {
                words: vec![Word::lit(":"), subject],
                ..SimpleCmd::default()
            })],
            background: false,
        }];
        loop {
            self.skip_newlines()?;
            if self.peek_plain("esac")? {
                self.next()?;
                break;
            }
            if matches!(self.peek()?, Tok::Op("(")) {
                self.next()?;
            }
            loop {
                match self.next()? {
                    Tok::Op(")") => break,
                    Tok::Word(_) | Tok::Op("|") => {}
                    t => return Err(format!("bad case pattern near {}", t.describe())),
                }
            }
            body.extend(self.list(End::Case)?);
            if matches!(self.peek()?, Tok::Op(";;" | ";&" | ";;&")) {
                self.next()?;
            }
        }
        let redirects = self.trailing_redirects()?;
        Ok(Node::Group {
            subshell: false,
            body,
            redirects,
        })
    }

    fn redirect(&mut self, tok: Tok) -> Result<Redirect, String> {
        match tok {
            Tok::Here(target) => Ok(Redirect {
                op: RedirOp::Here,
                target,
            }),
            Tok::Redir(op) => {
                let target = match self.next()? {
                    Tok::Word(w) => w,
                    t => {
                        return Err(format!(
                            "expected a redirection target, found {}",
                            t.describe()
                        ));
                    }
                };
                // `>&file` writes a file; `>&2` and `>&-` only duplicate.
                let op = match (op, target.plain()) {
                    (RedirOp::Dup, Some(t))
                        if t == "-" || t.chars().all(|c| c.is_ascii_digit()) =>
                    {
                        RedirOp::Dup
                    }
                    (RedirOp::Dup, _) => RedirOp::Write,
                    (op, _) => op,
                };
                Ok(Redirect { op, target })
            }
            _ => unreachable!("redirect called on a non-redirection token"),
        }
    }

    fn trailing_redirects(&mut self) -> Result<Vec<Redirect>, String> {
        let mut out = Vec::new();
        while matches!(self.peek()?, Tok::Redir(_) | Tok::Here(_)) {
            let tok = self.next()?;
            out.push(self.redirect(tok)?);
        }
        Ok(out)
    }

    fn simple(&mut self) -> Result<Node, String> {
        let mut cmd = SimpleCmd::default();
        loop {
            match self.peek()? {
                Tok::Word(_) => {
                    let Tok::Word(w) = self.next()? else {
                        unreachable!()
                    };
                    if cmd.words.is_empty() && !self.dbracket && assignment_name(&w).is_some() {
                        if let Some(a) = split_assignment(w) {
                            cmd.assigns.push(a);
                        }
                        continue;
                    }
                    if self.dbracket && w.plain() == Some("]]") {
                        self.dbracket = false;
                    } else if cmd.words.is_empty() && w.plain() == Some("[[") {
                        self.dbracket = true;
                    }
                    cmd.words.push(w);
                    if cmd.words.len() == 1
                        && cmd.assigns.is_empty()
                        && !self.dbracket
                        && matches!(self.peek()?, Tok::Op("("))
                    {
                        self.next()?;
                        self.expect_op(")")?;
                        return self.function_body();
                    }
                }
                Tok::Redir(_) | Tok::Here(_) => {
                    let tok = self.next()?;
                    let r = self.redirect(tok)?;
                    cmd.redirects.push(r);
                }
                _ => break,
            }
        }
        if self.dbracket {
            return Err("unterminated `[[`".into());
        }
        if cmd.words.is_empty() && cmd.assigns.is_empty() && cmd.redirects.is_empty() {
            return Err(format!("unexpected {}", self.peek()?.describe()));
        }
        Ok(Node::Simple(cmd))
    }
}

fn push_lit(parts: &mut Vec<Part>, c: char, quoted: bool) {
    match (parts.last_mut(), quoted) {
        (Some(Part::Lit(s)), false) | (Some(Part::Quoted(s)), true) => s.push(c),
        _ => parts.push(if quoted {
            Part::Quoted(c.to_string())
        } else {
            Part::Lit(c.to_string())
        }),
    }
}

/// Quote a string so a shell reads it back as one word.
pub fn quote(s: &str) -> String {
    if !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./=:,+@%".contains(c))
    {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(src: &str) -> Vec<Vec<String>> {
        fn walk(list: &List, out: &mut Vec<Vec<String>>) {
            for p in list {
                for n in &p.stages {
                    match n {
                        Node::Simple(c) => {
                            for w in &c.words {
                                for part in &w.parts {
                                    if let Part::Subst { body, .. } = part {
                                        walk(body, out);
                                    }
                                }
                            }
                            out.push(c.words.iter().map(|w| w.raw.clone()).collect());
                        }
                        Node::Group { body, .. } => walk(body, out),
                    }
                }
            }
        }
        let mut out = Vec::new();
        walk(&parse(src).unwrap(), &mut out);
        out.retain(|c| !c.is_empty());
        out
    }

    #[test]
    fn splits_chains_and_nesting() {
        assert_eq!(
            words("a 1 && (b; { c; }) | d || e & f\ng"),
            vec![
                vec!["a", "1"],
                vec!["b"],
                vec!["c"],
                vec!["d"],
                vec!["e"],
                vec!["f"],
                vec!["g"]
            ]
        );
        assert_eq!(
            words("x $(y `z`)"),
            vec![vec!["z"], vec!["y", "`z`"], vec!["x", "$(y `z`)"]]
        );
    }

    #[test]
    fn heredoc_inside_substitution() {
        let src = "git commit -m \"$(cat <<'EOF'\nfix: a ) b\n\nEOF\n)\" && git push";
        let w = words(src);
        assert_eq!(w[0], vec!["cat"]);
        assert_eq!(w[1][0], "git");
        assert_eq!(w[2], vec!["git", "push"]);
    }

    #[test]
    fn unbalanced_quotes_fail() {
        assert!(parse("echo 'abc").is_err());
        assert!(parse("echo \"abc").is_err());
        assert!(parse("echo $(abc").is_err());
        assert!(parse("(echo").is_err());
    }

    #[test]
    fn case_and_functions_parse() {
        assert_eq!(
            words("case $x in a|b) one;; *) two;; esac; f() { three; }; [[ -f x && y ]]"),
            vec![
                vec![":", "$x"],
                vec!["one"],
                vec!["two"],
                vec!["three"],
                vec!["[[", "-f", "x", "&&", "y", "]]"]
            ]
        );
    }
}
