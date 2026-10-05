//! An index over the schema text, read once: every enum's values and each
//! type's field declarations as they are asked for. Shared by `scaffold`
//! (which samples enum arguments and aligns fixture leaves to the schema's
//! enums) and lint's wire-enum audit (which compares them to the spec's).

use regex::Regex;
use std::collections::HashMap;
use std::sync::OnceLock;

fn re(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).unwrap())
}
static ENUM_RE: OnceLock<Regex> = OnceLock::new();
static FIELD_TYPE_RE: OnceLock<Regex> = OnceLock::new();

/// `[Widget!]!` → `Widget`.
pub fn base_type(t: &str) -> String {
    t.chars()
        .filter(|c| !matches!(c, '[' | ']' | '!'))
        .collect()
}

pub struct SdlIndex {
    sdl: String,
    enums: HashMap<String, Vec<String>>,
    fields: HashMap<String, HashMap<String, String>>,
}

impl SdlIndex {
    pub fn new(sdl: &str) -> SdlIndex {
        let code = crate::graphql::blank(sdl);
        let mut enums = HashMap::new();
        for m in
            re(&ENUM_RE, r"\benum\s+([A-Za-z_][A-Za-z0-9_]*)\s*\{([^}]*)\}").captures_iter(&code)
        {
            let values: Vec<String> = m[2]
                .split_whitespace()
                .filter(|w| w.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
                .map(str::to_string)
                .collect();
            if !values.is_empty() {
                enums.insert(m[1].to_string(), values);
            }
        }
        SdlIndex {
            sdl: sdl.to_string(),
            enums,
            fields: HashMap::new(),
        }
    }

    /// `enum Name { a b }` → the values, in order; None for any other type.
    pub fn enum_values(&self, name: &str) -> Option<&Vec<String>> {
        self.enums.get(name)
    }

    /// `type Name { field: Type }` → the declared type text of `field`.
    pub fn field_type(&mut self, type_name: &str, field: &str) -> Option<String> {
        if !self.fields.contains_key(type_name) {
            let mut map = HashMap::new();
            if let Some(body) = crate::graphql::type_body(&self.sdl, type_name) {
                let code = crate::graphql::blank(&body.body);
                for m in re(
                    &FIELD_TYPE_RE,
                    r"(?m)^\s*([A-Za-z_][A-Za-z0-9_]*)\s*(?:\([^)]*\))?\s*:\s*([\[\]!A-Za-z0-9_]+)",
                )
                .captures_iter(&code)
                {
                    map.entry(m[1].to_string())
                        .or_insert_with(|| m[2].to_string());
                }
            }
            self.fields.insert(type_name.to_string(), map);
        }
        self.fields
            .get(type_name)
            .and_then(|m| m.get(field))
            .cloned()
    }
}
