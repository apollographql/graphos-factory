//! Reconcile: the delta between what the schema does, what selection.yaml
//! asks for, and what inventory.json says the API offers.
//!
//! A REPORT, never an edit. Spans listed under `overrides:` carry an
//! engineer's intent: their drift from the selection is reported in its own
//! section and never counted as work to do, and their assertions are
//! checked against the current text. Spans are byte offsets into the schema
//! file as written, so `--baseline` can say "this span's text is
//! byte-for-byte what it was at <rev>" and list every span that moved. With
//! an `applied.lock.yaml` the report also names every span a human changed
//! since the agent last wrote the schema (see `spans.rs`).

use crate::graphql::{blank, directives, line_of, type_body, type_declarations};
use crate::json::{get, get_arr, get_obj, get_str, obj, truthy, Object};
use regex::Regex;
use serde_json::Value;
use std::collections::HashSet;
use std::sync::OnceLock;

const METHODS: [&str; 8] = [
    "GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS", "TRACE",
];

#[derive(Debug, Clone)]
pub struct Connect {
    pub method: Option<String>,
    pub path: Option<String>,
    pub selection: Option<String>,
    pub line: usize,
}

#[derive(Debug, Clone)]
pub struct FieldSpan {
    pub name: String,
    pub start: usize,
    pub decl_start: usize,
    pub end: usize,
    pub line: usize,
    pub text: String,
    pub decl: String,
    pub args: Vec<String>,
    pub tags: Vec<String>,
    pub connect: Option<Connect>,
}

fn is_ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_'
}
fn is_ident(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

/// Field spans of `type <root> { … }`, in declaration order. Each span runs
/// from the end of the previous field's declaration to the end of this one.
pub fn field_spans(sdl: &str, root: &str) -> Vec<FieldSpan> {
    let block = match type_body(sdl, root) {
        Some(b) => b,
        None => return vec![],
    };
    let body = block.body;
    let start = block.start;
    let code = blank(&body);
    let cb = code.as_bytes();
    let mut starts: Vec<(String, usize)> = Vec::new();
    let mut depth = 0i32;
    let mut i = 0;
    while i < cb.len() {
        let ch = cb[i];
        if matches!(ch, b'(' | b'{' | b'[') {
            depth += 1;
        } else if matches!(ch, b')' | b'}' | b']') {
            depth -= 1;
        } else if depth == 0
            && is_ident_start(ch)
            && (i == 0 || !(is_ident(cb[i - 1]) || cb[i - 1] == b'@'))
        {
            let mut j = i as i64 - 1;
            while j >= 0 && cb[j as usize].is_ascii_whitespace() {
                j -= 1;
            }
            let prev = if j >= 0 { cb[j as usize] } else { 0 };
            let mut end = i;
            while end < cb.len() && is_ident(cb[end]) {
                end += 1;
            }
            let name = code[i..end].to_string();
            if prev != b':' {
                starts.push((name, i));
            }
            i = end;
            continue;
        }
        i += 1;
    }
    let mut spans: Vec<FieldSpan> = Vec::new();
    for k in 0..starts.len() {
        let from = if k == 0 { 0 } else { spans[k - 1].end - start };
        let limit = if k + 1 < starts.len() {
            starts[k + 1].1
        } else {
            cb.len()
        };
        let mut end = limit;
        while end > starts[k].1 && cb[end - 1].is_ascii_whitespace() {
            end -= 1;
        }
        let text = body[from..end].to_string();
        let decl = body[starts[k].1..end].to_string();
        spans.push(FieldSpan {
            name: starts[k].0.clone(),
            start: start + from,
            decl_start: start + starts[k].1,
            end: start + end,
            line: line_of(sdl, start + starts[k].1),
            args: argument_names(&decl),
            tags: tag_names(&decl),
            connect: connect_of(&decl),
            text,
            decl,
        });
    }
    spans
}

fn argument_names(decl: &str) -> Vec<String> {
    let code = blank(decl);
    let cb = code.as_bytes();
    let mut name_end = 0;
    while name_end < cb.len() && is_ident(cb[name_end]) {
        name_end += 1;
    }
    let open = match code.find('(') {
        Some(o) => o,
        None => return vec![],
    };
    if code[name_end..open].chars().any(|c| !c.is_whitespace()) {
        return vec![];
    }
    let mut depth = 0i32;
    let mut close = None;
    for (idx, &c) in cb.iter().enumerate().skip(open) {
        if c == b'(' {
            depth += 1;
        } else if c == b')' {
            depth -= 1;
            if depth == 0 {
                close = Some(idx);
                break;
            }
        }
    }
    let close = match close {
        Some(c) => c,
        None => return vec![],
    };
    let inner = &code[open + 1..close];
    let ib = inner.as_bytes();
    // blank() keeps offsets, so the original bytes line up with `ib`. A byte
    // blanked to a space that was not whitespace is inside a string or a
    // comment: a string default or a description ended there.
    let ob = &decl.as_bytes()[open + 1..close];
    let blanked = |p: usize| ib[p] == b' ' && !ob[p].is_ascii_whitespace();
    let mut names = Vec::new();
    depth = 0;
    let mut i = 0;
    while i < ib.len() {
        let ch = ib[i];
        if matches!(ch, b'(' | b'[' | b'{') {
            depth += 1;
        } else if matches!(ch, b')' | b']' | b'}') {
            depth -= 1;
        } else if depth == 0
            && is_ident_start(ch)
            && (i == 0 || !(is_ident(ib[i - 1]) || ib[i - 1] == b'@'))
        {
            let mut end = i;
            while end < ib.len() && is_ident(ib[end]) {
                end += 1;
            }
            let word = &inner[i..end];
            let mut k = end;
            while k < ib.len() && ib[k].is_ascii_whitespace() {
                k += 1;
            }
            let followed_by_colon = k < ib.len() && ib[k] == b':';
            let mut j = i as i64 - 1;
            while j >= 0 && ib[j as usize].is_ascii_whitespace() && !blanked(j as usize) {
                j -= 1;
            }
            // Reaching a string or comment means the previous argument's
            // value (or this one's description) ended there, so the word
            // starts an argument; without this a string default's `=` read
            // as this word's own `=` and the argument was dropped.
            let prev = if j >= 0 && !blanked(j as usize) {
                ib[j as usize]
            } else {
                0
            };
            if followed_by_colon && prev != b':' && prev != b'=' {
                names.push(word.to_string());
            }
            i = end;
            continue;
        }
        i += 1;
    }
    names
}

fn tag_names(decl: &str) -> Vec<String> {
    let re = Regex::new(r#"name\s*:\s*"([^"]*)""#).unwrap();
    directives(decl, "tag")
        .iter()
        .filter_map(|d| re.captures(&d.args).map(|m| m[1].to_string()))
        .collect()
}

fn connect_of(decl: &str) -> Option<Connect> {
    let d = directives(decl, "connect").into_iter().next()?;
    let re = Regex::new(&format!(r#"\b({})\s*:\s*"([^"]*)""#, METHODS.join("|"))).unwrap();
    let http = re.captures(&d.args);
    Some(Connect {
        method: http.as_ref().map(|m| m[1].to_string()),
        path: http.as_ref().map(|m| connector_path(&m[2])),
        selection: string_arg(&d.args, "selection"),
        line: d.line,
    })
}

/// A connector URI as the inventory keys it: a literal query string
/// (`/query?q=SELECT+Id+FROM+Account`, no `{…}` expression in it) is part of
/// the path, since an inventory can key an operation by it (Salesforce's SOQL
/// lists, ADR 0054). A templated one (`/x?id={$args.id}`) is stripped, as the
/// inventory keys the path alone.
fn connector_path(uri: &str) -> String {
    match uri.split_once('?') {
        Some((path, query)) if query.contains('{') => path.to_string(),
        _ => uri.to_string(),
    }
}

/// The string value of `key: "…"` or `key: """…"""` inside raw directive args.
pub(crate) fn string_arg(args: &str, key: &str) -> Option<String> {
    let code = blank(args);
    let re = Regex::new(&format!(r"\b{}\s*:", regex::escape(key))).unwrap();
    let ab = args.as_bytes();
    for m in re.find_iter(&code) {
        let mut i = m.end();
        while i < ab.len() && ab[i].is_ascii_whitespace() {
            i += 1;
        }
        if args[i..].starts_with("\"\"\"") {
            return args[i + 3..]
                .find("\"\"\"")
                .map(|e| args[i + 3..i + 3 + e].to_string());
        }
        if ab.get(i) == Some(&b'"') {
            let mut j = i + 1;
            while j < ab.len() && ab[j] != b'"' {
                j += if ab[j] == b'\\' { 2 } else { 1 };
            }
            return Some(args[i + 1..j.min(ab.len())].to_string());
        }
    }
    None
}

#[derive(Debug, Clone)]
pub struct TypeConnector {
    pub type_name: String,
    pub line: usize,
    pub connect: Connect,
    pub keys: Vec<Option<String>>,
    pub text: String,
}

/// Type-level connectors: `type X @key(...) @connect(...) { … }` (entities).
pub fn type_connectors(sdl: &str) -> Vec<TypeConnector> {
    let code = blank(sdl);
    let cb = code.as_bytes();
    let key_re = Regex::new(r#"fields\s*:\s*"([^"]*)""#).unwrap();
    let mut out = Vec::new();
    for decl in type_declarations(sdl) {
        if decl.kind != "type" {
            continue;
        }
        let mut open = None;
        let mut depth = 0i32;
        for (i, &c) in cb.iter().enumerate().skip(decl.index) {
            if c == b'(' {
                depth += 1;
            } else if c == b')' {
                depth -= 1;
            } else if c == b'{' && depth == 0 {
                open = Some(i);
                break;
            }
        }
        let open = match open {
            Some(o) => o,
            None => continue,
        };
        let header = &sdl[decl.index..open];
        let connect = match connect_of(header) {
            Some(c) => c,
            None => continue,
        };
        let keys = directives(header, "key")
            .iter()
            .map(|k| key_re.captures(&k.args).map(|m| m[1].to_string()))
            .collect();
        out.push(TypeConnector {
            type_name: decl.name.clone(),
            line: decl.line,
            connect,
            keys,
            text: header.to_string(),
        });
    }
    out
}

/// A field-level relationship connector (ADR 0069): a field inside a
/// non-root type whose `@connect` is the canonical by-id read of the
/// parent's foreign key — `GET …/{$this.<fk>}` (`link_template`). No
/// `@key`, no type-level connector: one request per parent, resolved by the
/// by-id operation the `links:` entry names.
#[derive(Debug, Clone)]
pub struct LinkConnector {
    pub type_name: String,
    pub field: String,
    pub span: FieldSpan,
    /// Every `<fk>` the path reads through `{$this.<fk>}`, in path order.
    pub this_vars: Vec<String>,
}

/// A field-level connector on a non-root type that is not a link connector
/// (ADR 0069, R43): a sub-resource read (`/repos/{$this.owner}/{$this.name}
/// /issues`), a read past the parent's id (`/x/{$this.id}/receipt`), a write,
/// or a path with no `{$this.}` at all. Nothing in `links:` declares one — a
/// sub-resource GET is never a link target — so reconcile lists it and
/// checks nothing else.
#[derive(Debug, Clone)]
pub struct FieldConnector {
    pub type_name: String,
    pub field: String,
    pub span: FieldSpan,
}

/// Every field carrying a `@connect` on a non-root type, in declaration
/// order. `field_spans` is generic over the type name; `Query`, `Mutation`
/// and `Subscription` are the roots reconcile already walks, and a type
/// declared twice (`extend type`) is walked once, since `type_body` reads
/// the first declaration.
fn type_field_connectors(sdl: &str) -> Vec<FieldConnector> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    for decl in type_declarations(sdl) {
        if decl.kind != "type"
            || matches!(decl.name.as_str(), "Query" | "Mutation" | "Subscription")
            || !seen.insert(decl.name.clone())
        {
            continue;
        }
        for span in field_spans(sdl, &decl.name) {
            if span.connect.is_some() {
                out.push(FieldConnector {
                    type_name: decl.name.clone(),
                    field: span.name.clone(),
                    span,
                });
            }
        }
    }
    out
}

/// The final segment of a path template, the one a by-id read keys on: the
/// query string dropped and one trailing `/` trimmed, so `/deck/{deck_id}/`
/// (deck-of-cards' own spelling) ends in `{deck_id}`, as `/albums/{id}` does.
fn final_segment(path: &str) -> &str {
    let path = path.split('?').next().unwrap_or(path);
    let path = path.strip_suffix('/').unwrap_or(path);
    path.rsplit('/').next().unwrap_or(path)
}

/// Is `method path` the canonical GET-by-id template a link resolves
/// through (ADR 0069, R43)? `GET`, exactly one `{$this.<fk>}` in the path,
/// and that occurrence the whole final segment (`final_segment`), or the
/// segment followed only by an extension: `/albums/{$this.album_id}`,
/// `/deck/{$this.deck_id}/` and `/item/{$this.id}.json` are one;
/// `/repos/{$this.owner}/{$this.name}/issues`, `/x/{$this.id}/receipt`,
/// `/x/{$this.id}-raw` and `POST /x/{$this.id}` are not.
fn link_template(method: Option<&str>, path: Option<&str>) -> bool {
    static FINAL_RE: OnceLock<Regex> = OnceLock::new();
    let (Some("GET"), Some(path)) = (method, path) else {
        return false;
    };
    let bare = path.split('?').next().unwrap_or(path);
    bare.matches("{$this.").count() == 1
        && FINAL_RE
            .get_or_init(|| {
                Regex::new(r"^\{\$this\.[A-Za-z_][A-Za-z0-9_]*\}(?:\.[A-Za-z0-9]+)?$").unwrap()
            })
            .is_match(final_segment(path))
}

/// Every field-level link connector on a non-root type: the connectors
/// whose `@connect` is a GET-by-id template (`link_template`).
pub fn link_connectors(sdl: &str) -> Vec<LinkConnector> {
    static THIS_RE: OnceLock<Regex> = OnceLock::new();
    let this_re =
        THIS_RE.get_or_init(|| Regex::new(r"\{\$this\.([A-Za-z_][A-Za-z0-9_]*)\}").unwrap());
    type_field_connectors(sdl)
        .into_iter()
        .filter_map(|fc| {
            let c = fc.span.connect.as_ref()?;
            if !link_template(c.method.as_deref(), c.path.as_deref()) {
                return None;
            }
            let this_vars = this_re
                .captures_iter(c.path.as_deref().unwrap_or(""))
                .map(|m| m[1].to_string())
                .collect();
            Some(LinkConnector {
                type_name: fc.type_name,
                field: fc.field,
                span: fc.span,
                this_vars,
            })
        })
        .collect()
}

/// Every other field-level connector on a non-root type (R43): the ones
/// `link_connectors` leaves out.
pub fn field_connectors(sdl: &str) -> Vec<FieldConnector> {
    type_field_connectors(sdl)
        .into_iter()
        .filter(|fc| {
            let c = fc.span.connect.as_ref();
            !link_template(
                c.and_then(|c| c.method.as_deref()),
                c.and_then(|c| c.path.as_deref()),
            )
        })
        .collect()
}

/// The template a link's field-level connector reads: the by-id
/// operation's path with its parameter rewritten to `{$this.<fk>}`. Without
/// a recorded parameter the final `{p}` segment (`final_segment`, so
/// `/deck/{deck_id}/` names `deck_id`), bare or followed by an extension
/// (`/item/{id}.json`), is the parameter.
pub fn link_path_template(op_path: &str, parameter: Option<&str>, fk: &str) -> String {
    let param = parameter
        .map(str::to_string)
        .or_else(|| {
            final_segment(op_path)
                .strip_prefix('{')
                .and_then(|s| s.split_once('}'))
                .filter(|(_, rest)| rest.is_empty() || rest.starts_with('.'))
                .map(|(p, _)| p.to_string())
        })
        .unwrap_or_default();
    op_path.replace(&format!("{{{}}}", param), &format!("{{$this.{}}}", fk))
}

// ─── Selection-string reading ────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct Node {
    pub alias: Option<String>,
    pub key: Option<Vec<String>>,
    pub rooted: bool,
    pub methods: Vec<String>,
    pub children: Option<Vec<Node>>,
    pub opaque: bool,
    /// A literal-object value's own top-level key names (e.g. `["id"]` for
    /// `{ id: interviewId }`), captured but not otherwise parsed — this
    /// node stays `opaque` with no `children`, but a consumer that needs a
    /// real GraphQL sub-selection (`crate::cmd::scaffold`'s
    /// `render_selection`) can reconstruct a minimal one (`{ id }`) from
    /// this instead of refusing outright. Empty for every other opaque
    /// shape ($args.foo, $(...), a string, a number), which have no keys
    /// to offer.
    pub literal_keys: Vec<String>,
    /// The later operands of a `??`/`?!` chain (`a: x ?? y`), each parsed as
    /// its own node. They are reads, not output fields: a reader of what the
    /// connector consumes (`resolve_paths`, `source-coverage`, `sparse`)
    /// walks them beside this node, and a reader of what the schema outputs
    /// (lint, scaffold) leaves them alone. Empty without a chain (ADR 0057).
    pub fallbacks: Vec<Node>,
    /// Set on a node written `... expr` (ADR 0058). `Spread::Match` is the
    /// abstract-type idiom, `... disc->match([wire, { __typename: "T", … }],
    /// …)`: the node itself stays the discriminator's `->match` node, and
    /// each arm carries the fields it adds. `Spread::Unparsed` is every other
    /// spread: the node has no key, no children and is opaque, so a reader
    /// that does not look here skips it, and one that does reports it.
    pub spread: Option<Spread>,
    /// Set on `->map(@->match(…))`: the map's body is exactly one `->match`
    /// on the element, so each element's value is translated and nothing
    /// else is read (the element-wise form of ADR 0050's `->match` rule).
    pub element_match: bool,
}

/// What a `...` spread contributes to the object it sits in.
#[derive(Debug, Clone)]
pub enum Spread {
    Match(Vec<SpreadArm>),
    /// Why the spread is not the `->match` form, for an `unresolved` reason.
    Unparsed(String),
}

/// One object-valued arm of a spread's `->match`. An arm whose value is
/// `null` (`[@, null]`) adds nothing and is not recorded.
#[derive(Debug, Clone, Default)]
pub struct SpreadArm {
    /// The arm's candidate when it is a string literal (`"book"`); `None`
    /// for `@` and anything else.
    pub candidate: Option<String>,
    /// The arm object's literal `__typename` (`"Book"` or `$("Book")`);
    /// `None` when it has none or computes one, which does not compose.
    pub typename: Option<String>,
    /// The arm object's fields, `__typename` left out: they read from the
    /// object the spread sits in.
    pub children: Vec<Node>,
}

/// Drop `#` comments, which run to the end of the line. A string opens on
/// either quote and closes only on the same one, honouring `\` escapes, so
/// a `#` in `'Issue #1'` stays and a `"` in `'5" wide'` opens nothing.
pub(crate) fn strip_comments(text: &str) -> String {
    let mut out = String::new();
    let mut in_str: Option<char> = None;
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];
        if let Some(q) = in_str {
            out.push(ch);
            if ch == '\\' {
                if let Some(n) = chars.get(i + 1) {
                    out.push(*n);
                }
                i += 1;
            } else if ch == q {
                in_str = None;
            }
        } else if ch == '"' || ch == '\'' {
            in_str = Some(ch);
            out.push(ch);
        } else if ch == '#' {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            out.push('\n');
        } else {
            out.push(ch);
        }
        i += 1;
    }
    out
}

struct Parser {
    src: Vec<char>,
    i: usize,
    /// The argument text (between the parentheses) of the last `->match(…)`
    /// the method loop skipped, for a spread to read its arms from.
    last_match_args: Option<(usize, usize)>,
}

impl Parser {
    fn peek(&self) -> Option<char> {
        self.src.get(self.i).copied()
    }
    fn skip_ws(&mut self) {
        while self.i < self.src.len() && self.src[self.i].is_whitespace() {
            self.i += 1;
        }
    }
    fn ident(&mut self) -> Option<String> {
        let start = self.i;
        if !self
            .peek()
            .map(|c| c.is_ascii_alphabetic() || c == '_')
            .unwrap_or(false)
        {
            return None;
        }
        while self
            .peek()
            .map(|c| c.is_ascii_alphanumeric() || c == '_')
            .unwrap_or(false)
        {
            self.i += 1;
        }
        Some(self.src[start..self.i].iter().collect())
    }
    fn skip_balanced(&mut self, open: char, close: char) {
        let mut depth = 0;
        while self.i < self.src.len() {
            let ch = self.src[self.i];
            if ch == '"' || ch == '\'' {
                self.i += 1;
                while self.i < self.src.len() && self.src[self.i] != ch {
                    self.i += if self.src[self.i] == '\\' { 2 } else { 1 };
                }
            } else if ch == open {
                depth += 1;
            } else if ch == close {
                depth -= 1;
                if depth == 0 {
                    self.i += 1;
                    return;
                }
            }
            self.i += 1;
        }
    }
    /// Like `skip_balanced`, but also collects every `identifier:` found
    /// at depth 1 (immediately inside `open`, not nested any deeper) as a
    /// key -- the literal object's own top-level field names, its value
    /// expressions skipped over the same way `skip_balanced` skips
    /// anything else. Assumes `self.i` is at the opening `open`.
    fn skip_balanced_capturing_top_level_keys(&mut self, open: char, close: char) -> Vec<String> {
        let mut depth = 0;
        let mut keys = Vec::new();
        while self.i < self.src.len() {
            let ch = self.src[self.i];
            if ch == '"' {
                self.i += 1;
                while self.i < self.src.len() && self.src[self.i] != '"' {
                    self.i += if self.src[self.i] == '\\' { 2 } else { 1 };
                }
                self.i += 1;
                continue;
            } else if ch == open {
                depth += 1;
            } else if ch == close {
                depth -= 1;
                if depth == 0 {
                    self.i += 1;
                    return keys;
                }
            } else if depth == 1 && (ch.is_ascii_alphabetic() || ch == '_') {
                let start = self.i;
                while self
                    .peek()
                    .map(|c| c.is_ascii_alphanumeric() || c == '_')
                    .unwrap_or(false)
                {
                    self.i += 1;
                }
                let word: String = self.src[start..self.i].iter().collect();
                let mut j = self.i;
                while j < self.src.len() && self.src[j].is_whitespace() {
                    j += 1;
                }
                if self.src.get(j) == Some(&':') {
                    keys.push(word);
                }
                continue;
            }
            self.i += 1;
        }
        keys
    }
    /// Is `self.i` at a `?` that marks a path step optional (`owner?.name`,
    /// `size?->jsonStringify`, a closing `name?`) rather than opening a
    /// `??`/`?!` operator?
    fn at_optional_marker(&self) -> bool {
        self.peek() == Some('?') && !matches!(self.src.get(self.i + 1), Some('?') | Some('!'))
    }
    fn path_segments(&mut self, first: Option<String>) -> Vec<String> {
        let mut segs: Vec<String> = first.into_iter().collect();
        loop {
            // `?` may follow the head and every step (`PathTail ::= "?"?
            // (PathStep "?"?)*`). It changes what a missing step returns,
            // never which key is read, so `owner?.name` is the path
            // `owner.name` (ADR 0057).
            if self.at_optional_marker() {
                self.i += 1;
            }
            if self.peek() != Some('.') {
                break;
            }
            // A quoted segment (`."@context"`, `."@odata.nextLink"`) is one
            // key, quotes stripped — a JSON-LD/OData field name can carry
            // `@`, `-` and even an embedded `.` that must not itself split
            // the path. The unquoted branch below still requires an
            // identifier-shaped start, so a `.` before punctuation it does
            // not recognise (neither a quote nor an identifier char) simply
            // ends the path, same as before.
            if self.src.get(self.i + 1) == Some(&'"') {
                self.i += 2; // consume '.' and the opening '"'
                let start = self.i;
                while self.i < self.src.len() && self.src[self.i] != '"' {
                    self.i += if self.src[self.i] == '\\' { 2 } else { 1 };
                }
                segs.push(self.src[start..self.i.min(self.src.len())].iter().collect());
                if self.i < self.src.len() {
                    self.i += 1; // consume the closing '"'
                }
                continue;
            }
            let starts_segment = self
                .src
                .get(self.i + 1)
                .map(|c| c.is_ascii_alphabetic() || *c == '_' || *c == '*')
                .unwrap_or(false);
            if !starts_segment {
                break;
            }
            self.i += 1;
            if self.peek() == Some('*') {
                self.i += 1;
                segs.push("*".to_string());
            } else if let Some(id) = self.ident() {
                segs.push(id);
            }
        }
        segs
    }
    fn starts_with(&self, s: &str) -> bool {
        let sc: Vec<char> = s.chars().collect();
        self.src.len() >= self.i + sc.len() && self.src[self.i..self.i + sc.len()] == sc[..]
    }

    fn parse_items(&mut self, closer: Option<char>) -> Vec<Node> {
        let mut items = Vec::new();
        loop {
            self.skip_ws();
            let ch = match self.peek() {
                Some(c) => c,
                None => return items,
            };
            if Some(ch) == closer {
                self.i += 1;
                return items;
            }
            if ch == '}' || ch == ')' {
                self.i += 1;
                continue;
            }
            let before = self.i;
            match self.parse_item() {
                Some(item) => items.push(item),
                None => {
                    if self.i == before {
                        self.i += 1;
                    }
                }
            }
        }
    }

    /// `... expr`, with the `...` already consumed (ADR 0058).
    fn parse_spread(&mut self) -> Node {
        self.skip_ws();
        self.last_match_args = None;
        let start = self.i;
        let inner = if self.starts_with("...") {
            None
        } else {
            self.parse_item()
        };
        if self.i == start {
            self.i += 1;
        }
        let args = self.last_match_args.take();
        let spread = match (inner, args) {
            (Some(n), Some((a, b))) if n.methods == ["match"] && n.children.is_none() => {
                match parse_arms(&self.src[a..b]) {
                    Ok(arms) => {
                        let mut n = n;
                        n.spread = Some(Spread::Match(arms));
                        return n;
                    }
                    Err(why) => why,
                }
            }
            (Some(n), _) if !n.methods.is_empty() => format!(
                "a `...` spread of `->{}`, not of `->match` arms",
                n.methods.join("->")
            ),
            (Some(n), _) if n.children.is_some() => "a `...` spread of a sub-selection".to_string(),
            _ => "a `...` spread of an expression".to_string(),
        };
        Node {
            opaque: true,
            spread: Some(Spread::Unparsed(spread)),
            ..Node::default()
        }
    }

    fn parse_item(&mut self) -> Option<Node> {
        if self.starts_with("...") {
            self.i += 3;
            return Some(self.parse_spread());
        }
        let mut node = Node::default();
        let mut word = self.ident();
        self.skip_ws();
        // A quoted alias (`"100x100": _100x100`, or single-quoted) names an
        // output key that is not an identifier. On the left of `:` a quoted
        // string is a key at every connect version; only a quoted value
        // flips meaning.
        if let Some(q) = self
            .peek()
            .filter(|c| word.is_none() && (*c == '"' || *c == '\''))
        {
            let start = self.i;
            self.i += 1;
            while self.i < self.src.len() && self.src[self.i] != q {
                self.i += if self.src[self.i] == '\\' { 2 } else { 1 };
            }
            let quoted: String = self.src[start + 1..self.i.min(self.src.len())]
                .iter()
                .collect();
            self.i += 1;
            self.skip_ws();
            if self.peek() == Some(':') && self.src.get(self.i + 1) != Some(&':') {
                word = Some(quoted);
            } else {
                self.i = start;
            }
        }
        if word.is_some() && self.peek() == Some(':') && self.src.get(self.i + 1) != Some(&':') {
            self.i += 1;
            self.skip_ws();
            node.alias = word.take();
        }
        match word {
            None => {
                let ch = self.peek()?;
                if ch == '$' {
                    self.i += 1;
                    if self.peek() == Some('(') {
                        self.skip_balanced('(', ')');
                        node.opaque = true;
                    } else if self.ident().is_some() {
                        node.opaque = true; // $args / $this / $config / $env
                        self.path_segments(None);
                    } else {
                        node.rooted = true;
                        node.key = Some(self.path_segments(None));
                    }
                } else if ch == '"' || ch == '\'' {
                    self.i += 1;
                    while self.i < self.src.len() && self.src[self.i] != ch {
                        self.i += if self.src[self.i] == '\\' { 2 } else { 1 };
                    }
                    self.i += 1;
                    node.opaque = true;
                } else if ch.is_ascii_digit() || ch == '-' {
                    while self
                        .peek()
                        .map(|c| c.is_ascii_digit() || matches!(c, '.' | '-' | 'e' | 'E'))
                        .unwrap_or(false)
                    {
                        self.i += 1;
                    }
                    node.opaque = true;
                } else if ch == '@' {
                    self.i += 1;
                    node.opaque = true;
                } else if ch == '{' {
                    // A literal object value (the D-0031/R3
                    // entity-reference-stub idiom, e.g. `primaryLocation:
                    // { id: primaryLocationId }`) is opaque data, not a
                    // nested selection -- skip it as one balanced group
                    // (the same skip_balanced already used for a
                    // method's own `(...)` arguments) instead of falling
                    // through to the ident() fallback below, which
                    // cannot start at `{`: it failed silently, and the
                    // caller's own "stuck, force-advance by one" recovery
                    // then parsed the literal's own inner fields as flat
                    // siblings of whatever list contained this value, and
                    // consumed the literal's closing brace as if it were
                    // that list's own -- corrupting every field
                    // genuinely written after it. Its own top-level keys
                    // are captured (not otherwise parsed) so a consumer
                    // that needs a real sub-selection, not just
                    // reachability, can reconstruct a minimal one instead
                    // of treating every literal-object stub alike.
                    if node.alias.is_some() {
                        // `alias: { … }` is a JSONSelection `Alias SubSelection`:
                        // a read, whose fields resolve in the current shape
                        // context (`manager: { id: manager_id }` reads
                        // `manager_id` beside it). A literal object needs
                        // `$({ … })`, handled above. Parse the group as
                        // children; their aliases stay available as
                        // `literal_keys` for a consumer that wants only the
                        // output keys (scaffold's minimal selection).
                        let start = self.i;
                        node.literal_keys = self.skip_balanced_capturing_top_level_keys('{', '}');
                        self.i = start + 1;
                        node.children = Some(self.parse_items(Some('}')));
                    } else {
                        node.literal_keys = self.skip_balanced_capturing_top_level_keys('{', '}');
                        node.opaque = true;
                    }
                } else {
                    let w = self.ident()?;
                    node.key = Some(self.path_segments(Some(w)));
                }
            }
            Some(w) => {
                node.key = Some(self.path_segments(Some(w)));
            }
        }
        loop {
            self.skip_ws();
            // `?->` is `->` guarded against a `null` receiver; the `?` was
            // consumed by `path_segments` or by the tail read below.
            if self.starts_with("->") {
                self.i += 2;
                self.skip_ws();
                if let Some(m) = self.ident() {
                    node.methods.push(m);
                }
                self.skip_ws();
                if self.peek() == Some('(') {
                    let open = self.i;
                    self.skip_balanced('(', ')');
                    let close = self.i.saturating_sub(1).max(open + 1);
                    match node.methods.last().map(String::as_str) {
                        Some("match") => self.last_match_args = Some((open + 1, close)),
                        Some("map") => {
                            let body: String = self.src[open + 1..close].iter().collect();
                            node.element_match = is_element_match(&body);
                        }
                        _ => {}
                    }
                }
                // A path tail after a method (`errors?->first?.message`)
                // reads into the method's result: the node is already
                // opaque, so the steps are consumed rather than recorded,
                // and they no longer fall out as a sibling field.
                self.path_segments(None);
                continue;
            }
            break;
        }
        self.skip_ws();
        if self.peek() == Some('{') {
            self.i += 1;
            node.children = Some(self.parse_items(Some('}')));
        }
        // `a: x ?? y ?! z`: the value falls back to a later operand when the
        // first is null or missing. The node keeps the first operand's key;
        // the rest are reads recorded in `fallbacks`, a chain flattened.
        loop {
            self.skip_ws();
            if !(self.starts_with("??") || self.starts_with("?!")) {
                break;
            }
            self.i += 2;
            self.skip_ws();
            let before = self.i;
            match self.parse_item() {
                Some(mut operand) => {
                    let rest = std::mem::take(&mut operand.fallbacks);
                    node.fallbacks.push(operand);
                    node.fallbacks.extend(rest);
                }
                None => {
                    if self.i == before {
                        break;
                    }
                }
            }
        }
        // A node carrying a method is opaque, always. `lint::walk_enums` and
        // `lint::walk_int_overflow` skip a mapped leaf by testing `opaque`
        // alone and lean on this line being total; narrowing it — promoting
        // only some methods — silently makes both rules report mapped leaves
        // (ADR 0030). `tests/integration/reconcile.rs` pins it.
        if !node.methods.is_empty() {
            node.opaque = true;
        }
        Some(node)
    }
}

/// Parse a connector `selection` string into a tree, tolerantly.
pub fn parse_selection(text: &str) -> Vec<Node> {
    let mut p = Parser {
        src: strip_comments(text).chars().collect(),
        i: 0,
        last_match_args: None,
    };
    p.parse_items(None)
}

/// `@->match(…)` (or `@?->match(…)`) and nothing else: the body of a
/// `->map` that translates each element's value.
fn is_element_match(body: &str) -> bool {
    let Some(rest) = body.trim().strip_prefix('@') else {
        return false;
    };
    let rest = rest.trim_start();
    let rest = rest.strip_prefix('?').unwrap_or(rest);
    let Some(rest) = rest.strip_prefix("->") else {
        return false;
    };
    let Some(rest) = rest.trim_start().strip_prefix("match") else {
        return false;
    };
    let rest = rest.trim_start();
    if !rest.starts_with('(') {
        return false;
    }
    // The `(` must close at the very end: `@->match(…)->first` is a chain.
    let mut depth = 0i32;
    let mut in_str: Option<char> = None;
    let mut escaped = false;
    for (i, c) in rest.char_indices() {
        if let Some(q) = in_str {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == q {
                in_str = None;
            }
            continue;
        }
        match c {
            '"' | '\'' => in_str = Some(c),
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => {
                depth -= 1;
                if depth == 0 {
                    return rest[i + 1..].trim().is_empty();
                }
            }
            _ => {}
        }
    }
    false
}

/// Split `src` at the top-level commas: outside strings (either quote) and
/// outside any `(`, `[` or `{`.
fn split_top_level(src: &[char]) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    let mut i = 0;
    while i < src.len() {
        let ch = src[i];
        cur.push(ch);
        if let Some(q) = quote {
            if ch == '\\' {
                if let Some(n) = src.get(i + 1) {
                    cur.push(*n);
                }
                i += 1;
            } else if ch == q {
                quote = None;
            }
        } else {
            match ch {
                '"' | '\'' => quote = Some(ch),
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' => depth -= 1,
                ',' if depth == 0 => {
                    cur.pop();
                    out.push(std::mem::take(&mut cur));
                }
                _ => {}
            }
        }
        i += 1;
    }
    out.push(cur);
    out.into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// A string literal's contents, either quote, or `None` when `s` is not one.
fn string_literal(s: &str) -> Option<String> {
    let s = s.trim();
    ['"', '\''].iter().find_map(|q| {
        s.strip_prefix(*q)
            .and_then(|r| r.strip_suffix(*q))
            .filter(|r| !r.contains(*q))
            .map(str::to_string)
    })
}

/// The arms of a spread's `->match(…)`, from the text between its
/// parentheses. Each argument is a `[candidate, value]` pair whose value is
/// `null`, an object `{ … }` or `$ { … }`; anything else is not the
/// abstract-type form, and the reason says which part.
fn parse_arms(args: &[char]) -> Result<Vec<SpreadArm>, String> {
    let mut arms = Vec::new();
    for pair in split_top_level(args) {
        let inner = pair
            .strip_prefix('[')
            .and_then(|p| p.strip_suffix(']'))
            .ok_or_else(|| {
                "a `...` spread whose `->match` argument is not a `[candidate, value]` pair"
                    .to_string()
            })?;
        let parts = split_top_level(&inner.chars().collect::<Vec<_>>());
        let [candidate, value] = parts.as_slice() else {
            return Err(
                "a `...` spread whose `->match` argument is not a `[candidate, value]` pair"
                    .to_string(),
            );
        };
        if value == "null" {
            continue;
        }
        let body = value
            .strip_prefix('$')
            .map(str::trim_start)
            .unwrap_or(value)
            .strip_prefix('{')
            .and_then(|b| b.strip_suffix('}'))
            .ok_or_else(|| {
                "a `...` spread whose `->match` arm is not an object or `null`".to_string()
            })?;
        let typename = arm_typename(body);
        let children = parse_selection(body)
            .into_iter()
            .filter(|n| {
                n.alias.as_deref() != Some("__typename")
                    && n.key.as_deref() != Some(&["__typename".to_string()][..])
            })
            .collect();
        arms.push(SpreadArm {
            candidate: string_literal(candidate),
            typename,
            children,
        });
    }
    Ok(arms)
}

/// An arm object's literal `__typename`: `__typename: "T"` or
/// `__typename: $("T")`, either quote, at the object's top level.
fn arm_typename(body: &str) -> Option<String> {
    split_top_level(&body.chars().collect::<Vec<_>>())
        .iter()
        .flat_map(|entry| {
            // A whitespace-separated list is one "entry": find the alias in it.
            let mut hits = Vec::new();
            let mut rest = entry.as_str();
            while let Some(at) = rest.find("__typename") {
                let before_ok = rest[..at]
                    .chars()
                    .last()
                    .is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_'));
                let after = rest[at + "__typename".len()..].trim_start();
                if before_ok {
                    if let Some(v) = after.strip_prefix(':') {
                        let v = v.trim_start();
                        let v = v
                            .strip_prefix("$(")
                            .map(|w| w.split_once(')').map(|(x, _)| x).unwrap_or(w))
                            .unwrap_or(v);
                        let v = v.trim_start();
                        if let Some(q) = v.chars().next().filter(|c| matches!(c, '"' | '\'')) {
                            if let Some(end) = v[1..].find(q) {
                                hits.push(v[1..1 + end].to_string());
                            }
                        }
                    }
                }
                rest = &rest[at + "__typename".len()..];
            }
            hits
        })
        .next()
}

// ─── Shapes ──────────────────────────────────────────────────────────────────

fn shape_name(reference: &str) -> &str {
    reference.rsplit('/').next().unwrap_or(reference)
}

pub(crate) fn deref(
    shape: Option<&Value>,
    shapes: &Object,
    seen: &mut HashSet<String>,
) -> Option<Value> {
    let mut s = shape?.clone();
    while let Some(r) = s
        .as_object()
        .and_then(|o| o.get("$ref"))
        .and_then(Value::as_str)
        .map(str::to_string)
    {
        let name = shape_name(&r).to_string();
        if seen.contains(&name) {
            return None;
        }
        seen.insert(name.clone());
        s = shapes.get(&name)?.clone();
    }
    if let Some(all) = get_arr(&s, "allOf").cloned() {
        let mut merged = obj();
        merged.insert("type".into(), Value::from("object"));
        let mut props = obj();
        let mut additional: Option<Value> = None;
        for part in &all {
            if let Some(p) = deref(Some(part), shapes, &mut seen.clone()) {
                if let Some(pp) = get_obj(&p, "properties") {
                    for (k, v) in pp {
                        props.insert(k.clone(), v.clone());
                    }
                }
                if let Some(a) = crate::json::field(&p, "additionalProperties") {
                    additional = Some(a.clone());
                }
            }
        }
        if let Some(own) = get_obj(&s, "properties") {
            for (k, v) in own {
                props.insert(k.clone(), v.clone());
            }
        }
        merged.insert("properties".into(), Value::Object(props));
        if let Some(a) = additional {
            merged.insert("additionalProperties".into(), a);
        }
        return Some(Value::Object(merged));
    }
    Some(s)
}

/// `shape` dereferenced, with a `oneOf`/`anyOf` of objects merged into one
/// object (ADR 0058), for reading a `...` spread's arms and nothing else:
/// every key some variant offers is readable in an arm, first variant first.
/// Only when every variant is a closed object with properties; otherwise,
/// and for a shape that is not polymorphic, it is `deref`'s answer. An array
/// merges its items. `deref` itself does not merge: an error envelope's
/// `oneOf [success, error]` (an envelope API) is not a union, and merging it there
/// made `reconcile` report the error variant's keys as unmapped.
pub(crate) fn merge_variants(shape: Option<&Value>, shapes: &Object) -> Option<Value> {
    let s = deref(shape, shapes, &mut HashSet::new())?;
    if is_array(Some(&s)) {
        let items = get(&s, "items").cloned();
        let merged = merge_variants(items.as_ref(), shapes)?;
        let mut out = s.as_object().cloned().unwrap_or_default();
        out.insert("items".into(), merged);
        return Some(Value::Object(out));
    }
    if get(&s, "properties").is_some() {
        return Some(s);
    }
    let Some(variants) = get_arr(&s, "oneOf").or_else(|| get_arr(&s, "anyOf")) else {
        return Some(s);
    };
    let resolved: Option<Vec<Value>> = variants
        .iter()
        .map(|v| deref(Some(v), shapes, &mut HashSet::new()))
        .collect();
    let Some(resolved) = resolved.filter(|r| {
        !r.is_empty()
            && r.iter()
                .all(|v| get_obj(v, "properties").is_some() && !is_open(Some(v)))
    }) else {
        return Some(s);
    };
    let mut props = obj();
    for v in &resolved {
        for (k, p) in get_obj(v, "properties").into_iter().flatten() {
            if !props.contains_key(k) {
                props.insert(k.clone(), p.clone());
            }
        }
    }
    let mut merged = obj();
    merged.insert("type".into(), Value::from("object"));
    merged.insert("properties".into(), Value::Object(props));
    Some(Value::Object(merged))
}

/// Whether `nodes` hold a `... disc->match(…)` spread at their own level.
pub(crate) fn has_match_spread(nodes: &[Node]) -> bool {
    nodes
        .iter()
        .any(|n| matches!(n.spread, Some(Spread::Match(_))))
}

pub(crate) fn is_array(s: Option<&Value>) -> bool {
    match s {
        None => false,
        Some(s) => {
            get_str(s, "type") == Some("array")
                || get_arr(s, "type")
                    .map(|t| t.iter().any(|x| x.as_str() == Some("array")))
                    .unwrap_or(false)
                || (get(s, "items").is_some() && get(s, "properties").is_none())
        }
    }
}

fn is_open(s: Option<&Value>) -> bool {
    match s {
        None => true,
        Some(s) => {
            get(s, "properties").is_none()
                || get(s, "additionalProperties") == Some(&Value::Bool(true))
                || get(s, "additionalProperties")
                    .map(|a| a.is_object())
                    .unwrap_or(false)
                || get(s, "oneOf").is_some()
                || get(s, "anyOf").is_some()
        }
    }
}

pub struct Resolved {
    pub mapped: Vec<(String, Node)>,
    pub unknown: Vec<String>,
}

impl Resolved {
    pub fn has(&self, p: &str) -> bool {
        self.mapped.iter().any(|(k, _)| k == p)
    }
    pub fn node(&self, p: &str) -> Option<&Node> {
        self.mapped.iter().find(|(k, _)| k == p).map(|(_, n)| n)
    }
}

/// Resolve a selection tree against a shape into UI-grammar paths.
pub fn resolve_paths(nodes: &[Node], shape: &Value, shapes: &Object) -> Resolved {
    let mut out = Resolved {
        mapped: Vec::new(),
        unknown: Vec::new(),
    };
    fn walk(
        items: &[Node],
        ctx: Option<&Value>,
        prefix: &str,
        shapes: &Object,
        fallback: bool,
        out: &mut Resolved,
    ) {
        for node in items {
            // A `??`/`?!` operand is read in the same context as the node it
            // follows. It maps its path without claiming it: a field that
            // exposes the path keeps its own node (the rename check reads
            // its alias).
            walk(&node.fallbacks, ctx, prefix, shapes, true, out);
            // A spread's arms read from this same object (ADR 0058).
            if let Some(Spread::Match(arms)) = &node.spread {
                let merged = merge_variants(ctx, shapes);
                for arm in arms {
                    walk(
                        &arm.children,
                        merged.as_ref().or(ctx),
                        prefix,
                        shapes,
                        fallback,
                        out,
                    );
                }
            }
            if node.opaque && node.key.is_none() {
                continue;
            }
            let mut cur = deref(ctx, shapes, &mut HashSet::new());
            let mut p = prefix.to_string();
            let mut lost = false;
            let segs = node.key.clone().unwrap_or_default();
            for seg in &segs {
                if seg == "*" {
                    lost = true;
                    break;
                }
                if is_array(cur.as_ref()) {
                    let items = cur.as_ref().and_then(|c| get(c, "items")).cloned();
                    cur = deref(items.as_ref(), shapes, &mut HashSet::new());
                    // A bare-array response (no envelope): the selection is
                    // applied per element, so its paths live under `[]`, the
                    // same grammar `expected_paths` and the selection use.
                    if p.is_empty() {
                        p.push_str("[]");
                    }
                }
                let prop = cur
                    .as_ref()
                    .and_then(|c| get_obj(c, "properties"))
                    .and_then(|pp| pp.get(seg))
                    .cloned();
                match prop {
                    None => {
                        if cur.is_some() && !is_open(cur.as_ref()) {
                            out.unknown.push(if p.is_empty() {
                                seg.clone()
                            } else {
                                format!("{}>{}", p, seg)
                            });
                        }
                        lost = true;
                        p = if p.is_empty() {
                            seg.clone()
                        } else {
                            format!("{}>{}", p, seg)
                        };
                        break;
                    }
                    Some(prop) => {
                        let resolved = deref(Some(&prop), shapes, &mut HashSet::new());
                        p = if p.is_empty() {
                            seg.clone()
                        } else {
                            format!("{}>{}", p, seg)
                        };
                        if is_array(resolved.as_ref()) {
                            p.push_str("[]");
                        }
                        cur = resolved;
                    }
                }
            }
            if !segs.is_empty() && !(fallback && out.has(&p)) {
                out.mapped.retain(|(k, _)| k != &p);
                out.mapped.push((p.clone(), node.clone()));
            }
            if let Some(children) = &node.children {
                if !lost && !node.opaque {
                    walk(children, cur.as_ref(), &p, shapes, fallback, out);
                }
            }
        }
    }
    walk(nodes, Some(shape), "", shapes, false, &mut out);
    out
}

/// The envelope this operation's payload is read under, and where that
/// answer came from. The selection's `response.envelope` governs (ADR 0018);
/// when the selection is silent the tool falls back to its own suggestion
/// from the inventory's response facts, and reconcile says so in a note so
/// the judgement does not stay implicit.
pub fn envelope_for(entry: Option<&Value>, op: &Value) -> (Option<String>, bool) {
    // `field`, not `get`: `envelope: null` is the answer "no envelope", and
    // is as much a decision as naming one.
    if let Some(r) = entry.and_then(|e| get(e, "response")) {
        if crate::json::field(r, "envelope").is_some() {
            return (get_str(r, "envelope").map(str::to_string), true);
        }
    }
    (
        crate::envelope::suggest_envelope(get(op, "response")),
        false,
    )
}

/// The paths a selection of "all fields" is expected to cover: top-level
/// response properties and, under the envelope, item-level properties.
pub fn expected_paths(op: &Value, shapes: &Object, envelope: Option<&str>) -> Vec<String> {
    let root_ref = crate::json::object(vec![(
        "$ref",
        get(op, "response")
            .and_then(|r| get(r, "shape_ref"))
            .cloned()
            .unwrap_or(Value::Null),
    )]);
    let root = match deref(Some(&root_ref), shapes, &mut HashSet::new()) {
        Some(r) => r,
        None => return vec![],
    };
    let mut out = Vec::new();
    if is_array(Some(&root)) {
        let items = get(&root, "items").cloned();
        let top = deref(items.as_ref(), shapes, &mut HashSet::new());
        for name in top
            .as_ref()
            .and_then(|t| get_obj(t, "properties"))
            .into_iter()
            .flatten()
            .map(|(k, _)| k)
        {
            out.push(format!("[]>{}", name));
        }
        return out;
    }
    for (name, prop) in get_obj(&root, "properties").into_iter().flatten() {
        let resolved = deref(Some(prop), shapes, &mut HashSet::new());
        let list = is_array(resolved.as_ref());
        let label = format!("{}{}", name, if list { "[]" } else { "" });
        out.push(label.clone());
        if Some(name.as_str()) == envelope {
            let inner = if list {
                let items = resolved.as_ref().and_then(|r| get(r, "items")).cloned();
                deref(items.as_ref(), shapes, &mut HashSet::new())
            } else {
                resolved
            };
            for child in inner
                .as_ref()
                .and_then(|i| get_obj(i, "properties"))
                .into_iter()
                .flatten()
                .map(|(k, _)| k)
            {
                out.push(format!("{}>{}", label, child));
            }
        }
    }
    out
}

/// A path template with every `{…}` erased to `{}`: `/owners/{ownerId}`,
/// `/owners/{$args.ownerId}` and `/owners/{$this.owner_id}` are all
/// `/owners/{}`. The one comparison every operation match makes.
pub fn normalize_path(p: &str) -> String {
    static PARAM_RE: OnceLock<Regex> = OnceLock::new();
    PARAM_RE
        .get_or_init(|| Regex::new(r"\{[^}]*\}").unwrap())
        .replace_all(p, "{}")
        .to_string()
}

/// Match a connector's METHOD + path template to an inventory operation:
/// the literal path first, then every operation whose template is the same
/// once parameter names are erased. Several of those are settled by
/// `declared`, the operations the selection declares for the field or type;
/// a tie they do not settle is an error naming the candidates (ADR 0044).
pub fn match_operation<'a>(
    inventory: &'a Value,
    method: Option<&str>,
    connector_path: Option<&str>,
    declared: &[&str],
) -> Result<Option<&'a Value>, String> {
    let (Some(method), Some(path)) = (method, connector_path) else {
        return Ok(None);
    };
    let Some(ops) = get_arr(inventory, "operations") else {
        return Ok(None);
    };
    if let Some(exact) = ops
        .iter()
        .find(|op| get_str(op, "method") == Some(method) && get_str(op, "path") == Some(path))
    {
        return Ok(Some(exact));
    }
    let want = normalize_path(path);
    let candidates: Vec<&Value> = ops
        .iter()
        .filter(|op| {
            get_str(op, "method") == Some(method)
                && normalize_path(get_str(op, "path").unwrap_or("")) == want
        })
        .collect();
    // A literal query string the inventory does not key by (`/x?fixed=1`
    // against `/x`) matches as the path alone, as it did before ADR 0054.
    if candidates.is_empty() {
        if let Some((bare, _)) = path.split_once('?') {
            return match_operation(inventory, Some(method), Some(bare), declared);
        }
    }
    crate::op_match::disambiguate(candidates, declared)
        .map_err(|e| format!("{} {} {}", method, path, e))
}

// ─── The report ──────────────────────────────────────────────────────────────

/// Whether `path` is known to the expected paths: it names one or an
/// ancestor of one, or it runs deeper than they reach below one of them.
fn known_path(expected: &[String], path: &str) -> bool {
    expected.iter().any(|p| {
        p == path || p.starts_with(&format!("{}>", path)) || path.starts_with(&format!("{}>", p))
    })
}

/// The spelling of `path` with the `[]` the path grammar wants on its array
/// segments, when `path` as written is unknown to the expected paths and a
/// respelling is known: `[]` after one or more segments (fewest first,
/// leftmost first), or a leading `[]>` for a root-array response. The fewest
/// insertions that reach a known path all fall within the segments
/// `expected_paths` reaches, so nothing below them is guessed at. `None`
/// when no respelling is known.
fn array_spelling(expected: &[String], path: &str) -> Option<String> {
    if path.is_empty() || known_path(expected, path) {
        return None;
    }
    let segments: Vec<&str> = path.split('>').collect();
    let bare: Vec<usize> = (0..segments.len())
        .filter(|&i| !segments[i].ends_with("[]"))
        .collect();
    if bare.len() > 8 {
        return None;
    }
    let mut masks: Vec<u32> = (0..1u32 << bare.len()).collect();
    masks.sort_by_key(|&m| (m.count_ones(), m));
    let roots: &[&str] = if path.starts_with("[]") {
        &[""]
    } else {
        &["", "[]>"]
    };
    for root in roots {
        for &mask in &masks {
            if mask == 0 && root.is_empty() {
                continue;
            }
            let respelled: Vec<String> = segments
                .iter()
                .enumerate()
                .map(|(i, s)| match bare.iter().position(|&b| b == i) {
                    Some(bit) if mask & (1 << bit) != 0 => format!("{}[]", s),
                    _ => s.to_string(),
                })
                .collect();
            let candidate = format!("{}{}", root, respelled.join(">"));
            if known_path(expected, &candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

fn strip_stars(p: &str) -> String {
    let s = p.strip_suffix(">**").unwrap_or(p);
    if s == "**" {
        String::new()
    } else {
        s.to_string()
    }
}

fn covered_by(mapped: &Resolved, p: &str) -> bool {
    mapped.has(p)
        || mapped
            .mapped
            .iter()
            .any(|(k, _)| k.starts_with(&format!("{}>", p)) || k.starts_with(&format!("{}[]", p)))
}

fn is_envelope_only(p: &str, envelope: Option<&str>, mapped: &Resolved) -> bool {
    let env = match envelope {
        Some(e) => e,
        None => return false,
    };
    let node = mapped
        .mapped
        .iter()
        .find(|(k, _)| k == env || *k == format!("{}[]", env))
        .map(|(_, n)| n);
    match node {
        Some(n) => {
            n.children.is_none()
                && (p == env
                    || p.starts_with(&format!("{}>", env))
                    || p.starts_with(&format!("{}[]", env)))
        }
        None => false,
    }
}

/// One `overrides:` entry of selection.yaml, as read.
#[derive(Debug, Clone)]
pub struct Override {
    pub key: String,
    pub reason: Option<String>,
    pub decision: Option<String>,
    /// Background beyond the reason (`codify --context`, ADR 0113).
    pub context: Option<String>,
    pub assertions: Vec<(String, String)>,
    pub until: Option<String>,
    pub expires: Option<String>,
}

pub fn read_overrides(selection: &Value) -> Vec<Override> {
    get_arr(selection, "overrides")
        .into_iter()
        .flatten()
        .filter_map(|o| {
            let key = get_str(o, "key")?.to_string();
            let assertions = get_arr(o, "assert")
                .into_iter()
                .flatten()
                .filter_map(|a| {
                    let obj = a.as_object()?;
                    let (k, v) = obj.iter().next()?;
                    Some((
                        k.clone(),
                        match v {
                            Value::String(s) => s.clone(),
                            other => crate::json::compact(other),
                        },
                    ))
                })
                .collect();
            Some(Override {
                key,
                reason: get_str(o, "reason").map(str::to_string),
                decision: get_str(o, "decision").map(str::to_string),
                context: get_str(o, "context").map(str::to_string),
                assertions,
                until: get_str(o, "until").map(str::to_string),
                expires: get_str(o, "expires").map(str::to_string),
            })
        })
        .collect()
}

/// Is an override past its `expires` date (YYYY-MM-DD, compared to today)?
pub fn expired(o: &Override, today: &str) -> bool {
    o.expires.as_deref().map(|d| d < today).unwrap_or(false)
}

/// One `links:` entry of selection.yaml, as read (ADR 0069). `shape`,
/// `path`, `operation` and `parameter` reference an inventory fact;
/// `include`, `field`, `confirmed`, `reason` and `decision` are the
/// judgement. The host GraphQL type is never stored — `link_hosts` derives
/// it from the root fields that return the shape.
#[derive(Debug, Clone)]
pub struct Link {
    pub shape: String,
    pub path: String,
    pub operation: String,
    pub parameter: Option<String>,
    pub include: bool,
    pub field: Option<String>,
    /// Absent means confirmed — a hand-written entry is the user's word.
    pub confirmed: bool,
    pub reason: Option<String>,
    pub decision: Option<String>,
}

impl Link {
    /// `<shape> > <path>`: how every report names a link.
    pub fn key(&self) -> String {
        format!("{} > {}", self.shape, self.path)
    }

    /// The GraphQL field the link proposes: `field` when set, else the one
    /// derivation of the crate (`crate::cmd::inventory_links::link_field_name`)
    /// applied to the operation key's path (`get:/albums/{id}` → `/albums/{id}`).
    /// Every instrument reads the name through this accessor; none derives
    /// it again (R17).
    pub fn field_name(&self) -> String {
        self.field.clone().unwrap_or_else(|| {
            crate::cmd::inventory_links::link_field_name(
                self.operation
                    .split_once(':')
                    .map(|(_, p)| p)
                    .unwrap_or(&self.operation),
            )
        })
    }

    /// The object the link's field lands on: the path the last `>` steps
    /// out of — `""` for the shape's root, `songs[]` for `songs[]>album_id`.
    /// A field name is unique per (shape, object), so the same name on two
    /// objects is two fields, not a clash; lint's `link-duplicate-field`
    /// and `selection draft`'s collision rule both key on this.
    pub fn object_key(&self) -> String {
        self.path
            .rsplit_once('>')
            .map(|(parent, _)| parent.to_string())
            .unwrap_or_default()
    }

    /// The one draft message, for reconcile's note and lint's warning alike.
    pub fn draft_note(&self) -> String {
        format!(
            "link {} is still the tool's draft (field {} via {}); agree it with the user, then set confirmed: true or drop the entry",
            self.key(),
            self.field_name(),
            self.operation
        )
    }
}

pub fn read_links(selection: &Value) -> Vec<Link> {
    get_arr(selection, "links")
        .into_iter()
        .flatten()
        .filter_map(|l| {
            Some(Link {
                shape: get_str(l, "shape")?.to_string(),
                path: get_str(l, "path")?.to_string(),
                operation: get_str(l, "operation")?.to_string(),
                parameter: get_str(l, "parameter").map(str::to_string),
                include: truthy(get(l, "include")),
                field: get_str(l, "field").map(str::to_string),
                confirmed: get(l, "confirmed").and_then(Value::as_bool).unwrap_or(true),
                reason: get_str(l, "reason").map(str::to_string),
                decision: get_str(l, "decision").map(str::to_string),
            })
        })
        .collect()
}

/// What a link's fact references fail to reference. Reconcile reports each
/// as a `selection_errors` entry, lint as a `link-*` rule; the check itself
/// lives here once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkProblem {
    /// `shape` is not a key of `inventory.json`'s `shapes`.
    UnknownShape,
    /// `operation` is not a key of `inventory.json`'s `operations`.
    UnknownOperation,
    /// The link is included but its by-id operation is not — and that
    /// operation is the field's provenance.
    OperationExcluded,
    /// `path` does not resolve inside the shape (only reported when the
    /// shape exists: there is nothing to resolve it in otherwise).
    UnknownPath,
}

/// The four reference checks a `links:` entry must pass before anything
/// reads its judgement (ADR 0069), in report order.
pub fn link_reference_problems(
    link: &Link,
    inventory: &Value,
    selection: &Value,
) -> Vec<LinkProblem> {
    let no_shapes = Object::new();
    let shapes = get_obj(inventory, "shapes").unwrap_or(&no_shapes);
    let mut out = Vec::new();
    let shape = shapes.get(&link.shape);
    if shape.is_none() {
        out.push(LinkProblem::UnknownShape);
    }
    match get_arr(inventory, "operations")
        .into_iter()
        .flatten()
        .find(|o| get_str(o, "key") == Some(link.operation.as_str()))
    {
        None => out.push(LinkProblem::UnknownOperation),
        Some(_) => {
            let included = get_obj(selection, "operations")
                .and_then(|o| o.get(&link.operation))
                .map(|e| truthy(get(e, "include")))
                .unwrap_or(false);
            if link.include && !included {
                out.push(LinkProblem::OperationExcluded);
            }
        }
    }
    if let Some(s) = shape {
        if !resolve_link_path(s, &link.path, shapes) {
            out.push(LinkProblem::UnknownPath);
        }
    }
    out
}

/// Why a `links:` entry no longer stands on a fact (ADR 0098), computed
/// once per run from the inventory with the same code `inventory links`
/// prints: its by-id operation is one `inventory_links::refused_targets`
/// refuses (ADR 0085 — the response does not carry the key, or is a list),
/// or the inventory carries no `candidate_entity_link` fact at the entry's
/// shape and path pointing at its operation (the builder's other rules —
/// canonical targets, no self-links, generic names — no longer propose
/// it, or the spec changed). Nothing here re-states a rule.
pub struct LinkStaleness {
    refused: Vec<crate::cmd::inventory_links::RefusedTarget>,
    facts: HashSet<(String, String, String)>,
}

impl LinkStaleness {
    pub fn new(inventory: &Value) -> Self {
        LinkStaleness {
            refused: crate::cmd::inventory_links::refused_targets(inventory),
            facts: crate::cmd::inventory_links::candidate_links(inventory)
                .into_iter()
                .map(|c| (c.shape, c.path, c.operation))
                .collect(),
        }
    }

    /// `Some(reason)` when the entry's target is refused or its fact is
    /// gone; `None` when a current fact backs it. The refusal is named
    /// first: it is the more specific reason, and an inventory built before
    /// ADR 0085 still carries the fact the refusal would now remove.
    pub fn reason(&self, link: &Link) -> Option<String> {
        if let Some(r) = self.refused.iter().find(|r| r.operation == link.operation) {
            return Some(format!(
                "{} is a refused link target: {} (`inventory links` lists it under refused targets)",
                link.operation, r.reason
            ));
        }
        let key = (
            link.shape.clone(),
            link.path.clone(),
            link.operation.clone(),
        );
        if !self.facts.contains(&key) {
            return Some(format!(
                "inventory.json carries no candidate_entity_link fact for {} -> {}: the current rules no longer propose it (`inventory links` lists the facts they do)",
                link.key(),
                link.operation
            ));
        }
        None
    }
}

/// What the decision a stale `links:` entry names says about it (ADR 0113
/// §4). A stale confirmed link raises a decision for its field, choices
/// `keep` and `drop`; resolved `keep` (a chosen id `keep` or `keep-…`,
/// gitea's D-0018 `keep-guarded`) is the one exemption, and everything
/// else leaves the link stale with its own remedy. Findings never keep a
/// link: the rule reads the decision log only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkDecision {
    /// No `decision:` on the entry.
    None,
    /// `decision:` names an id the decision log does not record.
    Unrecorded(String),
    /// The decision is open: the question is pending.
    Open(String),
    /// Resolved `keep`: the user's recorded choice; the link is exempt.
    Keep(String),
    /// Resolved `drop`: set `include: false` citing it and remove the field.
    Drop(String),
    /// Superseded, or resolved without choosing keep or drop.
    Unanswered(String),
}

fn chose(rec: &Value, choice: &str) -> bool {
    crate::json::strings(get(rec, "resolution").and_then(|r| get(r, "chosen")))
        .iter()
        .any(|c| c == choice || c.starts_with(&format!("{}-", choice)))
}

/// The decision `link` names, read from the decision log.
pub fn link_decision(link: &Link, decisions: Option<&Value>) -> LinkDecision {
    let Some(id) = link.decision.as_deref().map(str::trim) else {
        return LinkDecision::None;
    };
    let rec = decisions
        .and_then(|d| get_arr(d, "decisions"))
        .into_iter()
        .flatten()
        .find(|d| get_str(d, "id") == Some(id));
    let Some(rec) = rec else {
        return LinkDecision::Unrecorded(id.to_string());
    };
    match get_str(rec, "status") {
        Some("open") => LinkDecision::Open(id.to_string()),
        Some("resolved") if chose(rec, "keep") => LinkDecision::Keep(id.to_string()),
        Some("resolved") if chose(rec, "drop") => LinkDecision::Drop(id.to_string()),
        _ => LinkDecision::Unanswered(id.to_string()),
    }
}

/// A stale entry the user chose to keep: its `decision:` names a resolved
/// decision that chose `keep` (gitea's `RepositoryMeta > owner`, D-0018).
pub fn link_kept_by_decision(link: &Link, decisions: Option<&Value>) -> bool {
    matches!(link_decision(link, decisions), LinkDecision::Keep(_))
}

/// One argument quoted for a POSIX shell.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// The exact `decisions add` command that raises the decision a stale
/// link needs (ADR 0113 §4): the refusal reason as context, choices `keep`
/// and `drop`, `affects` the field on each host type. The binary writes
/// decisions only through the verb, so reconcile, lint and `links apply`
/// print this rather than write it.
pub fn stale_link_decision_command(link: &Link, reason: &str, hosts: &[String]) -> String {
    let field = link.field_name();
    let affects: Vec<String> = if hosts.is_empty() {
        vec![format!("{}.{}", link.shape, field)]
    } else {
        hosts.iter().map(|h| format!("{}.{}", h, field)).collect()
    };
    let mut parts = vec![
        "graphos-factory-core decisions add .".to_string(),
        format!("--title {}", shell_quote(&format!("Stale link: {}", link.key()))),
        format!(
            "--question {}",
            shell_quote(&format!(
                "The current rules no longer back the confirmed link {} -> {} (field {}). Keep it, or drop it?",
                link.key(),
                link.operation,
                field
            ))
        ),
        format!("--context {}", shell_quote(reason)),
        format!(
            "--choice {}",
            shell_quote("keep:Keep the link: the relationship holds whatever the rules propose")
        ),
        format!(
            "--choice {}",
            shell_quote("drop:Drop the link: set include: false and remove the field")
        ),
    ];
    for a in &affects {
        parts.push(format!("--affects {}", shell_quote(a)));
    }
    parts.join(" ")
}

/// What to do about a stale confirmed link, by what its `decision:` says.
/// `pasted` is whether the field is already in the schema.
pub fn stale_link_remedy(
    link: &Link,
    verdict: &LinkDecision,
    reason: &str,
    hosts: &[String],
    pasted: bool,
) -> String {
    let remove = "remove only that field and its hand-written unit entries and e2e cases (never regenerate the schema)";
    let hold = if pasted {
        format!(
            "until a resolved keep names it the pasted field stays an error, and a drop means: {}, then set include: false citing the decision",
            remove
        )
    } else {
        "do not paste it; a drop means: set include: false citing the decision".to_string()
    };
    let raise = format!(
        "raise the decision for it, run: {} — and name its id in the entry's decision:",
        stale_link_decision_command(link, reason, hosts)
    );
    let text = match verdict {
        LinkDecision::Open(id) => format!(
            "{} is open: answer it (`graphos-factory-core decisions resolve . --id {} --chosen keep` or `--chosen drop`); {}",
            id, id, hold
        ),
        LinkDecision::Drop(id) if pasted => format!(
            "{} chose drop: {}, then set include: false citing {}",
            id, remove, id
        ),
        LinkDecision::Drop(id) => format!(
            "{} chose drop: do not paste it; set include: false citing {}",
            id, id
        ),
        LinkDecision::Unrecorded(id) => format!(
            "decision: {} is not recorded in the decision log; {}; {}",
            id, raise, hold
        ),
        LinkDecision::Unanswered(id) => format!(
            "{} neither keeps nor drops it (a resolved keep is the only exemption); {}; {}",
            id, raise, hold
        ),
        LinkDecision::None | LinkDecision::Keep(_) => format!("{}; {}", raise, hold),
    };
    format!("{} (connectors-language.md § Relationship fields)", text)
}

pub struct Inputs<'a> {
    /// The decision log, when present and readable: what a stale
    /// link's `decision:` says about it (`link_decision`, ADR 0113 §4).
    pub decisions: Option<&'a Value>,
    pub workspace: &'a Value,
    pub selection: &'a Value,
    pub inventory: &'a Value,
    pub sdl: &'a str,
    pub baseline: Option<&'a str>,
    pub schema_file: &'a str,
    /// `.factory/applied.lock.yaml`, when the workspace has one.
    pub lock: Option<&'a Value>,
    /// Today's date (`YYYY-MM-DD`) for `expires`; injectable for tests.
    pub today: &'a str,
}

/// The pattern `workspace.schema.json` itself declares for `directory`, read
/// from the embedded schema so this can never drift from what `lint`'s own
/// contract check enforces (kebab-case, lowercase, no `.` or `/`).
fn directory_pattern() -> &'static Regex {
    static PATTERN: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    PATTERN.get_or_init(|| {
        let schema = crate::json::parse(crate::schemas::WORKSPACE)
            .expect("embedded workspace.schema.json is valid JSON");
        let pattern = schema
            .pointer("/properties/directory/pattern")
            .and_then(Value::as_str)
            .expect("workspace.schema.json declares properties.directory.pattern");
        Regex::new(pattern).expect("workspace.schema.json's directory pattern is a valid regex")
    })
}

/// The workspace's schema file: `<directory>.graphql`, from `workspace.yaml`.
///
/// `directory` is checked against `workspace.schema.json`'s own pattern
/// before it is used to build a path. Without this, a hand-edited
/// `directory: .factory/probe` or `directory: ../../etc/passwd` makes this
/// "ordinary workspace file" resolve inside `.factory/` or outside the
/// workspace entirely — defeating the ADR 0025 custody boundary that exists
/// specifically to keep `.factory/*` reads from following a symlink out of
/// the workspace, and reads outside the workspace never went through
/// custody, or any check, at all (Phase 7az).
pub fn schema_file_of(workspace: &Value) -> Result<String, String> {
    let directory = get_str(workspace, "directory").unwrap_or("schema");
    if !directory_pattern().is_match(directory) {
        return Err(format!(
            "workspace.yaml: directory {:?} does not match workspace.schema.json's pattern for it; refusing to build a schema path from it",
            directory
        ));
    }
    Ok(format!("{}.graphql", directory))
}

/// The workspace's schema file, resolved and read safely: `schema_file_of`
/// validates `directory` first, and the read itself goes through
/// `factory_io::read_named_path`, which still refuses a symlink if the path
/// ever does resolve into `.factory/` despite the check above — the same
/// two-independent-mechanisms posture `factory_io` documents for `.factory`
/// paths generally (Phase 7az). Behaviour for a path outside `.factory/` is
/// unchanged: an ordinary read, the schema's since day one.
pub fn read_schema_file(
    dir: &std::path::Path,
    workspace: &Value,
) -> Result<(String, String), String> {
    let schema_file = schema_file_of(workspace)?;
    let bytes = crate::factory_io::read_named_path(&dir.join(&schema_file))?;
    let sdl = String::from_utf8(bytes)
        .map_err(|_| format!("{}: stream did not contain valid UTF-8", schema_file))?;
    Ok((schema_file, sdl))
}

/// `.factory/selection.yaml`'s text. A workspace that has none yet (a fresh
/// `init`) gets the step that writes it, never a bare "No such file".
pub fn read_selection(dir: &std::path::Path) -> Result<String, String> {
    crate::factory_io::read_to_string(dir, ".factory/selection.yaml").map_err(|e| {
        if e.is_not_found() {
            "no .factory/selection.yaml yet: run `graphos-factory-core selection draft .` to propose one from the inventory, then confirm it with the user".to_string()
        } else {
            String::from(e)
        }
    })
}

pub fn reconcile_workspace(dir: &std::path::Path, baseline: Option<&str>) -> Result<Value, String> {
    // `.factory/*` goes through custody; the schema is an ordinary workspace
    // file the user edits by hand (ADR 0025) — but only once `directory` is
    // proven not to point back inside `.factory/` itself (Phase 7az).
    let factory = |rel: &str| crate::factory_io::read_to_string(dir, rel).map_err(String::from);
    let workspace = crate::yaml::parse(&factory(".factory/workspace.yaml")?)?;
    let selection = crate::yaml::parse(&read_selection(dir)?)?;
    let inventory = crate::json::parse(&factory(".factory/inventory.json")?)?;
    let (schema_file, sdl) = read_schema_file(dir, &workspace)?;
    let lock = crate::spans::read_lock(dir)?;
    let today = crate::today();
    // An unreadable log keeps no stale link: the entry reads as drift,
    // and lint reports the file itself.
    let decisions = crate::decisions::load_present(dir, None).ok().flatten();
    let mut report = reconcile(&Inputs {
        decisions: decisions.as_ref(),
        workspace: &workspace,
        selection: &selection,
        inventory: &inventory,
        sdl: &sdl,
        baseline,
        schema_file: &schema_file,
        lock: lock.as_ref(),
        today: &today,
    });
    // Pinned source documents: hand edits to the spec, like hand edits to
    // the schema, block an apply until codified.
    let sources = crate::sources::statuses(dir, lock.as_ref())?;
    let unpinned = crate::sources::unpinned(dir, lock.as_ref());
    let out_of_sync =
        sources.iter().any(|s| s.blocks_apply(lock.is_some())) || !unpinned.is_empty();
    crate::json::set(
        &mut report,
        "sources",
        Value::Array(sources.iter().map(|s| s.to_value()).collect()),
    );
    if !unpinned.is_empty() {
        crate::json::set(
            &mut report,
            "sources_unpinned",
            Value::Array(unpinned.iter().map(|p| Value::from(p.as_str())).collect()),
        );
    }
    if out_of_sync {
        crate::json::set(&mut report, "clean", Value::Bool(false));
    }
    // The zero-match guard (ADR 0074): template.yaml is the user's own
    // file, read as an ordinary workspace file (ADR 0025).
    let optional_yaml = |rel: &str| {
        std::fs::read_to_string(dir.join(rel))
            .ok()
            .and_then(|t| crate::yaml::parse(&t).ok())
    };
    if let Some(z) = zero_match(&report, &inventory, optional_yaml("template.yaml").as_ref()) {
        crate::json::set(&mut report, "zero_match", z);
        crate::json::set(&mut report, "clean", Value::Bool(false));
    }
    Ok(report)
}

/// The zero-match guard (ADR 0074): the schema has connectors and not one
/// pairs with an inventory operation. That is never N separate mistakes; it
/// is one split between where the base URL's path ends and where the
/// connector paths begin (references/lessons.md "A base URL with a path
/// breaks every layer that compares paths"). The facts to see it by: the
/// inventory's server URLs, `BASE_URL` (template.yaml `test_default`) and
/// one connector path. None when at least one connector pairs, or there are
/// none.
pub fn zero_match(report: &Value, inventory: &Value, template: Option<&Value>) -> Option<Value> {
    let connectors = get(report, "connectors")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let unmatched = get_arr(report, "unmatched")?;
    if connectors == 0 || (unmatched.len() as u64) < connectors {
        return None;
    }
    let base_url = template.and_then(|t| {
        get_arr(t, "variables")
            .into_iter()
            .flatten()
            .find(|v| get_str(v, "name") == Some("BASE_URL"))
            .and_then(|v| get_str(v, "test_default"))
    });
    let sample = unmatched
        .iter()
        .find_map(|u| get_str(u, "path"))
        .map(str::to_string);
    Some(crate::json::object(vec![
        ("connectors", Value::from(connectors)),
        (
            "inventory_server_urls",
            get(inventory, "api")
                .and_then(|a| get(a, "base_urls"))
                .cloned()
                .unwrap_or(Value::Array(vec![])),
        ),
        ("base_url", base_url.map(Value::from).unwrap_or(Value::Null)),
        (
            "sample_connector_path",
            sample.map(Value::from).unwrap_or(Value::Null),
        ),
    ]))
}

/// The one message the zero-match guard prints.
pub fn zero_match_message(z: &Value) -> String {
    let shown = |k: &str| {
        get(z, k)
            .map(|v| match v {
                Value::String(s) => s.clone(),
                Value::Null => "(none)".to_string(),
                other => crate::json::compact(other),
            })
            .unwrap_or_else(|| "(none)".to_string())
    };
    format!(
        "reconcile: none of the {} connectors pairs with an inventory operation — the base URL is split in a way that keeps every connector path from matching an inventory path.\n  inventory server URL(s): {}\n  BASE_URL (template.yaml test_default): {}\n  a connector path: {}\nThe connector path, joined to BASE_URL, must equal an inventory path joined to its server URL, and reconcile compares the paths alone. Put the prefix on one side only: re-key the inventory to the paths the connectors write (google-drive did, artifacts #11), or move the prefix into BASE_URL and out of the connectors (references/lessons.md, \"A base URL with a path breaks every layer that compares paths\").",
        shown("connectors"),
        shown("inventory_server_urls"),
        shown("base_url"),
        shown("sample_connector_path")
    )
}

fn v_strs(items: &[String]) -> Value {
    Value::Array(items.iter().map(|s| Value::from(s.as_str())).collect())
}

/// The override report entry for one span: assertions evaluated against the
/// current text, byte-preservation against the baseline when there is one.
fn override_entry(
    o: &Override,
    span: Option<&crate::spans::Span>,
    baseline_spans: Option<&[crate::spans::Span]>,
    fields: Vec<String>,
    drift: Vec<Value>,
    today: &str,
) -> Value {
    let mut failed = 0usize;
    let assertions: Vec<Value> = o
        .assertions
        .iter()
        .map(|(kind, value)| {
            let ok = match span {
                Some(s) => match crate::spans::check_assertion(kind, value, s) {
                    Ok(b) => Value::Bool(b),
                    Err(e) => Value::from(e),
                },
                None => Value::Bool(false),
            };
            if ok != Value::Bool(true) {
                failed += 1;
            }
            crate::json::object(vec![
                ("assert", Value::from(kind.as_str())),
                ("value", Value::from(value.as_str())),
                ("ok", ok),
            ])
        })
        .collect();
    let preserved = match (span, baseline_spans) {
        (Some(s), Some(b)) => Value::Bool(
            b.iter()
                .find(|x| x.key == s.key)
                .map(|x| x.sha256 == s.sha256)
                .unwrap_or(false),
        ),
        (None, Some(_)) => Value::Bool(false),
        _ => Value::Null,
    };
    crate::json::object(vec![
        ("key", Value::from(o.key.as_str())),
        (
            "kind",
            span.map(|s| Value::from(s.kind)).unwrap_or(Value::Null),
        ),
        (
            "reason",
            o.reason.clone().map(Value::from).unwrap_or(Value::Null),
        ),
        (
            "decision",
            o.decision.clone().map(Value::from).unwrap_or(Value::Null),
        ),
        (
            "context",
            o.context.clone().map(Value::from).unwrap_or(Value::Null),
        ),
        ("fields", v_strs(&fields)),
        ("drift", Value::Array(drift)),
        ("assertions", Value::Array(assertions)),
        ("failed", Value::from(failed)),
        ("pinned", Value::Bool(o.assertions.is_empty())),
        ("preserved", preserved),
        ("expired", Value::Bool(expired(o, today))),
        (
            "until",
            o.until.clone().map(Value::from).unwrap_or(Value::Null),
        ),
    ])
}

pub fn reconcile(inputs: &Inputs) -> Value {
    let Inputs {
        decisions,
        workspace,
        selection,
        inventory,
        sdl,
        baseline,
        schema_file,
        lock,
        today,
    } = inputs;
    let prefix = get_str(workspace, "field_prefix").unwrap_or("");
    let inferred = get_str(workspace, "intake") == Some("discovered");
    let mut selection_errors: Vec<String> = Vec::new();
    let mut add: Vec<Value> = Vec::new();
    let mut remove: Vec<Value> = Vec::new();
    let mut change: Vec<Value> = Vec::new();
    let mut overrides_out: Vec<Value> = Vec::new();
    let mut unchanged: Vec<Value> = Vec::new();
    let mut unmatched: Vec<Value> = Vec::new();
    let mut notes: Vec<Value> = Vec::new();
    let mut connectors = 0usize;

    let ops = get_arr(inventory, "operations")
        .cloned()
        .unwrap_or_default();
    let by_key = |key: &str| ops.iter().find(|o| get_str(o, "key") == Some(key));
    let shapes = get_obj(inventory, "shapes").cloned().unwrap_or_default();
    let links = read_links(selection);
    // Only a link whose references all resolve derives a host or declares
    // a field-level connector's operation; the others are the selection
    // errors reported below, and nothing else (R38).
    let sound_links: Vec<Link> = links
        .iter()
        .filter(|l| link_reference_problems(l, inventory, selection).is_empty())
        .cloned()
        .collect();
    let hints =
        crate::op_match::OpHints::from_selection(Some(workspace), Some(selection), Some(sdl))
            .with_links(&sound_links, inventory, sdl);
    let current_spans = crate::spans::spans(sdl, Some(inventory), &hints);
    // An empty baseline — a first apply, whose schema is not in the baseline
    // commit — has no spans at all, not an empty header: every current span
    // reads as added.
    let baseline_spans: Option<Vec<crate::spans::Span>> = baseline.map(|b| {
        if b.is_empty() {
            Vec::new()
        } else {
            crate::spans::spans(b, Some(inventory), &hints)
        }
    });
    let span_of = |key: &str| current_spans.iter().find(|s| s.key == key);
    // The file's own contract, before anything is read out of it: a
    // selection lint rejects as a `[contract]` error is not "valid against
    // inventory.json", whatever its operation keys say (ADR 0018).
    if let Some(schema) = crate::schemas::load("selection.schema.json", None) {
        for e in crate::jsonschema::validate(selection, &schema) {
            selection_errors.push(format!("selection.yaml{}", e));
        }
    }
    let overrides = read_overrides(selection);
    let override_for = |key: &str| overrides.iter().find(|o| o.key == key);
    if get(selection, "customized").is_some() {
        selection_errors.push(
            "customized: is retired — move each entry to overrides: with a key, reason, decision and assertions (see workspace-contract.md)"
                .to_string(),
        );
    }
    for o in &overrides {
        let is_op = o.key.contains(":/");
        if is_op {
            if by_key(&o.key).is_none() {
                selection_errors.push(format!(
                    "overrides lists {}, which is not in inventory.json",
                    o.key
                ));
            }
        } else if span_of(&o.key).is_none() {
            selection_errors.push(format!(
                "overrides lists {}, which the schema has no span for (expected type:<Name>, Query.<field> or Mutation.<field>)",
                o.key
            ));
        }
        if o.reason.as_deref().map(str::trim).unwrap_or("").is_empty() {
            selection_errors.push(format!("override {} has no reason", o.key));
        }
        for (kind, _) in &o.assertions {
            if !crate::spans::ASSERTION_KINDS.contains(&kind.as_str()) {
                selection_errors.push(format!(
                    "override {} has an unknown assertion kind {:?} (one of: {})",
                    o.key,
                    kind,
                    crate::spans::ASSERTION_KINDS.join(", ")
                ));
            }
        }
    }
    let sel_ops = get_obj(selection, "operations")
        .cloned()
        .unwrap_or_default();

    for (key, entry) in &sel_ops {
        let op = match by_key(key) {
            Some(o) => o,
            None => {
                selection_errors.push(format!("{} is not in inventory.json", key));
                continue;
            }
        };
        if !truthy(get(entry, "include")) {
            continue;
        }
        let g = get(entry, "graphql");
        if g.and_then(|g| get_str(g, "root")).is_none() {
            selection_errors.push(format!("{} is included without graphql.root", key));
        }
        if g.and_then(|g| get_str(g, "name")).is_none() {
            selection_errors.push(format!("{} is included without graphql.name", key));
        }
        if get_str(op, "support") == Some("unsupported") && !truthy(get(entry, "force_reason")) {
            selection_errors.push(format!(
                "{} is unsupported ({}) and has no force_reason",
                key,
                get_str(op, "support_reason").unwrap_or("no reason")
            ));
        }
        // An envelope that names nothing would silently flatten the field to
        // a type with no fields; a typo must not do that quietly.
        if let Some(env) = get(entry, "response").and_then(|r| get_str(r, "envelope")) {
            let roots = root_properties(op, &shapes, get(entry, "response"));
            if !roots.iter().any(|r| r == env) {
                selection_errors.push(format!(
                    "{}: response.envelope {:?} is not a root property of its response shape ({})",
                    key,
                    env,
                    if roots.is_empty() {
                        "the shape has none".to_string()
                    } else {
                        roots.join(", ")
                    }
                ));
            }
        }
        if get(entry, "response").and_then(|r| get(r, "confirmed")) == Some(&Value::Bool(false)) {
            notes.push(crate::json::object(vec![
                ("key", Value::from(key.as_str())),
                ("message", Value::from(format!(
                    "response.envelope is still the tool's draft ({}); agree it with the user, then set confirmed: true or drop the key",
                    get(entry, "response")
                        .and_then(|r| get_str(r, "envelope"))
                        .map(|e| format!("{:?}", e))
                        .unwrap_or_else(|| "none".to_string())
                ))),
            ]));
        }
    }
    // links: (ADR 0069). The entry references a fact — the shape, the
    // property path, the by-id operation — and every reference is checked
    // against inventory.json before anything reads the judgement. An
    // unconfirmed link is a note, never drift: a draft never governs.
    for (i, link) in links.iter().enumerate() {
        let at = format!("links[{}] ({})", i, link.key());
        for problem in link_reference_problems(link, inventory, selection) {
            selection_errors.push(match problem {
                LinkProblem::UnknownShape => {
                    format!("{}: shape {} is not in inventory.json", at, link.shape)
                }
                LinkProblem::UnknownOperation => format!(
                    "{}: operation {} is not in inventory.json",
                    at, link.operation
                ),
                LinkProblem::OperationExcluded => format!(
                    "{}: operation {} is not included by the selection — the by-id operation is the field's provenance",
                    at, link.operation
                ),
                LinkProblem::UnknownPath => format!(
                    "{}: path {} does not resolve in shape {}",
                    at, link.path, link.shape
                ),
            });
        }
        if link.include && !link.confirmed {
            notes.push(crate::json::object(vec![
                ("key", Value::from(link.key())),
                ("message", Value::from(link.draft_note())),
            ]));
        }
    }
    let query_spans = field_spans(sdl, "Query");
    let mutation_spans = field_spans(sdl, "Mutation");
    let type_level = type_connectors(sdl);
    // op key -> [(root, span)]
    let mut field_by_key: Vec<(String, Vec<(&str, &FieldSpan)>)> = Vec::new();
    for (root, spans) in [("Query", &query_spans), ("Mutation", &mutation_spans)] {
        for span in spans {
            let c = match &span.connect {
                Some(c) => c,
                None => continue,
            };
            connectors += 1;
            let declared = hints.for_field(root, &span.name);
            match match_operation(
                inventory,
                c.method.as_deref(),
                c.path.as_deref(),
                &declared,
            ) {
                Err(e) => selection_errors.push(format!(
                    "{}.{}: {}; declare it with graphql.root and graphql.name on the operation this field serves",
                    root, span.name, e
                )),
                Ok(None) => unmatched.push(crate::json::object(vec![
                    ("root", Value::from(root)),
                    ("field", Value::from(span.name.as_str())),
                    (
                        "method",
                        c.method.clone().map(Value::from).unwrap_or(Value::Null),
                    ),
                    (
                        "path",
                        c.path.clone().map(Value::from).unwrap_or(Value::Null),
                    ),
                    ("line", Value::from(span.line)),
                ])),
                Ok(Some(op)) => {
                    let key = get_str(op, "key").unwrap_or("").to_string();
                    match field_by_key.iter_mut().find(|(k, _)| *k == key) {
                        Some((_, list)) => list.push((root, span)),
                        None => field_by_key.push((key, vec![(root, span)])),
                    }
                }
            }
        }
    }
    let mut entity_by_key: Vec<(String, &TypeConnector)> = Vec::new();
    for t in &type_level {
        connectors += 1;
        // A type-level connector serves the entity entry whose root field
        // returns its type (`graphql.entity: true`); a tie that settles
        // nothing is an error.
        match match_operation(
            inventory,
            t.connect.method.as_deref(),
            t.connect.path.as_deref(),
            &hints.for_type(&t.type_name),
        ) {
            Err(e) => selection_errors.push(format!(
                "type {}: {}; mark the operation it serves graphql.entity: true, with a root field that returns {}",
                t.type_name, e, t.type_name
            )),
            Ok(None) => unmatched.push(crate::json::object(vec![
                ("root", Value::from(format!("type {}", t.type_name))),
                ("field", Value::Null),
                (
                    "method",
                    t.connect
                        .method
                        .clone()
                        .map(Value::from)
                        .unwrap_or(Value::Null),
                ),
                (
                    "path",
                    t.connect
                        .path
                        .clone()
                        .map(Value::from)
                        .unwrap_or(Value::Null),
                ),
                ("line", Value::from(t.line)),
            ])),
            Ok(Some(op)) => entity_by_key.push((get_str(op, "key").unwrap_or("").to_string(), t)),
        }
    }
    // Field-level relationship connectors (ADR 0069): `{$this.<fk>}` inside
    // a non-root type. Each is matched with the `links:` entries' operations
    // as the declared tie-breaker (ADR 0044), counted as a connector, and
    // kept in its own list — never in `field_by_key`, where a second field
    // on the by-id operation would read as a duplicate root field.
    let field_level = link_connectors(sdl);
    let mut link_matched: Vec<Option<String>> = Vec::with_capacity(field_level.len());
    for lc in &field_level {
        connectors += 1;
        let c = lc
            .span
            .connect
            .as_ref()
            .expect("a link connector is found by its @connect");
        let declared = hints.for_link(&lc.type_name, &lc.field);
        let matched = match match_operation(
            inventory,
            c.method.as_deref(),
            c.path.as_deref(),
            &declared,
        ) {
            Err(e) => {
                selection_errors.push(format!(
                    "{}.{}: {}; declare it with a links: entry whose operation is the one this field serves",
                    lc.type_name, lc.field, e
                ));
                None
            }
            Ok(None) => {
                unmatched.push(crate::json::object(vec![
                    ("root", Value::from(lc.type_name.as_str())),
                    ("field", Value::from(lc.field.as_str())),
                    (
                        "method",
                        c.method.clone().map(Value::from).unwrap_or(Value::Null),
                    ),
                    (
                        "path",
                        c.path.clone().map(Value::from).unwrap_or(Value::Null),
                    ),
                    ("line", Value::from(lc.span.line)),
                ]));
                None
            }
            Ok(Some(op)) => get_str(op, "key").map(str::to_string),
        };
        link_matched.push(matched);
    }
    // Every other field-level connector (R43): a sub-resource read, a read
    // past the parent's id, a write. Nothing in `links:` declares one, so it
    // is counted and listed — never a `links` row, never against `clean`.
    let outside = field_connectors(sdl);
    let mut field_connector_rows: Vec<Value> = Vec::with_capacity(outside.len());
    let mut outside_named: Vec<String> = Vec::with_capacity(outside.len());
    for fc in &outside {
        connectors += 1;
        let c = fc.span.connect.as_ref();
        let method = c.and_then(|c| c.method.clone());
        let path = c.and_then(|c| c.path.clone());
        outside_named.push(format!(
            "{}.{} ({} {}, line {})",
            fc.type_name,
            fc.field,
            method.as_deref().unwrap_or("?"),
            path.as_deref().unwrap_or("?"),
            fc.span.line
        ));
        field_connector_rows.push(crate::json::object(vec![
            ("type", Value::from(fc.type_name.as_str())),
            ("field", Value::from(fc.field.as_str())),
            ("method", method.map(Value::from).unwrap_or(Value::Null)),
            ("path", path.map(Value::from).unwrap_or(Value::Null)),
            ("line", Value::from(fc.span.line)),
        ]));
    }
    if !outside.is_empty() {
        let one = outside.len() == 1;
        notes.push(crate::json::object(vec![
            ("key", Value::from("field_connectors")),
            (
                "message",
                Value::from(format!(
                    "{} field-level connector{} outside links: {} — no GET-by-id template, so no links: entry declares {} and reconcile does not check {}",
                    outside.len(),
                    if one { "" } else { "s" },
                    outside_named.join(", "),
                    if one { "it" } else { "them" },
                    if one { "it" } else { "them" }
                )),
            ),
        ]));
    }

    let mut seen: HashSet<String> = HashSet::new();
    for (key, entry) in &sel_ops {
        let op = match by_key(key) {
            Some(o) => o,
            None => continue,
        };
        let present: Vec<(&str, &FieldSpan)> = field_by_key
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, l)| l.clone())
            .unwrap_or_default();
        let entity = entity_by_key
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, t)| *t);
        seen.insert(key.clone());
        let fields: Vec<String> = present
            .iter()
            .map(|(r, s)| format!("{}.{}", r, s.name))
            .collect();

        if !truthy(get(entry, "include")) {
            if !present.is_empty() || entity.is_some() {
                let reason = get_str(entry, "reason");
                if let Some(o) = override_for(key) {
                    overrides_out.push(override_entry(
                        o,
                        span_of(key),
                        baseline_spans.as_deref(),
                        fields,
                        vec![Value::from(format!(
                            "selection excludes it ({}) but the schema still has it",
                            reason.unwrap_or("no reason")
                        ))],
                        today,
                    ));
                } else {
                    remove.push(crate::json::object(vec![
                        ("key", Value::from(key.as_str())),
                        ("fields", v_strs(&fields)),
                        (
                            "entity",
                            entity
                                .map(|e| Value::from(e.type_name.as_str()))
                                .unwrap_or(Value::Null),
                        ),
                        ("reason", reason.map(Value::from).unwrap_or(Value::Null)),
                    ]));
                }
            }
            continue;
        }

        let g = match get(entry, "graphql") {
            Some(g) => g,
            None => continue,
        };
        let (root_key, name) = match (get_str(g, "root"), get_str(g, "name")) {
            (Some(r), Some(n)) => (r, n),
            _ => continue,
        };
        let root = if root_key == "mutation" {
            "Mutation"
        } else {
            "Query"
        };
        let want_field = format!("{}_{}", prefix, name);
        if present.is_empty() {
            if let Some(o) = override_for(key) {
                overrides_out.push(override_entry(
                    o,
                    None,
                    baseline_spans.as_deref(),
                    vec![],
                    vec![Value::from("selected but no connector in the schema")],
                    today,
                ));
            } else {
                add.push(crate::json::object(vec![
                    ("key", Value::from(key.as_str())),
                    ("root", Value::from(root)),
                    ("field", Value::from(want_field.as_str())),
                    ("method", get(op, "method").cloned().unwrap_or(Value::Null)),
                    ("path", get(op, "path").cloned().unwrap_or(Value::Null)),
                    (
                        "semantics",
                        get(op, "semantics").cloned().unwrap_or(Value::Null),
                    ),
                    (
                        "request_body",
                        get(op, "request_body")
                            .and_then(|r| get(r, "shape_ref"))
                            .cloned()
                            .unwrap_or(Value::Null),
                    ),
                ]));
            }
            continue;
        }

        let mut drift: Vec<Value> = Vec::new();
        fn push_drift(
            drift: &mut Vec<Value>,
            kind: &str,
            message: String,
            extra: Option<(&str, Value)>,
        ) {
            let mut o = crate::json::object(vec![
                ("kind", Value::from(kind)),
                ("message", Value::from(message)),
            ]);
            if let Some((k, v)) = extra {
                crate::json::set(&mut o, k, v);
            }
            drift.push(o);
        }
        let on_root: Vec<&(&str, &FieldSpan)> =
            present.iter().filter(|(r, _)| *r == root).collect();
        let elsewhere: Vec<&(&str, &FieldSpan)> =
            present.iter().filter(|(r, _)| *r != root).collect();
        if on_root.is_empty() && !elsewhere.is_empty() {
            push_drift(
                &mut drift,
                "root",
                format!(
                    "selection wants {}.{}; the connector is {}.{}",
                    root, want_field, elsewhere[0].0, elsewhere[0].1.name
                ),
                None,
            );
        }
        let primary: &FieldSpan = on_root
            .first()
            .or(elsewhere.first())
            .map(|(_, s)| *s)
            .unwrap();
        if !on_root.is_empty() && primary.name != want_field {
            let o = crate::json::object(vec![
                ("kind", Value::from("rename")),
                ("from", Value::from(primary.name.as_str())),
                ("to", Value::from(want_field.as_str())),
                (
                    "message",
                    Value::from(format!(
                        "field is {}.{}; selection names it {}",
                        root, primary.name, want_field
                    )),
                ),
            ]);
            drift.push(o);
        }
        if present.len() > 1 {
            push_drift(
                &mut drift,
                "duplicate",
                format!(
                    "{} root fields connect to {} {}: {}",
                    present.len(),
                    get_str(op, "method").unwrap_or(""),
                    get_str(op, "path").unwrap_or(""),
                    fields.join(", ")
                ),
                None,
            );
        }

        let want_tags: Vec<String> = get_arr(entry, "tags")
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect();
        for t in &want_tags {
            if !primary.tags.contains(t) {
                push_drift(
                    &mut drift,
                    "tags",
                    format!(
                        "selection tags it {}; the field has no @tag(name: \"{}\")",
                        t, t
                    ),
                    None,
                );
            }
        }
        for t in &primary.tags {
            if !want_tags.contains(t) {
                push_drift(
                    &mut drift,
                    "tags",
                    format!(
                        "the field carries @tag(name: \"{}\"), which the selection does not list",
                        t
                    ),
                    None,
                );
            }
        }
        for arg in get(entry, "pagination")
            .and_then(|p| get_arr(p, "expose"))
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            if !primary.args.iter().any(|a| a == arg) {
                push_drift(
                    &mut drift,
                    "pagination",
                    format!(
                        "pagination exposes {}, but the field has no {} argument (has: {})",
                        arg,
                        arg,
                        if primary.args.is_empty() {
                            "none".to_string()
                        } else {
                            primary.args.join(", ")
                        }
                    ),
                    None,
                );
            }
        }
        let wants_entity = get(g, "entity").and_then(Value::as_bool) == Some(true);
        let key_field = get_str(g, "key");
        if wants_entity {
            match entity {
                None => push_drift(&mut drift,
                    "entity",
                    format!(
                        "selection marks it an entity (key {}) but no type carries a type-level @connect to {} {}",
                        key_field.unwrap_or("?"),
                        get_str(op, "method").unwrap_or(""),
                        get_str(op, "path").unwrap_or("")
                    ),
                    None,
                ),
                Some(e) => {
                    if let Some(k) = key_field {
                        if !e.keys.iter().any(|x| x.as_deref() == Some(k)) {
                            let keys: Vec<Value> = e.keys.iter().map(|x| x.clone().map(Value::from).unwrap_or(Value::Null)).collect();
                            push_drift(&mut drift, "entity", format!("entity {} has @key(fields: {}); selection says {}", e.type_name, crate::json::compact(&Value::Array(keys)), k), None);
                        }
                    }
                }
            }
        } else if let Some(e) = entity {
            push_drift(&mut drift,
                "entity",
                format!("type {} has a type-level @connect to {} {} but the selection does not mark it an entity", e.type_name, get_str(op, "method").unwrap_or(""), get_str(op, "path").unwrap_or("")),
                None,
            );
        }

        let shape_ref = get(op, "response").and_then(|r| get_str(r, "shape_ref"));
        match (
            shape_ref,
            primary.connect.as_ref().and_then(|c| c.selection.clone()),
        ) {
            (Some(sr), Some(sel_text)) => {
                let tree = parse_selection(&sel_text);
                let shape = crate::json::object(vec![("$ref", Value::from(sr))]);
                let mapped = resolve_paths(&tree, &shape, &shapes);
                let excluded: Vec<String> = get(entry, "fields")
                    .and_then(|f| get_arr(f, "exclude"))
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(strip_stars)
                    .collect();
                let included: Vec<String> = get(entry, "fields")
                    .and_then(|f| get_arr(f, "include"))
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(strip_stars)
                    .collect();
                // The path grammar spells an array segment with `[]`. A path
                // missing one matches none of the expected paths, so the
                // exclusion silently excludes nothing: name the entry as
                // written and the spelling that would match.
                let (exclude_envelope, _) = envelope_for(Some(entry), op);
                let expected = expected_paths(op, &shapes, exclude_envelope.as_deref());
                for raw in get(entry, "fields")
                    .and_then(|f| get_arr(f, "exclude"))
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                {
                    let e = strip_stars(raw);
                    if let Some(spelled) = array_spelling(&expected, &e) {
                        push_drift(
                            &mut drift,
                            "fields",
                            format!(
                                "fields.exclude {} leaves off an array's []: the path grammar spells it {}{}, and as written it excludes nothing",
                                raw,
                                spelled,
                                &raw[e.len()..]
                            ),
                            None,
                        );
                    }
                }
                for p in &excluded {
                    if covered_by(&mapped, p) {
                        push_drift(
                            &mut drift,
                            "fields",
                            format!(
                                "{} is excluded by the selection but the connector maps it",
                                p
                            ),
                            None,
                        );
                    }
                }
                for (p, alias) in get(entry, "fields")
                    .and_then(|f| get_obj(f, "rename"))
                    .into_iter()
                    .flatten()
                {
                    let alias = alias.as_str().unwrap_or("");
                    match mapped.node(p) {
                        None => {
                            if !excluded
                                .iter()
                                .any(|e| p == e || p.starts_with(&format!("{}>", e)))
                            {
                                push_drift(&mut drift, "fields", format!("{} is renamed to {} by the selection but the connector does not map it", p, alias), None);
                            }
                        }
                        Some(node) => {
                            let exposed = node
                                .alias
                                .clone()
                                .or_else(|| node.key.as_ref().and_then(|k| k.last().cloned()))
                                .unwrap_or_default();
                            if exposed != alias {
                                push_drift(&mut drift, "fields", format!("{} should be exposed as {}; the connector exposes it as {}", p, alias, exposed), None);
                            }
                        }
                    }
                }
                for p in &mapped.unknown {
                    if inferred {
                        notes.push(crate::json::object(vec![
                            ("key", Value::from(key.as_str())),
                            ("message", Value::from(format!("the connector selects {}, which no recorded sample has shown (shape inferred from samples)", p))),
                        ]));
                    } else {
                        push_drift(&mut drift, "fields", format!("the connector selects {}, which the documented response shape does not have", p), None);
                    }
                }
                let want_all = get(selection, "defaults")
                    .and_then(|df| get_str(df, "fields"))
                    .unwrap_or("all")
                    == "all"
                    && included.is_empty();
                if want_all {
                    // The envelope decides how deep "all fields" reaches.
                    // selection.yaml governs it; a silent selection falls
                    // back to the suggestion, with a note (ADR 0018).
                    let (envelope, from_selection) = envelope_for(Some(entry), op);
                    if !from_selection {
                        notes.push(crate::json::object(vec![
                            ("key", Value::from(key.as_str())),
                            ("message", Value::from(format!(
                                "selection.yaml declares no response.envelope; reading \"all fields\" against the suggestion {} from the inventory's response facts — run `graphos-factory-core selection draft` and confirm it",
                                envelope.as_deref().map(|e| format!("{:?}", e)).unwrap_or_else(|| "none".to_string())
                            ))),
                        ]));
                    }
                    let missing: Vec<String> = expected_paths(op, &shapes, envelope.as_deref())
                        .into_iter()
                        .filter(|p| {
                            !excluded
                                .iter()
                                .any(|e| p == e || p.starts_with(&format!("{}>", e)))
                                && !covered_by(&mapped, p)
                                && !is_envelope_only(p, envelope.as_deref(), &mapped)
                        })
                        .collect();
                    if !missing.is_empty() {
                        push_drift(
                            &mut drift,
                            "fields",
                            format!(
                                "selection says all fields; {} not mapped: {}",
                                missing.len(),
                                missing.join(", ")
                            ),
                            Some(("missing", v_strs(&missing))),
                        );
                    }
                } else {
                    for p in &included {
                        if !covered_by(&mapped, p) {
                            push_drift(&mut drift, "fields", format!("{} is included by the selection but the connector does not map it", p), None);
                        }
                    }
                }
            }
            (Some(_), None) => push_drift(
                &mut drift,
                "fields",
                "the connector has no selection string to compare".to_string(),
                None,
            ),
            _ => {}
        }

        if let Some(o) = override_for(key) {
            let msgs: Vec<Value> = drift
                .iter()
                .map(|x| get(x, "message").cloned().unwrap_or(Value::Null))
                .collect();
            overrides_out.push(override_entry(
                o,
                span_of(key),
                baseline_spans.as_deref(),
                fields,
                msgs,
                today,
            ));
        } else if !drift.is_empty() {
            change.push(crate::json::object(vec![
                ("key", Value::from(key.as_str())),
                ("fields", v_strs(&fields)),
                ("drift", Value::Array(drift)),
            ]));
        } else {
            unchanged.push(crate::json::object(vec![
                ("key", Value::from(key.as_str())),
                ("fields", v_strs(&fields)),
            ]));
        }
    }

    for (key, present) in &field_by_key {
        if seen.contains(key) {
            continue;
        }
        let fields: Vec<String> = present
            .iter()
            .map(|(r, s)| format!("{}.{}", r, s.name))
            .collect();
        remove.push(crate::json::object(vec![
            ("key", Value::from(key.as_str())),
            ("fields", v_strs(&fields)),
            (
                "entity",
                entity_by_key
                    .iter()
                    .find(|(k, _)| k == key)
                    .map(|(_, t)| Value::from(t.type_name.as_str()))
                    .unwrap_or(Value::Null),
            ),
            ("reason", Value::from("not in selection.yaml")),
        ]));
    }
    for (key, t) in &entity_by_key {
        if seen.contains(key) || field_by_key.iter().any(|(k, _)| k == key) {
            continue;
        }
        remove.push(crate::json::object(vec![
            ("key", Value::from(key.as_str())),
            ("fields", Value::Array(vec![])),
            ("entity", Value::from(t.type_name.as_str())),
            ("reason", Value::from("not in selection.yaml")),
        ]));
    }

    // Links drift (ADR 0069). An included link claims the field it names on
    // every host type the inventory and the SDL derive for its shape; only a
    // confirmed one drifts — an unconfirmed draft is the note pushed above
    // and nothing else. A `{$this.` connector no included link claims is a
    // removal. A link with a reference problem is a selection error only:
    // it derives no host and claims nothing (R38).
    let mut links_add: Vec<Value> = Vec::new();
    let mut links_remove: Vec<Value> = Vec::new();
    let mut links_change: Vec<Value> = Vec::new();
    let mut links_unchanged: Vec<Value> = Vec::new();
    let mut links_notes: Vec<Value> = Vec::new();
    let mut claimed: HashSet<usize> = HashSet::new();
    let mut sdl_index = crate::sdl_index::SdlIndex::new(sdl);
    let staleness = LinkStaleness::new(inventory);
    for link in sound_links.iter().filter(|l| l.include) {
        let link_key = link.key();
        // An unknown operation is already a selection error.
        let op = match by_key(&link.operation) {
            Some(o) => o,
            None => continue,
        };
        let op_path = get_str(op, "path").unwrap_or("");
        let op_method = get_str(op, "method").unwrap_or("GET");
        // The one accessor (R17): `field:` when set, else the shared derivation.
        let field = link.field_name();
        let hosts = link_hosts(link, inventory, sdl, &mut sdl_index, &hints);
        // A confirmed entry no current fact backs (ADR 0098) is drift, never
        // an add: reconcile never asks to paste a refused link. Only a
        // resolved `keep` decision the entry names keeps it (gitea's D-0018,
        // ADR 0113 §4); an open one, a `drop`, or none at all leaves it
        // drift, each with its remedy.
        let verdict = link_decision(link, *decisions);
        let stale = if link.confirmed && !matches!(verdict, LinkDecision::Keep(_)) {
            staleness.reason(link)
        } else {
            None
        };
        let host_names: Vec<String> = hosts.iter().map(|(h, _)| h.clone()).collect();
        let stale_row = |host: &str, line: Option<usize>| {
            let reason = stale.clone().unwrap_or_default();
            let remedy = stale_link_remedy(link, &verdict, &reason, &host_names, line.is_some());
            let action = match line {
                Some(l) => format!("pasted at line {}: {}", l, remedy),
                None => format!("not pasted, and not to be pasted: {}", remedy),
            };
            let mut row = crate::json::object(vec![
                ("key", Value::from(link_key.as_str())),
                ("type", Value::from(host)),
                ("field", Value::from(field.as_str())),
                ("operation", Value::from(link.operation.as_str())),
                ("stale", Value::Bool(true)),
            ]);
            if let Some(l) = line {
                crate::json::set(&mut row, "line", Value::from(l));
            }
            crate::json::set(
                &mut row,
                "drift",
                v_strs(&[stale.clone().unwrap_or_default(), action]),
            );
            row
        };
        if hosts.is_empty() {
            if stale.is_some() {
                links_change.push(stale_row(&link.shape, None));
                continue;
            }
            if link.confirmed {
                links_notes.push(crate::json::object(vec![
                    ("key", Value::from(link_key.as_str())),
                    (
                        "message",
                        Value::from(format!(
                            "no included operation's root field returns a type for shape {}; the link has no host type in the schema yet",
                            link.shape
                        )),
                    ),
                ]));
            }
            continue;
        }
        for (host, fk) in hosts {
            let want_path = link_path_template(op_path, link.parameter.as_deref(), &fk);
            let position = field_level
                .iter()
                .position(|lc| lc.type_name == host && lc.field == field);
            if stale.is_some() && position.is_none() {
                links_change.push(stale_row(&host, None));
                continue;
            }
            match position {
                None => {
                    if link.confirmed {
                        links_add.push(crate::json::object(vec![
                            ("key", Value::from(link_key.as_str())),
                            ("type", Value::from(host.as_str())),
                            ("field", Value::from(field.as_str())),
                            ("operation", Value::from(link.operation.as_str())),
                            ("method", Value::from(op_method)),
                            ("path", Value::from(want_path.as_str())),
                            (
                                "message",
                                Value::from(format!(
                                    "type {} has no field {} @connect({} {}) for link {}",
                                    host, field, op_method, want_path, link_key
                                )),
                            ),
                        ]));
                    }
                }
                Some(i) => {
                    claimed.insert(i);
                    if !link.confirmed {
                        continue;
                    }
                    let lc = &field_level[i];
                    let c = lc
                        .span
                        .connect
                        .as_ref()
                        .expect("a link connector is found by its @connect");
                    let mut drift: Vec<String> = Vec::new();
                    if let Some(k) = &link_matched[i] {
                        if k != &link.operation {
                            drift.push(format!(
                                "the connector resolves {}; the link's operation is {}",
                                k, link.operation
                            ));
                        }
                    }
                    if !lc.this_vars.iter().any(|v| v == &fk) {
                        drift.push(format!(
                            "the path reads {{$this.{}}}; the foreign key {} names on {} is {}",
                            lc.this_vars.join("}, {$this."),
                            link.path,
                            host,
                            fk
                        ));
                    }
                    if c.method.as_deref() != Some(op_method) {
                        drift.push(format!(
                            "the connector's verb is {}; {} is a {}",
                            c.method.as_deref().unwrap_or("?"),
                            link.operation,
                            op_method
                        ));
                    }
                    let mut row = crate::json::object(vec![
                        ("key", Value::from(link_key.as_str())),
                        ("type", Value::from(host.as_str())),
                        ("field", Value::from(field.as_str())),
                        ("operation", Value::from(link.operation.as_str())),
                        ("line", Value::from(lc.span.line)),
                    ]);
                    // A pasted stale link: its reason and remedy lead, the
                    // connector's own drift (if any) follows.
                    if stale.is_some() {
                        let mut lines: Vec<String> =
                            get_arr(&stale_row(&host, Some(lc.span.line)), "drift")
                                .into_iter()
                                .flatten()
                                .filter_map(|d| d.as_str().map(str::to_string))
                                .collect();
                        lines.append(&mut drift);
                        drift = lines;
                        crate::json::set(&mut row, "stale", Value::Bool(true));
                    }
                    if drift.is_empty() {
                        links_unchanged.push(row);
                    } else {
                        crate::json::set(&mut row, "drift", v_strs(&drift));
                        links_change.push(row);
                    }
                }
            }
        }
    }
    for (i, lc) in field_level.iter().enumerate() {
        if claimed.contains(&i) {
            continue;
        }
        links_remove.push(crate::json::object(vec![
            ("type", Value::from(lc.type_name.as_str())),
            ("field", Value::from(lc.field.as_str())),
            (
                "operation",
                link_matched[i]
                    .clone()
                    .map(Value::from)
                    .unwrap_or(Value::Null),
            ),
            ("line", Value::from(lc.span.line)),
            (
                "reason",
                Value::from("no included links: entry declares it"),
            ),
        ]));
    }

    // Overrides on types and on root fields the inventory does not explain:
    // no selection to drift from, only assertions (and bytes) to hold.
    for o in &overrides {
        if o.key.contains(":/") {
            if by_key(&o.key).is_some()
                && !overrides_out
                    .iter()
                    .any(|x| get_str(x, "key") == Some(&o.key))
            {
                // An operation override for something the selection never
                // mentions: report it so it is not silently ignored.
                overrides_out.push(override_entry(
                    o,
                    span_of(&o.key),
                    baseline_spans.as_deref(),
                    vec![],
                    vec![Value::from("not in selection.yaml operations")],
                    today,
                ));
            }
            continue;
        }
        overrides_out.push(override_entry(
            o,
            span_of(&o.key),
            baseline_spans.as_deref(),
            vec![],
            vec![],
            today,
        ));
    }

    // Hand edits: every span whose text differs from the applied lock, and
    // the inventory when its content hashes differently (ADR 0018 — the
    // inventory is built, never edited, so an edit there is invisible to
    // every other check and `inventory build` would silently revert it).
    let inventory_edited = lock
        .and_then(|l| get_str(l, "inventory"))
        .map(|recorded| crate::spans::sha256_hex(&crate::json::compact(inventory)) != recorded);
    let lock_report = match lock {
        None => crate::json::object(vec![
            ("present", Value::Bool(false)),
            ("hand_edits", Value::Array(vec![])),
        ]),
        Some(l) => {
            let edits: Vec<Value> = crate::spans::compare_with_lock(&current_spans, l)
                .into_iter()
                .map(|e| {
                    crate::json::object(vec![
                        ("key", Value::from(e.key.as_str())),
                        ("change", Value::from(e.change)),
                        ("line", e.line.map(Value::from).unwrap_or(Value::Null)),
                        ("override", Value::Bool(override_for(&e.key).is_some())),
                    ])
                })
                .collect();
            crate::json::object(vec![
                ("present", Value::Bool(true)),
                (
                    "written_at",
                    get(l, "written_at").cloned().unwrap_or(Value::Null),
                ),
                ("hand_edits", Value::Array(edits)),
                (
                    "inventory_edited",
                    inventory_edited.map(Value::Bool).unwrap_or(Value::Null),
                ),
            ])
        }
    };

    // Every span that differs from the baseline revision, whatever it is.
    let modified_since_baseline = baseline_spans.as_ref().map(|b| {
        let mut out: Vec<Value> = Vec::new();
        for s in &current_spans {
            match b.iter().find(|x| x.key == s.key) {
                None => out.push(crate::json::object(vec![
                    ("key", Value::from(s.key.as_str())),
                    ("change", Value::from("added")),
                ])),
                Some(x) if x.sha256 != s.sha256 => out.push(crate::json::object(vec![
                    ("key", Value::from(s.key.as_str())),
                    ("change", Value::from("modified")),
                ])),
                _ => {}
            }
        }
        for x in b {
            if !current_spans.iter().any(|s| s.key == x.key) {
                out.push(crate::json::object(vec![
                    ("key", Value::from(x.key.as_str())),
                    ("change", Value::from("removed")),
                ]));
            }
        }
        Value::Array(out)
    });

    // An override is fine when its assertions hold and it has not expired.
    // Only a *pinned* override (no assertions) must also be byte-for-byte
    // what the baseline had: the others are allowed to move.
    let overrides_ok = overrides_out.iter().all(|o| {
        get(o, "failed").and_then(Value::as_u64) == Some(0)
            && !(get(o, "pinned") == Some(&Value::Bool(true))
                && get(o, "preserved") == Some(&Value::Bool(false)))
            && get(o, "expired") != Some(&Value::Bool(true))
    });
    let clean = selection_errors.is_empty()
        && add.is_empty()
        && remove.is_empty()
        && change.is_empty()
        && unmatched.is_empty()
        && links_add.is_empty()
        && links_remove.is_empty()
        && links_change.is_empty()
        && overrides_ok;

    crate::json::object(vec![
        ("schema", Value::from(*schema_file)),
        ("selection_errors", v_strs(&selection_errors)),
        ("add", Value::Array(add)),
        ("remove", Value::Array(remove)),
        ("change", Value::Array(change)),
        ("overrides", Value::Array(overrides_out)),
        ("unchanged", Value::Array(unchanged)),
        ("unmatched", Value::Array(unmatched)),
        ("notes", Value::Array(notes)),
        (
            "links",
            crate::json::object(vec![
                ("add", Value::Array(links_add)),
                ("remove", Value::Array(links_remove)),
                ("change", Value::Array(links_change)),
                ("unchanged", Value::Array(links_unchanged)),
                ("notes", Value::Array(links_notes)),
            ]),
        ),
        ("field_connectors", Value::Array(field_connector_rows)),
        ("lock", lock_report),
        (
            "modified_since_baseline",
            modified_since_baseline.unwrap_or(Value::Null),
        ),
        ("connectors", Value::from(connectors)),
        ("spans", Value::from(current_spans.len())),
        ("clean", Value::Bool(clean)),
    ])
}

fn strs(v: &Value, key: &str) -> Vec<String> {
    get_arr(v, key)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

pub fn render_report(report: &Value, dir: &str, baseline_label: Option<&str>) -> String {
    let mut lines: Vec<String> = Vec::new();
    let count = |k: &str| get_arr(report, k).map(|a| a.len()).unwrap_or(0);
    let op_overrides = get_arr(report, "overrides")
        .into_iter()
        .flatten()
        .filter(|o| get_str(o, "key").map(|k| k.contains(":/")).unwrap_or(false))
        .count();
    let selected = count("add") + count("change") + count("unchanged") + op_overrides;
    lines.push(format!(
        "reconcile: {} — {}, {} connectors, {} operations in play",
        dir,
        get_str(report, "schema").unwrap_or(""),
        get(report, "connectors")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        selected
    ));
    lines.push(String::new());
    let errors = strs(report, "selection_errors");
    if !errors.is_empty() {
        lines.push(format!(
            "selection: {} error{} — fix selection.yaml before applying",
            errors.len(),
            if errors.len() == 1 { "" } else { "s" }
        ));
        for e in &errors {
            lines.push(format!("  ! {}", e));
        }
    } else {
        lines.push("selection: valid against inventory.json".to_string());
    }
    lines.push(String::new());
    lines.push(format!("add ({}):", count("add")));
    for a in get_arr(report, "add").into_iter().flatten() {
        let body = get_str(a, "request_body")
            .map(|b| format!("; body {}", shape_name(b)))
            .unwrap_or_default();
        lines.push(format!(
            "  + {}  →  {}.{}   ({} {}; {}{})",
            get_str(a, "key").unwrap_or(""),
            get_str(a, "root").unwrap_or(""),
            get_str(a, "field").unwrap_or(""),
            get_str(a, "method").unwrap_or(""),
            get_str(a, "path").unwrap_or(""),
            get_str(a, "semantics").unwrap_or(""),
            body
        ));
    }
    lines.push(format!("remove ({}):", count("remove")));
    for r in get_arr(report, "remove").into_iter().flatten() {
        let mut names = strs(r, "fields");
        if let Some(e) = get_str(r, "entity") {
            names.push(format!("type {}", e));
        }
        lines.push(format!(
            "  - {}  ←  {}   ({})",
            get_str(r, "key").unwrap_or(""),
            names.join(", "),
            get_str(r, "reason").unwrap_or("excluded, no reason given")
        ));
    }
    lines.push(format!("change ({}):", count("change")));
    for c in get_arr(report, "change").into_iter().flatten() {
        lines.push(format!(
            "  ~ {}  {}",
            get_str(c, "key").unwrap_or(""),
            strs(c, "fields").join(", ")
        ));
        for d in get_arr(c, "drift").into_iter().flatten() {
            lines.push(format!(
                "      [{}] {}",
                get_str(d, "kind").unwrap_or(""),
                get_str(d, "message").unwrap_or("")
            ));
        }
    }
    if count("unmatched") > 0 {
        lines.push(format!(
            "unmatched ({}) — connectors no inventory operation explains:",
            count("unmatched")
        ));
        for u in get_arr(report, "unmatched").into_iter().flatten() {
            lines.push(format!(
                "  ? {}{}  {} {}   (line {})",
                get_str(u, "root").unwrap_or(""),
                get_str(u, "field")
                    .map(|f| format!(".{}", f))
                    .unwrap_or_default(),
                get_str(u, "method").unwrap_or("?"),
                get_str(u, "path").unwrap_or("?"),
                get(u, "line").and_then(Value::as_u64).unwrap_or(0)
            ));
        }
    }
    if let Some(links) = get(report, "links") {
        let n = |k: &str| get_arr(links, k).map(|a| a.len()).unwrap_or(0);
        if n("add") + n("remove") + n("change") + n("unchanged") + n("notes") > 0 {
            lines.push(format!(
                "links (+{} −{} ~{} ={}) — relationship fields inside types (from links:):",
                n("add"),
                n("remove"),
                n("change"),
                n("unchanged")
            ));
            for a in get_arr(links, "add").into_iter().flatten() {
                lines.push(format!(
                    "  + {}.{}  {} {}   (link {})",
                    get_str(a, "type").unwrap_or(""),
                    get_str(a, "field").unwrap_or(""),
                    get_str(a, "method").unwrap_or(""),
                    get_str(a, "path").unwrap_or(""),
                    get_str(a, "key").unwrap_or("")
                ));
            }
            for r in get_arr(links, "remove").into_iter().flatten() {
                lines.push(format!(
                    "  - {}.{}   (line {}; {})",
                    get_str(r, "type").unwrap_or(""),
                    get_str(r, "field").unwrap_or(""),
                    get(r, "line").and_then(Value::as_u64).unwrap_or(0),
                    get_str(r, "reason").unwrap_or("")
                ));
            }
            for c in get_arr(links, "change").into_iter().flatten() {
                lines.push(format!(
                    "  ~ {}.{}   (link {})",
                    get_str(c, "type").unwrap_or(""),
                    get_str(c, "field").unwrap_or(""),
                    get_str(c, "key").unwrap_or("")
                ));
                for d in strs(c, "drift") {
                    lines.push(format!("      {}", d));
                }
            }
            for note in get_arr(links, "notes").into_iter().flatten() {
                lines.push(format!(
                    "  · {}   {}",
                    get_str(note, "key").unwrap_or(""),
                    get_str(note, "message").unwrap_or("")
                ));
            }
        }
    }
    if count("field_connectors") > 0 {
        lines.push(format!(
            "field connectors ({}) — field-level connectors outside links:, not reconciled:",
            count("field_connectors")
        ));
        for f in get_arr(report, "field_connectors").into_iter().flatten() {
            lines.push(format!(
                "  · {}.{}  {} {}   (line {})",
                get_str(f, "type").unwrap_or(""),
                get_str(f, "field").unwrap_or(""),
                get_str(f, "method").unwrap_or("?"),
                get_str(f, "path").unwrap_or("?"),
                get(f, "line").and_then(Value::as_u64).unwrap_or(0)
            ));
        }
    }
    lines.push(format!(
        "overrides ({}) — the engineer's intent; drift reported, not fixed:",
        count("overrides")
    ));
    for c in get_arr(report, "overrides").into_iter().flatten() {
        let pinned = get(c, "pinned") == Some(&Value::Bool(true));
        let state = match (get(c, "preserved"), pinned) {
            (Some(Value::Bool(true)), _) => format!(
                "  byte-for-byte as in {}",
                baseline_label.unwrap_or("baseline")
            ),
            (Some(Value::Bool(false)), true) => {
                format!("  MODIFIED since {}", baseline_label.unwrap_or("baseline"))
            }
            (Some(Value::Bool(false)), false) => {
                format!("  changed since {}", baseline_label.unwrap_or("baseline"))
            }
            _ => String::new(),
        };
        let fields = strs(c, "fields");
        lines.push(format!(
            "  = {}  {}{}{}",
            get_str(c, "key").unwrap_or(""),
            if fields.is_empty() {
                get_str(c, "kind").unwrap_or("(no connector)").to_string()
            } else {
                fields.join(", ")
            },
            get_str(c, "decision")
                .map(|d| format!("  [{}]", d))
                .unwrap_or_default(),
            state
        ));
        if let Some(r) = get_str(c, "reason") {
            lines.push(format!("      reason: {}", r));
        }
        if get(c, "expired") == Some(&Value::Bool(true)) {
            lines.push("      EXPIRED — revisit the override or extend `expires`".to_string());
        }
        if pinned {
            lines.push(
                "      pinned (no assertions): byte-for-byte only; add `assert:` so the agent may change it"
                    .to_string(),
            );
        }
        for a in get_arr(c, "assertions").into_iter().flatten() {
            let ok = get(a, "ok");
            lines.push(format!(
                "      {} {} {}{}",
                if ok == Some(&Value::Bool(true)) {
                    "ok  "
                } else {
                    "FAIL"
                },
                get_str(a, "assert").unwrap_or(""),
                crate::json::compact(get(a, "value").unwrap_or(&Value::Null)),
                match ok {
                    Some(Value::String(e)) => format!("  ({})", e),
                    _ => String::new(),
                }
            ));
        }
        let drift = strs(c, "drift");
        if drift.is_empty() && get_str(c, "kind") == Some("operation") {
            lines.push("      no drift".to_string());
        }
        for d in drift {
            lines.push(format!("      drift: {}", d));
        }
    }
    let lock = get(report, "lock");
    let hand_edits: Vec<&Value> = lock
        .and_then(|l| get_arr(l, "hand_edits"))
        .into_iter()
        .flatten()
        .collect();
    match lock.and_then(|l| get(l, "present")) {
        Some(Value::Bool(true)) => {
            if hand_edits.is_empty() {
                lines.push("hand edits since applied.lock.yaml: none".to_string());
            } else {
                lines.push(format!(
                    "hand edits since applied.lock.yaml ({}) — codify each (graphos-factory-core codify --key K --reason …) or acknowledge (graphos-factory-core lock) before applying:",
                    hand_edits.len()
                ));
                for e in hand_edits {
                    lines.push(format!(
                        "  {} {}{}{}",
                        match get_str(e, "change") {
                            Some("added") => "+",
                            Some("removed") => "-",
                            _ => "~",
                        },
                        get_str(e, "key").unwrap_or(""),
                        get(e, "line")
                            .and_then(Value::as_u64)
                            .map(|l| format!("   (line {})", l))
                            .unwrap_or_default(),
                        if get(e, "override") == Some(&Value::Bool(true)) {
                            "   [has an override — refresh it]"
                        } else {
                            ""
                        }
                    ));
                }
            }
            if lock.and_then(|l| get(l, "inventory_edited")) == Some(&Value::Bool(true)) {
                lines.push(
                    "  ! .factory/inventory.json changed since applied.lock.yaml — the inventory is built, never edited: rebuild it, or move the correction into the pinned spec (graphos-factory-core codify --source) or into selection.yaml when it is a judgement"
                        .to_string(),
                );
            }
        }
        _ => lines.push(
            "hand edits: no applied.lock.yaml — run `graphos-factory-core lock` after this apply so the next one can tell hand edits apart"
                .to_string(),
        ),
    }
    if let Some(Value::Array(moved)) = get(report, "modified_since_baseline") {
        let keys: Vec<String> = moved
            .iter()
            .map(|m| {
                format!(
                    "{}{}",
                    match get_str(m, "change") {
                        Some("added") => "+",
                        Some("removed") => "-",
                        _ => "~",
                    },
                    get_str(m, "key").unwrap_or("")
                )
            })
            .collect();
        lines.push(format!(
            "changed since {} ({}): {}",
            baseline_label.unwrap_or("baseline"),
            keys.len(),
            if keys.is_empty() {
                "—".to_string()
            } else {
                keys.join(", ")
            }
        ));
    }
    let unchanged: Vec<String> = get_arr(report, "unchanged")
        .into_iter()
        .flatten()
        .filter_map(|u| get_str(u, "key"))
        .map(str::to_string)
        .collect();
    lines.push(format!(
        "unchanged ({}): {}",
        unchanged.len(),
        if unchanged.is_empty() {
            "—".to_string()
        } else {
            unchanged.join(", ")
        }
    ));
    // The `field_connectors` note says in one line what the `field
    // connectors (n)` section above already lists one per line: the text
    // prints it once, there. `--json` keeps both.
    let notes: Vec<&Value> = get_arr(report, "notes")
        .into_iter()
        .flatten()
        .filter(|n| count("field_connectors") == 0 || get_str(n, "key") != Some("field_connectors"))
        .collect();
    if !notes.is_empty() {
        lines.push(format!("notes ({}) — not a delta:", notes.len()));
        for n in notes {
            lines.push(format!(
                "  · {}: {}",
                get_str(n, "key").unwrap_or(""),
                get_str(n, "message").unwrap_or("")
            ));
        }
    }
    lines.push(String::new());
    let todo = count("add") + count("remove") + count("change");
    let modified = get_arr(report, "overrides")
        .into_iter()
        .flatten()
        .filter(|c| {
            get(c, "preserved") == Some(&Value::Bool(false))
                && get(c, "pinned") == Some(&Value::Bool(true))
        })
        .count();
    let failed: u64 = get_arr(report, "overrides")
        .into_iter()
        .flatten()
        .filter_map(|c| get(c, "failed").and_then(Value::as_u64))
        .sum();
    let expired_n = get_arr(report, "overrides")
        .into_iter()
        .flatten()
        .filter(|c| get(c, "expired") == Some(&Value::Bool(true)))
        .count();
    let clean = get(report, "clean") == Some(&Value::Bool(true));
    if clean {
        lines.push(format!(
            "reconcile: clean — schema, selection and inventory agree{}",
            if count("overrides") > 0 {
                format!(
                    " ({} override{}, every assertion holds; drift above is the engineer's)",
                    count("overrides"),
                    if count("overrides") == 1 { "" } else { "s" }
                )
            } else {
                String::new()
            }
        ));
    } else {
        lines.push(format!(
            "reconcile: {} operation{} to apply{}{}{}{}{}",
            todo,
            if todo == 1 { "" } else { "s" },
            if count("unmatched") > 0 {
                format!(", {} unmatched", count("unmatched"))
            } else {
                String::new()
            },
            if modified > 0 {
                format!(
                    ", {} pinned override{} MODIFIED",
                    modified,
                    if modified == 1 { "" } else { "s" }
                )
            } else {
                String::new()
            },
            if failed > 0 {
                format!(
                    ", {} override assertion{} FAILED",
                    failed,
                    if failed == 1 { "" } else { "s" }
                )
            } else {
                String::new()
            },
            if expired_n > 0 {
                format!(
                    ", {} override{} expired",
                    expired_n,
                    if expired_n == 1 { "" } else { "s" }
                )
            } else {
                String::new()
            },
            if !errors.is_empty() {
                format!(
                    ", {} selection error{}",
                    errors.len(),
                    if errors.len() == 1 { "" } else { "s" }
                )
            } else {
                String::new()
            }
        ));
    }
    if let Some(unpinned) = get_arr(report, "sources_unpinned").filter(|s| !s.is_empty()) {
        for p in unpinned {
            lines.push(format!(
                "UNPINNED: {} is acknowledged in applied.lock.yaml but sources.lock.yaml no longer pins it — restore the entry, or run `graphos-factory-core lock` to stop watching it on purpose",
                p.as_str().unwrap_or("")
            ));
        }
    }
    if let Some(sources) = get_arr(report, "sources").filter(|s| !s.is_empty()) {
        lines.push(format!("pinned sources ({}):", sources.len()));
        for s in sources {
            let path = get_str(s, "path").unwrap_or("");
            let outside = get(s, "outside_workspace") == Some(&Value::Bool(true));
            let misplaced = get(s, "upstream_misplaced") == Some(&Value::Bool(true));
            let problems: Vec<String> = [
                (get(s, "hand_edit") == Some(&Value::Bool(true)), if get(s, "upstream_present") == Some(&Value::Bool(false)) && get(s, "upstream_recorded") != Some(&Value::Bool(true)) { format!("HAND EDIT since applied.lock.yaml — pin it first (graphos-factory-core sources pin --path {}), then codify later edits", path) } else if get(s, "upstream_ok") == Some(&Value::Bool(false)) || get(s, "upstream_present") == Some(&Value::Bool(false)) || get_str(s, "upstream_error").is_some() { "HAND EDIT since applied.lock.yaml — restore the vendor copy first, then codify it".to_string() } else { format!("HAND EDIT since applied.lock.yaml — codify it (graphos-factory-core codify --source {} --reason …)", path) }),
                (outside, format!("OUTSIDE THE WORKSPACE — {}; fix it in sources.lock.yaml (nothing is read or written through this entry)", get_str(s, "misplaced_reason").unwrap_or("a copy is outside the workspace"))),
                (misplaced, format!("NOT FOLLOWED — {}; fix it in sources.lock.yaml (nothing is read or written through this entry)", get_str(s, "misplaced_reason").unwrap_or("the vendor copy is misplaced"))),
                (get_str(s, "upstream_error").is_some(), format!("UPSTREAM UNREADABLE ({}) — restore it (git checkout -- {})", get_str(s, "upstream_error").unwrap_or(""), get_str(s, "upstream").unwrap_or(""))),
                (!outside && !misplaced && get(s, "working_present") == Some(&Value::Bool(false)), "WORKING COPY MISSING — the pinned file does not exist".to_string()),
                (get_str(s, "working_error").is_some(), format!("WORKING COPY UNREADABLE — {}", get_str(s, "working_error").unwrap_or(""))),
                (!outside && !misplaced && get(s, "upstream_present") == Some(&Value::Bool(false)) && get(s, "upstream_recorded") == Some(&Value::Bool(true)), format!("NO UPSTREAM COPY — restore it (git checkout -- {}); it is never re-created from the working copy, or replace the baseline with `graphos-factory-core sources pin --path {} --force --reason R`", get_str(s, "upstream").unwrap_or(""), path)),
                (!outside && !misplaced && get(s, "upstream_present") == Some(&Value::Bool(false)) && get(s, "upstream_recorded") != Some(&Value::Bool(true)), format!("NO UPSTREAM COPY — run `graphos-factory-core sources pin --path {}`; until then spec edits cannot be detected", path)),
                (!outside && get_str(s, "detected_kind").map(|d| d != "unknown" && Some(d) != get_str(s, "kind")).unwrap_or(false), format!("declared {} but the document is {} — `graphos-factory-core sources pin --path {}` fixes the record", get_str(s, "kind").unwrap_or(""), get_str(s, "detected_kind").unwrap_or(""), path)),
                (!outside && !misplaced && get(s, "locked") == Some(&Value::Bool(false)), "not in applied.lock.yaml — run `graphos-factory-core lock`".to_string()),
                (get(s, "upstream_ok") == Some(&Value::Bool(false)) && get_str(s, "upstream_error").is_none(), "UPSTREAM MODIFIED — restore the vendor copy".to_string()),
                (get(s, "patches_ok") == Some(&Value::Bool(false)), "patches do not reproduce the working copy".to_string()),
            ]
            .iter()
            .filter(|(bad, _)| *bad)
            .map(|(_, m)| m.clone())
            .collect();
            let version = get_str(s, "version").unwrap_or("");
            lines.push(format!(
                "  {} ({}{}) — {} patch{}; {}",
                get_str(s, "path").unwrap_or(""),
                get_str(s, "kind").unwrap_or(""),
                if version.is_empty() {
                    String::new()
                } else {
                    format!(" {}", version)
                },
                get(s, "patches").and_then(Value::as_u64).unwrap_or(0),
                if get(s, "patches").and_then(Value::as_u64) == Some(1) {
                    ""
                } else {
                    "es"
                },
                if problems.is_empty() {
                    "in sync".to_string()
                } else {
                    problems.join("; ")
                }
            ));
        }
    }
    format!("{}\n", lines.join("\n"))
}

/// The root property names of an operation's success response shape, as
/// `response.envelope` may name one. A two-branch success/error
/// `oneOf`/`anyOf` has no properties of its own at the wrapper level; when
/// the source made the branches unambiguous, `inventory.json`'s own
/// `response.referenced_shape` fact points at the actual payload shape
/// (ADR 0080). When the source left it ambiguous but a human or agent has
/// since reviewed the case, `selection_response` -- the operation's own
/// `response` entry in `selection.yaml`, when the caller has one -- may
/// carry the same key as a judgement; inventory.json is never touched
/// either way. Absent both, this returns the wrapper's own properties
/// (empty, for a real union), exactly as before this item.
pub fn root_properties(
    op: &Value,
    shapes: &Object,
    selection_response: Option<&Value>,
) -> Vec<String> {
    let response = get(op, "response");
    let effective_ref = response
        .and_then(|r| get(r, "referenced_shape"))
        .or_else(|| selection_response.and_then(|r| get(r, "referenced_shape")))
        .or_else(|| response.and_then(|r| get(r, "shape_ref")))
        .cloned()
        .unwrap_or(Value::Null);
    let root_ref = crate::json::object(vec![("$ref", effective_ref)]);
    match deref(Some(&root_ref), shapes, &mut HashSet::new()) {
        Some(root) => get_obj(&root, "properties")
            .into_iter()
            .flatten()
            .map(|(k, _)| k.clone())
            .collect(),
        None => vec![],
    }
}

// ─── Links (ADR 0069) ────────────────────────────────────────────────────────

/// One segment of a link path in the walker's wire grammar: its wire name
/// and how many list levels (`[]`) follow it — `matrix[][]` is `("matrix",
/// 2)`, a leading `[][]` is `("", 2)`. The wire name is taken as written: it
/// need not be a GraphQL name (`@type`, `owner.id`).
fn link_segment(segment: &str) -> (&str, usize) {
    let mut name = segment;
    let mut levels = 0;
    while let Some(stripped) = name.strip_suffix("[]") {
        name = stripped;
        levels += 1;
    }
    (name, levels)
}

/// Does `path` (the walker's wire grammar: `owner_id`, `[]>owner_id`,
/// `[][]>owner_id`, `songs[]>owner_id`, `matrix[][]>owner_id`,
/// `owner>account_id`) name a property reachable from `shape`? Each `[]`
/// steps into `items` once — a leading run of them is a root array's items,
/// one per list level — and a `$ref` is followed through `shapes` with the
/// same cycle guard `deref` uses everywhere else.
pub fn resolve_link_path(shape: &Value, path: &str, shapes: &Object) -> bool {
    let mut node = match deref(Some(shape), shapes, &mut HashSet::new()) {
        Some(n) => n,
        None => return false,
    };
    let segments: Vec<(&str, usize)> = path.split('>').map(link_segment).collect();
    // The path must end at a property, never at a bare `[]` run.
    if segments.last().map(|(name, _)| name.is_empty()) != Some(false) {
        return false;
    }
    for (i, (name, levels)) in segments.into_iter().enumerate() {
        // A leading `[]` run is the root-array shape itself: no property step.
        if !(i == 0 && name.is_empty()) {
            node = match get_obj(&node, "properties")
                .and_then(|p| p.get(name))
                .and_then(|child| deref(Some(child), shapes, &mut HashSet::new()))
            {
                Some(child) => child,
                None => return false,
            };
        }
        for _ in 0..levels {
            node = match get(&node, "items")
                .and_then(|items| deref(Some(items), shapes, &mut HashSet::new()))
            {
                Some(items) => items,
                None => return false,
            };
        }
    }
    true
}

/// The shape names a node stands for, and the object it resolves to: a
/// `$ref` chain is followed with each name recorded, and a list stands for
/// its items — the schema's list types are transparent, so an array of
/// `$ref: X` reads as `X` (and as the array's own name when a `$ref` named
/// it). An array of inline items keeps only the array's own name: that is
/// the shape a `[]>fk` path is written against. `shape_name` here is this
/// file's private helper, the one `deref` uses.
fn named_content(node: &Value, shapes: &Object) -> (Vec<String>, Option<Value>) {
    let mut names: Vec<String> = Vec::new();
    let mut current = node.clone();
    loop {
        if let Some(r) = get_str(&current, "$ref") {
            let name = shape_name(r).to_string();
            if names.contains(&name) {
                return (names, None);
            }
            let Some(next) = shapes.get(&name) else {
                return (names, None);
            };
            names.push(name);
            current = next.clone();
            continue;
        }
        if is_array(Some(&current)) {
            let Some(items) = get(&current, "items") else {
                return (names, None);
            };
            current = items.clone();
            continue;
        }
        break;
    }
    let object = deref(Some(&current), shapes, &mut HashSet::new());
    (names, object)
}

/// The inventory node a root field's connector hands its return type, and
/// the selection that fills that type: the operation's response shape and
/// the whole selection — or, when the connector's selection is one rooted
/// block (`$.widget { … }`, `$.widgets { … }`), the property under that
/// block and the block's own fields. This is the envelope as the schema
/// states it, so a link's host can be derived from the SDL alone.
fn content_node<'n>(op: &Value, nodes: &'n [Node], shapes: &Object) -> Option<(Value, &'n [Node])> {
    let root_ref = get(op, "response")
        .and_then(|r| get_str(r, "shape_ref"))?
        .to_string();
    let root = crate::json::object(vec![("$ref", Value::from(root_ref))]);
    if let [only] = nodes {
        if only.rooted {
            if let (Some([key]), Some(children)) = (only.key.as_deref(), only.children.as_deref()) {
                let parent = deref(Some(&root), shapes, &mut HashSet::new())?;
                let inner = get_obj(&parent, "properties")?.get(key)?.clone();
                return Some((inner, children));
            }
        }
    }
    Some((root, nodes))
}

/// The SDL field on `type_name` that carries wire property `wire`: declared
/// under its wire name, or under the camelCased name the generator gives a
/// wire key (`scaffold::camel_of`, the same rule scaffold uses to find a
/// nested field's wire property).
fn sdl_field(
    index: &mut crate::sdl_index::SdlIndex,
    type_name: &str,
    wire: &str,
) -> Option<(String, String)> {
    for candidate in [wire.to_string(), crate::cmd::scaffold::camel_of(wire)] {
        if let Some(t) = index.field_type(type_name, &candidate) {
            return Some((candidate, t));
        }
    }
    None
}

/// The SDL field on `type_name` that carries wire property `wire`, its
/// declared type, and the connector sub-selection that fills it: first the
/// field the connector's selection maps the wire key to (`madeBy:
/// owner_id`), then `sdl_field`'s wire-name and camelCase lookup. Only a
/// field the type declares is an answer.
fn sdl_field_via<'s>(
    index: &mut crate::sdl_index::SdlIndex,
    type_name: &str,
    selection: Option<&'s [Node]>,
    wire: &str,
) -> Option<(String, String, Option<&'s [Node]>)> {
    let selected = selection.into_iter().flatten().find(|n| {
        matches!(n.key.as_deref(), Some([k]) if k == wire)
            && n.methods.is_empty()
            && !n.opaque
            && n.spread.is_none()
    });
    if let Some(node) = selected {
        let name = node.alias.clone().unwrap_or_else(|| wire.to_string());
        if let Some(t) = index.field_type(type_name, &name) {
            return Some((name, t, node.children.as_deref()));
        }
    }
    sdl_field(index, type_name, wire).map(|(name, t)| (name, t, None))
}

/// What one root field's walk carries: the link's shape and path, the
/// inventory's shapes, the `(shape, type, selection)` visits already
/// entered, and every `(host type, fk field, declared)` it found.
struct HostWalk<'a> {
    target: &'a str,
    segments: &'a [(&'a str, usize)],
    shapes: &'a Object,
    seen: HashSet<(String, String, usize)>,
    found: Vec<(String, String, bool)>,
}

/// Walk one inventory node alongside the SDL type the schema gives it
/// (ADR 0069, R31): wherever the node stands for the link's shape, the
/// link's path is walked from that type; then every property the type
/// declares a field for is descended. So a shape only a `$ref` reaches —
/// a component referenced from another component's property — is found as
/// the GraphQL type the schema gives that property, not only a shape an
/// operation returns. Each `(shape, type)` pair is entered once per
/// connector sub-selection that reaches it, and the walk stops at
/// `openapi::LINK_WALK_DEPTH`.
fn walk_hosts(
    node: &Value,
    type_name: &str,
    selection: Option<&[Node]>,
    depth: usize,
    walk: &mut HostWalk,
    index: &mut crate::sdl_index::SdlIndex,
) {
    if depth > crate::openapi::LINK_WALK_DEPTH {
        return;
    }
    let (names, object) = named_content(node, walk.shapes);
    // A pair reached again under another connector sub-selection is a new
    // visit: that selection may be the one that declares the fk (`madeBy:
    // owner_id`) here or below, and `link_hosts` prefers a declared name
    // (R38). The selection is keyed by identity — the parsed tree is finite
    // and each of its nodes is one key — so the guard still ends every cycle.
    let under = selection.map_or(0, |s| s.as_ptr() as usize);
    let mut fresh = names.is_empty();
    for name in &names {
        fresh |= walk
            .seen
            .insert((name.clone(), type_name.to_string(), under));
    }
    if !fresh {
        return;
    }
    if names.iter().any(|n| n == walk.target) {
        if let Some(found) = host_of_path(type_name, selection, walk.segments, index) {
            walk.found.push(found);
        }
    }
    let Some(object) = object else {
        return;
    };
    for (wire, child) in get_obj(&object, "properties").into_iter().flatten() {
        if let Some((_, t, children)) = sdl_field_via(index, type_name, selection, wire) {
            let child_type = crate::sdl_index::base_type(&t);
            walk_hosts(child, &child_type, children, depth + 1, walk, index);
        }
    }
}

/// The link's path walked from `type_name`, the GraphQL type of the link's
/// shape: every segment but the last names a field whose type is the next
/// host (`[]` is transparent), and the last is the fk — named as the SDL
/// declares it (`true`), or by its wire name when the host does not carry
/// it yet (`false`), so the caller can say which field is missing. `None`
/// when an inner segment has no field.
fn host_of_path(
    type_name: &str,
    selection: Option<&[Node]>,
    segments: &[(&str, usize)],
    index: &mut crate::sdl_index::SdlIndex,
) -> Option<(String, String, bool)> {
    let ((last, _), inner) = segments.split_last()?;
    let mut host = type_name.to_string();
    let mut selection = selection;
    for (wire, _) in inner {
        let (_, t, children) = sdl_field_via(index, &host, selection, wire)?;
        host = crate::sdl_index::base_type(&t);
        selection = children;
    }
    Some(match sdl_field_via(index, &host, selection, last) {
        Some((name, _, _)) => (host, name, true),
        None => (host, last.to_string(), false),
    })
}

/// Where a link lives in the schema: `(host GraphQL type, fk field)` for
/// every type the schema gives `link.shape`, reached from a root field whose
/// connector returns the shape or a shape that references it through
/// `$ref` at any depth (R31), with `link.path` walked through the SDL's
/// field types (`[]` is transparent). The fk field is named as a connector
/// selection maps it or the SDL declares it, or by its wire name when no
/// root field finds it declared — so the caller can say which field is
/// missing; one fk per host. Each root field is attributed to its operation
/// with the selection's declared root fields as `declared`
/// (`hints.for_field`, ADR 0044): on a bare-`/{id}` API every root field's
/// path ties, and the entry the selection wrote for the operation
/// returning the shape is what makes the host deterministic (R11). The
/// judgement itself is not consulted: the schema is the record of what is
/// applied.
pub fn link_hosts(
    link: &Link,
    inventory: &Value,
    sdl: &str,
    index: &mut crate::sdl_index::SdlIndex,
    hints: &crate::op_match::OpHints,
) -> Vec<(String, String)> {
    let no_shapes = Object::new();
    let shapes = get_obj(inventory, "shapes").unwrap_or(&no_shapes);
    // A leading `[]` run is a root array's items: no field in the schema.
    let segments: Vec<(&str, usize)> = link
        .path
        .split('>')
        .map(link_segment)
        .enumerate()
        .filter(|(i, (name, _))| !(*i == 0 && name.is_empty()))
        .map(|(_, segment)| segment)
        .collect();
    if segments.is_empty() {
        return Vec::new();
    }
    let mut found: Vec<(String, String, bool)> = Vec::new();
    for root in ["Query", "Mutation"] {
        for span in field_spans(sdl, root) {
            let Some(c) = &span.connect else { continue };
            let Ok(Some(op)) = match_operation(
                inventory,
                c.method.as_deref(),
                c.path.as_deref(),
                &hints.for_field(root, &span.name),
            ) else {
                continue;
            };
            let Some(ret) = index.field_type(root, &span.name) else {
                continue;
            };
            let nodes = parse_selection(c.selection.as_deref().unwrap_or(""));
            let Some((content, selection)) = content_node(op, &nodes, shapes) else {
                continue;
            };
            let mut walk = HostWalk {
                target: &link.shape,
                segments: &segments,
                shapes,
                seen: HashSet::new(),
                found: Vec::new(),
            };
            let host_type = crate::sdl_index::base_type(&ret);
            walk_hosts(&content, &host_type, Some(selection), 0, &mut walk, index);
            found.extend(walk.found);
        }
    }
    // One fk per host, in the order the hosts were found: the field some
    // root field's walk found declared, else the wire name.
    let mut out: Vec<(String, String)> = Vec::new();
    for (host, _, _) in &found {
        if out.iter().any(|(h, _)| h == host) {
            continue;
        }
        let fk = found
            .iter()
            .find(|(h, _, declared)| h == host && *declared)
            .or_else(|| found.iter().find(|(h, _, _)| h == host))
            .map(|(_, fk, _)| fk.clone())
            .unwrap_or_default();
        out.push((host.clone(), fk));
    }
    out
}
