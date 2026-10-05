use graphos_factory_core::scrub::scrub;
use regex::Regex;
use serde_json::json;

#[test]
fn credential_named_keys_are_redacted_wholesale() {
    let r = scrub(
        &json!({"Authorization": "Token token=abc", "api_key": "short", "nested": {"password": "hunter2"}}),
        &[],
        "$",
    );
    assert_eq!(r.value["Authorization"], "<redacted:authorization>");
    assert_eq!(r.value["api_key"], "<redacted:api_key>");
    assert_eq!(r.value["nested"]["password"], "<redacted:password>");
    assert_eq!(r.replacements["key:authorization"], 1);
}

#[test]
fn token_shapes_are_replaced_inside_strings() {
    let r = scrub(
        &json!({
            "a": "Bearer abcdefghijklmnopqrstuvwxyz0123456789",
            "b": "Basic dXNlcjpwYXNzd29yZDEyMzQ1Njc4OTA=",
            "c": "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U",
            "d": "sk_test_4eC39HqLyjWDarjtT1zdp7dc",
            "e": "AKIAIOSFODNN7EXAMPLE",
            "f": "ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789",
            "g": "Token token=y_NbAkD1abcDEF2ghiJKL3"
        }),
        &[],
        "$",
    );
    assert_eq!(r.value["a"], "Bearer <redacted>");
    assert_eq!(r.value["b"], "Basic <redacted>");
    assert_eq!(r.value["c"], "<redacted:jwt>");
    assert_eq!(r.value["d"], "<redacted:stripe-key>");
    assert_eq!(r.value["e"], "<redacted:aws-key>");
    assert_eq!(r.value["f"], "<redacted:github-token>");
    assert_eq!(r.value["g"], "Token token=<redacted>");
}

#[test]
fn emails_become_deterministic_placeholders_and_example_domains_are_left_alone() {
    let r = scrub(
        &json!({"a": "ana@acme.io", "b": "ana@acme.io", "c": "bob@acme.io", "d": "test@example.com"}),
        &[],
        "$",
    );
    assert_eq!(r.value["a"], "person1@example.test");
    assert_eq!(r.value["b"], "person1@example.test");
    assert_eq!(r.value["c"], "person2@example.test");
    assert_eq!(r.value["d"], "test@example.com");
}

#[test]
fn a_long_high_entropy_token_that_matched_nothing_is_residual() {
    let r = scrub(
        &json!({"id": "PINC001", "token_like": "Zq8xK2mN4pL7vB9cD1fG3hJ5kR6tW0yA2sE4uI"}),
        &[],
        "$",
    );
    assert!(r.has_residual());
    assert_eq!(r.residual[0].path, "$.token_like");
}

#[test]
fn ordinary_payload_values_are_not_residual() {
    let r = scrub(
        &json!({"deck_id": "g501sytlspvc", "url": "https://deckofcardsapi.com/static/img/AS.png", "at": "2026-09-08T09:12:44Z", "suit": "HEARTS", "sha": "3f1c"}),
        &[],
        "$",
    );
    assert!(!r.has_residual(), "{:?}", r.residual);
    assert!(r.replacements.is_empty());
}

#[test]
fn arrays_and_nesting_are_walked_and_non_strings_pass_through() {
    let r = scrub(
        &json!({"list": [{"secret": "x"}, 42, null, "plain"], "flag": true}),
        &[],
        "$",
    );
    assert_eq!(
        r.value,
        json!({"list": [{"secret": "<redacted:secret>"}, 42, null, "plain"], "flag": true})
    );
}

#[test]
fn extra_patterns_are_honoured() {
    let r = scrub(
        &json!({"note": "customer ACME-00042 called"}),
        &[Regex::new(r"ACME-\d{5}").unwrap()],
        "$",
    );
    assert_eq!(r.value["note"], "customer <redacted> called");
    assert_eq!(r.replacements["extra:ACME-\\d{5}"], 1);
}
