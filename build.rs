//! Builds the web pages into `OUT_DIR/web`, where the server embeds them:
//! TypeScript from `web/` through esbuild, the translation module from
//! `crates/i18n-wasm` as WebAssembly, and each page in `static/` with its
//! `<script data-entry="…"></script>` placeholders replaced by the compiled code.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Scripts served as files, and the page scripts inlined into their HTML.
const ENTRIES: [&str; 6] = [
    "site",
    "avatars",
    "personal",
    "pages/index",
    "pages/community",
    "pages/login",
];
const PAGES: [&str; 5] = ["index", "community", "login", "personal", "privacy"];

type Error = Box<dyn std::error::Error>;

fn main() -> Result<(), Error> {
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR")?);
    let out = PathBuf::from(std::env::var("OUT_DIR")?).join("web");
    std::fs::create_dir_all(&out)?;
    for watched in [
        "web",
        "static",
        "crates/i18n",
        "crates/i18n-wasm",
        "Cargo.lock",
    ] {
        println!("cargo::rerun-if-changed={watched}");
    }
    println!("cargo::rerun-if-env-changed=ESBUILD");

    typescript(&root, &out)?;
    wasm(&root, &out)?;
    for page in PAGES {
        let source = std::fs::read_to_string(root.join(format!("static/{page}.html")))?;
        std::fs::write(out.join(format!("{page}.html")), inline(&source, &out)?)?;
    }
    Ok(())
}

fn esbuild(root: &Path) -> Result<Command, Error> {
    if let Some(path) = std::env::var_os("ESBUILD") {
        return Ok(Command::new(path));
    }
    let local = root.join("node_modules/.bin").join(if cfg!(windows) {
        "esbuild.cmd"
    } else {
        "esbuild"
    });
    if local.exists() {
        return Ok(Command::new(local));
    }
    match Command::new("esbuild").arg("--version").output() {
        Ok(_) => Ok(Command::new("esbuild")),
        Err(_) => {
            Err("esbuild not found: run `npm ci`, put esbuild on PATH, or set ESBUILD".into())
        }
    }
}

fn typescript(root: &Path, out: &Path) -> Result<(), Error> {
    let status = esbuild(root)?
        .current_dir(root)
        .args(ENTRIES.map(|entry| format!("web/{entry}.ts")))
        .args([
            "--bundle",
            "--format=iife",
            "--target=es2022",
            "--charset=utf8",
            "--log-level=warning",
            "--outbase=web",
        ])
        .arg(format!("--outdir={}", out.display()))
        .status()?;
    if !status.success() {
        return Err("esbuild failed".into());
    }
    Ok(())
}

fn wasm(root: &Path, out: &Path) -> Result<(), Error> {
    let target_dir = out.parent().unwrap().join("wasm-target");
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut command = Command::new(cargo);
    command
        .current_dir(root)
        .args([
            "build",
            "--package=ankiquest-i18n-wasm",
            "--target=wasm32-unknown-unknown",
            "--profile=wasm",
        ])
        .arg("--target-dir")
        .arg(&target_dir);
    // Flags meant for the server's own target must not reach the wasm build.
    for variable in [
        "CARGO_ENCODED_RUSTFLAGS",
        "RUSTFLAGS",
        "CARGO_BUILD_TARGET",
        "CARGO_TARGET_DIR",
        "CARGO_BUILD_RUSTFLAGS",
        "RUSTC_WORKSPACE_WRAPPER",
    ] {
        command.env_remove(variable);
    }
    let status = command.status()?;
    if !status.success() {
        return Err(
            "building the translation module failed; is the wasm32-unknown-unknown target installed? \
             (rustup target add wasm32-unknown-unknown)"
                .into(),
        );
    }
    std::fs::copy(
        target_dir.join("wasm32-unknown-unknown/wasm/ankiquest_i18n_wasm.wasm"),
        out.join("i18n.wasm"),
    )?;
    Ok(())
}

/// Replaces each `<script data-entry="name"></script>` with the compiled entry.
fn inline(page: &str, out: &Path) -> Result<String, Error> {
    const OPEN: &str = "<script data-entry=\"";
    const CLOSE: &str = "\"></script>";
    let mut result = String::with_capacity(page.len());
    let mut rest = page;
    while let Some(start) = rest.find(OPEN) {
        result += &rest[..start];
        let after = &rest[start + OPEN.len()..];
        let end = after.find(CLOSE).ok_or("unterminated script placeholder")?;
        let entry = &after[..end];
        if !ENTRIES.contains(&entry) {
            return Err(format!("no web entry {entry}").into());
        }
        let code = std::fs::read_to_string(out.join(format!("{entry}.js")))?;
        if code.contains("</script") {
            return Err(format!("{entry} cannot be inlined: it contains </script").into());
        }
        result += "<script>\n";
        result += &code;
        result += "</script>";
        rest = &after[end + CLOSE.len()..];
    }
    result += rest;
    Ok(result)
}
