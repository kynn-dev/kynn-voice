//! Download and cache management for DeepFilterNet ONNX model files
//! and ONNX Runtime shared library.

#[cfg(feature = "deepfilter")]
use std::path::Path;
use std::path::PathBuf;
#[cfg(feature = "deepfilter")]
use std::time::Duration;

use anyhow::{Result, anyhow};

#[cfg(feature = "deepfilter")]
const DEEPFILTER_MODEL_URL: &str = "https://github.com/Rikorose/DeepFilterNet/raw/84d57ec2c08fe08e68a13fb32a58cd7092060a0f/models/DeepFilterNet3_onnx.tar.gz";

#[cfg(feature = "deepfilter")]
const DEEPFILTER_SHA256: &str = "c94d91f70911001c946e0fabb4aa9adc37045f45a03b56008cb0c8244cb63616";

#[cfg(feature = "deepfilter")]
const DEEPFILTER_FILES: [(&str, &str); 4] = [
    (
        "enc.onnx",
        "7c5399d3da8a50ebef1c1a0ae421b33376aa5e45d0e92df16da7e83c9c131916",
    ),
    (
        "erb_dec.onnx",
        "ab669a1d10afe20911728b33053a452071042317a90581092b325da7b2f9d895",
    ),
    (
        "df_dec.onnx",
        "23114ce3b0f6464b763ee62f7bb8aab6b2a129a21eabd5bcfe59413db05f278a",
    ),
    (
        "config.ini",
        "415eb925d44990d938fb739f514aa3662c1ec0ea836cff044fa1291b82cb4290",
    ),
];

const MODELS_DIR: &str = "models";

pub fn deepfilter_model_dir() -> PathBuf {
    model_base_dir().join("deepfilter3")
}

pub fn is_deepfilter_available() -> bool {
    let dir = deepfilter_model_dir();
    #[cfg(feature = "deepfilter")]
    {
        DEEPFILTER_FILES
            .iter()
            .all(|(name, expected)| verify_file_sha256(&dir.join(name), expected).is_ok())
    }
    #[cfg(not(feature = "deepfilter"))]
    {
        let _ = dir;
        false
    }
}

#[cfg(feature = "deepfilter")]
pub fn ensure_deepfilter_model() -> Result<PathBuf> {
    let model_dir = deepfilter_model_dir();

    if is_deepfilter_available() {
        tracing::info!(
            "DeepFilterNet model already cached at {}",
            model_dir.display()
        );
        return Ok(model_dir);
    }

    tracing::info!("Downloading DeepFilterNet model from {DEEPFILTER_MODEL_URL}");

    let response = download_agent()
        .get(DEEPFILTER_MODEL_URL)
        .call()
        .map_err(|e| anyhow!("Failed to download DeepFilterNet model: {e}"))?;

    let bytes = response
        .into_body()
        .with_config()
        .limit(50 * 1024 * 1024) // 50 MB — model tar.gz is ~8 MB
        .read_to_vec()
        .map_err(|e| anyhow!("Failed to read model download: {e}"))?;

    verify_sha256(&bytes, DEEPFILTER_SHA256)?;

    std::fs::create_dir_all(&model_dir)?;
    extract_tar_gz(&bytes, &model_dir)?;

    if !is_deepfilter_available() {
        return Err(anyhow!(
            "Model extraction succeeded but expected files not found in {}",
            model_dir.display()
        ));
    }

    tracing::info!("DeepFilterNet model installed to {}", model_dir.display());
    Ok(model_dir)
}

#[cfg(feature = "deepfilter")]
fn download_agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(180)))
        .build()
        .into()
}

fn model_base_dir() -> PathBuf {
    crate::config::Config::path()
        .parent()
        .map(|p| p.join(MODELS_DIR))
        .unwrap_or_else(|| PathBuf::from(MODELS_DIR))
}

#[cfg(feature = "deepfilter")]
fn verify_sha256(data: &[u8], expected: &str) -> Result<()> {
    use sha2::{Digest, Sha256};
    let hash = Sha256::digest(data);
    let hex = format!("{hash:x}");
    if hex != expected {
        return Err(anyhow!("SHA256 mismatch: expected {expected}, got {hex}"));
    }
    Ok(())
}

#[cfg(feature = "deepfilter")]
fn verify_file_sha256(path: &Path, expected: &str) -> Result<()> {
    let bytes = std::fs::read(path).map_err(|e| {
        anyhow!(
            "Failed to read {} for SHA256 verification: {e}",
            path.display()
        )
    })?;
    verify_sha256(&bytes, expected)
}

#[cfg(feature = "deepfilter")]
fn extract_tar_gz(data: &[u8], dest: &Path) -> Result<()> {
    let gz = flate2::read::GzDecoder::new(data);
    let mut archive = tar::Archive::new(gz);
    archive.set_preserve_permissions(false);

    for entry in archive.entries()? {
        let mut entry = entry?;
        // Only extract regular files — skip symlinks, hardlinks, directories
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let path = entry.path()?;
        if let Some(file_name) = path.file_name() {
            let target = dest.join(file_name);
            let mut file = std::fs::File::create(&target)?;
            std::io::copy(&mut entry, &mut file)?;
        }
    }
    Ok(())
}

// ── ONNX Runtime shared library management ─────────────────────────────

/// Expected DLL/dylib filename per platform.
#[cfg(all(feature = "deepfilter", target_os = "windows"))]
const ORT_LIB_NAME: &str = "onnxruntime.dll";
#[cfg(all(feature = "deepfilter", target_os = "macos"))]
const ORT_LIB_NAME: &str = "libonnxruntime.dylib";
#[cfg(all(feature = "deepfilter", target_os = "linux"))]
const ORT_LIB_NAME: &str = "libonnxruntime.so";

#[cfg(all(feature = "deepfilter", target_os = "windows", target_arch = "x86_64"))]
const ORT_VERSION: &str = "1.30.0";
#[cfg(all(feature = "deepfilter", target_os = "windows", target_arch = "x86_64"))]
const ORT_ARCHIVE_URL: &str = "https://github.com/microsoft/onnxruntime/releases/download/v1.30.0/onnxruntime-win-x64-1.30.0.zip";
#[cfg(all(feature = "deepfilter", target_os = "windows", target_arch = "x86_64"))]
const ORT_ARCHIVE_SHA256: &str = "c6ba983baf5681af108599675d2a89c2d145512d02de28aed0bff177cd0ba949";
#[cfg(all(feature = "deepfilter", target_os = "windows", target_arch = "x86_64"))]
const ORT_DLL_SHA256: &str = "7e39e2bdbba836d98071ef28620735ba36a47c554cf794585269aecc50fab0da";
#[cfg(all(feature = "deepfilter", target_os = "windows", target_arch = "x86_64"))]
const ORT_SHARED_DLL_NAME: &str = "onnxruntime_providers_shared.dll";
#[cfg(all(feature = "deepfilter", target_os = "windows", target_arch = "x86_64"))]
const ORT_SHARED_DLL_SHA256: &str =
    "b9b7ab9e2a8b08ee7ae4a7ac1c8bfd44a741f17ad8d0f764953140a57de0b796";

/// Directory where ONNX Runtime library is cached.
#[cfg(feature = "deepfilter")]
fn ort_lib_dir() -> PathBuf {
    model_base_dir().join("ort")
}

/// Full path to the ONNX Runtime shared library.
#[cfg(feature = "deepfilter")]
pub fn ort_lib_path() -> PathBuf {
    ort_lib_dir().join(ORT_LIB_NAME)
}

/// Check that a bundled or cached Windows x64 runtime matches both pinned DLL digests.
#[cfg(all(feature = "deepfilter", target_os = "windows", target_arch = "x86_64"))]
pub fn is_ort_available() -> bool {
    bundled_ort_path().is_some_and(|path| path.parent().is_some_and(ort_files_match_hashes))
        || ort_files_match_hashes(&ort_lib_dir())
}

#[cfg(all(
    feature = "deepfilter",
    not(all(target_os = "windows", target_arch = "x86_64"))
))]
pub fn is_ort_available() -> bool {
    false
}

/// Download and extract the pinned CPU ONNX Runtime build for Windows x64.
#[cfg(all(feature = "deepfilter", target_os = "windows", target_arch = "x86_64"))]
pub fn ensure_onnxruntime() -> Result<PathBuf> {
    let lib_path = ort_lib_path();

    if let Some(path) = bundled_ort_path() {
        if path.parent().is_some_and(ort_files_match_hashes) {
            tracing::info!("Verified bundled ONNX Runtime at {}", path.display());
            return Ok(path);
        }
    }

    if ort_files_match_hashes(&ort_lib_dir()) {
        tracing::info!("Verified ONNX Runtime at {}", lib_path.display());
        return Ok(lib_path);
    }
    if lib_path.exists() || ort_lib_dir().join(ORT_SHARED_DLL_NAME).exists() {
        tracing::warn!("Cached ONNX Runtime failed SHA256 verification; replacing it");
    }

    tracing::info!("Downloading pinned CPU ONNX Runtime {ORT_VERSION}");

    let response = download_agent()
        .get(ORT_ARCHIVE_URL)
        .call()
        .map_err(|e| anyhow!("Failed to download ONNX Runtime: {e}"))?;

    let bytes = response
        .into_body()
        .with_config()
        .limit(100 * 1024 * 1024)
        .read_to_vec()
        .map_err(|e| anyhow!("Failed to read ONNX Runtime download: {e}"))?;

    verify_sha256(&bytes, ORT_ARCHIVE_SHA256)?;

    let dest = ort_lib_dir();
    std::fs::create_dir_all(&dest)?;

    extract_zip_lib(&bytes, &dest)?;

    if !lib_path.exists() || !dest.join(ORT_SHARED_DLL_NAME).exists() {
        return Err(anyhow!(
            "ONNX Runtime extraction succeeded but required runtime DLLs were not found"
        ));
    }

    verify_file_sha256(&lib_path, ORT_DLL_SHA256)?;
    verify_file_sha256(&dest.join(ORT_SHARED_DLL_NAME), ORT_SHARED_DLL_SHA256)?;

    tracing::info!("ONNX Runtime installed to {}", lib_path.display());
    Ok(lib_path)
}

#[cfg(all(feature = "deepfilter", target_os = "windows", target_arch = "x86_64"))]
fn bundled_ort_path() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()?
        .parent()
        .map(|directory| directory.join(ORT_LIB_NAME))
}

#[cfg(all(feature = "deepfilter", target_os = "windows", target_arch = "x86_64"))]
fn ort_files_match_hashes(dir: &Path) -> bool {
    verify_file_sha256(&dir.join(ORT_LIB_NAME), ORT_DLL_SHA256).is_ok()
        && verify_file_sha256(&dir.join(ORT_SHARED_DLL_NAME), ORT_SHARED_DLL_SHA256).is_ok()
}

#[cfg(all(
    feature = "deepfilter",
    not(all(target_os = "windows", target_arch = "x86_64"))
))]
pub fn ensure_onnxruntime() -> Result<PathBuf> {
    Err(anyhow!(
        "DeepFilterNet runtime assets are currently pinned and verified only for Windows x64"
    ))
}

/// Extract the shared library from the ONNX Runtime zip archive.
/// The pinned ZIP contains the runtime and shared provider DLL under its `lib/` directory.
#[cfg(all(feature = "deepfilter", target_os = "windows", target_arch = "x86_64"))]
fn extract_zip_lib(data: &[u8], dest: &Path) -> Result<()> {
    use std::io::Cursor;

    let reader = Cursor::new(data);
    let mut archive =
        zip::ZipArchive::new(reader).map_err(|e| anyhow!("Failed to open zip: {e}"))?;

    let mut found_runtime = false;
    let mut found_shared = false;
    for i in 0..archive.len() {
        let mut file = archive
            .by_index(i)
            .map_err(|e| anyhow!("Failed to read zip entry: {e}"))?;

        let name = file.name().to_string();

        // Look for the shared library in the lib/ directory
        if name.ends_with(ORT_LIB_NAME) || name.ends_with(ORT_SHARED_DLL_NAME) {
            let file_name = if name.ends_with(ORT_LIB_NAME) {
                found_runtime = true;
                ORT_LIB_NAME
            } else {
                found_shared = true;
                ORT_SHARED_DLL_NAME
            };
            let target = dest.join(file_name);
            let mut out = std::fs::File::create(&target)?;
            std::io::copy(&mut file, &mut out)?;
            tracing::debug!("Extracted {name} → {}", target.display());
        }
    }

    if found_runtime && found_shared {
        Ok(())
    } else {
        Err(anyhow!(
            "ONNX Runtime archive is missing required runtime DLLs"
        ))
    }
}

/// Initialize the ort crate with our cached ONNX Runtime library.
/// Must be called before any ort Session is created.
/// Idempotent — safe to call multiple times (only the first call initializes).
#[cfg(feature = "deepfilter")]
pub fn init_ort() -> Result<()> {
    static ORT_INIT: std::sync::Mutex<bool> = std::sync::Mutex::new(false);
    let mut initialized = ORT_INIT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if *initialized {
        return Ok(());
    }

    let lib_path = ensure_onnxruntime()?;
    ort::init_from(lib_path.to_string_lossy().to_string())
        .commit()
        .map_err(|error| anyhow!("ONNX Runtime initialization failed: {error}"))?;
    *initialized = true;
    tracing::info!("ONNX Runtime initialized");
    Ok(())
}

#[cfg(not(feature = "deepfilter"))]
pub fn init_ort() -> Result<()> {
    Ok(())
}

#[cfg(not(feature = "deepfilter"))]
pub fn ensure_deepfilter_model() -> Result<PathBuf> {
    Err(anyhow!(
        "DeepFilterNet support not compiled in. Rebuild with --features deepfilter"
    ))
}

#[cfg(not(feature = "deepfilter"))]
pub fn is_ort_available() -> bool {
    false
}
