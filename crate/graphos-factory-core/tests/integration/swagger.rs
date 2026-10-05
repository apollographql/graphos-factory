//! The Swagger 2.0 reader: what it rewrites into the OpenAPI 3 model, and
//! the inventory that model then produces.

use graphos_factory_core::jsonschema::validate;
use graphos_factory_core::openapi::build_inventory;
use graphos_factory_core::schemas;
use graphos_factory_core::spec::{self, Format};
use graphos_factory_core::swagger::to_openapi3;
use serde_json::{json, Value};

fn schema() -> Value {
    schemas::load("inventory.schema.json", None).unwrap()
}

/// Convert, then build — what `spec::read` does for a 2.0 file.
fn build(swagger: &Value) -> Value {
    let converted = to_openapi3(swagger).unwrap();
    let built = build_inventory(&converted.document).unwrap();
    assert_eq!(validate(&built.inventory, &schema()), Vec::<String>::new());
    built.inventory
}

fn convert(swagger: &Value) -> Value {
    to_openapi3(swagger).unwrap().document
}

// ── The conversion itself ───────────────────────────────────────────────────

#[test]
fn host_base_path_definitions_and_security_definitions_become_servers_and_components() {
    let swagger = json!({
        "swagger": "2.0", "info": {"title": "Legacy", "version": "1"}, "host": "api.legacy.test", "basePath": "/v2", "schemes": ["https", "http"],
        "definitions": {"Thing": {"type": "object", "properties": {"id": {"type": "string"}, "parent": {"$ref": "#/definitions/Thing"}}}},
        "parameters": {"Page": {"name": "page", "in": "query", "type": "integer"}},
        "responses": {"NotFound": {"description": "missing", "schema": {"$ref": "#/definitions/Thing"}}},
        "securityDefinitions": {
            "basicAuth": {"type": "basic"},
            "apiKey": {"type": "apiKey", "name": "X-Key", "in": "header"},
            "oauth": {"type": "oauth2", "flow": "accessCode", "authorizationUrl": "https://a", "tokenUrl": "https://t", "scopes": {"read": "r"}}
        },
        "x-vendor": {"kept": true},
        "paths": {"/things": {"get": {"parameters": [{"$ref": "#/parameters/Page"}], "responses": {"200": {"description": "ok", "schema": {"$ref": "#/definitions/Thing"}}, "404": {"$ref": "#/responses/NotFound"}}}}}
    });
    let d = convert(&swagger);
    assert_eq!(d["openapi"], "3.0.3");
    assert!(
        d.get("swagger").is_none() && d.get("host").is_none() && d.get("definitions").is_none()
    );
    assert_eq!(d["servers"], json!([{"url": "https://api.legacy.test/v2"}]));
    assert_eq!(
        d["components"]["schemas"]["Thing"]["properties"]["parent"]["$ref"],
        "#/components/schemas/Thing"
    );
    assert_eq!(
        d["components"]["parameters"]["Page"]["schema"]["type"],
        "integer"
    );
    assert_eq!(
        d["components"]["responses"]["NotFound"]["content"]["application/json"]["schema"]["$ref"],
        "#/components/schemas/Thing"
    );
    assert_eq!(
        d["components"]["securitySchemes"]["basicAuth"],
        json!({"type": "http", "scheme": "basic"})
    );
    assert_eq!(
        d["components"]["securitySchemes"]["apiKey"],
        json!({"type": "apiKey", "name": "X-Key", "in": "header"})
    );
    assert_eq!(
        d["components"]["securitySchemes"]["oauth"]["flows"]["authorizationCode"]["tokenUrl"],
        "https://t"
    );
    assert_eq!(d["x-vendor"]["kept"], true);
    let op = &d["paths"]["/things"]["get"];
    assert_eq!(op["parameters"][0]["$ref"], "#/components/parameters/Page");
    assert_eq!(
        op["responses"]["200"]["content"]["application/json"]["schema"]["$ref"],
        "#/components/schemas/Thing"
    );
    assert_eq!(
        op["responses"]["404"]["$ref"],
        "#/components/responses/NotFound"
    );

    // …and the inventory reads it as it read the OpenAPI 3 equivalent.
    let inv = build(&swagger);
    assert_eq!(
        inv["api"]["base_urls"],
        json!(["https://api.legacy.test/v2"])
    );
    assert_eq!(
        inv["operations"][0]["response"]["shape_ref"],
        "#/shapes/Thing"
    );
    let kinds: Vec<&str> = inv["api"]["auth"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, vec!["basic", "api_key", "oauth2"]);
}

#[test]
fn a_document_without_a_host_gets_no_servers_and_the_builder_warns() {
    let swagger = json!({"swagger": "2.0", "info": {"title": "Self-hosted", "version": "1"}, "basePath": "/api/v1", "paths": {}});
    let d = convert(&swagger);
    assert!(d.get("servers").is_none());
    let built = build_inventory(&d).unwrap();
    assert!(
        built.warnings.iter().any(|w| w.contains("no server URL")),
        "{:?}",
        built.warnings
    );
}

#[test]
fn parameters_move_their_type_under_schema_and_collection_format_becomes_style() {
    let swagger = json!({
        "swagger": "2.0", "info": {"title": "P", "version": "1"}, "host": "api.p.test",
        "paths": {"/t": {"get": {
            "parameters": [
                {"name": "ids", "in": "query", "type": "array", "items": {"type": "string"}, "collectionFormat": "multi", "description": "repeat me"},
                {"name": "tags", "in": "query", "type": "array", "items": {"type": "string"}, "collectionFormat": "csv"},
                {"name": "id", "in": "path", "required": true, "type": "string", "pattern": "^[a-z]+$", "x-nullable": false},
                {"name": "X-Trace", "in": "header", "type": "string", "collectionFormat": "csv"}
            ],
            "responses": {"200": {"description": "ok", "schema": {"type": "object", "properties": {"ok": {"type": "boolean"}}}}}
        }}}
    });
    let d = convert(&swagger);
    let p = &d["paths"]["/t"]["get"]["parameters"];
    assert_eq!(
        p[0],
        json!({"name": "ids", "in": "query", "description": "repeat me", "style": "form", "explode": true, "schema": {"type": "array", "items": {"type": "string"}}})
    );
    assert_eq!(
        (p[1]["style"].as_str(), p[1]["explode"].as_bool()),
        (Some("form"), Some(false))
    );
    assert_eq!(
        p[2]["schema"],
        json!({"type": "string", "pattern": "^[a-z]+$", "nullable": false}),
        "x-nullable is spelled nullable"
    );
    assert_eq!(p[3]["style"], "simple");
    // The builder flags the repeating array the way it flags an exploded OpenAPI 3 one.
    let inv = build(&swagger);
    assert_eq!(inv["operations"][0]["support"], "needs_review");
    assert!(inv["operations"][0]["support_reason"]
        .as_str()
        .unwrap()
        .contains("\"ids\""));
}

#[test]
fn a_body_parameter_becomes_a_request_body_with_the_consumes_content_type() {
    let mut swagger = json!({
        "swagger": "2.0", "info": {"title": "Bodies", "version": "1"}, "host": "api.bodies.test",
        "consumes": ["text/plain"],
        "definitions": {"Note": {"type": "object", "properties": {"text": {"type": "string", "x-nullable": true}}}},
        "paths": {"/notes": {"post": {
            "operationId": "createNote",
            "parameters": [{"name": "body", "in": "body", "required": true, "description": "the note", "schema": {"$ref": "#/definitions/Note"}}],
            "responses": {"200": {"description": "ok", "schema": {"$ref": "#/definitions/Note"}}}
        }}}
    });
    let d = convert(&swagger);
    let rb = &d["paths"]["/notes"]["post"]["requestBody"];
    assert_eq!(rb["required"], true);
    assert_eq!(rb["description"], "the note");
    assert_eq!(
        rb["content"]["text/plain"]["schema"]["$ref"],
        "#/components/schemas/Note"
    );
    assert!(
        d["paths"]["/notes"]["post"].get("parameters").is_none(),
        "the body parameter left the list"
    );
    assert_eq!(
        d["components"]["schemas"]["Note"]["properties"]["text"]["nullable"],
        true
    );
    let inv = build(&swagger);
    let o = &inv["operations"][0];
    assert_eq!(o["request_body"]["content_type"], "text/plain");
    assert_eq!(o["support"], "unsupported");
    assert!(o["support_reason"].as_str().unwrap().contains("text/plain"));
    assert_eq!(
        inv["shapes"]["Note"]["properties"]["text"]["nullable"],
        true
    );

    // The operation's own `consumes` wins over the root's, and a vendor JSON type counts as JSON.
    swagger["paths"]["/notes"]["post"]["consumes"] = json!(["application/vnd.notes+json"]);
    let inv = build(&swagger);
    assert_eq!(
        inv["operations"][0]["request_body"]["content_type"],
        "application/vnd.notes+json"
    );
    assert_eq!(inv["operations"][0]["support"], "supported");

    // A wildcard (go-swagger, Kubernetes) means JSON; an empty operation list falls through to the root.
    swagger["consumes"] = json!(["*/*"]);
    swagger["paths"]["/notes"]["post"]["consumes"] = json!([]);
    let inv = build(&swagger);
    assert_eq!(
        inv["operations"][0]["request_body"]["content_type"],
        "application/json"
    );
    assert_eq!(inv["operations"][0]["support"], "supported");
    swagger["consumes"] = json!(["application/x-www-form-urlencoded"]);
    let inv = build(&swagger);
    assert_eq!(
        inv["operations"][0]["request_body"]["content_type"],
        "application/x-www-form-urlencoded"
    );
    assert_eq!(inv["operations"][0]["support"], "supported");

    // No `consumes` anywhere: JSON.
    swagger.as_object_mut().unwrap().remove("consumes");
    let inv = build(&swagger);
    assert_eq!(
        inv["operations"][0]["request_body"]["content_type"],
        "application/json"
    );
}

#[test]
fn form_data_parameters_become_one_form_body() {
    let swagger = json!({
        "swagger": "2.0", "info": {"title": "Forms", "version": "1"}, "host": "api.forms.test",
        "paths": {"/labels": {"post": {
            "operationId": "createLabel",
            "consumes": ["application/x-www-form-urlencoded"],
            "parameters": [
                {"name": "owner", "in": "path", "required": true, "type": "string"},
                {"name": "name", "in": "formData", "required": true, "type": "string", "description": "label name"},
                {"name": "color", "in": "formData", "type": "string", "enum": ["red", "blue"]},
                {"name": "tags", "in": "formData", "type": "array", "items": {"type": "string"}}
            ],
            "responses": {"201": {"description": "created", "schema": {"type": "object", "properties": {"id": {"type": "integer"}}}}}
        }}}
    });
    let d = convert(&swagger);
    let op = &d["paths"]["/labels"]["post"];
    assert_eq!(
        op["parameters"].as_array().unwrap().len(),
        1,
        "only the path parameter stays"
    );
    let rb = &op["requestBody"];
    assert_eq!(rb["required"], true);
    let schema = &rb["content"]["application/x-www-form-urlencoded"]["schema"];
    assert_eq!(schema["type"], "object");
    assert_eq!(schema["required"], json!(["name"]));
    assert_eq!(schema["properties"]["name"]["description"], "label name");
    assert_eq!(
        schema["properties"]["color"]["enum"],
        json!(["red", "blue"])
    );
    assert_eq!(schema["properties"]["tags"]["items"]["type"], "string");

    let inv = build(&swagger);
    let o = &inv["operations"][0];
    let locs: Vec<&str> = o["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["in"].as_str().unwrap())
        .collect();
    assert_eq!(locs, vec!["path"]);
    assert_eq!(
        o["request_body"]["content_type"],
        "application/x-www-form-urlencoded"
    );
    assert_eq!(
        o["request_body"]["shape_ref"],
        "#/shapes/CreateLabelRequest"
    );
    assert_eq!(
        inv["shapes"]["CreateLabelRequest"]["required"],
        json!(["name"])
    );
    assert_eq!(o["support"], "supported");
}

#[test]
fn a_file_part_or_a_multipart_only_api_makes_the_body_multipart_and_unsupported() {
    let upload = json!({
        "swagger": "2.0", "info": {"title": "Uploads", "version": "1"}, "host": "api.uploads.test",
        "consumes": ["application/json"],
        "paths": {"/attachments": {"post": {
            "operationId": "upload",
            "parameters": [
                {"name": "attachment", "in": "formData", "required": true, "type": "file"},
                {"name": "name", "in": "formData", "type": "string"}
            ],
            "responses": {"201": {"description": "created", "schema": {"type": "object", "properties": {"id": {"type": "integer"}}}}}
        }}}
    });
    let d = convert(&upload);
    let media = &d["paths"]["/attachments"]["post"]["requestBody"]["content"];
    assert!(
        media.get("multipart/form-data").is_some(),
        "a file part forces multipart whatever consumes says: {}",
        media
    );
    assert_eq!(
        media["multipart/form-data"]["schema"]["properties"]["attachment"],
        json!({"type": "string", "format": "binary"})
    );
    let inv = build(&upload);
    let o = &inv["operations"][0];
    assert_eq!(o["support"], "unsupported");
    assert!(o["support_reason"]
        .as_str()
        .unwrap()
        .contains("multipart/form-data"));
    assert_eq!(
        inv["shapes"]["UploadRequest"]["properties"]["attachment"]["format"],
        "binary"
    );

    let mixed = json!({
        "swagger": "2.0", "info": {"title": "M", "version": "1"}, "host": "api.m.test",
        "paths": {"/t": {"post": {
            "operationId": "mixedParts",
            "consumes": ["multipart/mixed; boundary=x"],
            "parameters": [{"name": "part", "in": "formData", "type": "string"}],
            "responses": {"200": {"description": "ok", "schema": {"type": "object", "properties": {"ok": {"type": "boolean"}}}}}
        }}}
    });
    let inv = build(&mixed);
    assert_eq!(
        inv["operations"][0]["request_body"]["content_type"],
        "multipart/mixed"
    );
    assert_eq!(inv["operations"][0]["support"], "unsupported");
}

#[test]
fn form_parts_inherit_from_the_path_resolve_refs_and_override_by_name() {
    let swagger = json!({
        "swagger": "2.0", "info": {"title": "Forms", "version": "1"}, "host": "api.forms.test",
        "parameters": {"NameParam": {"name": "name", "in": "formData", "required": true, "type": "string", "pattern": "^[a-z]+$", "example": "abc"}},
        "paths": {"/things": {
            "parameters": [
                {"name": "shared", "in": "formData", "required": true, "type": "string"},
                {"$ref": "#/parameters/NameParam"},
                {"name": "X-Trace", "in": "header", "type": "string"}
            ],
            "post": {
                "operationId": "doThing",
                "parameters": [{"name": "name", "in": "formData", "required": false, "type": "string", "description": "overrides the path-level one"}],
                "responses": {"200": {"description": "ok", "schema": {"type": "object", "properties": {"ok": {"type": "boolean"}}}}}
            }
        }}
    });
    let d = convert(&swagger);
    let item = &d["paths"]["/things"];
    assert_eq!(
        item["parameters"].as_array().unwrap().len(),
        1,
        "only the header stays at path level: {}",
        item["parameters"]
    );
    assert!(
        d["components"].get("parameters").is_none(),
        "a formData component parameter has no OpenAPI 3 home; it is inlined where used"
    );
    let schema =
        &item["post"]["requestBody"]["content"]["application/x-www-form-urlencoded"]["schema"];
    let names: Vec<&String> = schema["properties"].as_object().unwrap().keys().collect();
    assert_eq!(
        names,
        vec!["shared", "name"],
        "first position kept, one property per name"
    );
    assert_eq!(
        schema["required"],
        json!(["shared"]),
        "the override's required: false wins"
    );
    assert_eq!(
        schema["properties"]["name"]["description"],
        "overrides the path-level one"
    );

    let inv = build(&swagger);
    let shape = &inv["shapes"]["DoThingRequest"];
    assert_eq!(shape["required"], json!(["shared"]));
    let mut with_constraints = swagger.clone();
    with_constraints["paths"]["/things"]["post"]["parameters"] = json!([]);
    let inv = build(&with_constraints);
    assert_eq!(
        inv["shapes"]["DoThingRequest"]["properties"]["name"]["pattern"],
        "^[a-z]+$"
    );
    assert_eq!(
        inv["shapes"]["DoThingRequest"]["properties"]["name"]["example"],
        "abc"
    );
}

#[test]
fn a_body_beside_form_data_keeps_the_body_and_names_the_parts_for_review() {
    let swagger = json!({
        "swagger": "2.0", "info": {"title": "Mixed", "version": "1"}, "host": "api.mixed.test",
        "paths": {"/t": {"post": {
            "operationId": "mixed",
            "parameters": [
                {"name": "q", "in": "query", "type": "string"},
                {"name": "body", "in": "body", "schema": {"type": "object", "properties": {"a": {"type": "string"}}}},
                {"name": "extra", "in": "formData", "required": true, "type": "string"}
            ],
            "responses": {"200": {"description": "ok", "schema": {"type": "object", "properties": {"ok": {"type": "boolean"}}}}}
        }}}
    });
    let d = convert(&swagger);
    assert_eq!(
        d["paths"]["/t"]["post"]["x-factory-unmodelled-parameters"],
        json!(["extra (formData, beside a body parameter)"])
    );
    let built = build_inventory(&d).unwrap();
    let o = &built.inventory["operations"][0];
    assert_eq!(o["support"], "needs_review");
    assert!(
        o["support_reason"].as_str().unwrap().contains("extra"),
        "{}",
        o["support_reason"]
    );
    assert_eq!(o["request_body"]["content_type"], "application/json");
    assert!(
        built
            .warnings
            .iter()
            .any(|w| w.starts_with("post:/t:") && w.contains("extra")),
        "{:?}",
        built.warnings
    );
}

#[test]
fn responses_take_their_content_type_from_produces_and_headers_gain_a_schema() {
    let swagger = json!({
        "swagger": "2.0", "info": {"title": "R", "version": "1"}, "host": "api.r.test",
        "produces": ["text/html", "application/json"],
        "paths": {
            "/json": {"get": {"responses": {"200": {"description": "ok", "schema": {"type": "object", "properties": {"ok": {"type": "boolean"}}}, "headers": {"X-Total": {"type": "integer", "description": "count"}}, "examples": {"application/json": {"ok": true}}}}}},
            "/csv": {"get": {"produces": ["text/csv"], "responses": {"200": {"description": "ok", "schema": {"type": "string"}}}}},
            "/any": {"get": {"produces": ["*/*"], "responses": {"200": {"description": "ok", "schema": {"type": "string"}}}}},
            "/jwks": {"get": {"produces": ["application/jwk-set+json"], "responses": {"200": {"description": "ok", "schema": {"type": "object", "properties": {"keys": {"type": "array", "items": {"type": "object"}}}}}}}},
            "/none": {"delete": {"responses": {"204": {"description": "gone"}}}}
        }
    });
    let d = convert(&swagger);
    let r = &d["paths"]["/json"]["get"]["responses"]["200"];
    assert_eq!(
        r["content"]["application/json"]["schema"]["type"], "object",
        "JSON is preferred over the first listed"
    );
    assert_eq!(
        r["content"]["application/json"]["example"],
        json!({"ok": true})
    );
    assert_eq!(
        r["headers"]["X-Total"],
        json!({"description": "count", "schema": {"type": "integer"}})
    );
    assert!(r.get("schema").is_none() && r.get("examples").is_none());
    assert!(d["paths"]["/csv"]["get"]["responses"]["200"]["content"]
        .get("text/csv")
        .is_some());
    assert!(d["paths"]["/none"]["delete"]["responses"]["204"]
        .get("content")
        .is_none());
    let inv = build(&swagger);
    let by_key = |k: &str| {
        inv["operations"]
            .as_array()
            .unwrap()
            .iter()
            .find(|o| o["key"] == k)
            .unwrap()
            .clone()
    };
    assert_eq!(by_key("get:/json")["support"], "supported");
    assert_eq!(by_key("get:/csv")["support"], "unsupported");
    assert_eq!(by_key("get:/any")["response"]["content_type"], "application/json", "a wildcard produces means JSON for a schema-described response (Kubernetes attach/exec/proxy)");
    assert_eq!(by_key("get:/any")["support"], "supported");
    assert_eq!(
        by_key("get:/jwks")["support"],
        "supported",
        "any +json structured syntax is JSON"
    );
    assert_eq!(
        by_key("delete:/none")["support"],
        "supported",
        "a 204 with no content is supported"
    );
}

#[test]
fn schema_spellings_are_rewritten_everywhere() {
    let swagger = json!({
        "swagger": "2.0", "info": {"title": "S", "version": "1"}, "host": "api.s.test",
        "definitions": {
            "Pet": {"type": "object", "discriminator": "kind", "properties": {"kind": {"type": "string"}, "photo": {"type": "file"}, "nick": {"type": "string", "x-nullable": true}}},
            "Pets": {"type": "array", "items": {"$ref": "#/definitions/Pet"}}
        },
        "paths": {}
    });
    let d = convert(&swagger);
    let pet = &d["components"]["schemas"]["Pet"];
    assert_eq!(pet["discriminator"], json!({"propertyName": "kind"}));
    assert_eq!(
        pet["properties"]["photo"],
        json!({"type": "string", "format": "binary"})
    );
    assert_eq!(
        pet["properties"]["nick"],
        json!({"type": "string", "nullable": true})
    );
    assert_eq!(
        d["components"]["schemas"]["Pets"]["items"]["$ref"],
        "#/components/schemas/Pet"
    );
}

// ── The front door ──────────────────────────────────────────────────────────

#[test]
fn spec_read_names_the_format_and_routes_each_dialect() {
    let oas = spec::read(
        "{\"openapi\":\"3.0.0\",\"info\":{\"title\":\"x\",\"version\":\"1\"},\"paths\":{}}",
        "x.json",
    )
    .unwrap();
    assert_eq!(oas.format, Format::OpenApi("3.0.0".into()));
    assert_eq!(oas.format.to_string(), "OpenAPI 3.0.0");
    assert!(oas.warnings.is_empty());

    let sw = spec::read(
        "swagger: 2.0\ninfo: {title: Y, version: '1'}\nhost: api.y.test\npaths: {}\n",
        "y.yaml",
    )
    .unwrap();
    assert_eq!(
        sw.format,
        Format::Swagger("2".into()),
        "an unquoted YAML 2.0 parses as a number and is still Swagger"
    );
    assert_eq!(sw.document["openapi"], "3.0.3");
    assert_eq!(sw.document["servers"][0]["url"], "https://api.y.test");

    let bare = spec::read(
        "{\"info\":{\"title\":\"Bare\",\"version\":\"1\"},\"paths\":{}}",
        "b.json",
    )
    .unwrap();
    assert_eq!(bare.format, Format::Undeclared);
    assert_eq!(bare.format.to_string(), "format undeclared");
    assert!(
        bare.warnings
            .iter()
            .any(|w| w.contains("neither `openapi` nor `swagger`")),
        "{:?}",
        bare.warnings
    );

    assert!(spec::load("- not\n- an object\n", "list.yaml").is_err());
}

#[test]
fn x_nullable_beside_a_ref_survives_into_the_inventory_shape() {
    // go-swagger writes `x-nullable: true` next to `$ref` for a reference that
    // may be null (every Gitea `Issue.milestone`). Siblings of `$ref` are
    // otherwise dropped; this one must reach the shape so the conformance
    // oracle can accept the null the API sends.
    let swagger = json!({
        "swagger": "2.0", "info": {"title": "Refs", "version": "1"}, "host": "api.refs.test",
        "definitions": {
            "Milestone": {"type": "object", "properties": {"id": {"type": "integer"}}},
            "Issue": {"type": "object", "properties": {
                "milestone": {"$ref": "#/definitions/Milestone", "x-nullable": true},
                "assignee": {"$ref": "#/definitions/Milestone"}
            }}
        },
        "paths": {"/issues/{id}": {"get": {
            "operationId": "getIssue",
            "parameters": [{"name": "id", "in": "path", "required": true, "type": "integer"}],
            "responses": {"200": {"description": "ok", "schema": {"$ref": "#/definitions/Issue"}}}
        }}}
    });
    let inv = build(&swagger);
    assert_eq!(
        inv["shapes"]["Issue"]["properties"]["milestone"],
        json!({"$ref": "#/shapes/Milestone", "nullable": true})
    );
    assert_eq!(
        inv["shapes"]["Issue"]["properties"]["assignee"],
        json!({"$ref": "#/shapes/Milestone"})
    );
}

#[test]
fn an_access_code_flow_and_the_root_security_reach_the_inventory_as_oauth2_facts() {
    let swagger = json!({
        "swagger": "2.0", "info": {"title": "Legacy", "version": "1"}, "host": "api.legacy.test",
        "securityDefinitions": {
            "oauth": {"type": "oauth2", "flow": "accessCode", "authorizationUrl": "https://a", "tokenUrl": "https://t",
                      "scopes": {"read": "Read", "write": "Write"}},
            "creds": {"type": "oauth2", "flow": "application", "tokenUrl": "https://t", "scopes": {"svc": "Service"}}
        },
        "security": [{"oauth": ["read"]}],
        "paths": {"/things": {
            "get": {"responses": {"200": {"description": "ok", "schema": {"type": "object", "properties": {"id": {"type": "string"}}}}}},
            "post": {"security": [{"oauth": ["write"]}],
                     "parameters": [{"name": "body", "in": "body", "schema": {"type": "object", "properties": {"id": {"type": "string"}}}}],
                     "responses": {"200": {"description": "ok", "schema": {"type": "object", "properties": {"id": {"type": "string"}}}}}}
        }}
    });
    let d = convert(&swagger);
    assert_eq!(d["security"], json!([{"oauth": ["read"]}]));
    let inv = build(&swagger);
    let auth = inv["api"]["auth"].as_array().unwrap();
    let oauth = auth.iter().find(|a| a["scheme_name"] == "oauth").unwrap();
    assert_eq!(
        oauth["oauth2"],
        json!({"flows": ["authorization_code"], "authorization_code": {
            "authorization_url": "https://a", "token_url": "https://t",
            "scopes": [{"name": "read", "description": "Read"}, {"name": "write", "description": "Write"}]
        }})
    );
    let creds = auth.iter().find(|a| a["scheme_name"] == "creds").unwrap();
    assert_eq!(creds["oauth2"], json!({"flows": ["client_credentials"]}));
    assert_eq!(inv["api"]["security"], json!([{"oauth": ["read"]}]));
    let post = inv["operations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["key"] == "post:/things")
        .unwrap();
    assert_eq!(post["security"], json!([{"oauth": ["write"]}]));
    assert_eq!(validate(&inv, &schema()), Vec::<String>::new());
}

// ── Parameter default/minimum/maximum ────────────────────────────────────────

#[test]
fn swagger_pagination_bounds_reach_the_inventory() {
    // Swagger 2.0 inline default/min/max on an integer query param survive
    // conversion (PARAM_SCHEMA_KEYS) and reach the inventory.
    let swagger = json!({
        "swagger": "2.0", "info": {"title": "Paginated", "version": "1"}, "host": "api.paginated.test",
        "paths": {"/items": {"get": {
            "operationId": "listItems",
            "parameters": [
                {"name": "cursor", "in": "query", "type": "string"},
                {"name": "limit", "in": "query", "type": "integer", "default": 25, "minimum": 1, "maximum": 200}
            ],
            "responses": {"200": {"description": "ok", "schema": {"type": "array", "items": {"type": "object", "properties": {"id": {"type": "string"}}}}}}
        }}}
    });
    let inv = build(&swagger);
    let limit = inv["operations"][0]["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "limit")
        .expect("limit parameter");
    assert_eq!(limit["type"], "integer");
    assert_eq!(limit["default"], 25);
    assert_eq!(limit["minimum"], 1);
    assert_eq!(limit["maximum"], 200);
    assert_eq!(validate(&inv, &schema()), Vec::<String>::new());
}

#[test]
fn swagger_explicit_null_default_and_bounds_are_treated_as_absent() {
    // Explicit null is malformed for minimum/maximum (JSON Schema requires
    // a number) and indistinguishable from absent for default — all three
    // keys are omitted (ADR 0025).
    let swagger = json!({
        "swagger": "2.0", "info": {"title": "Paginated", "version": "1"}, "host": "api.paginated.test",
        "paths": {"/items": {"get": {
            "operationId": "listItems",
            "parameters": [
                {"name": "limit", "in": "query", "type": "integer", "default": null, "minimum": null, "maximum": null}
            ],
            "responses": {"200": {"description": "ok", "schema": {"type": "array", "items": {"type": "object", "properties": {"id": {"type": "string"}}}}}}
        }}}
    });
    let inv = build(&swagger);
    let limit = inv["operations"][0]["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "limit")
        .expect("limit parameter");
    assert!(limit.get("default").is_none());
    assert!(limit.get("minimum").is_none());
    assert!(limit.get("maximum").is_none());
    assert_eq!(validate(&inv, &schema()), Vec::<String>::new());
}

/// A FullStory-shaped list (`GET /v2/users`): the request carries
/// `page_token`, the response `{results[], next_page_token, total_records}`.
/// Before `next_page_token` was a next key the response cursor was null.
fn page_token_list(request: &str, next: &str) -> Value {
    json!({
        "swagger": "2.0", "info": {"title": "Users", "version": "v2"}, "host": "api.users.test",
        "paths": {"/v2/users": {"get": {
            "operationId": "ListUsers",
            "parameters": [
                {"name": "email", "in": "query", "type": "string"},
                {"name": request, "in": "query", "type": "string"}
            ],
            "responses": {"200": {"description": "ok", "schema": {"$ref": "#/definitions/ListUsersResponse"}}}
        }}},
        "definitions": {"ListUsersResponse": {"type": "object", "properties": {
            "results": {"type": "array", "items": {"type": "object", "properties": {"id": {"type": "string"}}}},
            "total_records": {"type": "string", "format": "int64"},
            next: {"type": "string"}
        }}}
    })
}

#[test]
fn a_next_page_token_response_is_the_cursor() {
    let inv = build(&page_token_list("page_token", "next_page_token"));
    assert_eq!(
        inv["operations"][0]["pagination"],
        json!({"style": "cursor", "request": "page_token", "size_param": null, "response": "next_page_token"})
    );
}

#[test]
fn the_google_discovery_spelling_is_the_cursor_too() {
    let inv = build(&page_token_list("pageToken", "nextPageToken"));
    assert_eq!(
        inv["operations"][0]["pagination"],
        json!({"style": "cursor", "request": "pageToken", "size_param": null, "response": "nextPageToken"})
    );
}

// ── Exclusive bounds (ADR 0065) ─────────────────────────────────────────────

/// Swagger 2.0 is JSON Schema draft 4: `exclusiveMaximum: true` qualifies
/// `maximum`. `PARAM_SCHEMA_KEYS` lifts it into the parameter's schema and a
/// definition carries it through, so both reach the inventory as written.
#[test]
fn swagger_exclusive_bounds_reach_the_parameter_and_the_shape() {
    let swagger = json!({
        "swagger": "2.0", "info": {"title": "Cards", "version": "1"}, "host": "api.cards.test",
        "paths": {"/cards": {"get": {
            "operationId": "listCards",
            "parameters": [
                {"name": "limit", "in": "query", "type": "integer", "minimum": 0, "exclusiveMinimum": true, "maximum": 100, "exclusiveMaximum": true}
            ],
            "responses": {"200": {"description": "ok", "schema": {"type": "array", "items": {"$ref": "#/definitions/Card"}}}}
        }}},
        "definitions": {"Card": {"type": "object", "properties": {
            "card_number": {"type": "integer", "format": "int64", "minimum": 1000000000000000_i64, "exclusiveMaximum": true, "maximum": 10000000000000000_i64}
        }}}
    });
    let inv = build(&swagger);
    let limit = inv["operations"][0]["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "limit")
        .expect("limit parameter");
    assert_eq!(limit["exclusiveMinimum"], true);
    assert_eq!(limit["exclusiveMaximum"], true);
    assert_eq!(limit["maximum"], 100);
    let card = &inv["shapes"]["Card"]["properties"]["card_number"];
    assert_eq!(card["exclusiveMaximum"], true);
    assert_eq!(card["maximum"], json!(10000000000000000_i64));
}
