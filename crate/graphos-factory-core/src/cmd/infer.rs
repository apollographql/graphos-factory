//! infer — recorded samples -> .factory/inferred-schema.json, and optionally
//! into the inventory's shapes so the conformance layer has an oracle.
//!
//!   infer [workspace] [--out FILE] [--update-inventory] [--json] [--schemas DIR]

use crate::args::{Args, Flags};
use crate::infer::{
    collect_vocabulary, enum_groups, hints_for, infer_operations, infer_shape, Hints,
};
use crate::json::{get, get_arr, get_str, obj, pretty};
use serde_json::Value;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

fn is_ok(s: &Value) -> bool {
    let r = match get(s, "response") {
        Some(r) => r,
        None => return false,
    };
    let status = get(r, "status").and_then(Value::as_i64).unwrap_or(0);
    (200..300).contains(&status) && get(r, "json") != Some(&Value::Bool(false))
}

/// The flags this verb accepts; dispatch refuses any other (ADR 0097).
pub const FLAGS: Flags = Flags {
    boolean: &["update-inventory", "json"],
    valued: &["out", "schemas"],
};

pub fn main(argv: &[String]) -> i32 {
    let args = Args::parse(argv, &FLAGS);
    let dir = Path::new(&args.dir()).to_path_buf();
    const SAMPLES_ROOT: &str = ".factory/samples";
    let samples_root = dir.join(SAMPLES_ROOT);
    if !crate::factory_io::is_dir(&dir, SAMPLES_ROOT) {
        eprintln!(
            "infer: no {}; record samples with probe first",
            samples_root.display()
        );
        return 1;
    }
    let mut samples: Vec<Value> = Vec::new();
    let mut subs: Vec<String> = crate::factory_io::read_dir(&dir, SAMPLES_ROOT)
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
                .map(|e| e.file_name().to_string_lossy().to_string())
                .collect()
        })
        .unwrap_or_default();
    subs.sort();
    for d in subs {
        let sub_rel = format!("{}/{}", SAMPLES_ROOT, d);
        let mut files: Vec<String> = crate::factory_io::read_dir(&dir, &sub_rel)
            .map(|rd| {
                rd.flatten()
                    .map(|e| e.file_name().to_string_lossy().to_string())
                    .filter(|n| n.ends_with(".json"))
                    .collect()
            })
            .unwrap_or_default();
        files.sort();
        for f in files {
            let rel = format!("{}/{}", sub_rel, f);
            let text = crate::factory_io::read_to_string(&dir, &rel);
            match text
                .map_err(String::from)
                .and_then(|t| crate::json::parse(&t))
            {
                Ok(v) => samples.push(v),
                Err(e) => eprintln!("infer: skipping unreadable sample {}/{}: {}", d, f, e),
            }
        }
    }
    const HINTS: &str = ".factory/inference.yaml";
    let hints_doc = match crate::factory_io::read_to_string_optional(&dir, HINTS) {
        Ok(Some(text)) => crate::yaml::parse(&text).unwrap_or(Value::Object(obj())),
        Ok(None) => Value::Object(obj()),
        Err(e) => {
            eprintln!("infer: {}", e);
            return 1;
        }
    };
    let hints_doc = if hints_doc.is_null() {
        Value::Object(obj())
    } else {
        hints_doc
    };

    let ok_bodies: Vec<&Value> = samples
        .iter()
        .filter(|s| is_ok(s))
        .filter_map(|s| get(s, "response").and_then(|r| crate::json::field(r, "body")))
        .collect();
    let vocabulary: HashMap<String, BTreeSet<String>> =
        collect_vocabulary(&ok_bodies, &enum_groups(&hints_doc));
    let operations = infer_operations(&samples, &hints_doc, &vocabulary);

    let mut vocab_out = obj();
    let mut vocab_keys: Vec<&String> = vocabulary.keys().collect();
    // Insertion order of the Map in JavaScript follows the hint groups; sort
    // by group order for determinism.
    let group_order: Vec<String> = enum_groups(&hints_doc).into_iter().flatten().collect();
    vocab_keys.sort_by_key(|k| {
        group_order
            .iter()
            .position(|g| g == *k)
            .unwrap_or(usize::MAX)
    });
    for k in vocab_keys {
        let mut vals: Vec<String> = vocabulary[k].iter().cloned().collect();
        vals.sort();
        vocab_out.insert(
            k.clone(),
            Value::Array(vals.into_iter().map(Value::from).collect()),
        );
    }
    let result = crate::json::object(vec![
        ("contract_version", Value::from(1)),
        ("generated_at", Value::from(crate::now_iso())),
        ("samples", Value::from(samples.len())),
        (
            "hints",
            crate::json::object(vec![
                (
                    "maps",
                    get(&hints_doc, "maps")
                        .cloned()
                        .unwrap_or(Value::Array(vec![])),
                ),
                (
                    "enums",
                    get(&hints_doc, "enums")
                        .cloned()
                        .unwrap_or(Value::Array(vec![])),
                ),
            ]),
        ),
        ("vocabulary", Value::Object(vocab_out)),
        ("operations", Value::Object(operations.clone())),
    ]);
    let out = args
        .get("out")
        .map(PathBuf::from)
        .unwrap_or_else(|| dir.join(".factory").join("inferred-schema.json"));
    if let Err(e) = crate::factory_io::write_named_path(&out, pretty(&result).as_bytes()) {
        eprintln!("infer: {}", e);
        return 1;
    }

    let mut lines = vec![format!(
        "infer: {} samples over {} operations -> {}",
        samples.len(),
        operations.len(),
        crate::json::relative(&dir, &out)
    )];
    for (key, entry) in &operations {
        let errs = get(entry, "errors")
            .and_then(Value::as_object)
            .map(|e| {
                format!(
                    " errors:[{}]",
                    e.keys().cloned().collect::<Vec<_>>().join(",")
                )
            })
            .unwrap_or_default();
        let status = match get(entry, "response").and_then(|r| get(r, "status")) {
            Some(Value::Array(a)) => format!(
                " → {}",
                a.iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join("/")
            ),
            Some(Value::String(s)) => format!(" → {}", s),
            _ => " (no 2xx sample)".to_string(),
        };
        lines.push(format!(
            "  {}: {} sample(s){}{}",
            key,
            get(entry, "samples").and_then(Value::as_u64).unwrap_or(0),
            status,
            errs
        ));
    }

    if args.has("update-inventory") {
        const INVENTORY: &str = ".factory/inventory.json";
        let inventory_text = match crate::factory_io::read_to_string_optional(&dir, INVENTORY) {
            Ok(Some(text)) => text,
            Ok(None) => {
                eprintln!("infer: --update-inventory needs .factory/inventory.json");
                return 1;
            }
            Err(e) => {
                eprintln!("infer: {}", e);
                return 1;
            }
        };
        let mut inventory = match crate::json::parse(&inventory_text) {
            Ok(i) => i,
            Err(e) => {
                eprintln!("infer: {}", e);
                return 1;
            }
        };
        if get(&inventory, "shapes").is_none() {
            crate::json::set(&mut inventory, "shapes", Value::Object(obj()));
        }
        // shape name -> (bodies, ops)
        let mut by_shape: Vec<(String, Vec<Value>, Vec<String>)> = Vec::new();
        let mut missing: Vec<String> = Vec::new();
        let ops = get_arr(&inventory, "operations")
            .cloned()
            .unwrap_or_default();
        for op in &ops {
            let key = get_str(op, "key").unwrap_or("").to_string();
            let own: Vec<&Value> = samples
                .iter()
                .filter(|s| get_str(s, "operation") == Some(key.as_str()))
                .collect();
            if own.is_empty() {
                missing.push(key);
                continue;
            }
            let mut collect = |name: &str, bodies: Vec<Value>| match by_shape
                .iter_mut()
                .find(|(n, _, _)| n == name)
            {
                Some((_, b, o)) => {
                    b.extend(bodies);
                    if !o.contains(&key) {
                        o.push(key.clone());
                    }
                }
                None => by_shape.push((name.to_string(), bodies, vec![key.clone()])),
            };
            let ok_bodies: Vec<Value> = own
                .iter()
                .filter(|s| is_ok(s))
                .filter_map(|s| {
                    get(s, "response")
                        .and_then(|r| crate::json::field(r, "body"))
                        .cloned()
                })
                .collect();
            if let Some(r) = get(op, "response").and_then(|r| get_str(r, "shape_ref")) {
                if !ok_bodies.is_empty() {
                    collect(r.rsplit('/').next().unwrap_or(r), ok_bodies);
                }
            }
            for err in get_arr(op, "errors").into_iter().flatten() {
                let shape_ref = match get_str(err, "shape_ref") {
                    Some(s) => s,
                    None => continue,
                };
                let status = get(err, "status")
                    .map(crate::json::compact)
                    .unwrap_or_default()
                    .trim_matches('"')
                    .to_string();
                let bodies: Vec<Value> = own
                    .iter()
                    .filter(|s| {
                        let st = get(s, "response")
                            .and_then(|r| get(r, "status"))
                            .map(crate::json::compact)
                            .unwrap_or_default();
                        st == status
                            && get(s, "response").and_then(|r| get(r, "json"))
                                != Some(&Value::Bool(false))
                    })
                    .filter_map(|s| {
                        get(s, "response")
                            .and_then(|r| crate::json::field(r, "body"))
                            .cloned()
                    })
                    .collect();
                if !bodies.is_empty() {
                    collect(shape_ref.rsplit('/').next().unwrap_or(shape_ref), bodies);
                }
            }
        }
        let mut updated: Vec<String> = Vec::new();
        for (name, bodies, op_keys) in &by_shape {
            let mut h = Hints {
                maps: HashSet::new(),
                enums: HashSet::new(),
                vocabulary: vocabulary.clone(),
            };
            for k in op_keys {
                let hk = hints_for(&hints_doc, k, &vocabulary);
                h.maps.extend(hk.maps);
                h.enums.extend(hk.enums);
            }
            let refs: Vec<Option<&Value>> = bodies.iter().map(Some).collect();
            let mut shape = infer_shape(&refs, &h, "$");
            crate::json::set(
                &mut shape,
                "x-inferred-from",
                Value::from(format!(
                    "{} samples across {} operation(s)",
                    bodies.len(),
                    op_keys.len()
                )),
            );
            if let Some(Value::Object(shapes)) = inventory.get_mut("shapes") {
                shapes.insert(name.clone(), shape);
            }
            updated.push(format!("{} ({})", name, bodies.len()));
        }
        // A replaced shape changes the response facts that rest on it (how
        // many root properties it has, which are arrays, ADR 0018), so they
        // are recomputed rather than left to go stale.
        refresh_response_facts(&mut inventory);
        let schemas_dir = args.get("schemas").map(PathBuf::from);
        if let Some(schema) = crate::schemas::load("inventory.schema.json", schemas_dir.as_deref())
        {
            let errors = crate::jsonschema::validate(&inventory, &schema);
            if !errors.is_empty() {
                for e in errors.iter().take(10) {
                    eprintln!("  contract: {}", e);
                }
                eprintln!("infer: refusing to write an inventory that violates the contract");
                return 1;
            }
        }
        if let Err(e) =
            crate::factory_io::write_in_place(&dir, INVENTORY, pretty(&inventory).as_bytes())
        {
            eprintln!("infer: {}", e);
            return 1;
        }
        lines.push(format!(
            "infer: updated {} inventory shape(s): {}",
            updated.len(),
            updated.join(", ")
        ));
        if !missing.is_empty() {
            lines.push(format!(
                "infer: no samples for {} inventory operation(s): {}",
                missing.len(),
                missing.join(", ")
            ));
        }
    }

    if args.has("json") {
        print!("{}", pretty(&result));
    } else {
        println!("{}", lines.join("\n"));
    }
    0
}

/// Recompute every operation's response facts from the inventory's shapes.
/// Used after `--update-inventory` replaces a shape, and by `inventory build`
/// is unnecessary (the reader computes them as it goes).
pub fn refresh_response_facts(inventory: &mut Value) {
    let shapes = crate::json::get_obj(inventory, "shapes")
        .cloned()
        .unwrap_or_default();
    let ops = match inventory
        .get_mut("operations")
        .and_then(Value::as_array_mut)
    {
        Some(o) => o,
        None => return,
    };
    for op in ops.iter_mut() {
        let shape_ref = match get(op, "response").and_then(|r| get_str(r, "shape_ref")) {
            Some(r) => crate::json::object(vec![("$ref", Value::from(r))]),
            None => continue,
        };
        let facts = crate::envelope::response_facts(Some(&shape_ref), &shapes);
        let response = match op.get_mut("response") {
            Some(r) => r,
            None => continue,
        };
        for key in [
            "root_is_array",
            "root_property_count",
            "link_root_properties",
            "array_root_properties",
            "sole_root_property",
            "total_items_property",
            "cursor_root_properties",
        ] {
            crate::json::remove(response, key);
        }
        for (key, value) in facts {
            crate::json::set(response, key, value);
        }
    }
}
