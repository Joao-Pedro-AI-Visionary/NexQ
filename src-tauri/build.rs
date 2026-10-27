fn main() {
    // Fix CRT linking: ort_sys and whisper_rs_sys are compiled with /MD (dynamic CRT)
    // but Rust uses /MT (static CRT). We need to swap static CRT for dynamic CRT.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        // Remove static CRT, add dynamic CRT to resolve __imp_* symbols
        println!("cargo:rustc-link-arg=/NODEFAULTLIB:libucrt.lib");
        println!("cargo:rustc-link-arg=/DEFAULTLIB:ucrt.lib");
        println!("cargo:rustc-link-arg=ucrt.lib");
    }

    // Embed one application manifest (Common Controls v6, identical to tauri-build's
    // default) into every link target: the app, unit/integration tests and examples. Without it, test executables that link tauri's dialog code fail to
    // load with STATUS_ENTRYPOINT_NOT_FOUND (comctl32 TaskDialogIndirect needs v6).
    // tauri-build is told not to embed its own copy so the manifests don't collide.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("windows-app-manifest.xml");
        println!("cargo:rerun-if-changed={}", manifest.display());
        // Plain link-arg (not -bins/-tests): cargo only accepts the scoped forms when
        // the package declares such targets, and lib unit tests are not covered by them.
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());
    }

    let windows = tauri_build::WindowsAttributes::new_without_app_manifest();
    tauri_build::try_build(tauri_build::Attributes::new().windows_attributes(windows))
        .expect("failed to run tauri-build");
}
