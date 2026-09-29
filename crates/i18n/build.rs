//! Turns the JSON catalogs into static tables, so neither the server nor the
//! WASM module parses JSON at runtime. Keys are stored once, sorted, with one
//! column of translations per language.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;
use std::path::Path;

const LANGUAGES: [&str; 4] = ["es", "fr", "de", "pt"];

fn main() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../static");
    let mut catalogs = Vec::new();
    for language in LANGUAGES {
        let path = dir.join(format!("translations-{language}.json"));
        println!("cargo::rerun-if-changed={}", path.display());
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
        let catalog: BTreeMap<String, String> = serde_json::from_str(&source)
            .unwrap_or_else(|e| panic!("{} is not a valid catalog: {e}", path.display()));
        catalogs.push(catalog);
    }
    let keys: BTreeSet<&String> = catalogs.iter().flat_map(|c| c.keys()).collect();

    let mut out = String::from("pub(crate) static KEYS: &[&str] = &[\n");
    for key in &keys {
        writeln!(out, "    {key:?},").unwrap();
    }
    out += "];\n";
    for (language, catalog) in LANGUAGES.iter().zip(&catalogs) {
        writeln!(
            out,
            "pub(crate) static {}: &[Option<&str>] = &[",
            language.to_uppercase()
        )
        .unwrap();
        for key in &keys {
            match catalog.get(*key) {
                Some(value) => writeln!(out, "    Some({value:?}),").unwrap(),
                None => out += "    None,\n",
            }
        }
        out += "];\n";
    }
    let path = Path::new(&std::env::var("OUT_DIR").unwrap()).join("catalogs.rs");
    std::fs::write(path, out).unwrap();
}
