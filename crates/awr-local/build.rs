use std::{env, fs, path::Path};

fn collect(dir: &Path, files: &mut Vec<std::path::PathBuf>) {
    for entry in fs::read_dir(dir).expect("read progress assets") {
        let path = entry.expect("asset entry").path();
        if path.is_dir() {
            collect(&path, files);
        } else if path
            .extension()
            .is_some_and(|ext| matches!(ext.to_str(), Some("html" | "css" | "js")))
        {
            files.push(path);
        }
    }
}

fn main() {
    let root = Path::new("../../tools/project-progress-overview/public");
    println!("cargo:rerun-if-changed={}", root.display());
    let mut files = Vec::new();
    collect(root, &mut files);
    files.sort();
    let mut source = String::from("pub const ASSETS: &[(&str, &[u8])] = &[\n");
    for file in files {
        let name = file
            .strip_prefix(root)
            .unwrap()
            .to_str()
            .unwrap()
            .replace('\\', "/");
        let path = file.canonicalize().unwrap();
        source.push_str(&format!("({name:?}, include_bytes!({path:?})),\n"));
    }
    source.push_str("];\n");
    fs::write(
        Path::new(&env::var_os("OUT_DIR").unwrap()).join("assets.rs"),
        source,
    )
    .expect("write embedded progress assets");
}
