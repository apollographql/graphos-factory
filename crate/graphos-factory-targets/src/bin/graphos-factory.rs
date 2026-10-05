//! `graphos-factory`: the core plus the graphos target, and nothing else.
//! This file is its composition root: one target, so a workspace of any
//! other target is `target-unknown` here.

use graphos_factory_targets::targets::graphos;

static TARGETS: &[graphos_factory_core::target::Target] = &[graphos::TARGET];

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    // The binary's name is the target's: what `version`, `written_by` and
    // provenance's `binary.name` record.
    std::process::exit(graphos_factory_core::run(graphos::NAME, TARGETS, &argv));
}
