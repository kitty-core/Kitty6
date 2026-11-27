//! Fingerprints the shader translator, so that cached translations
//! (`src/shader_cache.rs`) last until the translator itself changes and not
//! only until the next build of anything.

use std::path::{Path, PathBuf};

fn files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            files(&path, out);
        } else {
            out.push(path);
        }
    }
}

/// The `[[package]]` entries of `lock` that `victory-shader` depends on,
/// itself included, directly or through others, sorted by name.
fn translator_lock_entries(lock: &str) -> Vec<String> {
    use std::collections::{BTreeMap, BTreeSet};
    // name -> (entry text, names it depends on)
    let mut packages: BTreeMap<String, (String, Vec<String>)> = BTreeMap::new();
    for block in lock.split("[[package]]").skip(1) {
        let block = block.trim().to_owned();
        let Some(name) = block.lines().find_map(|l| l.strip_prefix("name = ")).map(|n| n.trim_matches('"').to_owned())
        else {
            continue;
        };
        let mut deps = Vec::new();
        if let Some(at) = block.find("dependencies = [") {
            let rest = &block[at + "dependencies = [".len()..];
            let list = &rest[..rest.find(']').unwrap_or(rest.len())];
            // `"name"` or `"name version"`: the name is the first word.
            for dep in list.split(',') {
                if let Some(dep) = dep.trim().trim_matches('"').split(' ').next()
                    && !dep.is_empty()
                {
                    deps.push(dep.to_owned());
                }
            }
        }
        // Several versions of one crate: both are kept under one name.
        let e = packages.entry(name).or_default();
        e.0.push_str(&block);
        e.1.extend(deps);
    }
    let mut seen = BTreeSet::new();
    let mut todo = vec!["victory-shader".to_owned()];
    while let Some(name) = todo.pop() {
        if seen.insert(name.clone())
            && let Some((_, deps)) = packages.get(&name)
        {
            todo.extend(deps.iter().cloned());
        }
    }
    seen.iter().filter_map(|n| packages.get(n).map(|p| p.0.clone())).collect()
}

fn main() {
    let here = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap_or_default());
    let crates = here.join("..");
    let root = crates.join("..");
    // The translator, what it is built from, and the form entries are kept in.
    let mut all = vec![
        crates.join("victory-shader/Cargo.toml"),
        here.join("src/shader_cache.rs"),
        root.join("rust-toolchain.toml"),
    ];
    let source = crates.join("victory-shader/src");
    println!("cargo:rerun-if-changed={}", source.display());
    files(&source, &mut all);
    all.sort();
    let mut hash = blake3::Hasher::new();
    // Of Cargo.lock, only what the translator is built from (Kitty6): the
    // whole file made every translation stale whenever any crate of the
    // workspace took a new dependency, and with it the driver's pipelines
    // (a 20-minute `victory warm` for a change outside the translator).
    let lock = root.join("Cargo.lock");
    println!("cargo:rerun-if-changed={}", lock.display());
    let lock = std::fs::read_to_string(&lock).unwrap_or_default().replace('\r', "");
    for entry in translator_lock_entries(&lock) {
        hash.update(entry.as_bytes());
    }
    for path in &all {
        println!("cargo:rerun-if-changed={}", path.display());
        // Names and line endings as they are in every checkout.
        let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let bytes: Vec<u8> = std::fs::read(path).unwrap_or_default().into_iter().filter(|&b| b != 13).collect();
        hash.update(name.as_bytes());
        hash.update(&(bytes.len() as u64).to_le_bytes());
        hash.update(&bytes);
    }
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rustc-env=VICTORY_TRANSLATOR={}", hash.finalize().to_hex());
}
