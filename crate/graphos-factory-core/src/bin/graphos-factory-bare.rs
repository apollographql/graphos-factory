//! `graphos-factory-bare`: the core alone, never installed. It registers no
//! target, so every command runs against `target::BARE` and no workspace's
//! `skill.name` is held to a registered one. The core suite spawns it, so a
//! core test can never pass on a target's command or rule.

static TARGETS: &[graphos_factory_core::target::Target] = &[];

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(graphos_factory_core::run(
        "graphos-factory-bare",
        TARGETS,
        &argv,
    ));
}
