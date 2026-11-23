fn main() {
    #[cfg(windows)]
    {
        let icon = "assets/app.ico";
        let manifest = "assets/app.manifest";
        println!("cargo:rerun-if-changed={icon}");
        println!("cargo:rerun-if-changed={manifest}");
        winresource::WindowsResource::new()
            .set_icon(icon)
            .set_manifest_file(manifest)
            .set("FileDescription", "Связь")
            .set("ProductName", "Связь")
            .set("OriginalFilename", "Связь.exe")
            .compile()
            .expect("Windows executable icon, manifest and version");
    }
}
