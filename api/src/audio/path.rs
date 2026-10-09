use std::path::PathBuf;

/// Resolve a stored file_path against a storage root. Absolute paths are
/// returned unchanged; relative paths are joined onto `storage_root`.
pub fn resolve_file_path(storage_root: &str, file_path: &str) -> PathBuf {
    let path = PathBuf::from(file_path);
    if path.is_absolute() {
        path
    } else {
        PathBuf::from(storage_root).join(file_path)
    }
}
