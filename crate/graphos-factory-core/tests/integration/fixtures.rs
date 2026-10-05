use graphos_factory_core::cmd::fixtures::{
    assert_absent_siblings, mapping_from_sample, MappingOptions,
};
use serde_json::{json, Value};

fn sample() -> Value {
    json!({
        "operation": "get:/deck/new/shuffle/",
        "recorded_at": "2026-09-08T20:40:07Z",
        "request": {"method": "GET", "url": "https://deckofcardsapi.com/api/deck/new/shuffle/?cards=AS,2S,KS,AD", "headers": {"Accept": "application/json"}},
        "response": {"status": 200, "content_type": "application/json", "json": true, "body": {"success": true, "deck_id": "x", "remaining": 4, "shuffled": true}}
    })
}

fn map(s: &Value, base: &str, cases: Option<Vec<&str>>, note: Option<&str>) -> Value {
    mapping_from_sample(
        s,
        "get_deck_new_shuffle",
        "2",
        &MappingOptions {
            case_names: cases.map(|c| c.iter().map(|x| x.to_string()).collect()),
            note,
            base_path: base,
        },
    )
    .unwrap()
}

#[test]
fn the_base_url_path_is_stripped() {
    assert_eq!(
        map(&sample(), "/api", None, None)["request"]["urlPath"],
        "/deck/new/shuffle/"
    );
    assert_eq!(
        map(&sample(), "", None, None)["request"]["urlPath"],
        "/api/deck/new/shuffle/"
    );
}

#[test]
fn query_values_are_matched_decoded_and_repeated_parameters_use_has_exactly() {
    assert_eq!(
        map(&sample(), "/api", None, None)["request"]["queryParameters"],
        json!({"cards": {"equalTo": "AS,2S,KS,AD"}})
    );
    let mut s = sample();
    s["request"] =
        json!({"method": "GET", "url": "https://h/api/i?statuses[]=a&statuses[]=b", "headers": {}});
    assert_eq!(
        map(&s, "/api", None, None)["request"]["queryParameters"],
        json!({"statuses[]": {"hasExactly": [{"equalTo": "a"}, {"equalTo": "b"}]}})
    );
}

#[test]
fn the_response_carries_the_recorded_status_and_body_with_provenance() {
    let m = map(&sample(), "/api", None, None);
    assert_eq!(m["response"]["status"], 200);
    assert_eq!(
        m["response"]["jsonBody"],
        json!({"success": true, "deck_id": "x", "remaining": 4, "shuffled": true})
    );
    assert_eq!(m["response"]["headers"]["Content-Type"], "application/json");
    assert_eq!(
        m["metadata"]["x-recorded-from"],
        ".factory/samples/get_deck_new_shuffle/2.json"
    );
    assert!(m["metadata"].get("x-cases").is_none());
}

#[test]
fn a_non_json_body_is_kept_as_a_raw_body() {
    let mut s = sample();
    s["response"] = json!({"status": 500, "content_type": "text/html", "json": false, "body": "<html>boom</html>"});
    let m = map(&s, "/api", None, None);
    assert_eq!(m["response"]["status"], 500);
    assert_eq!(m["response"]["body"], "<html>boom</html>");
    assert!(m["response"].get("jsonBody").is_none());
}

#[test]
fn an_unreachable_recording_is_classified_with_empty_x_cases_and_a_note() {
    let m = map(
        &sample(),
        "/api",
        Some(vec![]),
        Some("no case can send this"),
    );
    assert_eq!(m["metadata"]["x-cases"], json!([]));
    assert_eq!(m["metadata"]["x-note"], "no case can send this");
}

#[test]
fn a_mapping_with_no_query_params_gets_absent_asserted_for_a_siblings_key() {
    // deck-of-cards' own real bug: a recording that sent none of an
    // endpoint's optional query parameters derives no `queryParameters` at
    // all, so its stub matches every sibling's request too once all stubs
    // load together.
    let mut plain = json!({"request": {"method": "GET", "urlPath": "/deck/new/"}});
    let mut jokers = json!({"request": {"method": "GET", "urlPath": "/deck/new/", "queryParameters": {"jokers_enabled": {"equalTo": "true"}}}});
    assert_absent_siblings(&mut [&mut plain, &mut jokers]);
    assert_eq!(
        plain["request"]["queryParameters"],
        json!({"jokers_enabled": {"absent": true}})
    );
    // The sibling that already asserts the key is untouched.
    assert_eq!(
        jokers["request"]["queryParameters"],
        json!({"jokers_enabled": {"equalTo": "true"}})
    );
}

#[test]
fn siblings_at_a_different_url_path_do_not_interfere() {
    let mut plain = json!({"request": {"method": "GET", "urlPath": "/deck/new/"}});
    let mut other = json!({"request": {"method": "GET", "urlPath": "/deck/new/shuffle/", "queryParameters": {"cards": {"equalTo": "AS,2S"}}}});
    assert_absent_siblings(&mut [&mut plain, &mut other]);
    assert!(plain["request"].get("queryParameters").is_none());
}

#[test]
fn a_credential_header_recorded_as_an_env_placeholder_matches_on_presence() {
    let mut s = sample();
    s["request"] = json!({"method": "GET", "url": "https://h/api/x", "headers": {"Authorization": "Bearer {$env.TOKEN}", "Accept": "application/json"}});
    assert_eq!(
        map(&s, "/api", None, None)["request"]["headers"],
        json!({"Authorization": {"matches": ".+"}})
    );
}
