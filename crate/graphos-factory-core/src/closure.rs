//! The schema model `source-coverage` reads
//! (docs/decisions/0037-spans-obligations.md): every object and input
//! type's fields with their bare type names, and every root field's
//! argument types. The type-reachability walks the source branch carries
//! (`forward_closure`, `providers`, `operation_closure`) had no caller here
//! and were dropped in review; the spans branch brings them back with their
//! own callers and tests.

use apollo_compiler::ast::{self, Definition};
use serde_json::Value;
use std::collections::BTreeMap;

/// One object or input type's own fields, by name, to their declared bare
/// type name (list/non-null wrappers stripped), and every root field's
/// argument types.
pub struct Schema {
    fields: BTreeMap<String, Vec<(String, String)>>,
    /// every Query/Mutation root field, keyed by "Query.<field>" or
    /// "Mutation.<field>" — Query and Mutation are separate namespaces, so
    /// an identically named field on both roots (legal GraphQL) must not
    /// collide.
    root_fields: BTreeMap<String, RootField>,
}

struct RootField {
    arg_types: Vec<(String, String)>,
}

fn bare_type_name(ty: &ast::Type) -> &str {
    match ty {
        ast::Type::Named(n) | ast::Type::NonNullNamed(n) => n.as_str(),
        ast::Type::List(inner) | ast::Type::NonNullList(inner) => bare_type_name(inner),
    }
}

/// Build the schema's type graph from the current SDL text. `inventory` is
/// accepted for the callers' convenience and not read.
pub fn build_schema(sdl: &str, _inventory: Option<&Value>) -> Result<Schema, String> {
    let doc = ast::Document::parse(sdl, "schema.graphql").map_err(|e| e.to_string())?;
    let mut fields = BTreeMap::new();

    for def in &doc.definitions {
        match def {
            Definition::ObjectTypeDefinition(o) => {
                let fs: Vec<(String, String)> = o
                    .fields
                    .iter()
                    .map(|f| (f.name.to_string(), bare_type_name(&f.ty).to_string()))
                    .collect();
                fields.insert(o.name.to_string(), fs);
            }
            // Input types (a mutation argument's own type) reference further
            // types the same way an object type's fields do; the request
            // walk recurses through them.
            Definition::InputObjectTypeDefinition(o) => {
                let fs: Vec<(String, String)> = o
                    .fields
                    .iter()
                    .map(|f| (f.name.to_string(), bare_type_name(&f.ty).to_string()))
                    .collect();
                fields.insert(o.name.to_string(), fs);
            }
            _ => {}
        }
    }

    let mut root_fields = BTreeMap::new();
    for root in ["Query", "Mutation"] {
        let Some(Definition::ObjectTypeDefinition(o)) = doc
            .definitions
            .iter()
            .find(|d| matches!(d, Definition::ObjectTypeDefinition(o) if o.name == root))
        else {
            continue;
        };
        for f in &o.fields {
            let arg_types = f
                .arguments
                .iter()
                .map(|a| (a.name.to_string(), bare_type_name(&a.ty).to_string()))
                .collect();
            root_fields.insert(format!("{}.{}", root, f.name), RootField { arg_types });
        }
    }

    Ok(Schema {
        fields,
        root_fields,
    })
}

impl Schema {
    /// The bare (List/NonNull-stripped) type name of one field on a known
    /// object or input-object type — `obligations`'s own request-mapped
    /// walk needs this to recurse into a forwarded argument's declared
    /// input type without re-parsing the AST itself.
    pub fn field_type(&self, type_name: &str, field_name: &str) -> Option<&str> {
        self.fields
            .get(type_name)?
            .iter()
            .find(|(f, _)| f == field_name)
            .map(|(_, t)| t.as_str())
    }

    /// A root field's own argument type, by name — `obligations`'s request
    /// walk starting point for a whole-argument forward.
    pub fn root_arg_type(&self, field_name: &str, arg_name: &str) -> Option<&str> {
        self.root_fields
            .get(field_name)?
            .arg_types
            .iter()
            .find(|(a, _)| a == arg_name)
            .map(|(_, t)| t.as_str())
    }
}
