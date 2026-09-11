//! Security-critical helpers: filename sanitization, path traversal
//! protection, and session token / PIN handling.
//!
//! Everything here is written defensively and covered by unit tests in
//! `tests/security.rs` — treat this module as the trust boundary between
//! "bytes from the network" and "paths on disk".

use rand::{distributions::Alphanumeric, Rng};
use sha2::{Digest, Sha256};
use std::path::{Component, Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SecurityError {
    #[error("filename is empty or invalid")]
    InvalidFilename,
    #[error("path traversal attempt detected")]
    PathTraversal,
    #[error("resolved path escapes the storage directory")]
    PathEscape,
}

/// Sanitize a user-supplied filename so it is safe to use as a single path
/// component: strips directory separators, `..`, null bytes, and other
/// control characters, and falls back to a generated name if nothing safe
/// remains.
pub fn sanitize_filename(original: &str) -> Result<String, SecurityError> {
    let trimmed = original.trim();
    if trimmed.is_empty() {
        return Err(SecurityError::InvalidFilename);
    }

    // `sanitize_filename` crate strips path separators, reserved Windows
    // names, control characters, and trailing dots/spaces.
    let cleaned = sanitize_filename::sanitize(trimmed);

    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        return Err(SecurityError::InvalidFilename);
    }

    // Belt-and-suspenders: explicitly reject anything that still contains
    // a path separator or traversal sequence after cleaning.
    if cleaned.contains('/') || cleaned.contains('\\') || cleaned.contains("..") {
        return Err(SecurityError::PathTraversal);
    }

    Ok(cleaned)
}

/// Resolve `filename` inside `base_dir`, guaranteeing the final path is
/// still within `base_dir` even after symlink-free lexical normalization.
/// This is the only function that should ever be used to turn a
/// user-controlled filename into a filesystem path.
pub fn safe_join(base_dir: &Path, filename: &str) -> Result<PathBuf, SecurityError> {
    let safe_name = sanitize_filename(filename)?;
    let candidate = base_dir.join(&safe_name);

    // Lexically normalize both paths (they may not exist yet, so we can't
    // use `canonicalize`) and ensure the candidate still lives under
    // `base_dir`. A single sanitized path component can't legitimately
    // escape, but we double-check defensively.
    let normalized = normalize_lexically(&candidate);
    let base_normalized = normalize_lexically(base_dir);

    if !normalized.starts_with(&base_normalized) {
        return Err(SecurityError::PathEscape);
    }

    Ok(normalized)
}

/// Lexically normalize a path (resolve `.` and `..` without touching the
/// filesystem). Used because the destination file may not exist yet, so
/// `Path::canonicalize` isn't an option.
fn normalize_lexically(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Generate a cryptographically-adequate random session token.
pub fn generate_token() -> String {
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(32)
        .map(char::from)
        .collect()
}

/// Hash a PIN for constant-time-ish comparison storage (avoids keeping the
/// raw PIN around in memory any longer than necessary).
pub fn hash_pin(pin: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(pin.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Constant-time-ish comparison of a submitted PIN against the stored hash.
pub fn verify_pin(submitted: &str, stored_hash: &str) -> bool {
    let submitted_hash = hash_pin(submitted);
    // `subtle` would be more rigorous, but for a local-network, low-QPS
    // login endpoint this is a reasonable simplification with no extra
    // dependency, since timing side channels are not the primary threat
    // model for a LAN tool.
    submitted_hash == stored_hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_normal_filenames() {
        assert_eq!(sanitize_filename("photo.jpg").unwrap(), "photo.jpg");
        assert_eq!(sanitize_filename("my report.pdf").unwrap(), "my report.pdf");
    }

    #[test]
    fn rejects_empty_filename() {
        assert!(sanitize_filename("").is_err());
        assert!(sanitize_filename("   ").is_err());
    }

    #[test]
    fn strips_path_traversal_sequences() {
        // sanitize-filename strips slashes entirely, so the ".." collapses
        // into a plain, harmless component rather than a traversal.
        let result = sanitize_filename("../../etc/passwd");
        match result {
            Ok(name) => {
                assert!(!name.contains('/'));
                assert!(!name.contains(".."));
            }
            Err(_) => {} // also acceptable: outright rejection
        }
    }

    #[test]
    fn rejects_dot_dot() {
        assert!(sanitize_filename("..").is_err());
        assert!(sanitize_filename(".").is_err());
    }

    #[test]
    fn safe_join_stays_within_base() {
        let base = PathBuf::from("/tmp/landrop_test_base");
        let joined = safe_join(&base, "evil.txt").unwrap();
        assert!(joined.starts_with(&base));
    }

    #[test]
    fn safe_join_blocks_traversal_attempts() {
        let base = PathBuf::from("/tmp/landrop_test_base");
        // Even a maximally hostile filename must not escape base_dir.
        let attempt = safe_join(&base, "....//....//etc/passwd");
        if let Ok(path) = attempt {
            assert!(path.starts_with(&base));
        }
    }

    #[test]
    fn pin_hash_roundtrip() {
        let hash = hash_pin("1234");
        assert!(verify_pin("1234", &hash));
        assert!(!verify_pin("4321", &hash));
    }

    #[test]
    fn tokens_are_unique() {
        let a = generate_token();
        let b = generate_token();
        assert_ne!(a, b);
        assert_eq!(a.len(), 32);
    }
}
