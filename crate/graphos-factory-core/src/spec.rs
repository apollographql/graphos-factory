//! Description documents: load, detect the dialect, normalise to one model.
//!
//! Every reader hands the inventory builder the same thing — an OpenAPI 3
//! document. `openapi.rs` reads OpenAPI 3.x as it is; `swagger.rs` reads
//! Swagger 2.0 and rewrites it into that model. This module is the front
//! door: `read` loads JSON or YAML, names the format, and routes it, so no
//! consumer has to know which dialect a file was.

use serde_json::Value;
use std::fmt;

/// What a document declares itself to be.
#[derive(Debug, Clone, PartialEq)]
pub enum Format {
    /// `openapi: 3.x.y`
    OpenApi(String),
    /// `swagger: 2.0`
    Swagger(String),
    /// Neither key; read as OpenAPI 3 with a warning.
    Undeclared,
}

impl fmt::Display for Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Format::OpenApi(v) => write!(f, "OpenAPI {}", v),
            Format::Swagger(v) => write!(f, "Swagger {}", v),
            Format::Undeclared => write!(f, "format undeclared"),
        }
    }
}

/// JavaScript `String(v)` for scalars: a YAML `2.0` that parsed as a number
/// prints as the version it is.
pub fn value_to_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "null".to_string(),
        other => crate::json::compact(other),
    }
}

/// The dialect a document declares, from its top-level key. The version may
/// be a YAML number (`swagger: 2.0` unquoted), so it is stringified, never
/// required to be a string.
pub fn detect(doc: &Value) -> Format {
    if let Some(v) = crate::json::get(doc, "swagger") {
        return Format::Swagger(value_to_string(v));
    }
    if let Some(v) = crate::json::get(doc, "openapi") {
        return Format::OpenApi(value_to_string(v));
    }
    Format::Undeclared
}

/// Parse a document's text, JSON or YAML.
pub fn load(text: &str, filename: &str) -> Result<Value, String> {
    if text.trim_start().starts_with('{') {
        return crate::json::parse(text);
    }
    let doc = crate::yaml::parse(text)?;
    if !doc.is_object() {
        return Err(format!("{} is not an API description document", filename));
    }
    Ok(doc)
}

/// A document in the OpenAPI 3 model, with what it was read from.
pub struct Loaded {
    pub format: Format,
    /// OpenAPI 3, whatever the input was.
    pub document: Value,
    pub warnings: Vec<String>,
}

/// Route a parsed document to its reader and return the OpenAPI 3 model.
pub fn normalise(doc: Value) -> Result<Loaded, String> {
    let format = detect(&doc);
    match &format {
        Format::Swagger(_) => {
            let converted = crate::swagger::to_openapi3(&doc)?;
            Ok(Loaded {
                format,
                document: converted.document,
                warnings: converted.warnings,
            })
        }
        Format::OpenApi(_) => Ok(Loaded {
            format,
            document: doc,
            warnings: vec![],
        }),
        Format::Undeclared => Ok(Loaded {
            format,
            document: doc,
            warnings: vec![
                "the document declares neither `openapi` nor `swagger`; it was read as OpenAPI 3"
                    .to_string(),
            ],
        }),
    }
}

/// Load and normalise in one step.
pub fn read(text: &str, filename: &str) -> Result<Loaded, String> {
    normalise(load(text, filename)?)
}
