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

fn developer_profile() -> io::Result<Option<PathBuf>> {
    #[cfg(all(debug_assertions, feature = "debug-ui"))]
    {
        if !std::env::var_os("JUI_DEBUG").is_some_and(|value| value == "1") {
            return Ok(None);
        }
        if portable_root().is_some() {
            return Err(io::Error::other(
                "developer sessions cannot use a portable package",
            ));
        }
        let root = std::env::var_os("JUI_DEBUG_PROFILE")
            .map(PathBuf::from)
            .ok_or_else(|| io::Error::other("JUI_DEBUG_PROFILE is required"))?;
        if !root.is_absolute() || root.parent().is_none() {
            return Err(io::Error::other(
                "JUI_DEBUG_PROFILE must be an absolute profile directory",
            ));
        }
        Ok(Some(root))
    }
    #[cfg(not(all(debug_assertions, feature = "debug-ui")))]
    Ok(None)
}

/// An explicit portable or developer data root; invalid developer configuration never falls back.
pub fn profile_root() -> io::Result<Option<PathBuf>> {
    if let Some(root) = developer_profile()? {
        return Ok(Some(root));
    }
    Ok(portable_root().map(|root| root.join("jvid-data")))
}

/// Validate developer isolation before settings, logs, recovery or a window can access a profile.
pub fn check_developer_profile() -> io::Result<()> {
    if let Some(root) = developer_profile()? {
        check_writable(&root)?;
    }
    Ok(())
}

pub fn config_dir() -> Option<PathBuf> {
    profile_root().ok()?.map_or_else(
        || platform().map(|d| d.config_dir().to_owned()),
        |r| Some(r.join("config")),
    )
}

pub fn data_dir() -> Option<PathBuf> {
    profile_root().ok()?.map_or_else(
        || platform().map(|d| d.data_dir().to_owned()),
        |r| Some(r.join("data")),
    )
}

pub fn data_local_dir() -> Option<PathBuf> {
    profile_root().ok()?.map_or_else(
        || platform().map(|d| d.data_local_dir().to_owned()),
        |r| Some(r.join("data")),
    )
}

pub fn cache_dir() -> Option<PathBuf> {
    profile_root().ok()?.map_or_else(
        || platform().map(|d| d.cache_dir().to_owned()),
        |r| Some(r.join("cache")),
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
    fn developer_profile_is_isolated_and_invalid_opt_in_fails_closed() {
        const CHILD: &str = "JVID_PROFILE_TEST_CHILD";
        if let Ok(case) = std::env::var(CHILD) {
            if !cfg!(all(debug_assertions, feature = "debug-ui")) || case == "off" {
                let installed = platform().unwrap();
                assert_eq!(config_dir(), Some(installed.config_dir().to_owned()));
                assert_eq!(data_dir(), Some(installed.data_dir().to_owned()));
                assert_eq!(
                    data_local_dir(),
                    Some(installed.data_local_dir().to_owned())
                );
                assert_eq!(cache_dir(), Some(installed.cache_dir().to_owned()));
                return;
            }
            if case == "missing" || case == "relative" || case == "portable" {
                assert!(check_developer_profile().is_err());
                assert!(profile_root().is_err());
                assert_eq!(config_dir(), None);
                assert_eq!(data_dir(), None);
                assert_eq!(data_local_dir(), None);
                assert_eq!(cache_dir(), None);
                return;
            }
            if case == "blocked" {
                assert!(check_developer_profile().is_err());
                return;
            }
            let root = PathBuf::from(std::env::var_os("JUI_DEBUG_PROFILE").unwrap());
            check_developer_profile().unwrap();
            assert_eq!(profile_root().unwrap(), Some(root.clone()));
            assert_eq!(config_dir(), Some(root.join("config")));
            assert_eq!(data_dir(), Some(root.join("data")));
            assert_eq!(data_local_dir(), Some(root.join("data")));
            assert_eq!(cache_dir(), Some(root.join("cache")));
            for relative in [
                "config/settings.toml",
                "config/themes/kept.toml",
                "data/autosave/Untitled.jvid",
            ] {
                let path = root.join(relative);
                if !path.exists() {
                    fs::create_dir_all(path.parent().unwrap()).unwrap();
                    fs::write(&path, b"retained developer work").unwrap();
                }
                assert_eq!(fs::read(path).unwrap(), b"retained developer work");
            }
            return;
        }
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("jvid-developer-{}-{nonce}", std::process::id()));
        fs::create_dir(&root).unwrap();
        let blocked = root.join("blocked");
        fs::write(&blocked, b"not a directory").unwrap();
        let mut cases = vec!["valid", "valid", "off", "missing", "relative", "blocked"];
        if cfg!(all(debug_assertions, feature = "debug-ui")) {
            cases.push("portable");
        }
        for case in cases {
            let mut executable = std::env::current_exe().unwrap();
            if case == "portable" {
                let package = root.join("portable");
                fs::create_dir(&package).unwrap();
                let copy = package.join(executable.file_name().unwrap());
                fs::copy(&executable, &copy).unwrap();
                fs::write(package.join("jvid.portable"), b"").unwrap();
                executable = copy;
            }
            let mut command = std::process::Command::new(executable);
            command
                .args([
                    "tests::developer_profile_is_isolated_and_invalid_opt_in_fails_closed",
                    "--exact",
                    "--nocapture",
                ])
                .env(CHILD, case)
                .env("JUI_DEBUG", "1")
                .env("JUI_DEBUG_PROFILE", root.join("profile"));
            match case {
                "off" => {
                    command.env("JUI_DEBUG", "0");
                }
                "missing" => {
                    command.env_remove("JUI_DEBUG_PROFILE");
                }
                "relative" => {
                    command.env("JUI_DEBUG_PROFILE", "relative-profile");
                }
                "blocked" => {
                    command.env("JUI_DEBUG_PROFILE", &blocked);
                }
                _ => {}
            }
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "{case}: {}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        fs::remove_dir_all(root).unwrap();
    }

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
