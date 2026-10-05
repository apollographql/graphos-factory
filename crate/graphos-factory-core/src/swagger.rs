//! Swagger 2.0 -> the OpenAPI 3 model the inventory builder reads.
//!
//! A reader for the 2.0 dialect. It rewrites a Swagger document into an
//! OpenAPI 3 document — nothing more — so `openapi.rs` reads one model and
//! the inventory is the same whichever dialect the vendor published:
//!
//! - `host` / `basePath` / `schemes`            -> `servers[0].url` (no `host`: no servers; the builder warns)
//! - `definitions`, `parameters`, `responses`,
//!   `securityDefinitions`                       -> `components.*`, every `$ref` rewritten
//! - a `body` parameter                          -> `requestBody`, content type from `consumes`
//!   (operation over root, empty operation list falls through; a wildcard or no
//!   `consumes` means JSON; JSON first, then form, else the first listed)
//! - `formData` parameters                       -> one `requestBody`: `application/x-www-form-urlencoded`
//!   with an object schema, or `multipart/form-data` when a part is `type: file`
//!   or the API accepts only multipart; a later same-named part replaces the
//!   earlier one in place (an operation overriding a path-level parameter)
//! - both a body and formData parameters         -> the body wins; the parts are named in
//!   `x-factory-unmodelled-parameters` so the builder reports them, never silently
//! - other parameters                            -> `schema` from the parameter's own type keys;
//!   `collectionFormat` -> `style`/`explode`
//! - a response `schema`                         -> `content[<produces>].schema`; `headers` gain a `schema`
//! - `type: file`                                -> `string`/`binary`;  `x-nullable` -> `nullable`;
//!   a string `discriminator`                    -> `{ propertyName }`
//! - the root `security` requirement             -> copied as is (same key and grammar in both dialects);
//!   an `oauth2` definition's `accessCode` flow   -> `flows.authorizationCode`
//!
//! Anything not in this list is copied through unchanged, `x-*` included.

use crate::json::{get, get_arr, get_obj, get_str, obj, truthy, Object};
use serde_json::Value;

pub struct Converted {
    pub document: Value,
    pub warnings: Vec<String>,
}

const METHODS: [&str; 8] = [
    "get", "put", "post", "delete", "patch", "head", "options", "trace",
];
const PARAM_SCHEMA_KEYS: [&str; 20] = [
    "type",
    "format",
    "items",
    "enum",
    "default",
    "minimum",
    "maximum",
    "exclusiveMinimum",
    "exclusiveMaximum",
    "minLength",
    "maxLength",
    "pattern",
    "minItems",
    "maxItems",
    "uniqueItems",
    "multipleOf",
    "title",
    "example",
    "x-nullable",
    "readOnly",
];

fn json_content(t: &str) -> bool {
    crate::openapi::is_json_content_type(t)
}
fn form_content(t: &str) -> bool {
    crate::openapi::is_form_content_type(t)
}

/// Convert a Swagger 2.0 document. Fails only on a document that is not an
/// object; a broken `$ref` is left for the builder, which records it.
pub fn to_openapi3(doc: &Value) -> Result<Converted, String> {
    let src = doc
        .as_object()
        .ok_or_else(|| "the Swagger document is not an object".to_string())?;
    let mut out = obj();
    out.insert("openapi".into(), Value::from("3.0.3"));
    if let Some(info) = src.get("info") {
        out.insert("info".into(), info.clone());
    }
    if let Some(host) = get_str(doc, "host") {
        let scheme = get_arr(doc, "schemes")
            .and_then(|s| s.first())
            .and_then(Value::as_str)
            .unwrap_or("https");
        out.insert(
            "servers".into(),
            Value::Array(vec![crate::json::object(vec![(
                "url",
                Value::from(format!(
                    "{}://{}{}",
                    scheme,
                    host,
                    get_str(doc, "basePath").unwrap_or("")
                )),
            )])]),
        );
    }
    for key in ["tags", "externalDocs", "security"] {
        if let Some(v) = src.get(key) {
            out.insert(key.into(), v.clone());
        }
    }
    for (k, v) in src {
        if k.starts_with("x-") {
            out.insert(k.clone(), v.clone());
        }
    }
    // The document-level security requirement has the same key and the
    // same grammar in both dialects (alternatives, each scheme -> scopes).
    if let Some(security) = src.get("security") {
        out.insert("security".into(), security.clone());
    }

    let root_consumes = string_list(get_arr(doc, "consumes"));
    let root_produces = string_list(get_arr(doc, "produces"));
    let root_params = get_obj(doc, "parameters").cloned().unwrap_or_default();

    // Paths.
    let mut paths = obj();
    for (path, item) in get_obj(doc, "paths").into_iter().flatten() {
        let item_obj = match item.as_object() {
            Some(i) => i,
            None => {
                paths.insert(path.clone(), item.clone());
                continue;
            }
        };
        let shared: Vec<Value> = get_arr(item, "parameters").cloned().unwrap_or_default();
        let mut new_item = obj();
        for (k, v) in item_obj {
            if METHODS.contains(&k.as_str()) || k == "parameters" {
                continue;
            }
            new_item.insert(k.clone(), v.clone());
        }
        // Path-level parameters that are not a body: converted in place. Body
        // and formData parts are folded into each operation below.
        let shared_plain: Vec<Value> = shared
            .iter()
            .filter(|p| !is_body_like(p, &root_params))
            .map(|p| convert_parameter(p))
            .collect();
        if !shared_plain.is_empty() {
            new_item.insert("parameters".into(), Value::Array(shared_plain));
        }
        for method in METHODS {
            if let Some(op) = item_obj.get(method) {
                new_item.insert(
                    method.into(),
                    convert_operation(op, &shared, &root_params, &root_consumes, &root_produces),
                );
            }
        }
        paths.insert(path.clone(), Value::Object(new_item));
    }
    out.insert("paths".into(), Value::Object(paths));

    // Components.
    let mut components = obj();
    if let Some(defs) = src.get("definitions") {
        components.insert("schemas".into(), defs.clone());
    }
    let mut comp_params = obj();
    for (name, p) in &root_params {
        if !is_body_like(p, &root_params) {
            comp_params.insert(name.clone(), convert_parameter(p));
        }
    }
    if !comp_params.is_empty() {
        components.insert("parameters".into(), Value::Object(comp_params));
    }
    if let Some(responses) = get_obj(doc, "responses") {
        let mut comp_responses = obj();
        for (name, r) in responses {
            comp_responses.insert(name.clone(), convert_response(r, &root_produces));
        }
        components.insert("responses".into(), Value::Object(comp_responses));
    }
    if let Some(defs) = get_obj(doc, "securityDefinitions") {
        let mut schemes = obj();
        for (name, d) in defs {
            schemes.insert(name.clone(), convert_security(d));
        }
        components.insert("securitySchemes".into(), Value::Object(schemes));
    }
    if !components.is_empty() {
        out.insert("components".into(), Value::Object(components));
    }

    let mut document = Value::Object(out);
    rewrite_refs(&mut document);
    convert_schemas(&mut document);
    Ok(Converted {
        document,
        warnings: vec![],
    })
}

fn string_list(v: Option<&Vec<Value>>) -> Vec<String> {
    v.map(|a| {
        a.iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect()
    })
    .unwrap_or_default()
}

/// Follow a `$ref: #/parameters/X` to the root parameter it names.
fn resolve_param<'a>(p: &'a Value, root_params: &'a Object) -> &'a Value {
    match get_str(p, "$ref").and_then(|r| r.strip_prefix("#/parameters/")) {
        Some(name) => root_params
            .get(&name.replace("~1", "/").replace("~0", "~"))
            .unwrap_or(p),
        None => p,
    }
}

fn location(p: &Value, root_params: &Object) -> Option<String> {
    get_str(resolve_param(p, root_params), "in").map(str::to_string)
}

fn is_body_like(p: &Value, root_params: &Object) -> bool {
    matches!(
        location(p, root_params).as_deref(),
        Some("body") | Some("formData")
    )
}

/// A non-body parameter in the OpenAPI 3 shape: the type keys move under
/// `schema`, `collectionFormat` becomes `style`/`explode`.
fn convert_parameter(p: &Value) -> Value {
    if get(p, "$ref").is_some() {
        return p.clone();
    }
    let src = match p.as_object() {
        Some(s) => s,
        None => return p.clone(),
    };
    let mut out = obj();
    let mut schema = obj();
    for (k, v) in src {
        if PARAM_SCHEMA_KEYS.contains(&k.as_str()) {
            schema.insert(k.clone(), v.clone());
        } else if k != "collectionFormat" {
            out.insert(k.clone(), v.clone());
        }
    }
    if let Some(cf) = get_str(p, "collectionFormat") {
        let location = get_str(p, "in").unwrap_or("query");
        let (style, explode) = match (cf, location) {
            ("multi", _) => ("form", true),
            ("ssv", _) => ("spaceDelimited", false),
            ("pipes", _) => ("pipeDelimited", false),
            (_, "path") | (_, "header") => ("simple", false),
            _ => ("form", false),
        };
        out.insert("style".into(), Value::from(style));
        out.insert("explode".into(), Value::Bool(explode));
    }
    if !schema.is_empty() {
        out.insert("schema".into(), Value::Object(schema));
    }
    Value::Object(out)
}

/// The content type a body travels as, from `consumes` (operation list when
/// it names anything, else the root's). See the module docs for the rules.
fn body_content_type(
    op_consumes: &[String],
    root_consumes: &[String],
    form_data: bool,
    has_file: bool,
) -> String {
    let listed: &[String] = if op_consumes.is_empty() {
        root_consumes
    } else {
        op_consumes
    };
    let bare = |t: &str| t.split(';').next().unwrap_or(t).trim().to_ascii_lowercase();
    if form_data {
        if has_file {
            return "multipart/form-data".into();
        }
        if !listed.iter().any(|t| form_content(t)) {
            if let Some(m) = listed.iter().find(|t| bare(t).starts_with("multipart/")) {
                return bare(m);
            }
        }
        return "application/x-www-form-urlencoded".into();
    }
    if listed.is_empty() {
        return "application/json".into();
    }
    if let Some(t) = listed.iter().find(|t| json_content(t)) {
        return t.clone();
    }
    if listed
        .iter()
        .any(|t| matches!(bare(t).as_str(), "*/*" | "application/*"))
    {
        return "application/json".into();
    }
    if let Some(t) = listed.iter().find(|t| form_content(t)) {
        return t.clone();
    }
    listed[0].clone()
}

/// The object schema a set of formData parts describes; a later part with
/// the same name replaces the earlier one in place.
fn form_schema(parts: &[&Value]) -> Value {
    let mut ordered: Vec<(String, &Value)> = Vec::new();
    for p in parts {
        let name = match get_str(p, "name") {
            Some(n) => n.to_string(),
            None => continue,
        };
        match ordered.iter_mut().find(|(n, _)| *n == name) {
            Some(slot) => slot.1 = p,
            None => ordered.push((name, p)),
        }
    }
    let mut properties = obj();
    let mut required: Vec<Value> = Vec::new();
    for (name, p) in &ordered {
        let mut field = obj();
        for key in PARAM_SCHEMA_KEYS.iter().chain(["description"].iter()) {
            if let Some(v) = get(p, key) {
                field.insert((*key).into(), v.clone());
            }
        }
        properties.insert(name.clone(), Value::Object(field));
        if truthy(get(p, "required")) {
            required.push(Value::from(name.as_str()));
        }
    }
    let mut schema = obj();
    schema.insert("type".into(), Value::from("object"));
    schema.insert("properties".into(), Value::Object(properties));
    if !required.is_empty() {
        schema.insert("required".into(), Value::Array(required));
    }
    Value::Object(schema)
}

fn convert_operation(
    op: &Value,
    shared: &[Value],
    root_params: &Object,
    root_consumes: &[String],
    root_produces: &[String],
) -> Value {
    let src = match op.as_object() {
        Some(s) => s,
        None => return op.clone(),
    };
    let mut out = obj();
    for (k, v) in src {
        if matches!(
            k.as_str(),
            "parameters" | "responses" | "consumes" | "produces" | "schemes"
        ) {
            continue;
        }
        out.insert(k.clone(), v.clone());
    }
    let op_consumes = string_list(get_arr(op, "consumes"));
    let produces = {
        let own = string_list(get_arr(op, "produces"));
        if own.is_empty() {
            root_produces.to_vec()
        } else {
            own
        }
    };

    // Parameters: path-level first, the operation's own after (an override
    // by name + location replaces the earlier one, as the 2.0 spec says).
    let own: Vec<Value> = get_arr(op, "parameters").cloned().unwrap_or_default();
    let mut merged: Vec<Value> = Vec::new();
    for p in shared.iter().chain(own.iter()) {
        let r = resolve_param(p, root_params);
        let key = (
            get_str(r, "name").unwrap_or("").to_string(),
            get_str(r, "in").unwrap_or("").to_string(),
        );
        match merged.iter().position(|m| {
            let mr = resolve_param(m, root_params);
            (
                get_str(mr, "name").unwrap_or("").to_string(),
                get_str(mr, "in").unwrap_or("").to_string(),
            ) == key
        }) {
            Some(i) => merged[i] = p.clone(),
            None => merged.push(p.clone()),
        }
    }
    let plain: Vec<Value> = merged
        .iter()
        .filter(|p| !is_body_like(p, root_params))
        .map(|p| convert_parameter(p))
        .collect();
    let body: Option<Value> = merged
        .iter()
        .find(|p| location(p, root_params).as_deref() == Some("body"))
        .map(|p| resolve_param(p, root_params).clone());
    let form: Vec<Value> = merged
        .iter()
        .filter(|p| location(p, root_params).as_deref() == Some("formData"))
        .map(|p| resolve_param(p, root_params).clone())
        .collect();
    if !plain.is_empty() {
        out.insert("parameters".into(), Value::Array(plain));
    }

    if let Some(b) = &body {
        let ct = body_content_type(&op_consumes, root_consumes, false, false);
        let mut media = obj();
        if let Some(s) = get(b, "schema") {
            media.insert("schema".into(), s.clone());
        }
        let mut rb = obj();
        if let Some(d) = get(b, "description") {
            rb.insert("description".into(), d.clone());
        }
        rb.insert("required".into(), Value::Bool(truthy(get(b, "required"))));
        rb.insert(
            "content".into(),
            Value::Object({
                let mut c = obj();
                c.insert(ct, Value::Object(media));
                c
            }),
        );
        out.insert("requestBody".into(), Value::Object(rb));
        if !form.is_empty() {
            // A body parameter and formData parts on one operation is invalid
            // 2.0 but seen in the wild: the body wins, the parts are reported.
            let names: Vec<Value> = form
                .iter()
                .map(|p| {
                    Value::from(format!(
                        "{} (formData, beside a body parameter)",
                        get_str(p, "name").unwrap_or("?")
                    ))
                })
                .collect();
            out.insert(
                "x-factory-unmodelled-parameters".into(),
                Value::Array(names),
            );
        }
    } else if !form.is_empty() {
        let parts: Vec<&Value> = form.iter().collect();
        let has_file = parts.iter().any(|p| get_str(p, "type") == Some("file"));
        let ct = body_content_type(&op_consumes, root_consumes, true, has_file);
        let mut rb = obj();
        rb.insert(
            "required".into(),
            Value::Bool(parts.iter().any(|p| truthy(get(p, "required")))),
        );
        let mut media = obj();
        media.insert("schema".into(), form_schema(&parts));
        rb.insert(
            "content".into(),
            Value::Object({
                let mut c = obj();
                c.insert(ct, Value::Object(media));
                c
            }),
        );
        out.insert("requestBody".into(), Value::Object(rb));
    }

    if let Some(responses) = get_obj(op, "responses") {
        let mut new_responses = obj();
        for (status, r) in responses {
            new_responses.insert(status.clone(), convert_response(r, &produces));
        }
        out.insert("responses".into(), Value::Object(new_responses));
    }
    Value::Object(out)
}

/// The media type a schema-described response body carries, from `produces`:
/// JSON first; a wildcard (`*/*`, `application/*`) means JSON, as it does for
/// `consumes`; else the first listed; nothing listed means JSON.
fn response_content_type(produces: &[String]) -> String {
    let bare = |t: &str| t.split(';').next().unwrap_or(t).trim().to_ascii_lowercase();
    if let Some(t) = produces.iter().find(|t| json_content(t)) {
        return t.clone();
    }
    if produces
        .iter()
        .any(|t| matches!(bare(t).as_str(), "*/*" | "application/*"))
    {
        return "application/json".into();
    }
    produces
        .first()
        .cloned()
        .unwrap_or_else(|| "application/json".to_string())
}

fn convert_response(r: &Value, produces: &[String]) -> Value {
    if get(r, "$ref").is_some() {
        return r.clone();
    }
    let src = match r.as_object() {
        Some(s) => s,
        None => return r.clone(),
    };
    let mut out = obj();
    for (k, v) in src {
        if matches!(k.as_str(), "schema" | "headers" | "examples") {
            continue;
        }
        out.insert(k.clone(), v.clone());
    }
    if let Some(headers) = get_obj(r, "headers") {
        let mut new_headers = obj();
        for (name, h) in headers {
            new_headers.insert(name.clone(), convert_header(h));
        }
        out.insert("headers".into(), Value::Object(new_headers));
    }
    if let Some(schema) = get(r, "schema") {
        let ct = response_content_type(produces);
        let mut media = obj();
        media.insert("schema".into(), schema.clone());
        if let Some(example) = get_obj(r, "examples").and_then(|e| e.get(&ct)) {
            media.insert("example".into(), example.clone());
        }
        let mut content = obj();
        content.insert(ct, Value::Object(media));
        out.insert("content".into(), Value::Object(content));
    }
    Value::Object(out)
}

fn convert_header(h: &Value) -> Value {
    let src = match h.as_object() {
        Some(s) => s,
        None => return h.clone(),
    };
    let mut out = obj();
    let mut schema = obj();
    for (k, v) in src {
        if PARAM_SCHEMA_KEYS.contains(&k.as_str()) {
            schema.insert(k.clone(), v.clone());
        } else if k != "collectionFormat" {
            out.insert(k.clone(), v.clone());
        }
    }
    if !schema.is_empty() {
        out.insert("schema".into(), Value::Object(schema));
    }
    Value::Object(out)
}

fn convert_security(d: &Value) -> Value {
    let kind = get_str(d, "type").unwrap_or("").to_lowercase();
    let mut out = obj();
    match kind.as_str() {
        "basic" => {
            out.insert("type".into(), Value::from("http"));
            out.insert("scheme".into(), Value::from("basic"));
        }
        "apikey" => {
            out.insert("type".into(), Value::from("apiKey"));
            for k in ["name", "in"] {
                if let Some(v) = get(d, k) {
                    out.insert(k.into(), v.clone());
                }
            }
        }
        "oauth2" => {
            out.insert("type".into(), Value::from("oauth2"));
            let flow_name = match get_str(d, "flow") {
                Some("implicit") => "implicit",
                Some("password") => "password",
                Some("application") => "clientCredentials",
                Some("accessCode") => "authorizationCode",
                _ => "implicit",
            };
            let mut flow = obj();
            for (from, to) in [
                ("authorizationUrl", "authorizationUrl"),
                ("tokenUrl", "tokenUrl"),
                ("scopes", "scopes"),
            ] {
                if let Some(v) = get(d, from) {
                    flow.insert(to.into(), v.clone());
                }
            }
            let mut flows = obj();
            flows.insert(flow_name.into(), Value::Object(flow));
            out.insert("flows".into(), Value::Object(flows));
        }
        _ => {
            if let Some(t) = get(d, "type") {
                out.insert("type".into(), t.clone());
            }
        }
    }
    if let Some(desc) = get(d, "description") {
        out.insert("description".into(), desc.clone());
    }
    Value::Object(out)
}

/// `#/definitions/X` -> `#/components/schemas/X`, and the same for
/// `parameters` and `responses`, everywhere in the document.
fn rewrite_refs(v: &mut Value) {
    match v {
        Value::Object(m) => {
            if let Some(Value::String(r)) = m.get_mut("$ref") {
                for (from, to) in [
                    ("#/definitions/", "#/components/schemas/"),
                    ("#/parameters/", "#/components/parameters/"),
                    ("#/responses/", "#/components/responses/"),
                ] {
                    if let Some(rest) = r.strip_prefix(from) {
                        *r = format!("{}{}", to, rest);
                        break;
                    }
                }
            }
            for x in m.values_mut() {
                rewrite_refs(x);
            }
        }
        Value::Array(a) => a.iter_mut().for_each(rewrite_refs),
        _ => {}
    }
}

/// The 2.0 schema spellings, everywhere in the document: `type: file`,
/// `x-nullable`, a string `discriminator`.
fn convert_schemas(v: &mut Value) {
    match v {
        Value::Object(m) => {
            if m.get("type").and_then(Value::as_str) == Some("file") {
                m.insert("type".into(), Value::from("string"));
                m.insert("format".into(), Value::from("binary"));
            }
            if let Some(n) = m.shift_remove("x-nullable") {
                m.entry("nullable").or_insert(n);
            }
            if let Some(Value::String(prop)) = m.get("discriminator").cloned() {
                m.insert(
                    "discriminator".into(),
                    crate::json::object(vec![("propertyName", Value::from(prop))]),
                );
            }
            for x in m.values_mut() {
                convert_schemas(x);
            }
        }
        Value::Array(a) => a.iter_mut().for_each(convert_schemas),
        _ => {}
    }
}
