use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Serialize, de::DeserializeOwned};

use crate::model::{AppState, History};

pub fn load_state(path: &Path) -> Result<AppState> {
    load_json(path, AppState::new)
}

pub fn load_history(path: &Path) -> Result<History> {
    load_json(path, History::new)
}

pub fn save_state(path: &Path, state: &AppState) -> Result<()> {
    save_json(path, state)
}

pub fn save_history(path: &Path, history: &History) -> Result<()> {
    save_json(path, history)
}

fn load_json<T: DeserializeOwned>(path: &Path, missing: impl FnOnce() -> T) -> Result<T> {
    let raw = match fs::read(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(missing()),
        Err(error) => return Err(error).with_context(|| format!("无法读取 {}", path.display())),
    };

    match serde_json::from_slice(&raw) {
        Ok(value) => Ok(value),
        Err(error) => {
            let backup = corrupt_backup_path(path);
            fs::copy(path, &backup).with_context(|| {
                format!(
                    "{} 已损坏，且无法创建备份 {}",
                    path.display(),
                    backup.display()
                )
            })?;
            set_file_permissions(&backup)?;
            Err(error).with_context(|| {
                format!(
                    "{} 不是有效的状态文件；原文件已保留，副本位于 {}",
                    path.display(),
                    backup.display()
                )
            })
        }
    }
}

fn save_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let parent_already_existed = parent.exists();
    fs::create_dir_all(parent).with_context(|| format!("无法创建状态目录 {}", parent.display()))?;
    if !parent_already_existed {
        set_dir_permissions(parent)?;
    }

    let file_name = path.file_name().unwrap_or_default().to_string_lossy();
    let temporary = parent.join(format!(".{file_name}.tmp-{}", std::process::id()));
    let result = (|| -> Result<()> {
        let mut options = OpenOptions::new();
        options.create(true).write(true).truncate(true);
        set_private_file_mode(&mut options);
        let mut file = options
            .open(&temporary)
            .with_context(|| format!("无法创建临时状态文件 {}", temporary.display()))?;
        serde_json::to_writer_pretty(&mut file, value).context("无法序列化状态")?;
        file.write_all(b"\n").context("无法写入状态文件")?;
        file.sync_all().context("无法同步状态文件")?;
        fs::rename(&temporary, path).with_context(|| format!("无法原子更新 {}", path.display()))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn corrupt_backup_path(path: &Path) -> PathBuf {
    let stamp = Utc::now().format("%Y%m%dT%H%M%SZ");
    let file_name = path.file_name().unwrap_or_default().to_string_lossy();
    path.with_file_name(format!("{file_name}.corrupt-{stamp}"))
}

#[cfg(unix)]
fn set_file_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .with_context(|| format!("无法设置文件权限 {}", path.display()))
}

#[cfg(not(unix))]
fn set_file_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn set_dir_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .with_context(|| format!("无法设置目录权限 {}", path.display()))
}

#[cfg(not(unix))]
fn set_dir_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn set_private_file_mode(options: &mut OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;
    options.mode(0o600);
}

#[cfg(not(unix))]
fn set_private_file_mode(_options: &mut OpenOptions) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_round_trip_and_missing_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data/state.json");
        assert_eq!(load_state(&path).unwrap().version, 1);
        let mut state = AppState::new();
        state.typical_change_seconds = Some(1234);
        save_state(&path, &state).unwrap();
        assert_eq!(
            load_state(&path).unwrap().typical_change_seconds,
            Some(1234)
        );
    }

    #[test]
    fn corrupted_file_is_not_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        fs::write(&path, "not json").unwrap();
        assert!(load_state(&path).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "not json");
        assert!(
            fs::read_dir(dir.path())
                .unwrap()
                .flatten()
                .any(|entry| entry.file_name().to_string_lossy().contains(".corrupt-"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn existing_parent_permissions_are_not_changed() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755)).unwrap();
        let path = dir.path().join("state.json");
        save_state(&path, &AppState::new()).unwrap();
        assert_eq!(
            fs::metadata(dir.path()).unwrap().permissions().mode() & 0o777,
            0o755
        );
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
