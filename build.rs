use std::{
    env, fs,
    path::{Path, PathBuf},
};

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            collect(&path, out);
        } else {
            out.push(path);
        }
    }
}

fn main() {
    let mut files = Vec::new();
    for dir in ["templates", "runtime", "scaffold"] {
        println!("cargo:rerun-if-changed={dir}");
        collect(Path::new(dir), &mut files);
    }
    files.sort();
    let entries: String = files
        .iter()
        .map(|path| {
            let absolute = fs::canonicalize(path).unwrap();
            // `cargo package` drops directories holding a Cargo.toml, so scaffolds ship it as Cargo.toml.in.
            let key = path.to_str().unwrap().replace("Cargo.toml.in", "Cargo.toml");
            format!("({key:?}, include_bytes!({absolute:?})),\n")
        })
        .collect();
    let out = Path::new(&env::var_os("OUT_DIR").unwrap()).join("assets.rs");
    fs::write(
        out,
        format!("pub const FILES: &[(&str, &[u8])] = &[\n{entries}];\n"),
    )
    .unwrap();
}
