use sha2::{Digest, Sha256};
use std::{
    env, fs,
    path::{Path, PathBuf},
};
fn collect(root: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.file_name().unwrap() == "__pycache__" {
            continue;
        }
        if path.is_dir() {
            collect(&path, out);
        } else {
            out.push(path);
        }
    }
}
fn main() {
    let mut inputs = vec![
        PathBuf::from("Cargo.toml"),
        PathBuf::from("Cargo.lock"),
        PathBuf::from("build.rs"),
    ];
    for directory in ["src", "templates", "runtime", "scripts"] {
        collect(Path::new(directory), &mut inputs);
    }
    inputs.sort();
    let mut hash = Sha256::new();
    let mut assets = String::from("pub const FILES: &[(&str, &[u8])] = &[\n");
    for path in inputs {
        println!("cargo:rerun-if-changed={}", path.display());
        let bytes = fs::read(&path).unwrap();
        hash.update(path.to_string_lossy().as_bytes());
        hash.update([0]);
        hash.update(&bytes);
        hash.update([0]);
        if ["templates", "runtime", "scripts"]
            .iter()
            .any(|p| path.starts_with(p))
        {
            assets.push_str(&format!(
                "({:?}, include_bytes!({:?})),\n",
                path.to_string_lossy(),
                fs::canonicalize(&path).unwrap()
            ));
        }
    }
    for directory in ["src", "templates", "runtime", "scripts"] {
        println!("cargo:rerun-if-changed={directory}");
    }
    assets.push_str("];\n");
    assets.push_str(&format!(
        "pub const ENGINE: &str = \"{:x}\";\n",
        hash.finalize()
    ));
    fs::write(
        Path::new(&env::var_os("OUT_DIR").unwrap()).join("assets.rs"),
        assets,
    )
    .unwrap();
}
