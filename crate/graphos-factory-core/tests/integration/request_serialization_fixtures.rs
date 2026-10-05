//! Cross-spec / no-name-coupling proof for `request_serialization`,
//! run against real, checked-in fixture directories rather than tempdirs
//! (task PROOF section: "Build compact fixtures from two different specs,
//! with one renamed variant to detect name coupling").

use graphos_factory_core::request_serialization::{obligations, ObligationStatus};
use std::path::Path;

fn assert_all_pass(dir: &Path, label: &str) {
    let obs = obligations(dir);
    assert_eq!(obs.len(), 4, "{}: {:#?}", label, obs);
    for o in &obs {
        assert!(
            matches!(o.status, ObligationStatus::Pass),
            "{}: obligation {} did not pass: {:#?}",
            label,
            o.id,
            o
        );
    }
}

#[test]
fn serialization_widgets_fixture_passes_every_obligation() {
    assert_all_pass(
        Path::new("tests/fixtures/serialization-widgets"),
        "serialization-widgets",
    );
}

#[test]
fn serialization_crm_fixture_passes_every_obligation() {
    assert_all_pass(
        Path::new("tests/fixtures/serialization-crm"),
        "serialization-crm",
    );
}

#[test]
fn renamed_widgets_fixture_produces_the_identical_obligation_set() {
    // Same shapes, same structure, every name mechanically substituted
    // (rename.py). If `request_serialization` were secretly keyed off a
    // literal string like "widget" anywhere, this fixture would diverge --
    // it must not.
    let original = obligations(Path::new("tests/fixtures/serialization-widgets"));
    let renamed = obligations(Path::new("tests/fixtures/serialization-widgets-renamed"));
    assert_eq!(original.len(), renamed.len());
    let mut o_sorted: Vec<(&str, String, Option<(usize, usize)>)> = original
        .iter()
        .map(|o| (o.id.as_str(), o.status.as_str().to_string(), o.denominator))
        .collect();
    let mut r_sorted: Vec<(&str, String, Option<(usize, usize)>)> = renamed
        .iter()
        .map(|o| (o.id.as_str(), o.status.as_str().to_string(), o.denominator))
        .collect();
    o_sorted.sort();
    r_sorted.sort();
    assert_eq!(
        o_sorted, r_sorted,
        "renamed fixture's (id, status, denominator) tuples diverged from the original -- \
         a sign of accidental name coupling in request_serialization"
    );
    // Both must actually be the fully-proven baseline, not two fixtures
    // that happen to fail identically.
    for o in &original {
        assert!(matches!(o.status, ObligationStatus::Pass), "{:#?}", o);
    }
}
