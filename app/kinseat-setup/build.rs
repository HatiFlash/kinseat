use std::{env, path::PathBuf, process::Command};
fn main() {
    // Charge utile Hôte embarquée : un seul Kinseat-Host-Setup-x.y.z.exe, plus de zip à côté.
    println!("cargo:rerun-if-env-changed=KINSEAT_HOST_PAYLOAD_ZIP");
    let (payload, sha) = match env::var_os("KINSEAT_HOST_PAYLOAD_ZIP").map(PathBuf::from) {
        Some(path) => {
            println!("cargo:rerun-if-changed={}", path.display());
            let bytes = std::fs::read(&path).expect("KINSEAT_HOST_PAYLOAD_ZIP illisible");
            (
                format!(
                    "Some(include_bytes!({:?}).as_slice())",
                    path.to_string_lossy()
                ),
                format!("{:x}", <sha2::Sha256 as sha2::Digest>::digest(&bytes)),
            )
        }
        None => ("None".to_owned(), String::new()),
    };
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    std::fs::write(
        out_dir.join("host_payload.rs"),
        format!("pub const HOST_PAYLOAD: Option<&[u8]> = {payload};\npub const HOST_PAYLOAD_SHA256: &str = {sha:?};\n"),
    )
    .expect("host_payload.rs");
    println!("cargo:rerun-if-changed=setup.manifest");
    println!("cargo:rerun-if-changed=setup.rc");
    let target = env::var("TARGET").expect("TARGET");
    if !target.contains("windows") {
        return;
    }
    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    // Logo Kinseat sur Kinseat-Host-Setup.exe (le manifeste reste embarqué ci-dessous).
    println!("cargo:rerun-if-changed=../kinseat/assets/kinseat.ico");
    let mut resource = winresource::WindowsResource::new();
    resource.set_icon("../kinseat/assets/kinseat.ico");
    resource.set("FileDescription", "Installation de Kinseat Hôte");
    resource.set("ProductName", "Kinseat Hôte");
    resource
        .compile()
        .expect("Impossible de générer la ressource icône");
    if target.contains("msvc") {
        // Le linker MSVC embarque lui-même le manifeste : pas besoin de rc.exe dans le PATH.
        let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"))
            .join("setup.manifest");
        println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}",
            manifest.display()
        );
    } else {
        let obj = out.join("setup.o");
        let status = Command::new("windres")
            .args(["-i", "setup.rc", "-o"])
            .arg(&obj)
            .status()
            .expect("windres requis pour embarquer le manifeste");
        assert!(status.success(), "échec windres : manifeste non embarqué");
        println!("cargo:rustc-link-arg={}", obj.display());
    }
}
