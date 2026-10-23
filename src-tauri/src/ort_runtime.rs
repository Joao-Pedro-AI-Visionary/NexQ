//! Lazy loading of the bundled ONNX Runtime shared library.
//!
//! `ort` is built with `load-dynamic`: no ONNX Runtime code is linked into the
//! executable. The official Microsoft library (staged by
//! `scripts/fetch-onnxruntime.mjs` and bundled next to the app) is loaded the
//! first time a local ONNX model is used. The previously linked static build
//! contained BMI2/AVX2 code that crashed the app at launch on CPUs without
//! those extensions (issue #6).
//!
//! Platform library names:
//!   - Windows:  `onnxruntime.dll` (bundled next to the executable)
//!   - macOS:    `libonnxruntime.dylib` (bundled in `Contents/Resources`)

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Absolute candidate paths to the ONNX Runtime shared library, in priority order.
fn library_candidates(exe_dir: &Path) -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    // Dev convenience: the copy staged into `src-tauri/ort/` by the fetch script.
    let staged = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ort");

    #[cfg(target_os = "windows")]
    {
        // Absolute paths on purpose: Windows 11 ships an older onnxruntime.dll in System32.
        candidates.push(exe_dir.join("onnxruntime.dll"));
        candidates.push(staged.join("onnxruntime.dll"));
    }

    #[cfg(target_os = "macos")]
    {
        // Bundled .app layout: Contents/MacOS/<exe> → Contents/Resources/libonnxruntime.dylib
        candidates.push(exe_dir.join("../Resources/libonnxruntime.dylib"));
        // Fallbacks: next to the executable, then the dev-staged copy.
        candidates.push(exe_dir.join("libonnxruntime.dylib"));
        candidates.push(staged.join("libonnxruntime.dylib"));
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        candidates.push(exe_dir.join("libonnxruntime.so"));
        candidates.push(staged.join("libonnxruntime.so"));
    }

    candidates
}

/// Load the bundled ONNX Runtime library. Must be called before any other `ort`
/// API. Safe to call repeatedly; the outcome of the first call is cached.
pub fn ensure_loaded() -> Result<(), String> {
    static INIT: OnceLock<Result<(), String>> = OnceLock::new();
    INIT.get_or_init(|| {
        let exe = std::env::current_exe()
            .map_err(|e| format!("Cannot locate executable: {}", e))?;
        let exe_dir = exe
            .parent()
            .ok_or("Executable has no parent directory")?;

        let candidates = library_candidates(exe_dir);
        let library = candidates
            .iter()
            .find(|p| p.exists())
            .cloned()
            .ok_or_else(|| {
                let searched = candidates
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                format!(
                    "ONNX Runtime not found. Looked in: {} — reinstall NexQ to restore it",
                    searched
                )
            })?;

        ort::init_from(&library)
            .map_err(|e| format!("Failed to load ONNX Runtime from {}: {}", library.display(), e))?
            .commit();
        log::info!("ONNX Runtime loaded from {}", library.display());
        Ok(())
    })
    .clone()
}
