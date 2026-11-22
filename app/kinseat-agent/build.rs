fn main() {
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        embed_manifest::embed_manifest(embed_manifest::new_manifest("Kinseat.Agent"))
            .expect("impossible d’embarquer le manifeste Windows");
    }
    println!("cargo:rerun-if-changed=build.rs");
}
