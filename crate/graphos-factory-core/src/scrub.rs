//! Scrubbing: credentials and personal data out of anything before it is
//! written to a workspace. Every recorded sample goes through here, and
//! `probe` refuses to write when something that still looks like a secret
//! survives — a committed token is unrecoverable, a re-run probe is cheap.
//!
//! Two mechanisms: structural (any key whose name says "credential" has its
//! value replaced wholesale) and textual (known token shapes and e-mail
//! addresses are replaced inside strings; e-mails deterministically per
//! input). A residual detector then flags long high-entropy tokens that
//! matched nothing.

use regex::Regex;
use serde_json::{Map, Value};
use std::collections::HashMap;

pub struct ScrubResult {
    pub value: Value,
    pub replacements: Map<String, Value>,
    pub residual: Vec<Residual>,
}

#[derive(Debug, Clone)]
pub struct Residual {
    pub path: String,
    pub sample: String,
}

impl ScrubResult {
    pub fn has_residual(&self) -> bool {
        !self.residual.is_empty()
    }
}

struct Pattern {
    name: &'static str,
    re: Regex,
    replace: &'static str,
}

fn patterns() -> Vec<Pattern> {
    let p = |name, re: &str, replace| Pattern {
        name,
        re: Regex::new(re).unwrap(),
        replace,
    };
    vec![
        p(
            "private-key",
            r"(?s)-----BEGIN [A-Z ]*PRIVATE KEY-----.*?-----END [A-Z ]*PRIVATE KEY-----",
            "<redacted:private-key>",
        ),
        p(
            "bearer",
            r"\bBearer\s+[A-Za-z0-9\-._~+/]{16,}=*",
            "Bearer <redacted>",
        ),
        p(
            "basic",
            r"\bBasic\s+[A-Za-z0-9+/]{16,}=*",
            "Basic <redacted>",
        ),
        p(
            "jwt",
            r"\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\b",
            "<redacted:jwt>",
        ),
        p(
            "stripe-key",
            r"\b[srp]k_(?:live|test)_[A-Za-z0-9]{8,}\b",
            "<redacted:stripe-key>",
        ),
        p(
            "aws-access-key",
            r"\bAKIA[0-9A-Z]{16}\b",
            "<redacted:aws-key>",
        ),
        p(
            "github-token",
            r"\bgh[opsur]_[A-Za-z0-9]{20,}\b",
            "<redacted:github-token>",
        ),
        p(
            "slack-token",
            r"\bxox[abeprs]-[A-Za-z0-9-]{10,}\b",
            "<redacted:slack-token>",
        ),
        p(
            "google-api-key",
            r"\bAIza[0-9A-Za-z_-]{35}\b",
            "<redacted:google-key>",
        ),
        p(
            "pagerduty-token-header",
            r"\bToken token=[A-Za-z0-9+/_-]{8,}",
            "Token token=<redacted>",
        ),
    ]
}

fn sensitive_key() -> Regex {
    Regex::new(r"(?i)^(authorization|proxy-authorization|cookie|set-cookie|x-api-key|api[-_]?key|apikey|access[-_]?token|refresh[-_]?token|id[-_]?token|token|secret|client[-_]?secret|password|passwd|private[-_]?key|credentials?)$").unwrap()
}

/// Whether a key name (a header, a JSON key, a query parameter) says its
/// value is a credential: `authorization`, `api_key`, `token`, `password`, ...
pub fn is_sensitive_key(name: &str) -> bool {
    sensitive_key().is_match(name)
}

fn email() -> Regex {
    Regex::new(r"\b([A-Za-z0-9._%+-]+)@([A-Za-z0-9.-]+\.[A-Za-z]{2,})\b").unwrap()
}

fn safe_email_domain(domain: &str) -> bool {
    Regex::new(r"(?i)(^|\.)(example\.(com|org|net|test)|test|invalid|localhost)$")
        .unwrap()
        .is_match(domain)
}

fn looks_like_secret(s: &str) -> bool {
    let residual = Regex::new(r"^[A-Za-z0-9+/=_-]{32,}$").unwrap();
    if !residual.is_match(s) {
        return false;
    }
    let all_digits = s.chars().all(|c| c.is_ascii_digit());
    let hex32 = Regex::new(r"(?i)^[a-f0-9]{32,}$").unwrap().is_match(s);
    let upper_snake = Regex::new(r"^[A-Z_]+$").unwrap().is_match(s);
    if all_digits || (!hex32 && upper_snake) {
        return false;
    }
    let classes = [
        s.chars().any(|c| c.is_ascii_lowercase()),
        s.chars().any(|c| c.is_ascii_uppercase()),
        s.chars().any(|c| c.is_ascii_digit()),
    ]
    .iter()
    .filter(|&&b| b)
    .count();
    classes >= 2 || Regex::new(r"(?i)^[a-f0-9]{40,}$").unwrap().is_match(s)
}

struct State {
    replacements: Map<String, Value>,
    residual: Vec<Residual>,
    emails: HashMap<String, String>,
    extra: Vec<Regex>,
    patterns: Vec<Pattern>,
    sensitive: Regex,
    email: Regex,
}

impl State {
    fn bump(&mut self, name: &str, n: u64) {
        let prev = self
            .replacements
            .get(name)
            .and_then(Value::as_u64)
            .unwrap_or(0);
        self.replacements
            .insert(name.to_string(), Value::from(prev + n));
    }
}

/// Scrub a JSON value (any shape); returns a new value plus what was replaced.
pub fn scrub(value: &Value, extra: &[Regex], path: &str) -> ScrubResult {
    let mut state = State {
        replacements: Map::new(),
        residual: Vec::new(),
        emails: HashMap::new(),
        extra: extra.to_vec(),
        patterns: patterns(),
        sensitive: sensitive_key(),
        email: email(),
    };
    let out = walk(value, path, &mut state, false);
    ScrubResult {
        value: out,
        replacements: state.replacements,
        residual: state.residual,
    }
}

fn walk(v: &Value, path: &str, state: &mut State, under_sensitive: bool) -> Value {
    match v {
        Value::Null => Value::Null,
        Value::String(s) => Value::String(scrub_string(s, path, state, under_sensitive)),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .enumerate()
                .map(|(i, item)| walk(item, &format!("{}[{}]", path, i), state, under_sensitive))
                .collect(),
        ),
        Value::Object(map) => {
            let mut out = Map::new();
            for (key, item) in map {
                let child_path = format!("{}.{}", path, key);
                let is_sensitive = state.sensitive.is_match(key);
                if is_sensitive && !item.is_null() && !item.is_object() && !item.is_array() {
                    let lower = key.to_lowercase();
                    state.bump(&format!("key:{}", lower), 1);
                    out.insert(key.clone(), Value::String(format!("<redacted:{}>", lower)));
                    continue;
                }
                out.insert(
                    key.clone(),
                    walk(item, &child_path, state, under_sensitive || is_sensitive),
                );
            }
            Value::Object(out)
        }
        other => other.clone(),
    }
}

fn scrub_string(s: &str, path: &str, state: &mut State, under_sensitive: bool) -> String {
    let mut out = s.to_string();
    let pats = std::mem::take(&mut state.patterns);
    for p in &pats {
        let count = p.re.find_iter(&out).count() as u64;
        if count > 0 {
            state.bump(p.name, count);
            out = p.re.replace_all(&out, p.replace).to_string();
        }
    }
    state.patterns = pats;
    let extras = std::mem::take(&mut state.extra);
    for re in &extras {
        let count = re.find_iter(&out).count() as u64;
        if count > 0 {
            state.bump(&format!("extra:{}", re.as_str()), count);
            out = re.replace_all(&out, "<redacted>").to_string();
        }
    }
    state.extra = extras;

    let email_re = state.email.clone();
    let mut replaced = String::with_capacity(out.len());
    let mut last = 0;
    for m in email_re.captures_iter(&out) {
        let whole = m.get(0).unwrap();
        replaced.push_str(&out[last..whole.start()]);
        let domain = &m[2];
        if safe_email_domain(domain) {
            replaced.push_str(whole.as_str());
        } else {
            let key = whole.as_str().to_string();
            let n = state.emails.len() + 1;
            let placeholder = state
                .emails
                .entry(key)
                .or_insert_with(|| format!("person{}@example.test", n))
                .clone();
            state.bump("email", 1);
            replaced.push_str(&placeholder);
        }
        last = whole.end();
    }
    replaced.push_str(&out[last..]);
    out = replaced;

    if under_sensitive && out == s && !out.is_empty() {
        state.bump("sensitive-context", 1);
        return "<redacted>".to_string();
    }
    let is_url = Regex::new(r"(?i)^https?://").unwrap().is_match(&out);
    if !is_url && !out.contains('/') && looks_like_secret(&out) {
        let chars: Vec<char> = out.chars().collect();
        let head: String = chars.iter().take(6).collect();
        let tail: String = chars
            .iter()
            .rev()
            .take(4)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        state.residual.push(Residual {
            path: path.to_string(),
            sample: format!("{}…{} ({} chars)", head, tail, chars.len()),
        });
    }
    out
}
