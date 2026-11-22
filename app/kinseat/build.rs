fn main() {
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        // Console du foyer : ouverture sans élévation ; le service n’accepte que les administrateurs
        // élevés ou les comptes administrateurs autorisés une fois (UAC au premier lancement).
        embed_manifest::embed_manifest(
            embed_manifest::new_manifest("Kinseat.Application")
                .requested_execution_level(embed_manifest::manifest::ExecutionLevel::AsInvoker),
        )
        .expect("impossible d’embarquer le manifeste Windows");
        // Logo Kinseat sur l'exécutable (et donc sur les raccourcis Bureau / menu Démarrer).
        println!("cargo:rerun-if-changed=assets/kinseat.ico");
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon("assets/kinseat.ico");
        resource.set("FileDescription", "Kinseat Hôte");
        resource.set("ProductName", "Kinseat Hôte");
        resource
            .compile()
            .expect("Impossible de générer la ressource Windows");
    }
    println!("cargo:rerun-if-changed=build.rs");
}
