//! Secure secret file I/O — writes with 0600 permissions and tightens existing files.

use std::path::Path;

/// Write secret bytes to a file with mode 0600 (owner-only read/write).
/// On non-Unix platforms, falls back to a normal write.
pub fn write_secret(path: &Path, data: &[u8]) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::fs::OpenOptions;
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;

        let mut f = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        f.write_all(data)?;
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, data)?;
    }
    Ok(())
}

/// If a secret file exists with permissions more open than 0600, tighten them
/// and log a warning. Call this on startup for pre-existing key files.
pub fn tighten_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(path) {
            let mode = meta.permissions().mode() & 0o777;
            if mode != 0o600 {
                tracing::warn!(
                    path = %path.display(),
                    current_mode = format!("{mode:04o}"),
                    "Secret file has overly permissive mode — tightening to 0600"
                );
                let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
            }
        }
    }
}
