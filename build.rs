use std::{env, fs, path::Path};

fn main() {
    println!("cargo:rerun-if-changed=linker.ld");
    println!("cargo:rerun-if-changed=initrd");

    // Genera la lista de archivos del initrd: cada archivo de `initrd/` se incrusta en
    // el binario del kernel con include_bytes!. Si la carpeta no existe, queda vacío.
    let root = env::var("CARGO_MANIFEST_DIR").unwrap();
    let dir = Path::new(&root).join("initrd");
    let mut entries: Vec<(String, String)> = Vec::new();
    if let Ok(rd) = fs::read_dir(&dir) {
        for e in rd.flatten() {
            let path = e.path();
            if path.is_file() {
                let name = e.file_name().to_string_lossy().into_owned();
                println!("cargo:rerun-if-changed={}", path.display());
                entries.push((name, path.display().to_string()));
            }
        }
    }
    entries.sort();

    let mut src = String::from("pub static FILES: &[(&str, &[u8])] = &[\n");
    for (name, path) in &entries {
        src.push_str(&format!("    ({:?}, include_bytes!({:?}) as &[u8]),\n", name, path));
    }
    src.push_str("];\n");
    let out = Path::new(&env::var("OUT_DIR").unwrap()).join("initrd.rs");
    fs::write(out, src).unwrap();
}
