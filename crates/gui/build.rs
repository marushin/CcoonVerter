//! Embeds the Windows icon and version resource into `CcooNverter.exe`.

fn main() {
    println!("cargo:rerun-if-changed=../../assets/icon.ico");
    println!("cargo:rerun-if-changed=build.rs");

    #[cfg(target_os = "windows")]
    {
        // Only meaningful when the *target* is Windows (build scripts run on the host).
        if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
            let version = env!("CARGO_PKG_VERSION");
            let mut res = winresource::WindowsResource::new();
            res.set_icon("../../assets/icon.ico")
                .set("ProductName", "CcooNverter")
                .set(
                    "FileDescription",
                    "CcooNverter - RAW to JPG batch converter",
                )
                .set("CompanyName", "ccoon")
                .set("LegalCopyright", "MIT License")
                .set("OriginalFilename", "CcooNverter.exe")
                .set("InternalName", "CcooNverter")
                .set("ProductVersion", version)
                .set("FileVersion", version);
            if let Err(e) = res.compile() {
                // A missing resource compiler must not break the build; the exe just has no icon.
                println!("cargo:warning=winresource failed: {e}");
            }
        }
    }
}
