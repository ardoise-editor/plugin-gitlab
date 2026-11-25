//! Files of the plugin: where its data is, and writing them whole.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

static DATA: OnceLock<PathBuf> = OnceLock::new();

/// Where the plugin keeps its data (given by the editor at the start).
pub fn set_data_dir(dir: PathBuf) {
    let _ = DATA.set(dir);
}

/// The folder of the plugin's data.
pub fn data_dir() -> Option<PathBuf> {
    #[cfg(test)]
    {
        Some(test_dir())
    }
    #[cfg(not(test))]
    {
        DATA.get().cloned()
    }
}

#[cfg(test)]
fn test_dir() -> PathBuf {
    static DIR: OnceLock<tempfile::TempDir> = OnceLock::new();
    DIR.get_or_init(|| tempfile::tempdir().expect("a folder for the tests"))
        .path()
        .to_path_buf()
}

/// Writes a file whole (a temporary file renamed over it); `private`: only
/// its owner may read it.
pub fn write_atomic(path: &Path, data: &[u8], private: bool) -> std::io::Result<()> {
    use std::io::Write;
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    if private {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| {
        let mut file = options.open(&tmp)?;
        file.write_all(data)?;
        file.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}
