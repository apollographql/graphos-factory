//! Flag parsing shared by every subcommand: `--name value`, `--name=value`,
//! bare `--flag`, repeatable flags, and positionals — the same grammar the
//! previous scripts accepted, so SKILL.md's command lines still work.
//!
//! Every command and verb declares the flags it accepts as a [`Flags`]
//! (ADR 0097). [`crate::cmd::dispatch`] holds the arguments to that set
//! before the command runs, so a flag no verb reads — a typo, or
//! `'--check --provenance'` passed as one argument — is a usage error with
//! exit 2, not a bare flag the command never asks for.

use std::collections::HashMap;

/// The flags one command or verb accepts: `boolean` ones never take a
/// value, `valued` ones take the next token (or `=value`).
#[derive(Debug)]
pub struct Flags {
    pub boolean: &'static [&'static str],
    pub valued: &'static [&'static str],
}

impl Flags {
    /// A command that takes no flags at all.
    pub const NONE: Flags = Flags {
        boolean: &[],
        valued: &[],
    };

    pub fn accepts(&self, name: &str) -> bool {
        self.boolean.contains(&name) || self.valued.contains(&name)
    }

    /// The accepted flags as a usage fragment: `--json --model <value>`.
    pub fn describe(&self) -> String {
        let mut parts: Vec<String> = self.boolean.iter().map(|f| format!("--{}", f)).collect();
        parts.extend(self.valued.iter().map(|f| format!("--{} <value>", f)));
        if parts.is_empty() {
            "no flags".to_string()
        } else {
            parts.join(" ")
        }
    }

    /// The first argument that is not one of these flags, walked with
    /// [`Args::parse`]'s grammar: a valued flag's value is skipped, so a
    /// value may start with a single `-`; any other token that starts with
    /// `--`, or is a dash and a letter (`-c`), is a flag and must be
    /// declared. `Err` carries the offending argument as given.
    pub fn check(&self, argv: &[String]) -> Result<(), String> {
        let mut i = 0;
        while i < argv.len() {
            let a = &argv[i];
            if let Some(rest) = a.strip_prefix("--") {
                let (name, inline) = match rest.split_once('=') {
                    Some((name, _)) => (name, true),
                    None => (rest, false),
                };
                if !self.accepts(name) {
                    return Err(a.clone());
                }
                if !inline
                    && self.valued.contains(&name)
                    && i + 1 < argv.len()
                    && !argv[i + 1].starts_with("--")
                {
                    i += 1;
                }
            } else if is_short_flag(a) {
                return Err(a.clone());
            }
            i += 1;
        }
        Ok(())
    }
}

/// `-c`, `-x=1`: a dash and a letter. A lone `-` and a negative number
/// stay positionals.
fn is_short_flag(a: &str) -> bool {
    let mut chars = a.chars();
    chars.next() == Some('-') && chars.next().is_some_and(|c| c.is_ascii_alphabetic())
}

#[derive(Debug, Default, Clone)]
pub struct Args {
    pub positional: Vec<String>,
    flags: HashMap<String, Vec<String>>,
    present: Vec<String>,
}

impl Args {
    /// `flags.boolean` never consume the following token. The grammar
    /// itself takes any flag; [`Flags::check`] is what refuses one the
    /// verb does not declare, and dispatch runs it before the verb.
    pub fn parse(argv: &[String], flags: &Flags) -> Args {
        let boolean_flags = flags.boolean;
        let mut out = Args::default();
        let mut i = 0;
        while i < argv.len() {
            let a = &argv[i];
            if let Some(rest) = a.strip_prefix("--") {
                if let Some((name, inline)) = rest.split_once('=') {
                    out.push(name, inline);
                } else if boolean_flags.contains(&rest) {
                    out.present.push(rest.to_string());
                } else if i + 1 < argv.len() && !argv[i + 1].starts_with("--") {
                    i += 1;
                    out.push(rest, &argv[i]);
                } else {
                    out.present.push(rest.to_string());
                }
            } else {
                out.positional.push(a.clone());
            }
            i += 1;
        }
        out
    }

    fn push(&mut self, name: &str, value: &str) {
        self.flags
            .entry(name.to_string())
            .or_default()
            .push(value.to_string());
        self.present.push(name.to_string());
    }

    pub fn has(&self, name: &str) -> bool {
        self.present.iter().any(|p| p == name)
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.flags
            .get(name)
            .and_then(|v| v.last())
            .map(String::as_str)
    }

    pub fn all(&self, name: &str) -> Vec<String> {
        self.flags.get(name).cloned().unwrap_or_default()
    }

    /// Every flag given, valued or bare, in order (repeats included).
    pub fn flag_names(&self) -> &[String] {
        &self.present
    }

    pub fn dir(&self) -> String {
        self.positional
            .first()
            .cloned()
            .unwrap_or_else(|| ".".to_string())
    }
}
