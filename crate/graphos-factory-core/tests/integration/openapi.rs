use graphos_factory_core::jsonschema::validate;
use graphos_factory_core::openapi::build_inventory;
use graphos_factory_core::schemas;
use serde_json::{json, Value};

fn schema() -> Value {
    schemas::load("inventory.schema.json", None).unwrap()
}

fn spec() -> Value {
    json!({
        "openapi": "3.0.3",
        "info": {"title": "Widgets API", "version": "1.2.0"},
        "servers": [{"url": "https://api.widgets.test/{stage}", "variables": {"stage": {"default": "v1"}}}],
        "components": {
            "securitySchemes": {"bearerAuth": {"type": "http", "scheme": "bearer"}},
            "schemas": {
                "Widget": {"type": "object", "required": ["id"], "properties": {"id": {"type": "string"}, "name": {"type": "string"}, "owner": {"$ref": "#/components/schemas/User"}}},
                "User": {"type": "object", "properties": {"id": {"type": "string"}, "widget": {"$ref": "#/components/schemas/Widget"}}},
                "WidgetList": {"type": "object", "properties": {
                    "widgets": {"type": "array", "items": {"$ref": "#/components/schemas/Widget"}},
                    "page_info": {"type": "object", "properties": {"next_cursor": {"type": "string"}}}
                }}
            }
        },
        "paths": {
            "/widgets": {
                "parameters": [{"name": "Accept", "in": "header", "schema": {"type": "string"}}],
                "get": {
                    "operationId": "listWidgets", "summary": "List widgets", "tags": ["Widgets"],
                    "parameters": [{"name": "cursor", "in": "query", "schema": {"type": "string"}}, {"name": "limit", "in": "query", "schema": {"type": "integer"}}],
                    "responses": {
                        "200": {"description": "ok", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/WidgetList"}}}},
                        "404": {"description": "missing"}
                    }
                },
                "post": {
                    "operationId": "createWidget", "tags": ["Widgets"],
                    "requestBody": {"required": true, "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Widget"}}}},
                    "responses": {"201": {"description": "created", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Widget"}}}}}
                }
            },
            "/widgets/{widgetId}": {
                "get": {
                    "operationId": "getWidget", "tags": ["Widgets"],
                    "parameters": [{"name": "widgetId", "in": "path", "required": true, "schema": {"type": "string"}}],
                    "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Widget"}}}}}
                },
                "delete": {
                    "operationId": "deleteWidget",
                    "parameters": [{"name": "widgetId", "in": "path", "required": true, "schema": {"type": "string"}}],
                    "responses": {"204": {"description": "gone"}}
                }
            },
            "/widgets/export": {
                "get": {"operationId": "exportWidgets", "responses": {"200": {"description": "ok", "content": {"text/csv": {"schema": {"type": "string"}}}}}}
            }
        }
    })
}

fn build(s: &Value) -> Value {
    build_inventory(s).unwrap().inventory
}

fn op<'a>(inv: &'a Value, key: &str) -> &'a Value {
    inv["operations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["key"] == key)
        .unwrap_or_else(|| panic!("no {}", key))
}

#[test]
fn the_built_inventory_satisfies_the_contract_schema() {
    assert_eq!(validate(&build(&spec()), &schema()), Vec::<String>::new());
}

#[test]
fn operation_keys_use_method_path_and_are_complete() {
    let inv = build(&spec());
    let mut keys: Vec<&str> = inv["operations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["key"].as_str().unwrap())
        .collect();
    keys.sort();
    assert_eq!(
        keys,
        vec![
            "delete:/widgets/{widgetId}",
            "get:/widgets",
            "get:/widgets/export",
            "get:/widgets/{widgetId}",
            "post:/widgets"
        ]
    );
}

#[test]
fn server_variables_are_expanded_into_base_urls() {
    assert_eq!(
        build(&spec())["api"]["base_urls"],
        json!(["https://api.widgets.test/v1"])
    );
}

#[test]
fn a_post_is_a_write_until_the_engineer_decides_otherwise() {
    let inv = build(&spec());
    assert_eq!(op(&inv, "get:/widgets")["semantics"], "read");
    assert_eq!(op(&inv, "post:/widgets")["semantics"], "write");
    assert!(op(&inv, "post:/widgets").get("read_hint").is_none());
    assert_eq!(op(&inv, "delete:/widgets/{widgetId}")["semantics"], "write");
}

#[test]
fn a_non_json_response_is_unsupported_with_a_reason() {
    let inv = build(&spec());
    let o = op(&inv, "get:/widgets/export");
    assert_eq!(o["support"], "unsupported");
    assert!(o["support_reason"].as_str().unwrap().contains("text/csv"));
}

#[test]
fn no_documented_response_is_unsupported_with_a_reason() {
    let mut s = spec();
    s["paths"]["/widgets/ping"] = json!({"get": {"operationId": "ping", "responses": {}}});
    let inv = build(&s);
    let o = op(&inv, "get:/widgets/ping");
    assert_eq!(o["support"], "unsupported");
    assert!(o["support_reason"]
        .as_str()
        .unwrap()
        .contains("no success response"));
}

#[test]
fn a_204_with_no_content_is_supported() {
    let inv = build(&spec());
    let o = op(&inv, "delete:/widgets/{widgetId}");
    assert_eq!(o["support"], "supported");
    assert_eq!(o["response"]["status"], "204");
    assert_eq!(o["response"]["content_type"], Value::Null);
}

#[test]
fn an_exploded_array_query_param_is_needs_review() {
    let mut s = spec();
    s["paths"]["/widgets"]["get"]["parameters"].as_array_mut().unwrap().push(json!({"name": "ids[]", "in": "query", "explode": true, "schema": {"type": "array", "items": {"type": "string"}}}));
    let inv = build(&s);
    let o = op(&inv, "get:/widgets");
    assert_eq!(o["support"], "needs_review");
    assert!(o["support_reason"].as_str().unwrap().contains("repeats"));
}

#[test]
fn a_deep_object_query_param_is_needs_review() {
    let mut s = spec();
    s["paths"]["/widgets"]["get"]["parameters"].as_array_mut().unwrap().push(json!({"name": "filter", "in": "query", "style": "deepObject", "schema": {"type": "object"}}));
    assert_eq!(op(&build(&s), "get:/widgets")["support"], "needs_review");
}

#[test]
fn path_level_parameters_are_inherited() {
    let inv = build(&spec());
    assert!(op(&inv, "get:/widgets")["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p["name"] == "Accept" && p["in"] == "header"));
}

#[test]
fn path_parameters_are_required_even_when_the_spec_forgets() {
    let mut s = spec();
    s["paths"]["/widgets/{widgetId}"]["get"]["parameters"][0]
        .as_object_mut()
        .unwrap()
        .remove("required");
    let inv = build(&s);
    let p = op(&inv, "get:/widgets/{widgetId}")["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "widgetId")
        .unwrap()
        .clone();
    assert_eq!(p["required"], true);
}

#[test]
fn named_schemas_become_shapes_and_recursive_refs_stay_refs() {
    let inv = build(&spec());
    assert_eq!(
        inv["shapes"]["Widget"]["properties"]["owner"]["$ref"],
        "#/shapes/User"
    );
    assert_eq!(
        inv["shapes"]["User"]["properties"]["widget"]["$ref"],
        "#/shapes/Widget"
    );
}

#[test]
fn the_response_records_the_facts_an_envelope_judgement_rests_on() {
    // The inventory never says which key the payload sits under — that is a
    // judgement, and it lives in selection.yaml (ADR 0018). What it records
    // is what a reader can check against the document.
    let inv = build(&spec());
    let response = &op(&inv, "get:/widgets")["response"];
    assert!(response.get("envelope").is_none());
    assert!(response.get("list").is_none());
    assert_eq!(response["root_property_count"], 2);
    assert_eq!(response["array_root_properties"], json!(["widgets"]));
    // `page_info` is an object, not a root cursor: no key is recorded.
    assert!(response.get("cursor_root_properties").is_none());
    // `{ widgets[], page_info }` — one array beside one sibling: proposed.
    assert_eq!(
        graphos_factory_core::envelope::suggest_envelope(Some(response)),
        Some("widgets".to_string())
    );
    assert!(graphos_factory_core::envelope::is_list(
        Some(response),
        Some("widgets")
    ));
}

#[test]
fn cursor_pagination_is_detected_from_params_and_the_response_shape() {
    assert_eq!(
        build(&spec())["api"]["pagination"],
        json!({"style": "cursor", "request": "cursor", "size_param": "limit", "response": "page_info.next_cursor", "counts": {"cursor": 1}})
    );
}

/// Three offset-paged lists and one cursor-paged one. Pooling every query
/// parameter name across the API made the one `cursor` outweigh the three
/// (Mailchimp: 1 cursor against 58 offset); the summary is now the majority
/// of the per-operation facts, with the counts beside it.
#[test]
fn the_api_summary_is_the_majority_of_the_operations_not_a_pooled_name() {
    let list = |cursor: bool| -> Value {
        let params = if cursor {
            json!([{"name": "cursor", "in": "query", "schema": {"type": "string"}}, {"name": "count", "in": "query", "schema": {"type": "integer"}}])
        } else {
            json!([{"name": "offset", "in": "query", "schema": {"type": "integer"}}, {"name": "count", "in": "query", "schema": {"type": "integer"}}])
        };
        json!({"get": {"parameters": params, "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {
            "type": "object", "properties": {"items": {"type": "array", "items": {"type": "string"}}, "next_cursor": {"type": "string"}}}}}}}}})
    };
    let spec = json!({
        "openapi": "3.0.3", "info": {"title": "Lists", "version": "1"},
        "servers": [{"url": "https://api.lists.test"}],
        "paths": {"/a": list(false), "/b": list(false), "/c": list(true), "/d": list(false), "/e": {"get": {"responses": {"200": {"description": "ok"}}}}}
    });
    let inv = build(&spec);
    assert_eq!(
        inv["api"]["pagination"],
        json!({"style": "offset", "request": "offset", "size_param": "count", "response": null, "counts": {"cursor": 1, "offset": 3}})
    );
    assert_eq!(inv["operations"][2]["pagination"]["style"], "cursor");
    assert_eq!(
        inv["operations"][2]["pagination"]["response"],
        "next_cursor"
    );
}

/// One cursor-paged list whose response is `{ data[], <paging> }`.
fn paged(paging: Value) -> Value {
    json!({
        "openapi": "3.0.3", "info": {"title": "Graph", "version": "1"},
        "servers": [{"url": "https://graph.example.test"}],
        "paths": {"/act_1/campaigns": {"get": {
            "parameters": [{"name": "after", "in": "query", "schema": {"type": "string"}}],
            "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {"type": "object", "properties": {
                "data": {"type": "array", "items": {"type": "object", "properties": {"id": {"type": "string"}}}},
                "paging": paging
            }}}}}}
        }}}
    })
}

/// Meta's Graph API: `paging.next` is a full URL; the cursor to hand back is
/// `paging.cursors.after`, and the URL is kept beside it.
#[test]
fn graph_api_paging_records_cursors_after_and_keeps_the_next_url() {
    let inv = build(&paged(json!({"type": "object", "properties": {
        "cursors": {"type": "object", "properties": {"before": {"type": "string"}, "after": {"type": "string"}}},
        "next": {"type": "string"},
        "previous": {"type": "string"}
    }})));
    assert_eq!(
        inv["operations"][0]["pagination"],
        json!({"style": "cursor", "request": "after", "size_param": null, "response": "paging.cursors.after", "next_url": "paging.next"})
    );
    assert_eq!(inv["api"]["pagination"]["response"], "paging.cursors.after");
    assert_eq!(inv["api"]["pagination"]["next_url"], "paging.next");
    assert_eq!(validate(&inv, &schema()), Vec::<String>::new());
}

#[test]
fn a_cursors_end_cursor_beside_next_page_is_the_cursor() {
    let inv = build(&paged(json!({"type": "object", "properties": {
        "cursors": {"type": "object", "properties": {"end_cursor": {"type": "string"}}},
        "next_page": {"type": "string"}
    }})));
    assert_eq!(
        inv["operations"][0]["pagination"]["response"],
        "paging.cursors.end_cursor"
    );
    assert_eq!(
        inv["operations"][0]["pagination"]["next_url"],
        "paging.next_page"
    );
}

/// Without a `cursors` sibling carrying `after` / `end_cursor`, `next` is
/// recorded as before and there is no `next_url`.
#[test]
fn a_bare_paging_next_is_unchanged() {
    let inv = build(&paged(json!({"type": "object", "properties": {
        "cursors": {"type": "object", "properties": {"before": {"type": "string"}}},
        "next": {"type": "string"}
    }})));
    assert_eq!(
        inv["operations"][0]["pagination"],
        json!({"style": "cursor", "request": "after", "size_param": null, "response": "paging.next"})
    );
}

#[test]
fn an_api_with_no_paginated_operation_has_no_counts() {
    let spec = json!({
        "openapi": "3.0.3", "info": {"title": "Flat", "version": "1"},
        "servers": [{"url": "https://api.flat.test"}],
        "paths": {"/a": {"get": {"responses": {"200": {"description": "ok"}}}}}
    });
    assert_eq!(
        build(&spec)["api"]["pagination"],
        json!({"style": "none", "request": null, "size_param": null, "response": null})
    );
}

/// FastAPI-style paging: `page_index` and `page_limit` query parameters.
#[test]
fn fastapi_page_index_and_page_limit_are_page_paging() {
    let spec = json!({
        "openapi": "3.0.3", "info": {"title": "Items", "version": "1"},
        "servers": [{"url": "https://api.items.test"}],
        "paths": {"/items": {"get": {
            "parameters": [
                {"name": "page_index", "in": "query", "schema": {"type": "integer"}},
                {"name": "page_limit", "in": "query", "schema": {"type": "integer"}}
            ],
            "responses": {"200": {"description": "ok"}}
        }}}
    });
    let inv = build(&spec);
    assert_eq!(
        inv["operations"][0]["pagination"],
        json!({"style": "page", "request": "page_index", "size_param": "page_limit", "response": null})
    );
    assert_eq!(inv["api"]["pagination"]["style"], "page");
    assert_eq!(inv["api"]["pagination"]["counts"], json!({"page": 1}));
}

/// Two flat `next_cursor` lists and one Graph-API list. The summary's
/// `next_url` belongs to the `response` it sits beside: the Graph list's
/// `paging.next` is not a URL for a `next_cursor` response.
#[test]
fn the_api_next_url_comes_only_from_operations_with_the_winning_response() {
    let flat = json!({"get": {
        "parameters": [{"name": "after", "in": "query", "schema": {"type": "string"}}],
        "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {"type": "object", "properties": {
            "items": {"type": "array", "items": {"type": "string"}},
            "next_cursor": {"type": "string"}
        }}}}}}
    }});
    let mut spec = paged(json!({"type": "object", "properties": {
        "cursors": {"type": "object", "properties": {"after": {"type": "string"}}},
        "next": {"type": "string"}
    }}));
    spec["paths"]["/a"] = flat.clone();
    spec["paths"]["/b"] = flat;
    let inv = build(&spec);
    assert_eq!(
        inv["api"]["pagination"],
        json!({"style": "cursor", "request": "after", "size_param": null, "response": "next_cursor", "counts": {"cursor": 3}})
    );
}

/// Three lists that take only a `limit` and two cursor-paged ones. `unknown`
/// is the absence of a style, not a style: it does not outvote the cursor.
#[test]
fn unknown_does_not_outvote_a_real_style() {
    let list = |cursor: bool| -> Value {
        let mut params =
            vec![json!({"name": "limit", "in": "query", "schema": {"type": "integer"}})];
        if cursor {
            params.push(json!({"name": "cursor", "in": "query", "schema": {"type": "string"}}));
        }
        json!({"get": {"parameters": params, "responses": {"200": {"description": "ok"}}}})
    };
    let spec = json!({
        "openapi": "3.0.3", "info": {"title": "Lists", "version": "1"},
        "servers": [{"url": "https://api.lists.test"}],
        "paths": {"/a": list(false), "/b": list(false), "/c": list(false), "/d": list(true), "/e": list(true)}
    });
    assert_eq!(
        build(&spec)["api"]["pagination"],
        json!({"style": "cursor", "request": "cursor", "size_param": "limit", "response": null, "counts": {"cursor": 2, "unknown": 3}})
    );
}

/// A response carrying both a bare `next` and a `next_page_token`: the
/// specific name wins over the one that is as often a URL.
#[test]
fn next_page_token_wins_over_a_bare_next() {
    let spec = json!({
        "openapi": "3.0.3", "info": {"title": "Users", "version": "1"},
        "servers": [{"url": "https://api.users.test"}],
        "paths": {"/users": {"get": {
            "parameters": [{"name": "page_token", "in": "query", "schema": {"type": "string"}}],
            "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {"type": "object", "properties": {
                "results": {"type": "array", "items": {"type": "string"}},
                "next": {"type": "string"},
                "next_page_token": {"type": "string"}
            }}}}}}
        }}}
    });
    assert_eq!(
        build(&spec)["operations"][0]["pagination"]["response"],
        "next_page_token"
    );
}

/// `counts` belongs to the API summary alone: the contract accepts it on
/// `api.pagination` and rejects it on an operation's own block.
#[test]
fn counts_is_accepted_on_the_api_summary_and_rejected_on_an_operation() {
    let inv = build(&spec());
    assert_eq!(inv["api"]["pagination"]["counts"], json!({"cursor": 1}));
    assert_eq!(validate(&inv, &schema()), Vec::<String>::new());

    let mut op_counts = inv.clone();
    let list = op_counts["operations"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|o| o["key"] == "get:/widgets")
        .unwrap();
    list["pagination"]["counts"] = json!({"cursor": 1});
    let errors = validate(&op_counts, &schema());
    assert_eq!(errors.len(), 1, "{:?}", errors);
    assert!(
        errors[0].ends_with("/pagination/counts is not a known property"),
        "{:?}",
        errors
    );

    // The API block reuses the operation block's field rules.
    let mut bad_style = inv.clone();
    bad_style["api"]["pagination"]["style"] = json!("sideways");
    assert_eq!(validate(&bad_style, &schema()).len(), 1);
}

/// With nothing but limit-only lists, the summary is still `unknown`.
#[test]
fn only_limit_only_lists_summarise_as_unknown() {
    let spec = json!({
        "openapi": "3.0.3", "info": {"title": "Lists", "version": "1"},
        "servers": [{"url": "https://api.lists.test"}],
        "paths": {"/a": {"get": {
            "parameters": [{"name": "limit", "in": "query", "schema": {"type": "integer"}}],
            "responses": {"200": {"description": "ok"}}
        }}}
    });
    assert_eq!(
        build(&spec)["api"]["pagination"],
        json!({"style": "unknown", "request": null, "size_param": "limit", "response": null, "counts": {"unknown": 1}})
    );
}

#[test]
fn bearer_auth_carries_the_prefix_outside_the_credential() {
    assert_eq!(
        build(&spec())["api"]["auth"],
        json!([{"kind": "bearer", "header": "Authorization", "prefix": "Bearer ", "scheme_name": "bearerAuth", "source": "spec"}])
    );
}

#[test]
fn an_api_key_scheme_claims_no_prefix() {
    let mut s = spec();
    s["components"]["securitySchemes"] =
        json!({"api_key": {"type": "apiKey", "in": "header", "name": "Authorization"}});
    assert_eq!(
        build(&s)["api"]["auth"],
        json!([{"kind": "api_key", "scheme_name": "api_key", "source": "spec", "header": "Authorization"}])
    );
}

#[test]
fn all_of_is_merged_with_the_sources_recorded() {
    let mut s = spec();
    s["components"]["schemas"]["Base"] =
        json!({"type": "object", "required": ["id"], "properties": {"id": {"type": "string"}}});
    s["components"]["schemas"]["Widget"] = json!({"allOf": [{"$ref": "#/components/schemas/Base"}, {"type": "object", "required": ["name"], "properties": {"name": {"type": "string"}}}]});
    let inv = build(&s);
    let mut keys: Vec<&String> = inv["shapes"]["Widget"]["properties"]
        .as_object()
        .unwrap()
        .keys()
        .collect();
    keys.sort();
    assert_eq!(keys, vec!["id", "name"]);
    let mut req: Vec<&str> = inv["shapes"]["Widget"]["required"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    req.sort();
    assert_eq!(req, vec!["id", "name"]);
    assert_eq!(
        inv["shapes"]["Widget"]["x-allof-sources"],
        json!(["#/components/schemas/Base"])
    );
    // A genuine object intersection legitimately produces `type: object`,
    // even though neither allOf member here declares `type` itself.
    assert_eq!(inv["shapes"]["Widget"]["type"], json!("object"));
}

#[test]
fn all_of_combining_a_oneof_ref_with_an_annotation_only_fragment_gets_no_type() {
    // Reproduces a vendor's OverlayAssessmentDataValue.value: an
    // allOf of a $ref'd `oneOf[string, number, boolean]` plus a sibling
    // `{"example": 10}` fragment. Neither member is object-shaped, so the
    // merge must not fabricate `type: object` beside the `oneOf` — that
    // combination is unsatisfiable by any instance and was rejecting
    // valid string/number/boolean fixture data at the conformance layer.
    let mut s = spec();
    s["components"]["schemas"]["ScalarValue"] = json!({
        "oneOf": [
            {"type": "string"},
            {"type": "number"},
            {"type": "boolean"}
        ]
    });
    s["components"]["schemas"]["Annotated"] = json!({
        "allOf": [
            {"$ref": "#/components/schemas/ScalarValue"},
            {"example": 10}
        ]
    });
    s["paths"]["/annotated"] = json!({"get": {"operationId": "getAnnotated", "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Annotated"}}}}}}});
    let inv = build(&s);
    let shape = &inv["shapes"]["Annotated"];
    assert!(
        shape.get("type").is_none(),
        "expected no `type` key, got {shape}"
    );
    assert!(shape.get("properties").is_none());
    assert_eq!(
        shape["oneOf"],
        json!([{"type": "string"}, {"type": "number"}, {"type": "boolean"}])
    );
    // The merged shape must still validate a bare scalar instance, per the
    // reproduced oneOf — this is the actual conformance-layer behavior the
    // bug broke (it rejected every one of these with "expected object, got
    // string/number/boolean").
    assert_eq!(validate(&json!("value-1"), shape), Vec::<String>::new());
    assert_eq!(validate(&json!(10), shape), Vec::<String>::new());
    assert_eq!(validate(&json!(true), shape), Vec::<String>::new());
}

#[test]
fn a_self_referential_all_of_does_not_hang() {
    let mut s = spec();
    s["components"]["schemas"]["Node"] = json!({"allOf": [{"$ref": "#/components/schemas/Node"}, {"type": "object", "properties": {"id": {"type": "string"}}}]});
    s["paths"]["/nodes"] = json!({"get": {"operationId": "listNodes", "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Node"}}}}}}});
    let inv = build(&s);
    assert!(inv["shapes"]["Node"].is_object());
}

#[test]
fn one_of_is_preserved() {
    let mut s = spec();
    s["components"]["schemas"]["Target"] = json!({"oneOf": [{"$ref": "#/components/schemas/Widget"}, {"$ref": "#/components/schemas/User"}], "discriminator": {"propertyName": "type"}});
    s["paths"]["/targets"] = json!({"get": {"operationId": "listTargets", "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Target"}}}}}}});
    let inv = build(&s);
    assert_eq!(
        inv["shapes"]["Target"]["oneOf"].as_array().unwrap().len(),
        2
    );
    assert_eq!(inv["shapes"]["Target"]["discriminator"], "type");
}

#[test]
fn an_unresolvable_ref_makes_its_operation_unsupported_and_is_recorded() {
    let mut s = spec();
    s["paths"]["/broken"] = json!({"get": {"operationId": "broken", "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Missing"}}}}}}});
    let inv = build(&s);
    let o = op(&inv, "get:/broken");
    assert_eq!(o["support"], "unsupported");
    assert!(o["support_reason"].as_str().unwrap().contains("Missing"));
    assert_eq!(
        inv["unresolved"]
            .as_array()
            .unwrap()
            .iter()
            .map(|u| u["hint"].clone())
            .collect::<Vec<_>>(),
        vec![json!("get:/broken")]
    );
    assert_eq!(validate(&inv, &schema()), Vec::<String>::new());
}

#[test]
fn an_inline_request_body_schema_is_kept_under_a_synthesized_name() {
    let mut s = spec();
    s["paths"]["/widgets"]["post"]["requestBody"]["content"]["application/json"]["schema"] = json!({"type": "object", "required": ["widget"], "properties": {"widget": {"type": "object", "required": ["title"], "properties": {"title": {"type": "string"}}}}});
    let inv = build(&s);
    assert_eq!(
        op(&inv, "post:/widgets")["request_body"]["shape_ref"],
        "#/shapes/CreateWidgetRequest"
    );
    assert_eq!(
        inv["shapes"]["CreateWidgetRequest"]["properties"]
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>(),
        vec!["widget"]
    );
    assert_eq!(
        inv["shapes"]["CreateWidgetRequest"]["properties"]["widget"]["required"],
        json!(["title"])
    );
    assert_eq!(validate(&inv, &schema()), Vec::<String>::new());
}

#[test]
fn a_synthesized_shape_name_never_shadows_a_component() {
    let mut s = spec();
    s["components"]["schemas"]["CreateWidgetRequest"] =
        json!({"type": "object", "properties": {"decoy": {"type": "string"}}});
    s["paths"]["/decoys"] = json!({"get": {"operationId": "listDecoys", "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/CreateWidgetRequest"}}}}}}});
    s["paths"]["/widgets"]["post"]["requestBody"]["content"]["application/json"]["schema"] =
        json!({"type": "object", "properties": {"real": {"type": "string"}}});
    let inv = build(&s);
    assert_eq!(
        op(&inv, "post:/widgets")["request_body"]["shape_ref"],
        "#/shapes/CreateWidgetRequest2"
    );
    assert_eq!(
        inv["shapes"]["CreateWidgetRequest"]["properties"]
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>(),
        vec!["decoy"]
    );
}

fn errors_of<'a>(inv: &'a Value, key: &str) -> Vec<&'a Value> {
    op(inv, key)["errors"].as_array().unwrap().iter().collect()
}

fn error_at<'a>(inv: &'a Value, key: &str, status: &str) -> &'a Value {
    errors_of(inv, key)
        .into_iter()
        .find(|e| e["status"] == status)
        .unwrap_or_else(|| panic!("no {} error on {}", status, key))
}

/// An error body written inline (not a `$ref`) used to be dropped —
/// `shape_ref` came back `null` and the fact that the body has a `message`
/// field vanished (ADR 0043). It now gets the same synthesized-name
/// treatment as an inline success response or request body.
#[test]
fn an_inline_error_response_schema_is_kept_under_a_synthesized_name() {
    let mut s = spec();
    s["paths"]["/widgets/{widgetId}"]["get"]["responses"]["422"] = json!({
        "description": "invalid",
        "content": {"application/json": {"schema": {"type": "object", "properties": {"message": {"type": "string"}}}}}
    });
    let inv = build(&s);
    let e422 = error_at(&inv, "get:/widgets/{widgetId}", "422");
    assert_eq!(e422["shape_ref"], "#/shapes/GetWidgetError422");
    assert_eq!(
        inv["shapes"]["GetWidgetError422"]["properties"]
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>(),
        vec!["message"]
    );
    assert_eq!(validate(&inv, &schema()), Vec::<String>::new());
}

/// An error response written as a `$ref` already carries a name — the
/// component's — and keeps pointing at it rather than getting a second,
/// synthesized shape.
#[test]
fn a_ref_error_response_keeps_its_component_shape_ref() {
    let mut s = spec();
    s["components"]["schemas"]["ErrorBody"] =
        json!({"type": "object", "properties": {"message": {"type": "string"}}});
    s["paths"]["/widgets/{widgetId}"]["get"]["responses"]["401"] = json!({
        "description": "unauthorized",
        "content": {"application/json": {"schema": {"$ref": "#/components/schemas/ErrorBody"}}}
    });
    let inv = build(&s);
    let e401 = error_at(&inv, "get:/widgets/{widgetId}", "401");
    assert_eq!(e401["shape_ref"], "#/shapes/ErrorBody");
    assert!(inv["shapes"].get("GetWidgetError401").is_none());
    assert_eq!(validate(&inv, &schema()), Vec::<String>::new());
}

/// A status with description only, and no `content` at all, documents that
/// the status can happen but not what its body looks like. `shape_ref` stays
/// `null` — never a fabricated shape for a body nobody described.
#[test]
fn an_error_response_with_no_documented_body_stays_shape_ref_null() {
    let inv = build(&spec());
    let e404 = error_at(&inv, "get:/widgets", "404");
    assert_eq!(e404["shape_ref"], Value::Null);
    assert!(inv["shapes"].get("ListWidgetsError404").is_none());
    assert_eq!(validate(&inv, &schema()), Vec::<String>::new());
}

/// Two inline error bodies on the same operation get distinct synthesized
/// names (`{Op}Error{Status}` keeps them apart by status), and when that
/// name is already taken — here by a component a sibling operation's `$ref`
/// pulled in — `unique_shape_name` de-duplicates it exactly like it does for
/// an inline response or request body, appending `2` rather than colliding.
#[test]
fn two_inline_error_shapes_on_one_operation_get_distinct_names_and_dedupe_on_collision() {
    let mut s = spec();
    s["components"]["schemas"]["CreateWidgetError400"] =
        json!({"type": "object", "properties": {"decoy": {"type": "string"}}});
    s["paths"]["/widgets/{widgetId}"]["delete"]["responses"]["410"] = json!({
        "description": "already gone",
        "content": {"application/json": {"schema": {"$ref": "#/components/schemas/CreateWidgetError400"}}}
    });
    s["paths"]["/widgets"]["post"]["responses"]["400"] = json!({
        "description": "bad request",
        "content": {"application/json": {"schema": {"type": "object", "properties": {"reason": {"type": "string"}}}}}
    });
    s["paths"]["/widgets"]["post"]["responses"]["409"] = json!({
        "description": "conflict",
        "content": {"application/json": {"schema": {"type": "object", "properties": {"conflict_id": {"type": "string"}}}}}
    });
    let inv = build(&s);
    let e400 = error_at(&inv, "post:/widgets", "400");
    let e409 = error_at(&inv, "post:/widgets", "409");
    assert_eq!(e400["shape_ref"], "#/shapes/CreateWidgetError4002");
    assert_eq!(e409["shape_ref"], "#/shapes/CreateWidgetError409");
    assert_ne!(e400["shape_ref"], e409["shape_ref"]);
    assert_eq!(
        inv["shapes"]["CreateWidgetError4002"]["properties"]
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>(),
        vec!["reason"]
    );
    assert_eq!(
        inv["shapes"]["CreateWidgetError409"]["properties"]
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>(),
        vec!["conflict_id"]
    );
    // The pre-existing component the collision came from is untouched.
    assert_eq!(
        inv["shapes"]["CreateWidgetError400"]["properties"]
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>(),
        vec!["decoy"]
    );
    assert_eq!(validate(&inv, &schema()), Vec::<String>::new());
}

/// The combination — a ref'd error, an inline one, a bodyless one and a
/// collision-deduplicated one all on the same document — still builds an
/// inventory that satisfies the contract schema.
#[test]
fn an_inventory_with_inline_error_shapes_satisfies_the_contract_schema() {
    let mut s = spec();
    s["components"]["schemas"]["ErrorBody"] =
        json!({"type": "object", "properties": {"message": {"type": "string"}}});
    s["paths"]["/widgets/{widgetId}"]["get"]["responses"]["401"] = json!({
        "description": "unauthorized",
        "content": {"application/json": {"schema": {"$ref": "#/components/schemas/ErrorBody"}}}
    });
    s["paths"]["/widgets/{widgetId}"]["get"]["responses"]["422"] = json!({
        "description": "invalid",
        "content": {"application/json": {"schema": {"type": "object", "properties": {"message": {"type": "string"}}}}}
    });
    s["paths"]["/widgets"]["post"]["responses"]["400"] = json!({
        "description": "bad request",
        "content": {"application/json": {"schema": {"type": "object", "properties": {"reason": {"type": "string"}}}}}
    });
    let inv = build(&s);
    assert_eq!(validate(&inv, &schema()), Vec::<String>::new());
    assert_eq!(
        error_at(&inv, "get:/widgets/{widgetId}", "401")["shape_ref"],
        "#/shapes/ErrorBody"
    );
    assert_eq!(
        error_at(&inv, "get:/widgets/{widgetId}", "422")["shape_ref"],
        "#/shapes/GetWidgetError422"
    );
    assert_eq!(
        error_at(&inv, "post:/widgets", "400")["shape_ref"],
        "#/shapes/CreateWidgetError400"
    );
    assert_eq!(
        error_at(&inv, "get:/widgets", "404")["shape_ref"],
        Value::Null
    );
}

#[test]
fn an_openapi_3_document_carrying_swagger_spellings_is_read_as_openapi_3() {
    // `x-nullable`, `type: file` and `consumes` are 2.0 spellings; an OpenAPI 3
    // reader leaves them alone. A `body` parameter is not an OpenAPI 3
    // location: it is dropped from `parameters` and the operation is
    // needs_review naming it, never silently modelled or silently lost.
    let oas3 = json!({
        "openapi": "3.0.3", "info": {"title": "O", "version": "1"}, "servers": [{"url": "https://api.o.test"}],
        "consumes": ["application/octet-stream"],
        "components": {"schemas": {"Note": {"type": "object", "properties": {
            "text": {"type": "string", "x-nullable": true},
            "blob": {"type": "file"}
        }}}},
        "paths": {"/notes": {"post": {
            "operationId": "createNote",
            "parameters": [{"name": "body", "in": "body", "schema": {"$ref": "#/components/schemas/Note"}}],
            "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Note"}}}}}
        }}}
    });
    let built = build_inventory(&oas3).unwrap();
    let inv = &built.inventory;
    let note = &inv["shapes"]["Note"]["properties"];
    assert!(note["text"].get("nullable").is_none(), "{}", note["text"]);
    assert_eq!(note["blob"]["type"], "file");
    let o = &inv["operations"][0];
    assert!(o["parameters"].as_array().unwrap().is_empty());
    assert!(o["request_body"].is_null());
    assert_eq!(o["support"], "needs_review");
    assert!(
        o["support_reason"]
            .as_str()
            .unwrap()
            .contains("body (body: not an OpenAPI 3 location)"),
        "{}",
        o["support_reason"]
    );
    assert!(
        built.warnings.iter().any(|w| w.starts_with("post:/notes:")),
        "{:?}",
        built.warnings
    );
    assert_eq!(validate(inv, &schema()), Vec::<String>::new());
}

// ── OAuth2 flow facts (ADR 0019) ──────────────────────────────────────────

fn oauth_spec() -> Value {
    let mut s = spec();
    s["components"]["securitySchemes"] = json!({
        "oauth": {"type": "oauth2", "flows": {
            "authorizationCode": {
                "authorizationUrl": "https://accounts.example/o/oauth2/v2/auth",
                "tokenUrl": "https://oauth2.example/token",
                "refreshUrl": "https://oauth2.example/refresh",
                "scopes": {"read": "Read things", "write": "Write things", "admin": ""}
            },
            "clientCredentials": {"tokenUrl": "https://oauth2.example/token", "scopes": {}}
        }},
        "bare": {"type": "oauth2"},
        "oidc": {"type": "openIdConnect", "openIdConnectUrl": "https://accounts.example/.well-known/openid-configuration"}
    });
    s["security"] = json!([{"oauth": ["read"]}]);
    s
}

#[test]
fn an_authorization_code_flow_is_recorded_verbatim_with_its_scopes_and_the_other_flows_named() {
    let inv = build(&oauth_spec());
    let auth = inv["api"]["auth"].as_array().unwrap();
    let oauth = auth
        .iter()
        .find(|a| a["scheme_name"] == "oauth")
        .expect("the oauth scheme");
    assert_eq!(oauth["kind"], "oauth2");
    assert_eq!(oauth["prefix"], "Bearer ");
    assert_eq!(
        oauth["oauth2"],
        json!({
            "flows": ["authorization_code", "client_credentials"],
            "authorization_code": {
                "authorization_url": "https://accounts.example/o/oauth2/v2/auth",
                "token_url": "https://oauth2.example/token",
                "refresh_url": "https://oauth2.example/refresh",
                "scopes": [
                    {"name": "read", "description": "Read things"},
                    {"name": "write", "description": "Write things"},
                    {"name": "admin", "description": null}
                ]
            }
        })
    );
    // A scheme with no flows records none; an OpenID Connect scheme is
    // oauth2 to the request but has no flow to run.
    for name in ["bare", "oidc"] {
        let s = auth.iter().find(|a| a["scheme_name"] == name).unwrap();
        assert_eq!(s["kind"], "oauth2");
        assert!(s.get("oauth2").is_none(), "{}: {:?}", name, s);
    }
    assert_eq!(validate(&inv, &schema()), Vec::<String>::new());
}

#[test]
fn a_flow_without_both_endpoints_is_named_but_not_recorded() {
    let mut s = oauth_spec();
    s["components"]["securitySchemes"]["oauth"]["flows"]["authorizationCode"] =
        json!({"authorizationUrl": "https://accounts.example/auth", "scopes": {"read": "r"}});
    let inv = build(&s);
    let oauth = inv["api"]["auth"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["scheme_name"] == "oauth")
        .unwrap()
        .clone();
    assert_eq!(
        oauth["oauth2"],
        json!({"flows": ["authorization_code", "client_credentials"]})
    );
}

#[test]
fn security_requirements_are_recorded_where_the_document_declares_them() {
    let mut s = oauth_spec();
    s["paths"]["/widgets"]["post"] = json!({
        "operationId": "createWidget",
        "security": [{"oauth": ["write"]}, {"oauth": ["admin"]}],
        "requestBody": {"content": {"application/json": {"schema": {"type": "object", "properties": {"name": {"type": "string"}}}}}},
        "responses": {"201": {"description": "created", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Widget"}}}}}
    });
    s["paths"]["/health"] = json!({"get": {
        "operationId": "health", "security": [],
        "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {"type": "object", "properties": {"ok": {"type": "boolean"}}}}}}}
    }});
    let inv = build(&s);
    assert_eq!(inv["api"]["security"], json!([{"oauth": ["read"]}]));
    // The document default is not copied onto each operation …
    assert!(op(&inv, "get:/widgets").get("security").is_none());
    // … an operation's own requirement is recorded as declared …
    assert_eq!(
        op(&inv, "post:/widgets")["security"],
        json!([{"oauth": ["write"]}, {"oauth": ["admin"]}])
    );
    // … and an explicit empty list says the operation takes no credential.
    assert_eq!(op(&inv, "get:/health")["security"], json!([]));
    assert_eq!(validate(&inv, &schema()), Vec::<String>::new());
}

#[test]
fn a_document_with_no_security_requirement_records_none() {
    let inv = build(&spec());
    assert!(inv["api"].get("security").is_none());
    assert!(op(&inv, "get:/widgets").get("security").is_none());
}

// ── Parameter default/minimum/maximum ────────────────────────────────────────

#[test]
fn pagination_parameter_default_and_bounds_are_captured() {
    let mut s = spec();
    s["paths"]["/widgets"]["get"]["parameters"] = json!([
        {"name": "cursor", "in": "query", "schema": {"type": "string"}},
        {"name": "limit", "in": "query", "schema": {"type": "integer", "default": 20, "minimum": 1, "maximum": 100}}
    ]);
    let inv = build(&s);
    let limit = op(&inv, "get:/widgets")["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "limit")
        .expect("limit parameter");
    assert_eq!(limit["type"], "integer");
    assert_eq!(limit["default"], 20);
    assert_eq!(limit["minimum"], 1);
    assert_eq!(limit["maximum"], 100);
    assert_eq!(validate(&inv, &schema()), Vec::<String>::new());
}

#[test]
fn pagination_parameter_without_bounds_has_no_spurious_fields() {
    let mut s = spec();
    s["paths"]["/widgets"]["get"]["parameters"] = json!([
        {"name": "cursor", "in": "query", "schema": {"type": "string"}},
        {"name": "limit", "in": "query", "schema": {"type": "integer"}}
    ]);
    let inv = build(&s);
    let limit = op(&inv, "get:/widgets")["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "limit")
        .expect("limit parameter");
    assert!(
        limit.get("default").is_none(),
        "absent default means no key, not null"
    );
    assert!(
        limit.get("minimum").is_none(),
        "absent minimum means no key, not null"
    );
    assert!(
        limit.get("maximum").is_none(),
        "absent maximum means no key, not null"
    );
    assert_eq!(validate(&inv, &schema()), Vec::<String>::new());
}

#[test]
fn explicit_null_default_and_bounds_are_treated_as_absent() {
    // A malformed spec may write `"minimum": null` — JSON Schema requires a
    // number there, so null carries no "no bound" semantic. The inventory
    // omits the key, identically to an absent keyword; likewise for an
    // explicit `"default": null` (ADR 0025). The schema forbids null for
    // minimum/maximum, so validate() also proves nothing null was emitted.
    let mut s = spec();
    s["paths"]["/widgets"]["get"]["parameters"] = json!([
        {"name": "limit", "in": "query",
         "schema": {"type": "integer", "default": null, "minimum": null, "maximum": null}}
    ]);
    let inv = build(&s);
    let limit = op(&inv, "get:/widgets")["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "limit")
        .expect("limit parameter");
    assert!(
        limit.get("default").is_none(),
        "explicit null default means no key, not null"
    );
    assert!(
        limit.get("minimum").is_none(),
        "explicit null minimum means no key, not null"
    );
    assert!(
        limit.get("maximum").is_none(),
        "explicit null maximum means no key, not null"
    );
    assert_eq!(validate(&inv, &schema()), Vec::<String>::new());
}

#[test]
fn string_default_is_passed_through_verbatim() {
    // A default of any JSON type is cloned as-is.
    let mut s = spec();
    s["paths"]["/widgets"]["get"]["parameters"] = json!([
        {"name": "filter", "in": "query", "schema": {"type": "string", "default": "all"}}
    ]);
    let inv = build(&s);
    let filter = op(&inv, "get:/widgets")["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "filter")
        .expect("filter parameter");
    assert_eq!(filter["default"], "all");
    // A non-numeric default does not yield minimum/maximum keys.
    assert!(filter.get("minimum").is_none());
    assert!(filter.get("maximum").is_none());
    assert_eq!(validate(&inv, &schema()), Vec::<String>::new());
}

// ADR 0033 — candidate_entity_link (Half 1). A GET-by-id operation's path
// parameter is cross-referenced, by case-normalized name and type family,
// against every other operation's response shape; a match is recorded on
// the matching property, never on the parameter.

#[test]
fn a_path_parameter_matches_a_differently_cased_property_on_another_shape() {
    let mut s = spec();
    s["components"]["schemas"]["Order"] =
        json!({"type": "object", "properties": {"id": {"type": "string"}}});
    // `order_id` (snake_case) vs. the parameter's `orderId` (camelCase) —
    // the exact fold PR #9's TypeScript generator got bitten by.
    s["components"]["schemas"]["Invoice"] = json!({"type": "object", "properties": {
        "id": {"type": "string"},
        "order_id": {"type": "string"}
    }});
    s["paths"]["/orders/{orderId}"] = json!({
        "get": {
            "operationId": "getOrder",
            "parameters": [{"name": "orderId", "in": "path", "required": true, "schema": {"type": "string"}}],
            "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Order"}}}}}
        }
    });
    s["paths"]["/invoices/{invoiceId}"] = json!({
        "get": {
            "operationId": "getInvoice",
            "parameters": [{"name": "invoiceId", "in": "path", "required": true, "schema": {"type": "string"}}],
            "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Invoice"}}}}}
        }
    });
    let inv = build(&s);
    // getOrder's own response is a single Order, not a list.
    assert_eq!(
        inv["shapes"]["Invoice"]["properties"]["order_id"]["candidate_entity_link"],
        json!({"operation": "get:/orders/{orderId}", "parameter": "orderId", "list_context": false})
    );
    // `id` does not name-match `orderId`/`invoiceId`: no fact.
    assert!(inv["shapes"]["Order"]["properties"]["id"]
        .get("candidate_entity_link")
        .is_none());
    assert!(inv["shapes"]["Invoice"]["properties"]["id"]
        .get("candidate_entity_link")
        .is_none());
    assert_eq!(validate(&inv, &schema()), Vec::<String>::new());
}

#[test]
fn a_get_by_id_returning_a_bare_array_is_not_a_link_target() {
    // Until ADR 0085 this was the one shape in which a canonical target was
    // a list, and the fact said `list_context: true`. A list is not the one
    // record the key names, so the target is refused and the property
    // carries no fact; `list_context` is false on every fact since.
    let mut s = spec();
    s["components"]["schemas"]["Event"] =
        json!({"type": "object", "properties": {"id": {"type": "string"}}});
    s["components"]["schemas"]["Report"] = json!({"type": "object", "properties": {
        "id": {"type": "string"},
        "account_id": {"type": "string"}
    }});
    s["paths"]["/accounts/{accountId}"] = json!({
        "get": {
            "operationId": "getAccountEvents",
            "parameters": [{"name": "accountId", "in": "path", "required": true, "schema": {"type": "string"}}],
            "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {
                "type": "array", "items": {"$ref": "#/components/schemas/Event"}
            }}}}}
        }
    });
    s["paths"]["/reports/{reportId}"] = json!({
        "get": {
            "operationId": "getReport",
            "parameters": [{"name": "reportId", "in": "path", "required": true, "schema": {"type": "string"}}],
            "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Report"}}}}}
        }
    });
    let inv = build(&s);
    assert!(inv["shapes"]["Report"]["properties"]["account_id"]
        .get("candidate_entity_link")
        .is_none());
    let refused = graphos_factory_core::cmd::inventory_links::refused_targets(&inv);
    assert_eq!(refused.len(), 1);
    assert_eq!(refused[0].operation, "get:/accounts/{accountId}");
    assert_eq!(refused[0].reason, "its response is a list, not one record");
    assert_eq!(validate(&inv, &schema()), Vec::<String>::new());
}

/// A spec with one host (`Share.email`, `Share.album_id`) and one by-id
/// target per case; `target` is the target's response schema.
fn key_carrying_spec(param: &str, param_type: &str, target: Value) -> Value {
    let mut s = spec();
    s["components"]["schemas"]["Share"] = json!({"type": "object", "properties": {
        "share_id": {"type": "string"},
        "email": {"type": "string"},
        "album_id": {"type": "integer"},
        "pet_id": {"type": "integer"}
    }});
    s["paths"]["/shares/{share_id}"] = by_id_path("share_id", "getShare", "Share");
    s["paths"][format!("/targets/{{{}}}", param)] = json!({
        "get": {
            "operationId": "getTarget",
            "parameters": [{"name": param, "in": "path", "required": true, "schema": {"type": param_type}}],
            "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": target}}}}
        }
    });
    s
}

#[test]
fn a_get_by_id_whose_response_does_not_carry_its_key_is_not_a_link_target() {
    // AppWorld splitwise: `get:/splitwise/balance/person/{email}` returns
    // the caller's balance with that person — `direction`, `total`,
    // `breakdown`, no `email` — and 35 `email` properties pointed at it
    // (56 facts -> 21). The field `selection draft --links` proposed for
    // them was named `person` and answered 422 on the caller's own email.
    let s = key_carrying_spec(
        "email",
        "string",
        json!({"type": "object", "properties": {
            "direction": {"type": "string"},
            "total": {"type": "number"},
            "breakdown": {"type": "array", "items": {"type": "object", "properties": {"amount": {"type": "number"}}}}
        }}),
    );
    let inv = build(&s);
    assert!(inv["shapes"]["Share"]["properties"]["email"]
        .get("candidate_entity_link")
        .is_none());
    let refused = graphos_factory_core::cmd::inventory_links::refused_targets(&inv);
    assert_eq!(
        refused.len(),
        1,
        "{:?}",
        refused.iter().map(|r| &r.reason).collect::<Vec<_>>()
    );
    assert_eq!(refused[0].operation, "get:/targets/{email}");
    assert_eq!(refused[0].parameter, "email");
    assert_eq!(
        refused[0].reason,
        "its response carries no `email` of the parameter's type: it describes something other than the record the key identifies"
    );
    assert_eq!(validate(&inv, &schema()), Vec::<String>::new());
}

#[test]
fn a_target_carries_its_key_by_name_by_a_qualified_name_by_its_id_or_inside_an_envelope() {
    let fact = |op: &str, p: &str| json!({"operation": op, "parameter": p, "list_context": false});
    // The same name.
    let inv = build(&key_carrying_spec(
        "email",
        "string",
        json!({"type": "object", "properties": {"email": {"type": "string"}, "name": {"type": "string"}}}),
    ));
    assert_eq!(
        inv["shapes"]["Share"]["properties"]["email"]["candidate_entity_link"],
        fact("get:/targets/{email}", "email")
    );
    // A qualified name: the record spells `{album_id}` as `lp_album_id`.
    let inv = build(&key_carrying_spec(
        "album_id",
        "integer",
        json!({"type": "object", "properties": {"lp_album_id": {"type": "integer"}}}),
    ));
    assert_eq!(
        inv["shapes"]["Share"]["properties"]["album_id"]["candidate_entity_link"],
        fact("get:/targets/{album_id}", "album_id")
    );
    // The canonical id: `/pets/{petId}` returning `Pet {id}`.
    let inv = build(&key_carrying_spec(
        "petId",
        "integer",
        json!({"type": "object", "properties": {"id": {"type": "integer"}, "name": {"type": "string"}}}),
    ));
    assert_eq!(
        inv["shapes"]["Share"]["properties"]["pet_id"]["candidate_entity_link"],
        fact("get:/targets/{petId}", "petId")
    );
    // A one-property envelope around the record.
    let inv = build(&key_carrying_spec(
        "petId",
        "integer",
        json!({"type": "object", "properties": {"pet": {"type": "object", "properties": {"id": {"type": "integer"}}}}}),
    ));
    assert_eq!(
        inv["shapes"]["Share"]["properties"]["pet_id"]["candidate_entity_link"],
        fact("get:/targets/{petId}", "petId")
    );
    assert!(graphos_factory_core::cmd::inventory_links::refused_targets(&inv).is_empty());
}

#[test]
fn a_key_of_another_type_family_or_an_id_for_a_non_id_parameter_does_not_count() {
    // An integer `id` does not carry `{username}` (gitea `User` spells the
    // key `login`), and a string `email` does not carry an integer `{email}`.
    for (param, ty, target) in [
        (
            "email",
            "string",
            json!({"type": "object", "properties": {"id": {"type": "string"}, "login": {"type": "string"}}}),
        ),
        (
            "pet_id",
            "integer",
            json!({"type": "object", "properties": {"id": {"type": "string"}}}),
        ),
    ] {
        let inv = build(&key_carrying_spec(param, ty, target));
        let refused = graphos_factory_core::cmd::inventory_links::refused_targets(&inv);
        assert_eq!(refused.len(), 1, "{}", param);
        assert_eq!(refused[0].parameter, param);
    }
}

#[test]
fn a_name_match_with_a_mismatched_type_family_is_not_a_candidate() {
    let mut s = spec();
    // `ticketId` is integer-shaped; `ticket_id` on Comment is string-shaped.
    // The name matches after case-normalization but the family does not, so
    // this must not become a candidate_entity_link — proving the check is a
    // type-family match, not a bare string comparison.
    s["components"]["schemas"]["Comment"] = json!({"type": "object", "properties": {
        "id": {"type": "string"},
        "ticket_id": {"type": "string"}
    }});
    s["paths"]["/tickets/{ticketId}"] = json!({
        "get": {
            "operationId": "getTicket",
            "parameters": [{"name": "ticketId", "in": "path", "required": true, "schema": {"type": "integer"}}],
            "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {"type": "object", "properties": {"id": {"type": "integer"}}}}}}}
        }
    });
    s["paths"]["/comments/{commentId}"] = json!({
        "get": {
            "operationId": "getComment",
            "parameters": [{"name": "commentId", "in": "path", "required": true, "schema": {"type": "string"}}],
            "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Comment"}}}}}
        }
    });
    let inv = build(&s);
    assert!(inv["shapes"]["Comment"]["properties"]["ticket_id"]
        .get("candidate_entity_link")
        .is_none());
    assert_eq!(validate(&inv, &schema()), Vec::<String>::new());
}

// ADR 0069 — candidate_entity_link, second pass. A link target is a
// canonical GET-by-id operation (GET, one trailing path parameter, a
// response shape, no other required non-header parameter); a property is
// never linked to the operation family that already identifies its own
// shape; a bare `id`/`name` parameter is never a target.

#[test]
fn a_get_by_id_is_preferred_over_a_delete_on_the_same_path() {
    let mut s = spec();
    s["components"]["schemas"]["Playlist"] = json!({"type": "object", "properties": {
        "playlist_id": {"type": "integer"},
        "name": {"type": "string"}
    }});
    s["components"]["schemas"]["PlaylistReview"] = json!({"type": "object", "properties": {
        "review_id": {"type": "integer"},
        "playlist_id": {"type": "integer"}
    }});
    // A DELETE carrying the same parameter name comes FIRST in document
    // order — under first-match-wins it used to be the target. Spotify's
    // real case is the same-path sibling `delete:/playlists/{playlist_id}`,
    // which METHODS visits right after the excluded own GET. It is given a
    // response shape here (unusual for a DELETE, but some APIs echo the
    // deleted record) so the method check alone is what excludes it — the
    // response-shape check would otherwise reject it regardless.
    s["paths"]["/archive/{playlist_id}"] = json!({
        "delete": {
            "operationId": "archivePlaylist",
            "parameters": [{"name": "playlist_id", "in": "path", "required": true, "schema": {"type": "integer"}}],
            "responses": {"200": {"description": "archived", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Playlist"}}}}}
        }
    });
    s["paths"]["/playlists/{playlist_id}"] = json!({
        "get": {
            "operationId": "getPlaylist",
            "parameters": [{"name": "playlist_id", "in": "path", "required": true, "schema": {"type": "integer"}}],
            "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Playlist"}}}}}
        },
        "delete": {
            "operationId": "deletePlaylist",
            "parameters": [{"name": "playlist_id", "in": "path", "required": true, "schema": {"type": "integer"}}],
            "responses": {"204": {"description": "gone"}}
        }
    });
    s["paths"]["/reviews/{review_id}"] = json!({
        "get": {
            "operationId": "getReview",
            "parameters": [{"name": "review_id", "in": "path", "required": true, "schema": {"type": "integer"}}],
            "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/PlaylistReview"}}}}}
        }
    });
    let inv = build(&s);
    // The foreign key resolves through the GET-by-id, not the earlier DELETE.
    assert_eq!(
        inv["shapes"]["PlaylistReview"]["properties"]["playlist_id"]["candidate_entity_link"],
        json!({"operation": "get:/playlists/{playlist_id}", "parameter": "playlist_id", "list_context": false})
    );
    // The record's own id gets no fact at all — not the GET, not either DELETE.
    assert!(inv["shapes"]["Playlist"]["properties"]["playlist_id"]
        .get("candidate_entity_link")
        .is_none());
    let text = inv.to_string();
    assert!(
        !text.contains("\"operation\":\"delete:"),
        "a DELETE is never a link target: {}",
        text
    );
    assert_eq!(validate(&inv, &schema()), Vec::<String>::new());
}

#[test]
fn a_required_non_header_parameter_disqualifies_a_target() {
    let mut s = spec();
    s["components"]["schemas"]["Order"] =
        json!({"type": "object", "properties": {"order_id": {"type": "integer"}}});
    s["components"]["schemas"]["LineItem"] = json!({"type": "object", "properties": {
        "line_item_id": {"type": "integer"},
        "order_id": {"type": "integer"}
    }});
    // A required query parameter is a second argument the record's own id
    // can't stand in for — the operation is disqualified as a target.
    s["paths"]["/orders/{orderId}"] = json!({
        "get": {
            "operationId": "getOrder",
            "parameters": [
                {"name": "orderId", "in": "path", "required": true, "schema": {"type": "integer"}},
                {"name": "expand", "in": "query", "required": true, "schema": {"type": "string"}}
            ],
            "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Order"}}}}}
        }
    });
    s["paths"]["/line-items/{lineItemId}"] = json!({
        "get": {
            "operationId": "getLineItem",
            "parameters": [{"name": "lineItemId", "in": "path", "required": true, "schema": {"type": "integer"}}],
            "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/LineItem"}}}}}
        }
    });
    s["components"]["schemas"]["Customer"] =
        json!({"type": "object", "properties": {"customer_id": {"type": "integer"}}});
    s["components"]["schemas"]["Invoice"] = json!({"type": "object", "properties": {
        "invoice_id": {"type": "integer"},
        "customer_id": {"type": "integer"}
    }});
    // A required header, by contrast, is the credential, not an argument —
    // it does not disqualify the operation.
    s["paths"]["/customers/{customerId}"] = json!({
        "get": {
            "operationId": "getCustomer",
            "parameters": [
                {"name": "customerId", "in": "path", "required": true, "schema": {"type": "integer"}},
                {"name": "X-Api-Version", "in": "header", "required": true, "schema": {"type": "string"}}
            ],
            "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Customer"}}}}}
        }
    });
    s["paths"]["/invoices/{invoiceId}"] = json!({
        "get": {
            "operationId": "getInvoice",
            "parameters": [{"name": "invoiceId", "in": "path", "required": true, "schema": {"type": "integer"}}],
            "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Invoice"}}}}}
        }
    });
    let inv = build(&s);
    assert!(inv["shapes"]["LineItem"]["properties"]["order_id"]
        .get("candidate_entity_link")
        .is_none());
    assert_eq!(
        inv["shapes"]["Invoice"]["properties"]["customer_id"]["candidate_entity_link"],
        json!({"operation": "get:/customers/{customerId}", "parameter": "customerId", "list_context": false})
    );
    assert_eq!(validate(&inv, &schema()), Vec::<String>::new());
}

#[test]
fn a_sub_resource_get_is_not_a_link_target() {
    let mut s = spec();
    s["components"]["schemas"]["Receipt"] =
        json!({"type": "object", "properties": {"total": {"type": "integer"}}});
    s["components"]["schemas"]["Invoice"] = json!({"type": "object", "properties": {
        "invoice_id": {"type": "string"},
        "order_id": {"type": "string"}
    }});
    // The only operation carrying `orderId` is a sub-resource: the parameter
    // is not the path's last segment, so it does not identify an Order.
    s["paths"]["/orders/{orderId}/receipt"] = json!({
        "get": {
            "operationId": "getOrderReceipt",
            "parameters": [{"name": "orderId", "in": "path", "required": true, "schema": {"type": "string"}}],
            "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Receipt"}}}}}
        }
    });
    s["paths"]["/invoices/{invoiceId}"] = json!({
        "get": {
            "operationId": "getInvoice",
            "parameters": [{"name": "invoiceId", "in": "path", "required": true, "schema": {"type": "string"}}],
            "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Invoice"}}}}}
        }
    });
    let inv = build(&s);
    assert_eq!(
        inv["shapes"]["Invoice"]["properties"]["order_id"]["type"],
        "string"
    );
    assert!(inv["shapes"]["Invoice"]["properties"]["order_id"]
        .get("candidate_entity_link")
        .is_none());
    assert_eq!(validate(&inv, &schema()), Vec::<String>::new());
}

#[test]
fn a_shapes_own_id_is_never_linked_even_when_another_operation_returns_the_shape() {
    let mut s = spec();
    // gitea's `Hook`: returned by post:/admin/hooks AND get:/admin/hooks/{id}.
    // Visited from the POST, the old check (`pp.operation != op_key`) let
    // Hook.id link to its own GET-by-id. The guard must be per shape.
    s["components"]["schemas"]["Hook"] = json!({"type": "object", "properties": {
        "hook_id": {"type": "integer"},
        "url": {"type": "string"}
    }});
    s["paths"]["/hooks"] = json!({
        "post": {
            "operationId": "createHook",
            "requestBody": {"required": true, "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Hook"}}}},
            "responses": {"201": {"description": "created", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Hook"}}}}}
        }
    });
    s["paths"]["/hooks/{hook_id}"] = json!({
        "get": {
            "operationId": "getHook",
            "parameters": [{"name": "hook_id", "in": "path", "required": true, "schema": {"type": "integer"}}],
            "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Hook"}}}}}
        }
    });
    // A different shape that carries hook_id as a real foreign key still links.
    s["components"]["schemas"]["Delivery"] = json!({"type": "object", "properties": {
        "delivery_id": {"type": "integer"},
        "hook_id": {"type": "integer"}
    }});
    s["paths"]["/deliveries/{delivery_id}"] = json!({
        "get": {
            "operationId": "getDelivery",
            "parameters": [{"name": "delivery_id", "in": "path", "required": true, "schema": {"type": "integer"}}],
            "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Delivery"}}}}}
        }
    });
    let inv = build(&s);
    assert!(inv["shapes"]["Hook"]["properties"]["hook_id"]
        .get("candidate_entity_link")
        .is_none());
    assert_eq!(
        inv["shapes"]["Delivery"]["properties"]["hook_id"]["candidate_entity_link"],
        json!({"operation": "get:/hooks/{hook_id}", "parameter": "hook_id", "list_context": false})
    );
    assert_eq!(validate(&inv, &schema()), Vec::<String>::new());
}

#[test]
fn a_shape_identified_by_a_non_canonical_operation_still_owns_that_parameter() {
    let mut s = spec();
    // `get:/repos/{repo}/commits/{sha}` is not itself a canonical target (two
    // path parameters), but it is still the operation that identifies
    // Commit — `sha` is Commit's own id, not a foreign key to Blob, even
    // though `get:/blobs/{sha}` is a perfectly canonical GET-by-id with a
    // matching name and type family.
    s["components"]["schemas"]["Blob"] = json!({"type": "object", "properties": {
        "sha": {"type": "string"},
        "size": {"type": "integer"}
    }});
    s["components"]["schemas"]["Commit"] = json!({"type": "object", "properties": {
        "sha": {"type": "string"},
        "message": {"type": "string"}
    }});
    s["paths"]["/blobs/{sha}"] = json!({
        "get": {
            "operationId": "getBlob",
            "parameters": [{"name": "sha", "in": "path", "required": true, "schema": {"type": "string"}}],
            "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Blob"}}}}}
        }
    });
    s["paths"]["/repos/{repo}/commits/{sha}"] = json!({
        "get": {
            "operationId": "getCommit",
            "parameters": [
                {"name": "repo", "in": "path", "required": true, "schema": {"type": "string"}},
                {"name": "sha", "in": "path", "required": true, "schema": {"type": "string"}}
            ],
            "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Commit"}}}}}
        }
    });
    let inv = build(&s);
    assert_eq!(
        inv["shapes"]["Commit"]["properties"]["sha"]["type"],
        "string"
    );
    assert!(inv["shapes"]["Commit"]["properties"]["sha"]
        .get("candidate_entity_link")
        .is_none());
    assert_eq!(validate(&inv, &schema()), Vec::<String>::new());
}

#[test]
fn a_bare_id_parameter_is_not_a_link_target() {
    let mut s = spec();
    // gitea: 26 `id` -> get:/admin/hooks/{id} and 18 `name` ->
    // get:/gitignore/templates/{name}, every one a collision.
    s["components"]["schemas"]["Hook"] =
        json!({"type": "object", "properties": {"id": {"type": "integer"}}});
    s["components"]["schemas"]["Template"] =
        json!({"type": "object", "properties": {"name": {"type": "string"}}});
    s["components"]["schemas"]["Organization"] = json!({"type": "object", "properties": {
        "id": {"type": "integer"},
        "name": {"type": "string"},
        "org_name": {"type": "string"}
    }});
    s["paths"]["/admin/hooks/{id}"] = json!({
        "get": {
            "operationId": "getHook",
            "parameters": [{"name": "id", "in": "path", "required": true, "schema": {"type": "integer"}}],
            "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Hook"}}}}}
        }
    });
    s["paths"]["/gitignore/templates/{name}"] = json!({
        "get": {
            "operationId": "getTemplate",
            "parameters": [{"name": "name", "in": "path", "required": true, "schema": {"type": "string"}}],
            "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Template"}}}}}
        }
    });
    s["paths"]["/orgs/{org_name}"] = json!({
        "get": {
            "operationId": "getOrg",
            "parameters": [{"name": "org_name", "in": "path", "required": true, "schema": {"type": "string"}}],
            "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Organization"}}}}}
        }
    });
    let inv = build(&s);
    assert_eq!(
        inv["shapes"]["Organization"]["properties"]["id"]["type"],
        "integer"
    );
    assert_eq!(
        inv["shapes"]["Organization"]["properties"]["name"]["type"],
        "string"
    );
    assert!(inv["shapes"]["Organization"]["properties"]["id"]
        .get("candidate_entity_link")
        .is_none());
    assert!(inv["shapes"]["Organization"]["properties"]["name"]
        .get("candidate_entity_link")
        .is_none());
    assert_eq!(validate(&inv, &schema()), Vec::<String>::new());
}

#[test]
fn a_multi_parameter_path_is_not_a_target() {
    let mut s = spec();
    s["components"]["schemas"]["Repository"] =
        json!({"type": "object", "properties": {"full_name": {"type": "string"}}});
    s["components"]["schemas"]["Issue"] = json!({"type": "object", "properties": {
        "issue_id": {"type": "integer"},
        "repo": {"type": "string"}
    }});
    // Two path parameters: `{$this.repo}` alone could never address it.
    s["paths"]["/orgs/{org}/repos/{repo}"] = json!({
        "get": {
            "operationId": "getRepo",
            "parameters": [
                {"name": "org", "in": "path", "required": true, "schema": {"type": "string"}},
                {"name": "repo", "in": "path", "required": true, "schema": {"type": "string"}}
            ],
            "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Repository"}}}}}
        }
    });
    s["paths"]["/issues/{issue_id}"] = json!({
        "get": {
            "operationId": "getIssue",
            "parameters": [{"name": "issue_id", "in": "path", "required": true, "schema": {"type": "integer"}}],
            "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Issue"}}}}}
        }
    });
    let inv = build(&s);
    assert_eq!(
        inv["shapes"]["Issue"]["properties"]["repo"]["type"],
        "string"
    );
    assert!(inv["shapes"]["Issue"]["properties"]["repo"]
        .get("candidate_entity_link")
        .is_none());
    assert_eq!(validate(&inv, &schema()), Vec::<String>::new());
}

#[test]
fn a_path_item_parameter_redeclared_on_the_operation_still_qualifies() {
    let mut s = spec();
    s["components"]["schemas"]["Order"] =
        json!({"type": "object", "properties": {"order_id": {"type": "string"}}});
    s["components"]["schemas"]["Invoice"] = json!({"type": "object", "properties": {
        "invoice_id": {"type": "string"},
        "order_id": {"type": "string"}
    }});
    // OpenAPI lets an operation re-declare (override) a path-item-level
    // parameter; the parser appends both copies (`shared.iter().chain(own.iter())`),
    // so this GET's own `parameters` carries two `in: path` `orderId`
    // entries with the same name — one path parameter, redeclared, not two.
    s["paths"]["/orders/{orderId}"] = json!({
        "parameters": [{"name": "orderId", "in": "path", "required": true, "schema": {"type": "string"}}],
        "get": {
            "operationId": "getOrder",
            "parameters": [{"name": "orderId", "in": "path", "required": true, "schema": {"type": "string"}, "description": "override"}],
            "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Order"}}}}}
        }
    });
    s["paths"]["/invoices/{invoiceId}"] = json!({
        "get": {
            "operationId": "getInvoice",
            "parameters": [{"name": "invoiceId", "in": "path", "required": true, "schema": {"type": "string"}}],
            "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Invoice"}}}}}
        }
    });
    let inv = build(&s);
    assert_eq!(
        inv["shapes"]["Invoice"]["properties"]["order_id"]["candidate_entity_link"],
        json!({"operation": "get:/orders/{orderId}", "parameter": "orderId", "list_context": false})
    );
    assert_eq!(validate(&inv, &schema()), Vec::<String>::new());
}

#[test]
fn a_nullable_anyof_property_has_a_type_family() {
    let mut s = spec();
    s["components"]["schemas"]["Album"] = json!({"type": "object", "properties": {
        "album_id": {"type": "integer"},
        "title": {"type": "string"}
    }});
    s["components"]["schemas"]["Artist"] = json!({"type": "object", "properties": {
        "artist_id": {"type": "integer"},
        "name": {"type": "string"}
    }});
    s["components"]["schemas"]["Label"] = json!({"type": "object", "properties": {
        "label_id": {"type": "integer"},
        "name": {"type": "string"}
    }});
    // Three ways a spec writes "a nullable integer": FastAPI's anyOf with a
    // null branch (Spotify's 5 missed foreign keys — `type` is absent),
    // oneOf with a null branch, and OpenAPI 3.0 `nullable: true` beside a
    // `type` (already handled; pinned here so the fold cannot regress it).
    s["components"]["schemas"]["Song"] = json!({"type": "object", "properties": {
        "song_id": {"type": "integer"},
        "album_id": {"anyOf": [{"type": "integer"}, {"type": "null"}], "title": "Album Id"},
        "artist_id": {"oneOf": [{"type": "integer"}, {"type": "null"}]},
        "label_id": {"type": "integer", "nullable": true}
    }});
    for (path, id, name, shape) in [
        ("/albums/{album_id}", "album_id", "getAlbum", "Album"),
        ("/artists/{artist_id}", "artist_id", "getArtist", "Artist"),
        ("/labels/{label_id}", "label_id", "getLabel", "Label"),
        ("/songs/{song_id}", "song_id", "getSong", "Song"),
    ] {
        s["paths"][path] = json!({
            "get": {
                "operationId": name,
                "parameters": [{"name": id, "in": "path", "required": true, "schema": {"type": "integer"}}],
                "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {"$ref": format!("#/components/schemas/{}", shape)}}}}}
            }
        });
    }
    let inv = build(&s);
    assert_eq!(
        inv["shapes"]["Song"]["properties"]["album_id"]["candidate_entity_link"],
        json!({"operation": "get:/albums/{album_id}", "parameter": "album_id", "list_context": false})
    );
    assert_eq!(
        inv["shapes"]["Song"]["properties"]["artist_id"]["candidate_entity_link"],
        json!({"operation": "get:/artists/{artist_id}", "parameter": "artist_id", "list_context": false})
    );
    assert_eq!(
        inv["shapes"]["Song"]["properties"]["label_id"]["candidate_entity_link"],
        json!({"operation": "get:/labels/{label_id}", "parameter": "label_id", "list_context": false})
    );
    // The fold never rewrites the stored node: the anyOf stays as the spec wrote it.
    assert_eq!(
        inv["shapes"]["Song"]["properties"]["album_id"]["anyOf"],
        json!([{"type": "integer"}, {"type": "null"}])
    );
    assert_eq!(validate(&inv, &schema()), Vec::<String>::new());
}

#[test]
fn a_two_type_union_does_not() {
    let mut s = spec();
    s["components"]["schemas"]["Mixed"] = json!({"type": "object", "properties": {
        "mixed_id": {"type": "integer"},
        "label": {"type": "string"}
    }});
    // A real union — two typed branches, with or without a null third — is
    // not "a nullable integer"; it has no family and never candidates, even
    // though the name matches exactly. The integer branch comes FIRST so a
    // fold that took "the first typed branch" would match the integer
    // parameter and write a fact: that is what makes this test bite.
    s["components"]["schemas"]["Tag"] = json!({"type": "object", "properties": {
        "tag_id": {"type": "integer"},
        "mixed_id": {"anyOf": [{"type": "integer"}, {"type": "string"}]},
        "other_mixed_id": {"oneOf": [{"type": "integer"}, {"type": "string"}, {"type": "null"}]}
    }});
    s["paths"]["/mixed/{mixed_id}"] = json!({
        "get": {
            "operationId": "getMixed",
            "parameters": [{"name": "mixed_id", "in": "path", "required": true, "schema": {"type": "integer"}}],
            "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Mixed"}}}}}
        }
    });
    s["paths"]["/other-mixed/{other_mixed_id}"] = json!({
        "get": {
            "operationId": "getOtherMixed",
            "parameters": [{"name": "other_mixed_id", "in": "path", "required": true, "schema": {"type": "integer"}}],
            "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Mixed"}}}}}
        }
    });
    s["paths"]["/tags/{tag_id}"] = json!({
        "get": {
            "operationId": "getTag",
            "parameters": [{"name": "tag_id", "in": "path", "required": true, "schema": {"type": "integer"}}],
            "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Tag"}}}}}
        }
    });
    let inv = build(&s);
    assert!(inv["shapes"]["Tag"]["properties"]["mixed_id"]
        .get("candidate_entity_link")
        .is_none());
    assert!(inv["shapes"]["Tag"]["properties"]["other_mixed_id"]
        .get("candidate_entity_link")
        .is_none());
    assert_eq!(validate(&inv, &schema()), Vec::<String>::new());
}

#[test]
fn a_root_array_responses_items_are_walked() {
    let mut s = spec();
    s["components"]["schemas"]["Album"] = json!({"type": "object", "properties": {
        "id": {"type": "string"},
        "title": {"type": "string"}
    }});
    // The canonical GET-by-id for albums.
    s["paths"]["/albums/{albumId}"] = json!({
        "get": {
            "operationId": "getAlbum",
            "parameters": [{"name": "albumId", "in": "path", "required": true, "schema": {"type": "string"}}],
            "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Album"}}}}}
        }
    });
    // A bare-array list whose items are inline: the shape the builder
    // names `ListSongsResponse` is `{type: array, items: {properties…}}`
    // with no top-level `properties` — the case ADR 0033's walk skipped
    // outright (21 of 92 Spotify operations; 24 of the 36 benchmark
    // relationship fields live on exactly these item types).
    s["paths"]["/songs"] = json!({
        "get": {
            "operationId": "listSongs",
            "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {
                "type": "array",
                "items": {"type": "object", "properties": {
                    "id": {"type": "string"},
                    "album_id": {"type": "string"}
                }}
            }}}}}
        }
    });
    let inv = build(&s);
    assert_eq!(
        inv["shapes"]["ListSongsResponse"]["items"]["properties"]["album_id"]
            ["candidate_entity_link"],
        json!({"operation": "get:/albums/{albumId}", "parameter": "albumId", "list_context": false})
    );
    // The item's own `id` is a generic name (Task 1) and Album.id does not
    // name-match anything: neither carries a fact.
    assert!(
        inv["shapes"]["ListSongsResponse"]["items"]["properties"]["id"]
            .get("candidate_entity_link")
            .is_none()
    );
    assert!(inv["shapes"]["Album"]["properties"]["id"]
        .get("candidate_entity_link")
        .is_none());
    assert_eq!(validate(&inv, &schema()), Vec::<String>::new());
}

#[test]
fn nested_lists_and_objects_are_walked_and_a_ref_cycle_terminates() {
    let mut s = spec();
    s["components"]["schemas"]["Song"] =
        json!({"type": "object", "properties": {"id": {"type": "string"}}});
    s["components"]["schemas"]["Account"] =
        json!({"type": "object", "properties": {"id": {"type": "string"}}});
    // Playlist embeds an inline list of inline objects (`songs[]>song_id`),
    // an inline object (`owner>account_id`), and a `$ref` into the
    // fixture's own Widget <-> User cycle (`creator`). The walk must reach
    // the first two, must not follow the third, and must come back.
    s["components"]["schemas"]["Playlist"] = json!({"type": "object", "properties": {
        "id": {"type": "string"},
        "songs": {"type": "array", "items": {"type": "object", "properties": {
            "position": {"type": "integer"},
            "song_id": {"type": "string"}
        }}},
        "owner": {"type": "object", "properties": {
            "account_id": {"type": "string"},
            "display_name": {"type": "string"}
        }},
        "creator": {"$ref": "#/components/schemas/User"}
    }});
    for (path, id, name, schema) in [
        ("/songs/{songId}", "songId", "getSong", "Song"),
        (
            "/accounts/{accountId}",
            "accountId",
            "getAccount",
            "Account",
        ),
        (
            "/playlists/{playlistId}",
            "playlistId",
            "getPlaylist",
            "Playlist",
        ),
    ] {
        s["paths"][path] = json!({
            "get": {
                "operationId": name,
                "parameters": [{"name": id, "in": "path", "required": true, "schema": {"type": "string"}}],
                "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {"$ref": format!("#/components/schemas/{}", schema)}}}}}
            }
        });
    }
    // Returning at all is the termination proof: Widget.owner -> User and
    // User.widget -> Widget are both reachable from Playlist.creator.
    let inv = build(&s);
    assert_eq!(
        inv["shapes"]["Playlist"]["properties"]["songs"]["items"]["properties"]["song_id"]
            ["candidate_entity_link"],
        json!({"operation": "get:/songs/{songId}", "parameter": "songId", "list_context": false})
    );
    assert_eq!(
        inv["shapes"]["Playlist"]["properties"]["owner"]["properties"]["account_id"]
            ["candidate_entity_link"],
        json!({"operation": "get:/accounts/{accountId}", "parameter": "accountId", "list_context": false})
    );
    // A `$ref` node is not descended and carries nothing itself; the named
    // shape it points at is walked only in its own right.
    assert_eq!(
        inv["shapes"]["Playlist"]["properties"]["creator"],
        json!({"$ref": "#/shapes/User"})
    );
    assert!(inv["shapes"]["User"]["properties"]["id"]
        .get("candidate_entity_link")
        .is_none());
    // Playlist's own id is never linked to its own GET-by-id (Task 1).
    assert!(inv["shapes"]["Playlist"]["properties"]["id"]
        .get("candidate_entity_link")
        .is_none());
    assert_eq!(validate(&inv, &schema()), Vec::<String>::new());
}

#[test]
fn the_relationship_walk_stops_at_depth_twelve() {
    // `l1 > l2 > … > l<levels> > account_id`, every level an inline object.
    // The walk calls `f` on `account_id` at depth `levels`, so 12 levels is
    // the last one it reaches and 13 is the first it does not.
    fn deep(levels: usize) -> Value {
        let mut node = json!({"type": "object", "properties": {"account_id": {"type": "string"}}});
        for k in (1..=levels).rev() {
            let mut props = serde_json::Map::new();
            props.insert(format!("l{}", k), node);
            node = json!({"type": "object", "properties": props});
        }
        node
    }
    let mut s = spec();
    s["components"]["schemas"]["Account"] =
        json!({"type": "object", "properties": {"id": {"type": "string"}}});
    s["components"]["schemas"]["Deep12"] = deep(12);
    s["components"]["schemas"]["Deep13"] = deep(13);
    for (path, id, name, schema) in [
        (
            "/accounts/{accountId}",
            "accountId",
            "getAccount",
            "Account",
        ),
        ("/deep12/{deep12Id}", "deep12Id", "getDeep12", "Deep12"),
        ("/deep13/{deep13Id}", "deep13Id", "getDeep13", "Deep13"),
    ] {
        s["paths"][path] = json!({
            "get": {
                "operationId": name,
                "parameters": [{"name": id, "in": "path", "required": true, "schema": {"type": "string"}}],
                "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {"$ref": format!("#/components/schemas/{}", schema)}}}}}
            }
        });
    }
    let inv = build(&s);
    let mut at = &inv["shapes"]["Deep12"];
    for k in 1..=12 {
        at = &at["properties"][format!("l{}", k)];
    }
    assert_eq!(
        at["properties"]["account_id"]["candidate_entity_link"],
        json!({"operation": "get:/accounts/{accountId}", "parameter": "accountId", "list_context": false}),
        "l1>…>l12>account_id sits at depth 12 and is walked"
    );
    let mut at = &inv["shapes"]["Deep13"];
    for k in 1..=13 {
        at = &at["properties"][format!("l{}", k)];
    }
    assert_eq!(at["properties"]["account_id"], json!({"type": "string"}));
    assert!(
        at["properties"]["account_id"]
            .get("candidate_entity_link")
            .is_none(),
        "l1>…>l13>account_id sits at depth 13 and is past LINK_WALK_DEPTH"
    );
    assert_eq!(validate(&inv, &schema()), Vec::<String>::new());
}

/// A canonical GET-by-id path item for `schema`, keyed by the caller.
fn by_id_path(id: &str, op: &str, schema: &str) -> Value {
    json!({
        "get": {
            "operationId": op,
            "parameters": [{"name": id, "in": "path", "required": true, "schema": {"type": "string"}}],
            "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {"$ref": format!("#/components/schemas/{}", schema)}}}}}
        }
    })
}

/// No operation's response is `name` itself.
fn returned_directly(inv: &Value, name: &str) -> bool {
    let target = format!("#/shapes/{}", name);
    inv["operations"]
        .as_array()
        .unwrap()
        .iter()
        .any(|o| o["response"]["shape_ref"] == target.as_str())
}

#[test]
fn a_component_reached_only_through_a_list_items_ref_carries_a_fact() {
    // ADR 0069 whole-branch review, finding B (R30): on a components-based
    // spec a list endpoint is `{type: array, items: {$ref: Item}}`, and an
    // item component no operation returns directly was never walked
    // (gitea: 57 such shapes, 5 facts). Pass 2 now walks every named shape
    // a response reaches through `$ref`, transitively: Track through the
    // list's `items.$ref`, Credit through Track's own `credits[].$ref`.
    let mut s = spec();
    // The by-id target's record carries its key (ADR 0085).
    s["components"]["schemas"]["Album"] = json!({"type": "object", "properties": {"id": {"type": "string"}, "title": {"type": "string"}}});
    // The by-id target's record carries its key (ADR 0085).
    s["components"]["schemas"]["Artist"] = json!({"type": "object", "properties": {"id": {"type": "string"}, "title": {"type": "string"}}});
    s["components"]["schemas"]["Track"] = json!({"type": "object", "properties": {
        "title": {"type": "string"},
        "album_id": {"type": "string"},
        "credits": {"type": "array", "items": {"$ref": "#/components/schemas/Credit"}}
    }});
    s["components"]["schemas"]["Credit"] = json!({"type": "object", "properties": {
        "role": {"type": "string"},
        "artist_id": {"type": "string"}
    }});
    s["paths"]["/albums/{albumId}"] = by_id_path("albumId", "getAlbum", "Album");
    s["paths"]["/artists/{artistId}"] = by_id_path("artistId", "getArtist", "Artist");
    s["paths"]["/tracks"] = json!({
        "get": {
            "operationId": "listTracks",
            "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {
                "type": "array", "items": {"$ref": "#/components/schemas/Track"}
            }}}}}
        }
    });
    let inv = build(&s);
    // The list's own shape is nothing but the `$ref`: no property to host a
    // fact, and the walk does not descend it.
    assert_eq!(
        inv["shapes"]["ListTracksResponse"],
        json!({"type": "array", "items": {"$ref": "#/shapes/Track"}})
    );
    assert!(!returned_directly(&inv, "Track"));
    assert!(!returned_directly(&inv, "Credit"));
    assert_eq!(
        inv["shapes"]["Track"]["properties"]["album_id"]["candidate_entity_link"],
        json!({"operation": "get:/albums/{albumId}", "parameter": "albumId", "list_context": false})
    );
    assert_eq!(
        inv["shapes"]["Credit"]["properties"]["artist_id"]["candidate_entity_link"],
        json!({"operation": "get:/artists/{artistId}", "parameter": "artistId", "list_context": false})
    );
    assert_eq!(validate(&inv, &schema()), Vec::<String>::new());
}

#[test]
fn a_ref_cycle_between_two_components_terminates_and_each_is_walked_once() {
    // Parent -> children[] -> Child -> parent -> Parent, and Child -> Child
    // through `sibling`: the closure pass 2 walks must come back, and must
    // list each name once.
    let mut s = spec();
    // The by-id target's record carries its key (ADR 0085).
    s["components"]["schemas"]["Owner"] = json!({"type": "object", "properties": {"id": {"type": "string"}, "title": {"type": "string"}}});
    // The by-id target's record carries its key (ADR 0085).
    s["components"]["schemas"]["Toy"] = json!({"type": "object", "properties": {"id": {"type": "string"}, "title": {"type": "string"}}});
    s["components"]["schemas"]["Parent"] = json!({"type": "object", "properties": {
        "owner_id": {"type": "string"},
        "children": {"type": "array", "items": {"$ref": "#/components/schemas/Child"}}
    }});
    s["components"]["schemas"]["Child"] = json!({"type": "object", "properties": {
        "toy_id": {"type": "string"},
        "parent": {"$ref": "#/components/schemas/Parent"},
        "sibling": {"$ref": "#/components/schemas/Child"}
    }});
    s["paths"]["/owners/{ownerId}"] = by_id_path("ownerId", "getOwner", "Owner");
    s["paths"]["/toys/{toyId}"] = by_id_path("toyId", "getToy", "Toy");
    s["paths"]["/parents"] = json!({
        "get": {
            "operationId": "listParents",
            "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {
                "type": "array", "items": {"$ref": "#/components/schemas/Parent"}
            }}}}}
        }
    });
    // Returning at all is the termination proof.
    let inv = build(&s);
    assert!(!returned_directly(&inv, "Parent"));
    assert!(!returned_directly(&inv, "Child"));
    assert_eq!(
        inv["shapes"]["Parent"]["properties"]["owner_id"]["candidate_entity_link"],
        json!({"operation": "get:/owners/{ownerId}", "parameter": "ownerId", "list_context": false})
    );
    assert_eq!(
        inv["shapes"]["Child"]["properties"]["toy_id"]["candidate_entity_link"],
        json!({"operation": "get:/toys/{toyId}", "parameter": "toyId", "list_context": false})
    );
    // The `$ref` nodes themselves carry nothing.
    assert_eq!(
        inv["shapes"]["Child"]["properties"]["parent"],
        json!({"$ref": "#/shapes/Parent"})
    );
    assert_eq!(
        inv["shapes"]["Child"]["properties"]["sibling"],
        json!({"$ref": "#/shapes/Child"})
    );
    // The closure pass 2 seeds from (`inventory::shape_closure`) names each
    // shape of the cycle once.
    let closure = graphos_factory_core::inventory::shape_closure(
        ["ListParentsResponse".to_string()],
        inv["shapes"].as_object().unwrap(),
    );
    assert_eq!(closure, vec!["ListParentsResponse", "Parent", "Child"]);
    assert_eq!(validate(&inv, &schema()), Vec::<String>::new());
}

#[test]
fn a_component_referenced_only_from_a_request_body_is_not_walked() {
    // Request shapes are not hosts: the fact describes what a response
    // carries. Draft is reached only from post:/drafts' request body; the
    // same property on a response-reached shape (Track, through the list's
    // `items.$ref`) is the control that proves the name would match.
    let mut s = spec();
    // The by-id target's record carries its key (ADR 0085).
    s["components"]["schemas"]["Album"] = json!({"type": "object", "properties": {"id": {"type": "string"}, "title": {"type": "string"}}});
    s["components"]["schemas"]["Draft"] = json!({"type": "object", "properties": {
        "title": {"type": "string"},
        "album_id": {"type": "string"}
    }});
    s["components"]["schemas"]["Track"] = json!({"type": "object", "properties": {
        "title": {"type": "string"},
        "album_id": {"type": "string"}
    }});
    s["paths"]["/albums/{albumId}"] = by_id_path("albumId", "getAlbum", "Album");
    s["paths"]["/drafts"] = json!({
        "post": {
            "operationId": "createDraft",
            "requestBody": {"required": true, "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Draft"}}}},
            "responses": {"204": {"description": "saved"}}
        }
    });
    s["paths"]["/tracks"] = json!({
        "get": {
            "operationId": "listTracks",
            "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {
                "type": "array", "items": {"$ref": "#/components/schemas/Track"}
            }}}}}
        }
    });
    let inv = build(&s);
    assert_eq!(
        op(&inv, "post:/drafts")["request_body"]["shape_ref"],
        "#/shapes/Draft"
    );
    assert_eq!(
        inv["shapes"]["Draft"]["properties"]["album_id"]["type"],
        "string"
    );
    assert!(inv["shapes"]["Draft"]["properties"]["album_id"]
        .get("candidate_entity_link")
        .is_none());
    assert_eq!(
        inv["shapes"]["Track"]["properties"]["album_id"]["candidate_entity_link"],
        json!({"operation": "get:/albums/{albumId}", "parameter": "albumId", "list_context": false})
    );
    assert_eq!(validate(&inv, &schema()), Vec::<String>::new());
}

// ─── x-expansion survives conversion (ADR 0047) ─────────────────────────────

fn expansion_spec() -> Value {
    let verified = json!({"target": "Node", "mechanism": "fields", "default": ["id", "name"],
        "evidence": {"kind": "doc", "ref": "https://developers.example.test/graph/v21/node"}});
    let unverified = json!({"target": "Label", "mechanism": "fields", "default": "unverified"});
    json!({
        "openapi": "3.0.3",
        "info": {"title": "Graph", "version": "21.0"},
        "servers": [{"url": "https://graph.example.test/v21.0"}],
        "components": {"schemas": {
            "Node": {"type": "object", "properties": {"id": {"type": "string"}, "name": {"type": "string"}, "kind": {"type": "string"}}},
            "Label": {"type": "object", "properties": {"id": {"type": "string"}, "text": {"type": "string"}}},
            "Account": {"type": "object", "properties": {
                "id": {"type": "string"},
                "owner": {"$ref": "#/components/schemas/Node", "x-expansion": verified},
                "labels": {"type": "array", "items": {"$ref": "#/components/schemas/Label", "x-expansion": unverified}},
                "parent": {"$ref": "#/components/schemas/Node", "nullable": true, "x-expansion": verified},
                "wrapped": {"allOf": [{"$ref": "#/components/schemas/Node"}], "nullable": true, "x-expansion": verified}
            }}
        }},
        "paths": {"/{account_id}": {"get": {
            "operationId": "getAccount",
            "parameters": [{"name": "account_id", "in": "path", "required": true, "schema": {"type": "string"}}],
            "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Account"}}}}}
        }}}
    })
}

/// Salesforce SOQL: the list takes no paging parameter and returns
/// `nextRecordsUrl`, a path the client follows to a separate operation,
/// `GET …/query/{queryLocator}` (ADR 0052).
fn soql_spec(extra_paths: Value) -> Value {
    let result = json!({"type": "object", "properties": {
        "totalSize": {"type": "integer"},
        "done": {"type": "boolean"},
        "nextRecordsUrl": {"type": "string", "nullable": true},
        "records": {"type": "array", "items": {"type": "object"}}
    }});
    let mut paths = json!({
        "/services/data/v67.0/query?q=SELECT+Id+FROM+Account": {"get": {
            "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": result.clone()}}}}
        }},
        "/services/data/v67.0/query/{queryLocator}": {"get": {
            "parameters": [{"name": "queryLocator", "in": "path", "required": true, "schema": {"type": "string"}}],
            "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": result}}}}
        }},
        "/services/data/v67.0/sobjects/Account/{id}": {"get": {
            "parameters": [{"name": "id", "in": "path", "required": true, "schema": {"type": "string"}}],
            "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {"type": "object", "properties": {"Id": {"type": "string"}}}}}}}
        }}
    });
    for (k, v) in extra_paths.as_object().unwrap() {
        paths[k] = v.clone();
    }
    json!({
        "openapi": "3.0.3", "info": {"title": "Salesforce", "version": "67.0"},
        "servers": [{"url": "https://example.my.salesforce.com"}],
        "paths": paths
    })
}

#[test]
fn a_response_next_link_with_no_paging_parameter_is_next_link_paging() {
    let inv = build(&soql_spec(json!({})));
    let list = op(
        &inv,
        "get:/services/data/v67.0/query?q=SELECT+Id+FROM+Account",
    );
    assert_eq!(
        list["pagination"],
        json!({"style": "next_link", "request": null, "size_param": null, "response": null,
               "next_url": "nextRecordsUrl",
               "next_operation": "get:/services/data/v67.0/query/{queryLocator}"})
    );
    // The follow-up returns the same link and pages through itself; only a
    // separate operation at `{path}/{locator}` is recorded, so it gets none.
    let next = op(&inv, "get:/services/data/v67.0/query/{queryLocator}");
    assert_eq!(next["pagination"]["style"], "next_link");
    assert!(next["pagination"].get("next_operation").is_none());
    // A fetch by id carries no link and stays unpaginated.
    assert!(op(&inv, "get:/services/data/v67.0/sobjects/Account/{id}")
        .get("pagination")
        .is_none());
    assert_eq!(
        inv["api"]["pagination"],
        json!({"style": "next_link", "request": null, "size_param": null, "response": null,
               "next_url": "nextRecordsUrl", "counts": {"next_link": 2}})
    );
    assert_eq!(validate(&inv, &schema()), Vec::<String>::new());
}

/// Two GETs could fetch the linked page, both returning the same link: the
/// fact is the link alone.
#[test]
fn an_ambiguous_next_link_follow_up_is_not_recorded() {
    let page = json!({"type": "object", "properties": {"nextRecordsUrl": {"type": "string"}}});
    let inv = build(&soql_spec(json!({
        "/services/data/v67.0/query/{nextToken}": {"get": {
            "parameters": [{"name": "nextToken", "in": "path", "required": true, "schema": {"type": "string"}}],
            "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": page}}}}
        }}
    })));
    let list = op(
        &inv,
        "get:/services/data/v67.0/query?q=SELECT+Id+FROM+Account",
    );
    assert_eq!(list["pagination"]["style"], "next_link");
    assert!(list["pagination"].get("next_operation").is_none());
}

/// A cursor-paged list that also returns a next link keeps ADR 0038's
/// cursor reading: the next-link rule only fills in where no style is found.
#[test]
fn a_cursor_list_with_a_next_link_stays_cursor_paging() {
    let spec = json!({
        "openapi": "3.0.3", "info": {"title": "Items", "version": "1"},
        "servers": [{"url": "https://api.items.test"}],
        "paths": {"/items": {"get": {
            "parameters": [{"name": "cursor", "in": "query", "schema": {"type": "string"}}],
            "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {
                "type": "object", "properties": {"next_cursor": {"type": "string"}, "@odata.nextLink": {"type": "string"}}
            }}}}}
        }}}
    });
    assert_eq!(
        build(&spec)["operations"][0]["pagination"],
        json!({"style": "cursor", "request": "cursor", "size_param": null, "response": "next_cursor"})
    );
}

/// A list at `path` with `@odata.nextLink` (Microsoft Graph's
/// `BaseCollectionPaginationCountResponse`, reached through `allOf`) and one
/// child GET at `path/{param}` whose response is `child`.
fn graph_spec(path: &str, param: &str, child: Value) -> Value {
    let child_path = format!("{}/{{{}}}", path, param);
    json!({
        "openapi": "3.0.4", "info": {"title": "Graph", "version": "1"},
        "servers": [{"url": "https://graph.example.com/v1.0"}],
        "paths": {
            path: {"get": {
                "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/collection"}}}}}
            }},
            child_path: {"get": {
                "parameters": [{"name": param, "in": "path", "required": true, "schema": {"type": "string"}}],
                "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": child}}}}
            }}
        },
        "components": {"schemas": {
            "BaseCollectionPaginationCountResponse": {"type": "object", "properties": {
                "@odata.count": {"type": "integer", "nullable": true},
                "@odata.nextLink": {"type": "string", "nullable": true}
            }},
            "collection": {"allOf": [
                {"$ref": "#/components/schemas/BaseCollectionPaginationCountResponse"},
                {"type": "object", "properties": {"value": {"type": "array", "items": {"type": "object"}}}}
            ]}
        }}
    })
}

/// A Graph key parameter such as `{sitePage-id}` contains `page` but ends in
/// `id`: it names a resource, not a locator. Only the parameter's last word
/// counts, so even a child that returns the same link is not the follow-up.
#[test]
fn a_key_parameter_containing_page_is_not_a_next_link_follow_up() {
    let inv = build(&graph_spec(
        "/sites/{site-id}/pages",
        "sitePage-id",
        json!({"$ref": "#/components/schemas/collection"}),
    ));
    let list = op(&inv, "get:/sites/{site-id}/pages");
    assert_eq!(list["pagination"]["style"], "next_link");
    assert_eq!(list["pagination"]["next_url"], "@odata.nextLink");
    assert!(list["pagination"].get("next_operation").is_none());
    assert_eq!(validate(&inv, &schema()), Vec::<String>::new());
}

/// A child whose parameter is named like a locator but whose response
/// carries no next link fetches one item, not the next page.
#[test]
fn a_next_link_follow_up_must_return_the_same_link() {
    let item = json!({"type": "object", "properties": {"id": {"type": "string"}}});
    let inv = build(&graph_spec("/users/{user-id}/tokens", "token", item));
    let list = op(&inv, "get:/users/{user-id}/tokens");
    assert_eq!(list["pagination"]["style"], "next_link");
    assert!(list["pagination"].get("next_operation").is_none());
}

/// The link is found with the cursor's own string test: an OpenAPI 3.1
/// nullable union and a `$ref` to a string schema are strings too.
#[test]
fn a_nullable_union_or_referenced_next_link_is_next_link_paging() {
    let spec = json!({
        "openapi": "3.1.0", "info": {"title": "Links", "version": "1"},
        "servers": [{"url": "https://api.links.test"}],
        "paths": {
            "/unions": {"get": {"responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {
                "type": "object", "properties": {"@odata.nextLink": {"anyOf": [{"type": "string"}, {"type": "null"}]}}
            }}}}}}},
            "/refs": {"get": {"responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {
                "type": "object", "properties": {"nextRecordsUrl": {"$ref": "#/components/schemas/Link"}}
            }}}}}}}
        },
        "components": {"schemas": {"Link": {"type": "string"}}}
    });
    let inv = build(&spec);
    for key in ["get:/unions", "get:/refs"] {
        assert_eq!(op(&inv, key)["pagination"]["style"], "next_link", "{}", key);
    }
    assert_eq!(validate(&inv, &schema()), Vec::<String>::new());
}

/// A size parameter alone is `unknown`; with a next link the list is
/// `next_link`, and the size parameter is kept.
#[test]
fn a_next_link_list_keeps_its_size_parameter() {
    let spec = json!({
        "openapi": "3.0.3", "info": {"title": "Items", "version": "1"},
        "servers": [{"url": "https://api.items.test"}],
        "paths": {"/items": {"get": {
            "parameters": [{"name": "limit", "in": "query", "schema": {"type": "integer"}}],
            "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {
                "type": "object", "properties": {"nextRecordsUrl": {"type": "string"}}
            }}}}}
        }}}
    });
    assert_eq!(
        build(&spec)["operations"][0]["pagination"],
        json!({"style": "next_link", "request": null, "size_param": "limit", "response": null,
               "next_url": "nextRecordsUrl"})
    );
}

/// `next_link` applies to any method, so a POST at `{path}/{cursor}` that
/// returns the same link is itself `next_link`; the follow-up is a GET, so
/// the POST is not recorded as the list's `next_operation`.
#[test]
fn a_post_returning_the_same_link_is_not_a_next_link_follow_up() {
    let page = json!({"type": "object", "properties": {"nextRecordsUrl": {"type": "string"}}});
    let ok =
        json!({"200": {"description": "ok", "content": {"application/json": {"schema": page}}}});
    let spec = json!({
        "openapi": "3.0.3", "info": {"title": "Items", "version": "1"},
        "servers": [{"url": "https://api.items.test"}],
        "paths": {
            "/items": {"get": {"responses": ok.clone()}},
            "/items/{cursor}": {"post": {
                "parameters": [{"name": "cursor", "in": "path", "required": true, "schema": {"type": "string"}}],
                "responses": ok
            }}
        }
    });
    let inv = build(&spec);
    assert_eq!(
        op(&inv, "post:/items/{cursor}")["pagination"]["style"],
        "next_link"
    );
    let list = op(&inv, "get:/items");
    assert_eq!(list["pagination"]["style"], "next_link");
    assert!(list["pagination"].get("next_operation").is_none());
}

/// The link is found inside a meta/pagination object too, the places the
/// cursor search looks, and its dotted path is recorded.
#[test]
fn a_next_link_inside_a_meta_object_records_its_dotted_path() {
    let spec = json!({
        "openapi": "3.0.3", "info": {"title": "Items", "version": "1"},
        "servers": [{"url": "https://api.items.test"}],
        "paths": {"/items": {"get": {
            "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {
                "type": "object", "properties": {
                    "records": {"type": "array", "items": {"type": "object"}},
                    "meta": {"type": "object", "properties": {"nextRecordsUrl": {"type": "string"}}}
                }
            }}}}}
        }}}
    });
    assert_eq!(
        build(&spec)["operations"][0]["pagination"],
        json!({"style": "next_link", "request": null, "size_param": null, "response": null,
               "next_url": "meta.nextRecordsUrl"})
    );
}

#[test]
fn x_expansion_is_kept_beside_a_ref_on_a_property_an_array_item_and_a_nullable_ref() {
    let inv = build(&expansion_spec());
    let props = &inv["shapes"]["Account"]["properties"];
    let verified = json!({"target": "Node", "mechanism": "fields", "default": ["id", "name"],
        "evidence": {"kind": "doc", "ref": "https://developers.example.test/graph/v21/node"}});
    // The reference is kept, and the annotation sits beside it.
    assert_eq!(props["owner"]["$ref"], "#/shapes/Node");
    assert_eq!(props["owner"]["x-expansion"], verified);
    assert_eq!(props["labels"]["items"]["$ref"], "#/shapes/Label");
    assert_eq!(
        props["labels"]["items"]["x-expansion"]["default"],
        "unverified"
    );
    assert_eq!(props["parent"]["$ref"], "#/shapes/Node");
    assert_eq!(props["parent"]["nullable"], true);
    assert_eq!(props["parent"]["x-expansion"], verified);
    // An allOf-wrapped reference is flattened inline, as before; the
    // annotation is copied onto the flattened value.
    assert_eq!(props["wrapped"]["x-expansion"], verified);
    // The targets stay reachable as named shapes, children intact.
    assert!(inv["shapes"]["Node"]["properties"]["kind"].is_object());
    assert!(inv["shapes"]["Label"]["properties"]["text"].is_object());
    assert_eq!(validate(&inv, &schema()), Vec::<String>::new());
}

/// The Granola dry run (2026-09-29): `AddLegalHoldCustodiansBody.custodians`
/// is `minItems: 1, maxItems: 100`, and the inventory dropped both, so an
/// agent reading the inventory (never the raw spec) could not state the
/// bound on the argument.
#[test]
fn array_item_bounds_are_kept_on_a_body_property() {
    let mut s = spec();
    s["paths"]["/widgets"]["post"]["requestBody"]["content"]["application/json"]["schema"] = json!({"type": "object", "properties": {
        "custodians": {"type": "array", "minItems": 1, "maxItems": 100, "items": {"type": "string"}},
        "tags": {"type": "array", "items": {"type": "string"}}
    }});
    let inv = build(&s);
    let props = &inv["shapes"]["CreateWidgetRequest"]["properties"];
    assert_eq!(props["custodians"]["minItems"], 1);
    assert_eq!(props["custodians"]["maxItems"], 100);
    assert!(props["tags"].get("minItems").is_none(), "{}", props["tags"]);
    assert!(props["tags"].get("maxItems").is_none(), "{}", props["tags"]);
    assert_eq!(validate(&inv, &schema()), Vec::<String>::new());
}

// ── Exclusive bounds (ADR 0065) ─────────────────────────────────────────────

fn limit_param(inv: &Value) -> &Value {
    op(inv, "get:/widgets")["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "limit")
        .expect("limit parameter")
}

/// OpenAPI 3.1 spells an exclusive bound as the number itself. It reaches
/// the parameter record and the shape verbatim, beside `minimum`/`maximum`.
#[test]
fn numeric_exclusive_bounds_reach_the_parameter_and_the_shape() {
    let mut s = spec();
    s["openapi"] = json!("3.1.0");
    s["paths"]["/widgets"]["get"]["parameters"] = json!([
        {"name": "limit", "in": "query",
         "schema": {"type": "integer", "exclusiveMinimum": 0, "exclusiveMaximum": 9223372036854775807_i64}}
    ]);
    s["components"]["schemas"]["Widget"]["properties"]["card_number"] = json!({"type": "integer", "exclusiveMinimum": 999999, "exclusiveMaximum": 10000000000000000000_u64});
    let inv = build(&s);
    let limit = limit_param(&inv);
    assert_eq!(limit["exclusiveMinimum"], 0);
    assert_eq!(limit["exclusiveMaximum"], json!(9223372036854775807_i64));
    assert!(limit.get("maximum").is_none(), "no maximum is invented");
    let card = &inv["shapes"]["Widget"]["properties"]["card_number"];
    assert_eq!(card["exclusiveMinimum"], 999999);
    assert_eq!(card["exclusiveMaximum"], json!(10000000000000000000_u64));
    assert_eq!(validate(&inv, &schema()), Vec::<String>::new());
}

/// OpenAPI 3.0 spells it as a boolean that qualifies `minimum`/`maximum`.
/// The inventory copies the boolean as written; it does not fold it into
/// the number (ADR 0065).
#[test]
fn boolean_exclusive_bounds_are_recorded_as_written() {
    let mut s = spec();
    s["paths"]["/widgets"]["get"]["parameters"] = json!([
        {"name": "limit", "in": "query",
         "schema": {"type": "integer", "minimum": 0, "exclusiveMinimum": true, "maximum": 100, "exclusiveMaximum": false}}
    ]);
    s["components"]["schemas"]["Widget"]["properties"]["score"] =
        json!({"type": "number", "maximum": 1, "exclusiveMaximum": true});
    let inv = build(&s);
    let limit = limit_param(&inv);
    assert_eq!(limit["minimum"], 0);
    assert_eq!(limit["exclusiveMinimum"], true);
    assert_eq!(limit["maximum"], 100);
    assert_eq!(limit["exclusiveMaximum"], false);
    let score = &inv["shapes"]["Widget"]["properties"]["score"];
    assert_eq!(score["maximum"], 1);
    assert_eq!(score["exclusiveMaximum"], true);
    assert_eq!(validate(&inv, &schema()), Vec::<String>::new());
}

/// A malformed exclusive bound (neither number nor boolean) is dropped from
/// the parameter record, as a non-numeric `minimum` is.
#[test]
fn a_malformed_exclusive_bound_is_not_recorded_on_the_parameter() {
    let mut s = spec();
    s["paths"]["/widgets"]["get"]["parameters"] = json!([
        {"name": "limit", "in": "query",
         "schema": {"type": "integer", "exclusiveMaximum": "100", "exclusiveMinimum": null}}
    ]);
    let inv = build(&s);
    let limit = limit_param(&inv);
    assert!(limit.get("exclusiveMaximum").is_none(), "{}", limit);
    assert!(limit.get("exclusiveMinimum").is_none(), "{}", limit);
    assert_eq!(validate(&inv, &schema()), Vec::<String>::new());
}

// ── ADR 0080: the success-shape picker (item 6) ─────────────────────────
// property-name matching alone is never sufficient evidence to write the
// `referenced_shape` fact; only a status-code correlation or a shared
// boolean discriminator does.

fn success_error_spec(discriminator_property: &str) -> Value {
    let mut s = spec();
    s["components"]["schemas"]["KeySuccess"] = json!({
        "type": "object",
        "required": [discriminator_property, "results"],
        "properties": {
            "results": {"type": "object", "properties": {"title": {"type": "string"}}}
        }
    });
    s["components"]["schemas"]["KeySuccess"]["properties"][discriminator_property] =
        json!({"type": "boolean", "const": true});
    s["components"]["schemas"]["KeyError"] = json!({
        "type": "object",
        "required": [discriminator_property, "errors"],
        "properties": {
            "errors": {"type": "array", "items": {"type": "string"}}
        }
    });
    s["components"]["schemas"]["KeyError"]["properties"][discriminator_property] =
        json!({"type": "boolean", "const": false});
    s["paths"]["/apiKey.info"] = json!({
        "post": {
            "operationId": "apiKeyInfo",
            "responses": {
                "200": {"description": "ok", "content": {"application/json": {"schema": {
                    "oneOf": [
                        {"$ref": "#/components/schemas/KeySuccess"},
                        {"$ref": "#/components/schemas/KeyError"}
                    ]
                }}}}
            }
        }
    });
    s
}

#[test]
fn an_unambiguous_boolean_discriminated_union_records_a_referenced_shape_fact() {
    let inv = build(&success_error_spec("success"));
    let response = &op(&inv, "post:/apiKey.info")["response"];
    assert_eq!(response["referenced_shape"], "#/shapes/KeySuccess");
    // Envelope facts see through to the success branch's own two
    // properties (`success`, `results`), not the union wrapper, which has
    // none at its own top level -- root_property_count would be absent
    // entirely without the fix (the oneOf/anyOf fallback further down).
    assert_eq!(response["root_property_count"], 2);
    assert_eq!(validate(&inv, &schema()), Vec::<String>::new());
}

#[test]
fn the_discriminator_propertys_name_never_decides_which_branch_wins() {
    // Same shapes, differently-named discriminator: the resolved branch
    // does not depend on the property being called `success`.
    let inv = build(&success_error_spec("ok"));
    let response = &op(&inv, "post:/apiKey.info")["response"];
    assert_eq!(response["referenced_shape"], "#/shapes/KeySuccess");
}

#[test]
fn a_branch_matching_a_documented_non_2xx_response_is_status_correlated() {
    let mut s = spec();
    s["components"]["schemas"]["Problem"] =
        json!({"type": "object", "properties": {"detail": {"type": "string"}}});
    s["paths"]["/widgets/{widgetId}/report"] = json!({
        "get": {
            "operationId": "getWidgetReport",
            "parameters": [{"name": "widgetId", "in": "path", "required": true, "schema": {"type": "string"}}],
            "responses": {
                "200": {"description": "ok", "content": {"application/json": {"schema": {
                    "oneOf": [
                        {"$ref": "#/components/schemas/Widget"},
                        {"$ref": "#/components/schemas/Problem"}
                    ]
                }}}},
                "404": {"description": "missing", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Problem"}}}}
            }
        }
    });
    let inv = build(&s);
    let response = &op(&inv, "get:/widgets/{widgetId}/report")["response"];
    assert_eq!(response["referenced_shape"], "#/shapes/Widget");
    // Widget has 3 properties (id, name, owner); the union wrapper has none.
    assert_eq!(response["root_property_count"], 3);
}

#[test]
fn a_legitimate_success_variant_with_an_errors_field_writes_no_referenced_shape_fact() {
    // The exact false positive the retired specialist-pipeline heuristic
    // produced: a genuinely successful (2xx-only) bulk response reporting
    // partial per-item failures via an `errors` field, alongside a real
    // error variant -- no status-code or discriminator evidence separates
    // them. `inventory build` must leave this union alone.
    let mut s = spec();
    s["components"]["schemas"]["BulkResult"] = json!({
        "type": "object",
        "required": ["created", "errors"],
        "properties": {
            "created": {"type": "array", "items": {"type": "string"}},
            "errors": {"type": "array", "items": {"type": "object"}}
        }
    });
    s["components"]["schemas"]["ErrorEnvelope"] = json!({
        "type": "object",
        "required": ["message"],
        "properties": {"message": {"type": "string"}}
    });
    s["paths"]["/widgets/bulk"] = json!({
        "post": {
            "operationId": "bulkCreateWidgets",
            "responses": {
                "200": {"description": "ok", "content": {"application/json": {"schema": {
                    "oneOf": [
                        {"$ref": "#/components/schemas/BulkResult"},
                        {"$ref": "#/components/schemas/ErrorEnvelope"}
                    ]
                }}}}
            }
        }
    });
    let inv = build(&s);
    let response = &op(&inv, "post:/widgets/bulk")["response"];
    assert!(response.get("referenced_shape").is_none(), "{}", response);
    // Unresolved: envelope facts are empty, exactly as for any other
    // unclassified oneOf/anyOf response, matching today's behavior.
    assert!(response.get("root_property_count").is_none());
    assert!(response.get("sole_root_property").is_none());
    assert_eq!(validate(&inv, &schema()), Vec::<String>::new());
}
