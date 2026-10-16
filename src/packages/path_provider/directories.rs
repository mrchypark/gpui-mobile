use std::path::PathBuf;

pub(super) fn temporary_directory(cache: PathBuf) -> Result<PathBuf, String> {
    let path = cache.join("tmp");
    std::fs::create_dir_all(&path).map_err(|e| format!("Create temporary directory: {e}"))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn temporary_directory_exists_preserves_files_and_reports_obstructions() {
        struct Scratch(PathBuf);
        impl Drop for Scratch {
            fn drop(&mut self) {
                std::fs::remove_dir_all(&self.0).unwrap();
            }
        }
        let root = std::env::temp_dir().join(format!(
            "gpui-path-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let scratch = Scratch(root);
        let cache = scratch.0.join("native-cache");
        let path = temporary_directory(cache.clone()).unwrap();
        assert!(path.is_dir());
        let sentinel = path.join("existing");
        std::fs::write(&sentinel, b"preserve").unwrap();
        assert_eq!(temporary_directory(cache).unwrap(), path);
        assert_eq!(std::fs::read(sentinel).unwrap(), b"preserve");
        let blocked = scratch.0.join("blocked");
        std::fs::create_dir(&blocked).unwrap();
        std::fs::write(blocked.join("tmp"), b"do not replace").unwrap();
        assert!(temporary_directory(blocked.clone()).is_err());
        assert_eq!(
            std::fs::read(blocked.join("tmp")).unwrap(),
            b"do not replace"
        );
        let cleanup = scratch.0.clone();
        drop(scratch);
        assert!(!cleanup.exists());
    }
}
