//! Rejects removal or type changes in API examples consumed by Android (#230).

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde_json::Value;

/// `names.json` groups whose names Android reads.
const CONSUMED: [&str; 3] = ["consumed", "boards", "challenges"];

/// Where the server keeps its examples, relative to the repository root.
const SERVER_EXAMPLES: &str = "docs/api/examples";

/// Length of a full hexadecimal git commit SHA.
const SHA_LEN: usize = 40;

/// A key path; `None` stands for any array element.
type KeyPath = Vec<Option<String>>;

/// Every key path in a document with the JSON types seen there.
type Shape = BTreeMap<KeyPath, BTreeSet<&'static str>>;

/// Parsed command-line flags.
struct Args {
    server: PathBuf,
    resources: PathBuf,
}

/// Runs the guard and reports the result as CI expects.
pub fn run(args: &[String]) -> ExitCode {
    let args = match parse(args) {
        Ok(args) => args,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };

    // A broken catalog, pin or file fails the job instead of skipping it.
    let errors = match check(&args.server, &args.resources) {
        Ok(errors) => errors,
        Err(error) => {
            eprintln!("Android API compatibility check failed: {error}");
            return ExitCode::FAILURE;
        }
    };

    // Android has not published its catalog yet: notice, not failure.
    let Some(errors) = errors else {
        println!(
            "::notice::Android names.json is not published yet; \
             compatibility guard awaits Android #203."
        );
        return ExitCode::SUCCESS;
    };
    if errors.is_empty() {
        println!("Android's consumed example names, key paths and JSON types are preserved.");
        return ExitCode::SUCCESS;
    }

    eprintln!("Android's consumed API examples are no longer compatible:");
    for error in errors {
        eprintln!("- {error}");
    }
    ExitCode::FAILURE
}

/// Reads `--android-resources` (required) and `--server-examples`.
fn parse(args: &[String]) -> Result<Args, String> {
    let mut server = PathBuf::from(SERVER_EXAMPLES);
    let mut resources = None;
    let mut iter = args.iter();
    while let Some(flag) = iter.next() {
        let value = iter.next().ok_or(format!("{flag} needs a value"))?;
        match flag.as_str() {
            "--android-resources" => resources = Some(PathBuf::from(value)),
            "--server-examples" => server = PathBuf::from(value),
            _ => return Err(format!("unknown argument {flag}")),
        }
    }
    let resources = resources.ok_or("--android-resources is required")?;
    Ok(Args { server, resources })
}

/// The JSON type name of a value.
fn kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Collects the shape of `value` below `path` into `shape`.
fn walk(value: &Value, path: &mut KeyPath, shape: &mut Shape) {
    shape.entry(path.clone()).or_default().insert(kind(value));

    // Object members get their key; array elements share one path.
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                path.push(Some(key.clone()));
                walk(child, path, shape);
                path.pop();
            }
        }
        Value::Array(items) => {
            for child in items {
                path.push(None);
                walk(child, path, shape);
                path.pop();
            }
        }
        _ => {}
    }
}

/// The shape of a whole document.
fn shape(value: &Value) -> Shape {
    let mut shape = Shape::new();
    walk(value, &mut Vec::new(), &mut shape);
    shape
}

/// A key path as JSONPath, such as `$["items"][*]["id"]`.
fn where_(path: &KeyPath) -> String {
    let mut text = String::from("$");
    for key in path {
        let part = match key {
            Some(key) => format!("[{}]", Value::String(key.clone())),
            None => "[*]".to_owned(),
        };
        text.push_str(&part);
    }
    text
}

/// Joins sorted type names with commas.
fn join(kinds: &BTreeSet<&str>) -> String {
    kinds.iter().copied().collect::<Vec<_>>().join(", ")
}

/// Removed key paths and lost JSON types between two documents.
fn changes(before: &Value, after: &Value) -> Vec<String> {
    let new = shape(after);
    let mut errors = Vec::new();
    for (path, kinds) in shape(before) {
        let Some(now) = new.get(&path) else {
            errors.push(format!("{} was removed", where_(&path)));
            continue;
        };
        let lost: BTreeSet<&str> = kinds.difference(now).copied().collect();
        if lost.is_empty() {
            continue;
        }
        errors.push(format!(
            "{} lost JSON type(s) {}; now {}",
            where_(&path),
            join(&lost),
            join(now)
        ));
    }
    errors
}

/// Reads and parses a JSON file.
fn read_json(path: &Path) -> Result<Value, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// Whether `name` is a safe example name: `[a-z][a-z0-9_]*`, not `names`.
fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_lowercase() || name == "names" {
        return false;
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// The sorted names in one consumed group; a missing group is empty.
fn group(catalog: &Value, group: &str) -> Result<Vec<String>, String> {
    let invalid = || format!("names.json has an invalid {group} array");
    let Some(names) = catalog.get(group) else {
        return Ok(Vec::new());
    };
    let names = names.as_array().ok_or_else(invalid)?;
    names
        .iter()
        .map(|name| match name.as_str() {
            Some(name) if valid_name(name) => Ok(name.to_owned()),
            _ => Err(invalid()),
        })
        .collect()
}

/// Every example name Android consumes, sorted.
fn names(file: &Path) -> Result<BTreeSet<String>, String> {
    let catalog = read_json(file)?;
    if catalog.get("consumed").is_none() {
        return Err("names.json must contain a consumed array".to_owned());
    }
    let mut result = BTreeSet::new();
    for name in CONSUMED {
        result.extend(group(&catalog, name)?);
    }
    Ok(result)
}

/// Whether the pin is a full lowercase commit SHA.
fn valid_pin(pin: &str) -> bool {
    pin.len() == SHA_LEN
        && pin
            .chars()
            .all(|c| c.is_ascii_digit() || matches!(c, 'a'..='f'))
}

/// Compares one consumed example with the server's copy.
fn compare(name: &str, old: &Path, new: &Path) -> Result<Vec<String>, String> {
    if !old.is_file() {
        return Ok(vec![format!(
            "Android consumes {name} but its vendored example is missing"
        )]);
    }
    if !new.is_file() {
        return Ok(vec![format!("Consumed example {name}.json was removed")]);
    }
    let found = changes(&read_json(old)?, &read_json(new)?);
    Ok(found
        .into_iter()
        .map(|c| format!("{name}.json: {c}"))
        .collect())
}

/// All incompatibilities, or `None` while Android has no catalog.
fn check(server: &Path, resources: &Path) -> Result<Option<Vec<String>>, String> {
    let examples = resources.join("ankiquest-api");
    let catalog = examples.join("names.json");
    if !catalog.exists() {
        return Ok(None);
    }

    // The pin proves which server commit the vendored data came from.
    let pin_file = resources.join("ankiquest-server.sha");
    let pin = fs::read_to_string(&pin_file).map_err(|e| format!("{}: {e}", pin_file.display()))?;
    if !valid_pin(pin.trim()) {
        return Err("Android's ankiquest-server.sha must contain a full commit SHA".to_owned());
    }

    // Compare every consumed example with the server's current one.
    let mut errors = Vec::new();
    for name in names(&catalog)? {
        let file = format!("{name}.json");
        errors.extend(compare(&name, &examples.join(&file), &server.join(&file))?);
    }
    Ok(Some(errors))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    /// A server examples folder and an Android resources folder.
    struct Fixture {
        _root: TempDir,
        server: PathBuf,
        android: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let root = TempDir::new().unwrap();
            let server = root.path().join("server");
            let android = root.path().join("android");
            fs::create_dir(&server).unwrap();
            fs::create_dir_all(android.join("ankiquest-api")).unwrap();
            fs::write(android.join("ankiquest-server.sha"), "a".repeat(40) + "\n").unwrap();
            Self {
                _root: root,
                server,
                android,
            }
        }

        fn api(&self) -> PathBuf {
            self.android.join("ankiquest-api")
        }

        fn catalog(&self, groups: Value) {
            write(&self.api(), "names", &groups);
        }

        fn check(&self) -> Result<Option<Vec<String>>, String> {
            check(&self.server, &self.android)
        }
    }

    fn write(parent: &Path, name: &str, value: &Value) {
        fs::write(parent.join(format!("{name}.json")), value.to_string()).unwrap();
    }

    #[test]
    fn values_new_keys_and_variants_pass() {
        let before = json!({"items": [{"id": 1, "name": "old"}], "count": 1});
        let after = json!({
            "items": [{"id": 2.0, "name": "new", "avatar": null}, {"id": 3}],
            "count": 20,
            "next": null
        });
        assert!(changes(&before, &after).is_empty());
    }

    #[test]
    fn removed_keys_and_bool_fail() {
        let before = json!({"items": [{"id": 1, "label": "word"}]});
        let after = json!({"items": [{"id": true}]});
        let errors = changes(&before, &after);
        assert_eq!(2, errors.len());
        assert!(
            errors
                .iter()
                .any(|e| e.contains(r#"$["items"][*]["id"]"#) && e.contains("number"))
        );
        assert!(
            errors
                .iter()
                .any(|e| e.contains(r#"$["items"][*]["label"] was removed"#))
        );
    }

    #[test]
    fn null_and_containers_differ() {
        let cases = [
            (json!(null), json!("text")),
            (json!([]), json!({})),
            (json!({"x": 1}), json!([])),
            (json!([{"id": 1}]), json!([])),
        ];
        for (before, after) in cases {
            assert!(!changes(&before, &after).is_empty(), "{before} -> {after}");
        }
    }

    #[test]
    fn only_absent_names_skip() {
        let fixture = Fixture::new();
        assert_eq!(Ok(None), fixture.check());
        fixture.catalog(json!({"unused": []}));
        assert!(fixture.check().unwrap_err().contains("consumed"));
    }

    #[test]
    fn consumed_groups_are_guarded() {
        let fixture = Fixture::new();
        fixture.catalog(json!({
            "consumed": ["profile"],
            "boards": ["leaderboard"],
            "challenges": ["challenges"],
            "unused": ["unused"],
            "not_yet": ["future"]
        }));
        for name in ["profile", "leaderboard", "challenges"] {
            write(&fixture.api(), name, &json!({"id": 1}));
            write(&fixture.server, name, &json!({"id": 2, "new": true}));
        }
        write(&fixture.server, "new_endpoint", &json!({"anything": []}));
        assert_eq!(Ok(Some(Vec::new())), fixture.check());

        fs::remove_file(fixture.server.join("profile.json")).unwrap();
        let removed = vec!["Consumed example profile.json was removed".to_owned()];
        assert_eq!(Ok(Some(removed)), fixture.check());
    }

    #[test]
    fn missing_or_invalid_cannot_pass() {
        let fixture = Fixture::new();
        fixture.catalog(json!({"consumed": ["profile"]}));
        let errors = fixture.check().unwrap().unwrap();
        assert!(errors[0].contains("vendored example is missing"));

        fixture.catalog(json!({"consumed": ["../secret"]}));
        assert!(fixture.check().unwrap_err().contains("invalid"));

        fixture.catalog(json!({"consumed": []}));
        fs::write(fixture.android.join("ankiquest-server.sha"), "master").unwrap();
        assert!(fixture.check().unwrap_err().contains("full commit SHA"));
    }

    #[test]
    fn names_are_checked() {
        assert!(valid_name("profile_2"));
        for name in ["", "names", "2fa", "Profile", "a-b", "../x"] {
            assert!(!valid_name(name), "{name}");
        }
    }

    #[test]
    fn flags_are_parsed() {
        let args = ["--android-resources", "res"].map(String::from);
        let parsed = parse(&args).unwrap();
        assert_eq!(PathBuf::from("res"), parsed.resources);
        assert_eq!(PathBuf::from(SERVER_EXAMPLES), parsed.server);
        assert!(parse(&[]).is_err());
    }
}
