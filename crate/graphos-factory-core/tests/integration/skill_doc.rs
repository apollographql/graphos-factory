//! The binary's own text never cites the repository's design documents.
//!
//! Design records, the plan and proposals are written for the people and
//! agents developing the skill; an agent using it sees only what the skill
//! ships, so a message that says "see ADR 0069" points at nothing. This
//! test walks every workspace crate's `src/`, lexes each `.rs` file far
//! enough to tell string literals (plain, byte and raw, across lines) from
//! comments, and fails on a literal carrying one of the patterns
//! `graphos-factory-core/scripts/skill-doc-check.sh` refuses in the skill's
//! files. Comments may keep their citations: no agent reads them as output.

use std::fs;
use std::path::{Path, PathBuf};

/// The patterns, checked on each line of a literal. This mirrors the one
/// pattern set documented in the header of
/// `graphos-factory-core/scripts/skill-doc-check.sh`, which is the source
/// of truth: change the two together. The shell reads them as ERE, so each
/// arm below spells out the same boundary rules by hand.
fn citation(line: &str) -> Option<&'static str> {
    let bytes = line.as_bytes();
    let word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let digits = |s: &[u8]| s.iter().take_while(|b| b.is_ascii_digit()).count();

    // ADR[ -]?[0-9]{3,4}, not glued to a word on its left.
    for (i, _) in line.match_indices("ADR") {
        if i >= 1 && word(bytes[i - 1]) {
            continue;
        }
        let mut rest = &bytes[i + 3..];
        if matches!(rest.first(), Some(b' ' | b'-')) {
            rest = &rest[1..];
        }
        if digits(rest) >= 3 {
            return Some("ADR");
        }
    }

    // docs/(decisions|plan|proposals) as a path: not preceded by
    // [A-Za-z0-9/-], unless that slash closes a "./" or "../".
    for (i, _) in line.match_indices("docs/") {
        let rest = &line[i + 5..];
        if !["decisions", "plan", "proposals"]
            .iter()
            .any(|d| rest.starts_with(d))
        {
            continue;
        }
        let glued = i >= 1
            && (bytes[i - 1].is_ascii_alphanumeric()
                || bytes[i - 1] == b'-'
                || bytes[i - 1] == b'/')
            && !line[..i].ends_with("./");
        if !glued {
            return Some("docs/");
        }
    }

    // [Pp]hase [0-9]+[a-z]*, as a reference and not as prose: not glued to
    // a word or a hyphen on its left and followed by ` ?[(,):]`, ` as
    // built`, ` amendment` or the end of the line; or opened by `(` and
    // followed by a non-word character or the end of the line.
    for (i, _) in line.match_indices("hase ") {
        if i < 1 || !matches!(bytes[i - 1], b'P' | b'p') {
            continue;
        }
        let start = i + 5;
        let n = digits(&bytes[start..]);
        if n == 0 {
            continue;
        }
        let mut end = start + n;
        while bytes.get(end).is_some_and(|b| b.is_ascii_lowercase()) {
            end += 1;
        }
        let rest = &line[end..];
        let before = i.checked_sub(2).map(|j| bytes[j]);
        let opened = before == Some(b'(');
        let boundary = !before.is_some_and(|b| word(b) || b == b'-');
        let after_open = rest.bytes().next().is_none_or(|b| !word(b));
        let tail = rest.strip_prefix(' ').unwrap_or(rest);
        let referenced = rest.is_empty()
            || tail.starts_with(['(', ',', ')', ':'])
            || rest.starts_with(" as built")
            || rest.starts_with(" amendment");
        if (boundary && referenced) || (opened && after_open) {
            return Some("Phase N");
        }
    }

    // [0-9]+[a-z]? amendment\)
    for (i, _) in line.match_indices(" amendment)") {
        let mut j = i;
        if j >= 1 && bytes[j - 1].is_ascii_lowercase() {
            j -= 1;
        }
        let d = bytes[..j]
            .iter()
            .rev()
            .take_while(|b| b.is_ascii_digit())
            .count();
        if d >= 1 && (j - d == 0 || !word(bytes[j - d - 1])) {
            return Some("N amendment");
        }
    }

    // not yet on .?main
    for (i, _) in line.match_indices("not yet on ") {
        let rest = &line[i + 11..];
        let mut chars = rest.chars();
        let first = chars.next();
        if rest.starts_with("main") || (first.is_some() && chars.as_str().starts_with("main")) {
            return Some("not yet on main");
        }
    }
    None
}

/// Every string literal in `src` with the line it starts on.
fn literals(src: &str) -> Vec<(usize, String)> {
    let c: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    let mut line = 1;
    let ident = |ch: char| ch.is_alphanumeric() || ch == '_';
    while i < c.len() {
        let ch = c[i];
        if ch == '\n' {
            line += 1;
            i += 1;
        } else if ch == '/' && c.get(i + 1) == Some(&'/') {
            while i < c.len() && c[i] != '\n' {
                i += 1;
            }
        } else if ch == '/' && c.get(i + 1) == Some(&'*') {
            let mut depth = 1;
            i += 2;
            while i < c.len() && depth > 0 {
                if c[i] == '\n' {
                    line += 1;
                }
                if c[i] == '/' && c.get(i + 1) == Some(&'*') {
                    depth += 1;
                    i += 1;
                } else if c[i] == '*' && c.get(i + 1) == Some(&'/') {
                    depth -= 1;
                    i += 1;
                }
                i += 1;
            }
        } else if ch == '\'' {
            // A char literal ('x', '\n', '\''), or a lifetime ('a), which
            // has no closing quote right after one character.
            if c.get(i + 1) == Some(&'\\') {
                // Skip the backslash and the escaped character, so that the
                // quote of '\'' is not taken for the closing one; then scan
                // on to the closing quote ('\\', '\x41', '\u{1F600}').
                i += 3;
                while i < c.len() && c[i] != '\'' {
                    i += 1;
                }
                i += 1;
            } else if c.get(i + 2) == Some(&'\'') {
                i += 3;
            } else {
                i += 1;
            }
        } else if (ch == 'r' || (ch == 'b' && c.get(i + 1) == Some(&'r')))
            && (i == 0 || !ident(c[i - 1]))
            && {
                let mut j = i + if ch == 'b' { 2 } else { 1 };
                while c.get(j) == Some(&'#') {
                    j += 1;
                }
                c.get(j) == Some(&'"')
            }
        {
            let mut j = i + if ch == 'b' { 2 } else { 1 };
            let mut hashes = 0;
            while c[j] == '#' {
                hashes += 1;
                j += 1;
            }
            j += 1;
            let start = line;
            let mut text = String::new();
            loop {
                if j >= c.len() {
                    break;
                }
                if c[j] == '"' && (0..hashes).all(|k| c.get(j + 1 + k) == Some(&'#')) {
                    j += 1 + hashes;
                    break;
                }
                if c[j] == '\n' {
                    line += 1;
                }
                text.push(c[j]);
                j += 1;
            }
            out.push((start, text));
            i = j;
        } else if ch == '"' {
            let start = line;
            let mut text = String::new();
            i += 1;
            while i < c.len() && c[i] != '"' {
                if c[i] == '\\' {
                    text.push(c[i]);
                    i += 1;
                }
                if i < c.len() {
                    if c[i] == '\n' {
                        line += 1;
                    }
                    text.push(c[i]);
                }
                i += 1;
            }
            i += 1;
            out.push((start, text));
        } else {
            i += 1;
        }
    }
    out
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn no_string_literal_cites_a_design_document() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = Vec::new();
    for krate in fs::read_dir(&workspace).unwrap().flatten() {
        rust_files(&krate.path().join("src"), &mut files);
    }
    files.sort();
    assert!(
        files.len() > 20,
        "expected the workspace's sources, found {} files under {}",
        files.len(),
        workspace.display()
    );
    let mut hits = Vec::new();
    for file in &files {
        let src = fs::read_to_string(file).unwrap();
        for (start, text) in literals(&src) {
            for (k, l) in text.lines().enumerate() {
                if let Some(p) = citation(l) {
                    hits.push(format!(
                        "{}:{}: {} in {:?}",
                        file.strip_prefix(&workspace).unwrap_or(file).display(),
                        start + k,
                        p,
                        l.trim()
                    ));
                }
            }
        }
    }
    assert!(
        hits.is_empty(),
        "string literals cite design documents the skill does not ship; state the rule instead:\n{}",
        hits.join("\n")
    );
}

#[test]
fn the_lexer_sees_literals_and_skips_comments() {
    let src = "// ADR 0001 in a comment\n/* ADR 0002 */ let a = 'x'; fn f<'a>() {}\nlet s = \"one\\\n (ADR 0003)\";\nlet r = r#\"raw \"q\" Phase 7as\"#;\n";
    let lits = literals(src);
    let hit: Vec<_> = lits
        .iter()
        .flat_map(|(n, t)| t.lines().filter_map(move |l| citation(l).map(|p| (*n, p))))
        .collect();
    assert_eq!(hit, vec![(3, "ADR"), (5, "Phase N")], "{:?}", lits);
    assert_eq!(citation("an ADRIFT boat, READRs"), None);
    assert_eq!(citation("a pre-ADR-0113 log"), Some("ADR"));
    assert_eq!(citation("the --phase live flag"), None);
}

/// The hits of `src`, as (line, label), the way the walking test reads them.
fn cites(src: &str) -> Vec<(usize, &'static str)> {
    literals(src)
        .iter()
        .flat_map(|(n, t)| {
            t.lines()
                .enumerate()
                .filter_map(move |(k, l)| citation(l).map(|p| (n + k, p)))
        })
        .collect()
}

#[test]
fn the_lexer_survives_adversarial_snippets() {
    // (source, expected hits). Every snippet ends in a literal that cites,
    // or a comment that must not, after something that could derail the
    // lexer.
    let cases: &[(&str, Vec<(usize, &str)>)] = &[
        // An escaped quote in a char literal must not open a new one.
        (
            r####"let q = ['\'','"']; let m = "see ADR 0004";"####,
            vec![(1, "ADR")],
        ),
        (
            r####"let q = '\\'; let m = "see ADR 0004";"####,
            vec![(1, "ADR")],
        ),
        (
            r####"let q = '\x41'; let m = "see ADR 0004";"####,
            vec![(1, "ADR")],
        ),
        (
            r####"let q = '\u{1F600}'; let m = "see ADR 0004";"####,
            vec![(1, "ADR")],
        ),
        (
            r####"let q = ['"','\"',b'"',b'\'','\'']; let m = "ADR 0004";"####,
            vec![(1, "ADR")],
        ),
        // A lifetime opens no literal.
        (
            r####"fn f<'a>(x: &'a str) { let m = "ADR 0004"; }"####,
            vec![(1, "ADR")],
        ),
        // A raw string holds quotes and `//` without ending or commenting.
        (
            r####"let r = r#"a "q" // ADR 0004 "#; let n = 1;"####,
            vec![(1, "ADR")],
        ),
        (r####"let r = br#"a "q" ADR 0004 "#;"####, vec![(1, "ADR")]),
        // A byte string with an escaped quote.
        (r####"let b = b"x\"y ADR 0004";"####, vec![(1, "ADR")]),
        // `//` and `/*` inside strings are text, not comments.
        (
            r####"let u = "http://x ADR 0004"; let v = "/* ADR 0005";"####,
            vec![(1, "ADR"), (1, "ADR")],
        ),
        // A quote or an apostrophe in a comment starts nothing.
        (
            "// it's a \"quote ADR 0004\nlet m = \"ADR 0005\";",
            vec![(2, "ADR")],
        ),
        (
            "/* \" ' ADR 0004 */ let m = \"ADR 0005\";",
            vec![(1, "ADR")],
        ),
        // The line a multi-line literal's hit is on.
        (
            "let a = 'x';\nlet m = \"one\n two (Phase 7as)\";",
            vec![(3, "Phase N")],
        ),
    ];
    for (src, want) in cases {
        assert_eq!(&cites(src), want, "{src}");
    }
}

#[test]
fn the_patterns_match_the_script() {
    let hit = |l: &str| citation(l).is_some();
    // What the script's header lists as citations.
    for l in [
        "see ADR 0004",
        "ADR-0113 says",
        "a pre-ADR-0113 log",
        "docs/decisions/0114.md",
        "see ../docs/plan.md",
        "see (docs/proposals/x)",
        "Phase 7as (c)",
        "(Phase 4d)",
        "Phase 8l: the skill",
        "**Phase 7, as built**",
        "Phase 8f amendment",
        "the lock (ADR 0114, 8e amendment)",
        "the 8f amendment)",
        "not yet on `main`",
        "not yet on main",
    ] {
        assert!(hit(l), "{l}");
    }
    // What it lets through.
    for l in [
        "an ADRIFT boat, READRs, ADR",
        "an ADR",
        "the --phase live flag",
        "a two-phase 2 commit",
        "phase 2 of the rollout",
        "the second phase 3 times",
        "pricing-plan.md",
        "https://example.com/docs/plan/x",
        "mydocs/plan and my-docs/decisions",
        "the 2nd amendment",
        "an 8f amendment of the rules",
    ] {
        assert!(!hit(l), "{l}");
    }
}
