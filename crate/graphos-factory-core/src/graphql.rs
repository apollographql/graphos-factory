//! Just enough GraphQL SDL reading for lint and reconcile. Deliberately
//! textual: the schema is the artifact the engineer edits, and findings must
//! say "line 41" about the file as written. Real parsing is rover's job.
//!
//! All offsets are byte offsets into the SDL. The SDL is treated as ASCII-ish
//! text: blanking replaces every non-newline byte of a string or comment with
//! a space of the same byte width, so offsets between the blanked and the
//! original text agree.

use regex::Regex;

/// Replace string literals and comments with spaces, preserving offsets.
pub fn blank(sdl: &str) -> String {
    let b = sdl.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    let blank_range = |out: &mut Vec<u8>, from: usize, to: usize| {
        for &c in &b[from..to] {
            out.push(if c == b'\n' { b'\n' } else { b' ' });
        }
    };
    while i < b.len() {
        if b[i..].starts_with(b"\"\"\"") {
            let end = find(b, b"\"\"\"", i + 3);
            let stop = end.map(|e| e + 3).unwrap_or(b.len());
            blank_range(&mut out, i, stop);
            i = stop;
            continue;
        }
        if b[i] == b'"' {
            let mut j = i + 1;
            while j < b.len() && b[j] != b'"' && b[j] != b'\n' {
                j += if b[j] == b'\\' { 2 } else { 1 };
            }
            let stop = (j + 1).min(b.len());
            blank_range(&mut out, i, stop);
            i = stop;
            continue;
        }
        if b[i] == b'#' {
            let end = find(b, b"\n", i).unwrap_or(b.len());
            for _ in i..end {
                out.push(b' ');
            }
            i = end;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    // Every byte >= 0x80 inside code (outside strings/comments) is invalid
    // GraphQL anyway; keep it as-is so the lengths agree.
    String::from_utf8(out).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).to_string())
}

fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if from > hay.len() {
        return None;
    }
    hay[from..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| p + from)
}

pub fn line_of(text: &str, index: usize) -> usize {
    text.as_bytes()[..index.min(text.len())]
        .iter()
        .filter(|&&c| c == b'\n')
        .count()
        + 1
}

#[cfg(test)]
thread_local! {
    /// How many times this thread has blanked a whole document to read it
    /// (`type_declarations`, `type_body`, `type_body_at`, `BodyIndex::new`),
    /// so a test can prove a caller scans once and not once per operation.
    pub static DOCUMENT_SCANS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
fn count_document_scan() {
    DOCUMENT_SCANS.with(|c| c.set(c.get() + 1));
}

#[derive(Debug, Clone)]
pub struct Declaration {
    pub kind: String,
    pub name: String,
    pub index: usize,
    pub line: usize,
}

/// Every `type|interface|input|enum|union|scalar Name` declaration: only
/// outside every `{…}` and `(…)`, so a value or field of a declaration
/// (an enum value named `enum`, `type` or `input` followed by another
/// value) is never read as a declaration of its own.
pub fn type_declarations(sdl: &str) -> Vec<Declaration> {
    #[cfg(test)]
    count_document_scan();
    let code = blank(sdl);
    let re = Regex::new(r"\b(type|interface|input|enum|union|scalar)\s+([A-Za-z_][A-Za-z0-9_]*)")
        .unwrap();
    // Bracket depth before each byte; strings and comments are blanked.
    let mut depth_at = Vec::with_capacity(code.len() + 1);
    let mut depth = 0i32;
    for b in code.bytes() {
        depth_at.push(depth);
        match b {
            b'{' | b'(' => depth += 1,
            b'}' | b')' => depth -= 1,
            _ => {}
        }
    }
    depth_at.push(depth);
    re.captures_iter(&code)
        .filter(|m| depth_at[m.get(0).unwrap().start()] <= 0)
        .map(|m| {
            let whole = m.get(0).unwrap();
            Declaration {
                kind: m[1].to_string(),
                name: m[2].to_string(),
                index: whole.start(),
                line: line_of(sdl, whole.start()),
            }
        })
        .collect()
}

pub struct TypeBody {
    pub body: String,
    pub start: usize,
}

/// The body of `type Name { ... }` (also `interface`, `input`, and their
/// `extend` forms), brace-matched. The first declaration of that name wins;
/// `type_body_at` reads a specific one.
pub fn type_body(sdl: &str, name: &str) -> Option<TypeBody> {
    #[cfg(test)]
    count_document_scan();
    let code = blank(sdl);
    let re = Regex::new(&format!(
        r"\b(?:extend\s+)?(?:type|interface|input)\s+{}\b",
        regex::escape(name)
    ))
    .unwrap();
    let m = re.find(&code)?;
    body_from(sdl, &code, m.end())
}

/// `type_body` for a document read many times. Each `type_body` call blanks
/// the whole document and searches it from the start, so a caller asking
/// for many bodies pays O(document) per body. This blanks once and records,
/// in one pass, where each name's body starts, so a lookup costs only the
/// body itself. It owns the blanked text, so the offsets it reads `sdl` at
/// always come from blanking that same `sdl`.
pub struct BodyIndex<'s> {
    sdl: &'s str,
    code: String,
    /// Just past the name of each name's first `type`/`interface`/`input`
    /// declaration (an `extend` form included), the position `type_body`'s
    /// leftmost match ends at.
    starts: std::collections::HashMap<String, usize>,
}

impl<'s> BodyIndex<'s> {
    pub fn new(sdl: &'s str) -> Self {
        #[cfg(test)]
        count_document_scan();
        let code = blank(sdl);
        let re = Regex::new(r"\b(?:type|interface|input)\s+([A-Za-z_][A-Za-z0-9_]*)\b").unwrap();
        let mut starts = std::collections::HashMap::new();
        let mut at = 0;
        // Resume just past each keyword, not past the whole match, so a
        // keyword that is itself the name read by an earlier match (an
        // enum's values `input type Foo`) is still tried, as `type_body`'s
        // per-name search would.
        while let Some(c) = re.captures_at(&code, at) {
            starts
                .entry(c[1].to_string())
                .or_insert_with(|| c.get(0).unwrap().end());
            at = c.get(0).unwrap().start() + 1;
        }
        BodyIndex { sdl, code, starts }
    }

    /// What `type_body(sdl, name)` returns.
    pub fn type_body(&self, name: &str) -> Option<TypeBody> {
        let from = *self.starts.get(name)?;
        body_from(self.sdl, &self.code, from)
    }
}

/// The body of the declaration starting at `index` (a `Declaration.index`),
/// so an `extend type` is read as itself and not as the base type.
pub fn type_body_at(sdl: &str, index: usize) -> Option<TypeBody> {
    #[cfg(test)]
    count_document_scan();
    let code = blank(sdl);
    // Past the keyword(s) and the name.
    let re =
        Regex::new(r"^\s*(?:extend\s+)?(?:type|interface|input)\s+[A-Za-z_][A-Za-z0-9_]*").unwrap();
    let m = re.find(&code[index.min(code.len())..])?;
    body_from(sdl, &code, index + m.end())
}

/// From `from` (just past a declaration's name), skip `implements …` and
/// every `@directive( … )` — whose arguments may carry braces of their own
/// (`http: { GET: … }`) — and return the brace-matched body that follows.
fn body_from(sdl: &str, code: &str, from: usize) -> Option<TypeBody> {
    let bytes = code.as_bytes();
    let keyword = Regex::new(
        r"^(?:extend|type|interface|input|enum|union|scalar|schema|directive|fragment|query|mutation|subscription)\b",
    )
    .unwrap();
    let mut i = from;
    let mut parens = 0i32;
    let mut open: Option<usize> = None;
    while i < bytes.len() {
        match bytes[i] {
            b'(' => parens += 1,
            b')' => parens -= 1,
            b'{' if parens == 0 => {
                open = Some(i);
                break;
            }
            // A new declaration before any body: this one has none.
            b'}' if parens == 0 => return None,
            b'a'..=b'z' | b'A'..=b'Z'
                if parens == 0
                    && (i == 0
                        || !(bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_'))
                    && keyword.is_match(&code[i..])
                    && !code[i..].starts_with("implements") =>
            {
                return None;
            }
            _ => {}
        }
        i += 1;
    }
    let open = open?;
    let mut depth = 0i32;
    for i in open..bytes.len() {
        match bytes[i] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(TypeBody {
                        body: sdl[open + 1..i].to_string(),
                        start: open + 1,
                    });
                }
            }
            _ => {}
        }
    }
    None
}

/// Field names declared directly in a type body (not arguments, not nested).
pub fn field_names(body: &str) -> Vec<String> {
    let code = blank(body);
    let re = Regex::new(r"^([A-Za-z_][A-Za-z0-9_]*)\s*[(:]").unwrap();
    let mut out = Vec::new();
    let mut depth: i32 = 0;
    for raw in code.split('\n') {
        let line = raw.trim();
        let opens = line.chars().filter(|c| matches!(c, '(' | '{')).count() as i32;
        let closes = line.chars().filter(|c| matches!(c, ')' | '}')).count() as i32;
        if depth == 0 {
            if let Some(m) = re.captures(line) {
                out.push(m[1].to_string());
            }
        }
        depth += opens - closes;
        if depth < 0 {
            depth = 0;
        }
    }
    out
}

#[derive(Debug, Clone)]
pub struct Directive {
    pub args: String,
    pub index: usize,
    pub line: usize,
}

/// All `@name(...)` directive applications, with their raw argument text.
pub fn directives(sdl: &str, name: &str) -> Vec<Directive> {
    let code = blank(sdl);
    let re = Regex::new(&format!(r"@{}\b", regex::escape(name))).unwrap();
    let bytes = code.as_bytes();
    let mut out = Vec::new();
    for m in re.find_iter(&code) {
        let after = m.end();
        let open = code[after..].find('(').map(|p| p + after);
        let gap_has_content = open
            .map(|o| code[after..o].chars().any(|c| !c.is_whitespace()))
            .unwrap_or(true);
        if open.is_none() || gap_has_content {
            out.push(Directive {
                args: String::new(),
                index: m.start(),
                line: line_of(sdl, m.start()),
            });
            continue;
        }
        let open = open.unwrap();
        let mut depth = 0i32;
        let mut end = open;
        for i in open..bytes.len() {
            match bytes[i] {
                b'(' => depth += 1,
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        end = i;
                        break;
                    }
                }
                _ => {}
            }
        }
        out.push(Directive {
            args: sdl[open + 1..end].to_string(),
            index: m.start(),
            line: line_of(sdl, m.start()),
        });
    }
    out
}

#[derive(Debug, Clone)]
pub struct Placeholder {
    pub name: String,
    pub index: usize,
    pub line: usize,
}

/// `{{PLACEHOLDER}}` occurrences with their line numbers.
pub fn placeholders(sdl: &str) -> Vec<Placeholder> {
    let re = Regex::new(r"\{\{\s*([A-Z0-9_]+)\s*\}\}").unwrap();
    re.captures_iter(sdl)
        .map(|m| {
            let whole = m.get(0).unwrap();
            Placeholder {
                name: m[1].to_string(),
                index: whole.start(),
                line: line_of(sdl, whole.start()),
            }
        })
        .collect()
}

/// Root fields of Query / Mutation, in declaration order.
pub fn root_fields(sdl: &str, root: &str) -> Vec<String> {
    type_body(sdl, root)
        .map(|b| field_names(&b.body))
        .unwrap_or_default()
}

/// Doc comment (`"""…"""` or `"…"`) immediately preceding an index, if any.
/// `index` is the first byte of the declaration (a field name, say); only
/// whitespace may separate the comment from it, so a description on the
/// enclosing type does not read as the first field's.
pub fn doc_comment_before(sdl: &str, index: usize) -> Option<String> {
    let mut index = index.min(sdl.len());
    while !sdl.is_char_boundary(index) {
        index -= 1;
    }
    let before = &sdl[..index];
    let line_start = before.rfind('\n').map(|p| p + 1).unwrap_or(0);
    if !before[line_start..].trim().is_empty() {
        return None; // something other than indentation precedes it on its line
    }
    let preceding = before[..line_start].trim_end();
    // Scan backwards for the block's opening `"""` rather than matching
    // `(?s)"""(.*?)"""$`: that pattern is leftmost-first, so it opened at the
    // *first* `"""` in the whole file and returned everything up to here as
    // the doc comment (ADR 0032). Every caller that reads the text — the
    // `list-completion-missing` and `copy-state-undocumented` lint rules —
    // then saw the file, not the field's description.
    const FENCE: &str = "\"\"\"";
    if let Some(body_end) = preceding.len().checked_sub(FENCE.len()) {
        if preceding.ends_with(FENCE) {
            if let Some(open) = preceding[..body_end].rfind(FENCE) {
                return Some(preceding[open + FENCE.len()..body_end].trim().to_string());
            }
        }
    }
    let single = Regex::new(r#""((?:[^"\\\n]|\\.)*)"$"#).unwrap();
    single.captures(preceding).map(|m| m[1].trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `BodyIndex` is a cache of `type_body`, so it must answer exactly
    /// what `type_body` answers, for every name: the first declaration
    /// wins, an `extend` form counts, a name that only prefixes another
    /// (`Foo`, `FooBar`) is not confused with it, a name read through the
    /// keyword-shaped enum values `input type Late` is tried exactly as
    /// `type_body` tries it (both stop there, with no body), and a name
    /// inside a string or comment is not a declaration.
    #[test]
    fn a_body_index_answers_what_type_body_answers_for_every_name() {
        let sdl = r#"
"""type Ghost { x: Int }"""
# input Comment { y: Int }
extend type Query { b: Int }
type Query @connect(http: { GET: "/q" }) { a: Int }
input FooBar { long: Int }
input Foo { short: Int, nested: FooBar }
enum Kind { input type Late }
type Late implements Node { id: ID }
interface Node { id: ID }
enum NoBody { A }
scalar Json
"#;
        let index = BodyIndex::new(sdl);
        let names = [
            "Query", "FooBar", "Foo", "Kind", "Late", "Node", "NoBody", "Json", "Ghost", "Comment",
            "Missing", "input", "type",
        ];
        for name in names {
            let want = type_body(sdl, name).map(|b| (b.body, b.start));
            let got = index.type_body(name).map(|b| (b.body, b.start));
            assert_eq!(got, want, "{name}");
        }
        assert!(index.type_body("Query").unwrap().body.contains("b: Int"));
        assert!(index.type_body("Late").is_none());
        assert!(index.type_body("Ghost").is_none());
    }
}
