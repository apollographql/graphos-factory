//! The binary's environment variables: `GRAPHOS_FACTORY_CORE_<NAME>` (ADR 0114). Phase
//! 8b removed the fallback to the pre-split spelling; only these names are
//! read.

use std::ffi::OsString;

/// `GRAPHOS_FACTORY_CORE_<name>`, the name to tell a user to set.
pub fn name(name: &str) -> String {
    format!("GRAPHOS_FACTORY_CORE_{}", name)
}

/// The variable's value and the name it was read under, `GRAPHOS_FACTORY_CORE_<name>`,
/// when set (even empty).
pub fn var_os_named(name: &str) -> Option<(String, OsString)> {
    let new = self::name(name);
    std::env::var_os(&new).map(|value| (new, value))
}

/// The variable's value.
pub fn var_os(name: &str) -> Option<OsString> {
    var_os_named(name).map(|(_, value)| value)
}

/// The variable's value as a string; `None` when unset or not Unicode.
pub fn var(name: &str) -> Option<String> {
    var_os(name).and_then(|value| value.into_string().ok())
}

/// Whether the variable is unset.
pub fn is_unset(name: &str) -> bool {
    var_os_named(name).is_none()
}
