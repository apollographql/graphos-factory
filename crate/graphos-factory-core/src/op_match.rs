//! Operation attribution: which inventory operation a connector or a test
//! targets when its path alone cannot say.
//!
//! Two sites match a path to an operation, each from its own input:
//! reconcile matches the SDL's path *template* (`/{$args.campaignId}`),
//! conformance a concrete request *URL* (`/120210000000000001`). Both can
//! end with several equally good candidates — a Graph-style API serves every
//! node at a bare `/{id}` — so each site keeps its own candidate collection
//! and both share the choice made here: one candidate is the answer; several
//! are settled by the operation the caller already knows about the thing it
//! is matching (`declared`); anything else is an error naming every
//! candidate, never the inventory's iteration order (ADR 0044).

use crate::json::{get, get_obj, get_str, truthy};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};

/// `a`, `both a and b`, `a, b and c` — for naming claimants in an error.
fn claimants(keys: &[&str]) -> String {
    match keys {
        [] => String::new(),
        [one] => one.to_string(),
        [a, b] => format!("both {} and {}", a, b),
        [rest @ .., last] => format!("{} and {}", rest.join(", "), last),
    }
}

/// Pick the operation among equally good `candidates`.
///
/// Zero is `Ok(None)` (nothing matches), one is that one, several are the
/// one candidate whose key is among `declared` — the operations the
/// selection or the test declares for the thing being matched. A tie that
/// leaves no declared candidate, or more than one, is an `Err` listing the
/// candidates' keys.
pub fn disambiguate<'a>(
    candidates: Vec<&'a Value>,
    declared: &[&str],
) -> Result<Option<&'a Value>, String> {
    if candidates.len() < 2 {
        return Ok(candidates.first().copied());
    }
    let key = |op: &Value| get_str(op, "key").unwrap_or("?").to_string();
    let chosen: Vec<&Value> = candidates
        .iter()
        .copied()
        .filter(|op| declared.contains(&key(op).as_str()))
        .collect();
    if chosen.len() == 1 {
        return Ok(Some(chosen[0]));
    }
    let mut keys: Vec<String> = candidates.iter().map(|op| key(op)).collect();
    keys.sort_unstable();
    let equally = format!(
        "matches {} operations equally ({})",
        keys.len(),
        keys.join(", ")
    );
    let mut chosen_keys: Vec<String> = chosen.iter().map(|op| key(op)).collect();
    chosen_keys.sort_unstable();
    let chosen_keys: Vec<&str> = chosen_keys.iter().map(String::as_str).collect();
    Err(match declared {
        [] => format!("{} and nothing declares which", equally),
        _ if chosen.len() > 1 => format!(
            "{} and it is claimed by {}",
            equally,
            claimants(&chosen_keys)
        ),
        [one] => format!(
            "{} and the declared operation {} is not one of them",
            equally, one
        ),
        many => format!(
            "{} and none of the declared operations ({}) is one of them",
            equally,
            many.join(", ")
        ),
    })
}

/// The operations `selection.yaml` declares. Included entries claim first;
/// an excluded entry's `graphql` block declares a name only when no included
/// entry claims it. So it never cancels an included hint, and the field it
/// still names until `apply` removes it stays attributed — which is what
/// lets reconcile report the removal and lock record it:
///
/// - for each root field, `Query.<prefix>_<name>` from `graphql.root` and
///   `graphql.name`;
/// - for each case scaffold names after an entry, `snake(name)` — the stem
///   of `tests/cases/<case>.graphql` and `tests/fixtures/mappings/<case>.json`;
/// - for each entity type, the entries with `graphql.entity: true` whose
///   root field returns that type — what a type-level `@connect` serves.
///
/// A name two entries of the same rank both claim lists both, so a tie
/// between exactly those two stays an error that names them.
#[derive(Debug, Clone, Default)]
pub struct OpHints {
    fields: BTreeMap<String, Vec<String>>,
    cases: BTreeMap<String, Vec<String>>,
    types: BTreeMap<String, Vec<String>>,
    /// The workspace's sparse-fieldsets parameter (`sparse_fieldsets.param`,
    /// `fields` when absent): scaffold names a narrowed variant
    /// `<case>_<param>_narrowed` (ADR 0045).
    sparse_param: Option<String>,
    /// Field-level relationship connectors (ADR 0069): `(host type, field)`
    /// → the operations the included `links:` entries declare for it.
    links: HashMap<(String, String), Vec<String>>,
}

fn claim(map: &mut BTreeMap<String, Vec<String>>, name: String, key: &str) {
    let keys = map.entry(name).or_default();
    if !keys.iter().any(|k| k == key) {
        keys.push(key.to_string());
    }
}

fn lookup<'a>(map: &'a BTreeMap<String, Vec<String>>, name: &str) -> Vec<&'a str> {
    map.get(name)
        .map(|keys| keys.iter().map(String::as_str).collect())
        .unwrap_or_default()
}

impl OpHints {
    /// `sdl`, when given, is what ties an entity entry to its type: the type
    /// its root field returns.
    pub fn from_selection(
        workspace: Option<&Value>,
        selection: Option<&Value>,
        sdl: Option<&str>,
    ) -> Self {
        let prefix = workspace
            .and_then(|w| get_str(w, "field_prefix"))
            .unwrap_or("");
        let mut index = sdl.map(crate::sdl_index::SdlIndex::new);
        let mut included = OpHints::default();
        let mut excluded = OpHints::default();
        for (key, entry) in selection
            .and_then(|s| get_obj(s, "operations"))
            .into_iter()
            .flatten()
        {
            let hints = if truthy(get(entry, "include")) {
                &mut included
            } else {
                &mut excluded
            };
            let Some(g) = get(entry, "graphql") else {
                continue;
            };
            let Some(name) = get_str(g, "name") else {
                continue;
            };
            let root = if get_str(g, "root") == Some("mutation") {
                "Mutation"
            } else {
                "Query"
            };
            let field = format!("{}_{}", prefix, name);
            if get(g, "entity").and_then(Value::as_bool) == Some(true) {
                if let Some(t) = index.as_mut().and_then(|i| i.field_type(root, &field)) {
                    claim(&mut hints.types, crate::sdl_index::base_type(&t), key);
                }
            }
            claim(&mut hints.fields, format!("{}.{}", root, field), key);
            claim(&mut hints.cases, crate::cmd::scaffold::snake(name), key);
        }
        for (mine, theirs) in [
            (&mut included.fields, excluded.fields),
            (&mut included.cases, excluded.cases),
            (&mut included.types, excluded.types),
        ] {
            for (name, keys) in theirs {
                mine.entry(name).or_insert(keys);
            }
        }
        included.sparse_param = workspace.map(crate::sparse::param_name);
        included
    }

    /// What a unit entry's `target` declares: a root field by its
    /// root-qualified name (`Query.meta_ads_campaign`), or an entity type by
    /// its name (`Meta_Campaign`, a type-level connector's entry).
    pub fn for_target(&self, target: &str) -> Vec<&str> {
        if target.contains('.') {
            lookup(&self.fields, target)
        } else {
            self.for_type(target)
        }
    }

    pub fn for_field(&self, root: &str, field: &str) -> Vec<&str> {
        lookup(&self.fields, &format!("{}.{}", root, field))
    }

    /// The entity entries whose root field returns `type_name`.
    pub fn for_type(&self, type_name: &str) -> Vec<&str> {
        lookup(&self.types, type_name)
    }

    /// Add the `links:` entries' declarations (ADR 0069). Each included link
    /// names an operation; its host type and field are derived from the
    /// inventory and the SDL (`reconcile::link_hosts`, which attributes the
    /// root fields through the hints this value already holds — so on a
    /// bare-`/{id}` API the host is the root field the selection declares
    /// for the operation returning the shape, R11). A second step after
    /// `from_selection` that needs both. Every holder of hints that matches
    /// a field-level connector builds them through this: reconcile here,
    /// `lock::load` for obligations and `validate` in PR C/D (R12) — a
    /// `for_link` on hints built without it is always empty. A link whose
    /// shape, operation or path does not resolve in the inventory declares
    /// nothing and never reaches `link_hosts` (R38); one whose by-id
    /// operation the selection excludes is the caller's to drop, as
    /// reconcile does through `link_reference_problems`.
    pub fn with_links(
        mut self,
        links: &[crate::reconcile::Link],
        inventory: &Value,
        sdl: &str,
    ) -> Self {
        let mut index = crate::sdl_index::SdlIndex::new(sdl);
        let ops = crate::json::get_arr(inventory, "operations")
            .cloned()
            .unwrap_or_default();
        let shapes = get_obj(inventory, "shapes");
        for link in links.iter().filter(|l| l.include) {
            // An unknown operation, shape or path is reconcile's selection
            // error, not a hint.
            let known_operation = ops
                .iter()
                .any(|o| get_str(o, "key") == Some(link.operation.as_str()));
            let known_path = shapes.is_some_and(|all| {
                all.get(&link.shape).is_some_and(|shape| {
                    crate::reconcile::resolve_link_path(shape, &link.path, all)
                })
            });
            if !known_operation || !known_path {
                continue;
            }
            let field = link.field_name();
            let hosts = crate::reconcile::link_hosts(link, inventory, sdl, &mut index, &self);
            for (host, _fk) in hosts {
                let keys = self.links.entry((host, field.clone())).or_default();
                if !keys.iter().any(|k| k == &link.operation) {
                    keys.push(link.operation.clone());
                }
            }
        }
        self
    }

    /// What the `links:` entries declare for the field-level connector
    /// `type_name.field`.
    pub fn for_link(&self, type_name: &str, field: &str) -> Vec<&str> {
        self.links
            .get(&(type_name.to_string(), field.to_string()))
            .map(|keys| keys.iter().map(String::as_str).collect())
            .unwrap_or_default()
    }

    /// What a case document (`tests/cases/<case>.graphql`) declares: the
    /// operations of the root fields it selects, of `Mutation` when it is a
    /// mutation and of `Query` otherwise. A field is found by name, as
    /// lint's case coverage finds it.
    pub fn for_document(&self, text: &str) -> Vec<&str> {
        let code = crate::graphql::blank(text);
        let root = if code.trim_start().starts_with("mutation") {
            "Mutation."
        } else {
            "Query."
        };
        let mut out: Vec<&str> = Vec::new();
        for (target, keys) in &self.fields {
            let Some(field) = target.strip_prefix(root) else {
                continue;
            };
            let word = regex::Regex::new(&format!(r"\b{}\b", regex::escape(field))).unwrap();
            if word.is_match(&code) {
                for k in keys {
                    if !out.contains(&k.as_str()) {
                        out.push(k);
                    }
                }
            }
        }
        out
    }

    /// What a case or stub file stem declares by its name: the case itself,
    /// the `<case>_minimal` variant scaffold writes beside it, or a
    /// sparse-fieldsets GET's `<case>_<param>_narrowed` variant, `<param>`
    /// exactly the workspace's sparse parameter (ADR 0045), so neither
    /// `campaigns_status_narrowed` nor a `field_mask` parameter's stub is
    /// claimed by a shorter case name. A variant named for its own case
    /// (`campaign_not_found`) is resolved through that case's document
    /// instead (`validate`).
    pub fn for_case(&self, stem: &str) -> Vec<&str> {
        let own = lookup(&self.cases, stem);
        if !own.is_empty() {
            return own;
        }
        if let Some(s) = stem.strip_suffix("_minimal") {
            return lookup(&self.cases, s);
        }
        let suffix = format!(
            "_{}_narrowed",
            self.sparse_param.as_deref().unwrap_or("fields")
        );
        stem.strip_suffix(suffix.as_str())
            .map(|s| lookup(&self.cases, s))
            .unwrap_or_default()
    }
}
