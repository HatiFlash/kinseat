use sha2::{Digest, Sha256};
use std::{env, fs, path::PathBuf};
fn main() {
    println!("cargo:rerun-if-env-changed=KINSEAT_CLIENT_EXE");
    let payload = env::var_os("KINSEAT_CLIENT_EXE").map(PathBuf::from);
    let (source, expected) = if let Some(path) = payload {
        println!("cargo:rerun-if-changed={}", path.display());
        let bytes = fs::read(&path).expect("KINSEAT_CLIENT_EXE illisible");
        (
            format!(
                "Some(include_bytes!({:?}).as_slice())",
                path.to_string_lossy()
            ),
            format!("{:x}", Sha256::digest(&bytes)),
        )
    } else {
        ("None".to_owned(), String::new())
    };
    // Moteur de streaming facultatif (archive Moonlight portable, GPLv3), extrait dans moonlight\\.
    println!("cargo:rerun-if-env-changed=KINSEAT_MOONLIGHT_ZIP");
    let (engine, engine_sha) = match env::var_os("KINSEAT_MOONLIGHT_ZIP").map(PathBuf::from) {
        Some(path) => {
            println!("cargo:rerun-if-changed={}", path.display());
            let bytes = fs::read(&path).expect("KINSEAT_MOONLIGHT_ZIP illisible");
            (
                format!(
                    "Some(include_bytes!({:?}).as_slice())",
                    path.to_string_lossy()
                ),
                format!("{:x}", Sha256::digest(&bytes)),
            )
        }
        None => ("None".to_owned(), String::new()),
    };
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("payload.rs"),
        format!(
            "const PAYLOAD: Option<&[u8]> = {source};\nconst EXPECTED_SHA256: &str = {expected:?};\n\
             #[allow(dead_code)]\nconst ENGINE_ZIP: Option<&[u8]> = {engine};\n\
             #[allow(dead_code)]\nconst ENGINE_SHA256: &str = {engine_sha:?};\n"
        ),
    )
    .unwrap();
    #[cfg(windows)]
    {
        let mut r = winresource::WindowsResource::new();
        r.set_icon("../kinseat-client/assets/kinseat.ico");
        r.set("FileDescription", "Installation Kinseat Client");
        r.set("ProductName", "Kinseat Client");
        r.compile().expect("Ressource Windows indisponible");
    }
}
