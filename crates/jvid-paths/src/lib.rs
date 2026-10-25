//! Installed builds retain the historical platform directories. A `jvid.portable` file
//! beside the executable selects `jvid-data` at the package root, independent of cwd.

use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

/// The portable package root, when explicitly marked. Never falls back on a write error.
pub fn portable_root() -> Option<PathBuf> {
    portable_root_for(&std::env::current_exe().ok()?)
}

fn portable_root_for(exe: &Path) -> Option<PathBuf> {
    let bin = exe.parent()?;
    if !bin.join("jvid.portable").is_file() {
        return None;
    }
    // Keep macOS data outside the signed .app, and Linux data outside bin/.
    let root = if bin.file_name()? == "MacOS" && bin.parent()?.file_name()? == "Contents" {
        bin.parent()?.parent()?.parent()?
    } else if bin.file_name()? == "bin" {
        bin.parent()?
    } else {
        bin
    };
    Some(root.to_owned())
}

fn platform() -> Option<directories::ProjectDirs> {
    directories::ProjectDirs::from("", "", "jvid")
}

pub fn config_dir() -> Option<PathBuf> {
    portable_root().map_or_else(
        || platform().map(|d| d.config_dir().to_owned()),
        |r| Some(r.join("jvid-data").join("config")),
    )
}

pub fn data_dir() -> Option<PathBuf> {
    portable_root().map_or_else(
        || platform().map(|d| d.data_dir().to_owned()),
        |r| Some(r.join("jvid-data").join("data")),
    )
}

pub fn data_local_dir() -> Option<PathBuf> {
    portable_root().map_or_else(
        || platform().map(|d| d.data_local_dir().to_owned()),
        |r| Some(r.join("jvid-data").join("data")),
    )
}

pub fn cache_dir() -> Option<PathBuf> {
    portable_root().map_or_else(
        || platform().map(|d| d.cache_dir().to_owned()),
        |r| Some(r.join("jvid-data").join("cache")),
    )
}

/// Check before loading settings or creating a window. An unwritable portable copy must
/// explain how to recover, rather than silently start writing into the user's profile.
pub fn check_portable_writable() -> io::Result<()> {
    if let Some(root) = portable_root() {
        check_writable(&root.join("jvid-data"))?;
    }
    Ok(())
}

fn check_writable(data: &Path) -> io::Result<()> {
    for dir in [
        data.to_owned(),
        data.join("config"),
        data.join("data"),
        data.join("cache"),
    ] {
        fs::create_dir_all(&dir)?;
        let probe = dir.join(format!(".jvid-write-check-{}", std::process::id()));
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&probe)?;
        drop(file);
        fs::remove_file(probe)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_marker_and_all_three_layouts() {
        let tmp = std::env::temp_dir().join(format!("jvid-layouts-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        for exe in [
            "windows/jvid-app.exe",
            "linux/bin/jvid-app",
            "mac/jvid.app/Contents/MacOS/jvid-app",
        ] {
            let exe = tmp.join(exe);
            let bin = exe.parent().unwrap();
            fs::create_dir_all(bin).unwrap();
            assert_eq!(portable_root_for(&exe), None);
            fs::write(bin.join("jvid.portable"), "").unwrap();
            let root = portable_root_for(&exe).unwrap();
            assert_eq!(root.parent(), Some(tmp.as_path()));
            check_writable(&root.join("jvid-data")).unwrap();
            let settings = root.join("jvid-data/config/settings.toml");
            let model = root.join("jvid-data/data/models/kept.bin");
            fs::write(&settings, "theme = 'light'").unwrap();
            fs::create_dir_all(model.parent().unwrap()).unwrap();
            fs::write(&model, b"existing model").unwrap();
            check_writable(&root.join("jvid-data")).unwrap();
            assert_eq!(fs::read_to_string(settings).unwrap(), "theme = 'light'");
            assert_eq!(fs::read(model).unwrap(), b"existing model");
        }
        fs::remove_dir_all(tmp).unwrap();
    }

    #[test]
    fn unavailable_portable_data_is_an_error_without_profile_fallback() {
        let tmp = std::env::temp_dir().join(format!("jvid-unwritable-{}", std::process::id()));
        fs::create_dir_all(&tmp).unwrap();
        let data = tmp.join("jvid-data");
        fs::write(&data, "a file blocks the directory").unwrap();
        assert!(check_writable(&data).is_err());
        fs::remove_dir_all(tmp).unwrap();
    }
}
