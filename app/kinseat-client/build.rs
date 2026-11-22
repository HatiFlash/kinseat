fn main() {
    #[cfg(windows)]
    {
        println!("cargo:rerun-if-changed=assets/kinseat.ico");
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon("assets/kinseat.ico");
        resource.set("FileDescription", "Kinseat Client");
        resource.set("ProductName", "Kinseat Client");
        resource
            .compile()
            .expect("Impossible de générer la ressource Windows");
    }
}
