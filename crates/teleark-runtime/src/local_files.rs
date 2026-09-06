//! Local filesystem observations, separate from historical transfer outcomes.
use std::path::{Path, PathBuf};
use teleark_core::{ApplicationError, ApplicationErrorKind};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VolumeSpace {
    pub available_bytes: u64,
    pub total_bytes: u64,
}

/// Resolves symlinks before selecting the most specific mounted volume.
/// A missing destination is not silently attributed to the system disk.
pub fn volume_space(path: &Path) -> Result<VolumeSpace, ApplicationError> {
    let canonical = path.canonicalize().map_err(crate::map_filesystem_error)?;
    let disks = sysinfo::Disks::new_with_refreshed_list();
    disks
        .list()
        .iter()
        .filter(|disk| canonical.starts_with(disk.mount_point()))
        .max_by_key(|disk| disk.mount_point().components().count())
        .map(|disk| VolumeSpace {
            available_bytes: disk.available_space(),
            total_bytes: disk.total_space(),
        })
        .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::Persistence))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LocalFilePresence {
    Present,
    Missing,
    SizeChanged,
    Unavailable,
}

/// Metadata checks do not imply cryptographic verification. If the parent
/// cannot be reached (including an offline external disk), deletion is unknown.
pub fn local_file_presence(path: &Path, expected_size: u64) -> LocalFilePresence {
    match std::fs::metadata(path) {
        Ok(meta) if meta.is_file() && meta.len() == expected_size => LocalFilePresence::Present,
        Ok(_) => LocalFilePresence::SizeChanged,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            match path
                .parent()
                .and_then(|parent| std::fs::metadata(parent).ok())
            {
                Some(parent) if parent.is_dir() => LocalFilePresence::Missing,
                _ => LocalFilePresence::Unavailable,
            }
        }
        Err(_) => LocalFilePresence::Unavailable,
    }
}

pub const LOCAL_FILE_PROBE_LIMIT: usize = 128;

pub fn probe_local_files(
    files: &[(PathBuf, u64)],
) -> Result<Vec<(PathBuf, LocalFilePresence)>, ApplicationError> {
    if files.len() > LOCAL_FILE_PROBE_LIMIT {
        return Err(ApplicationError::new(ApplicationErrorKind::InvalidRequest));
    }
    Ok(files
        .iter()
        .map(|(path, size)| (path.clone(), local_file_presence(path, *size)))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn observes_external_deletion_recreation_and_size_changes() {
        let fixture = tempfile::tempdir().expect("test root");
        let root = fixture.path();
        let path = root.join("download.bin");
        std::fs::write(&path, b"abc").expect("test file");
        assert_eq!(local_file_presence(&path, 3), LocalFilePresence::Present);
        std::fs::remove_file(&path).expect("external deletion");
        assert_eq!(local_file_presence(&path, 3), LocalFilePresence::Missing);
        std::fs::write(&path, b"abc").expect("external recreation");
        assert_eq!(local_file_presence(&path, 3), LocalFilePresence::Present);
        std::fs::write(&path, b"changed").expect("external modification");
        assert_eq!(
            local_file_presence(&path, 3),
            LocalFilePresence::SizeChanged
        );
        assert_eq!(
            local_file_presence(&root.join("offline/file"), 3),
            LocalFilePresence::Unavailable
        );
    }
    #[test]
    fn probe_rejects_unbounded_work_and_does_not_create_missing_volumes() {
        let missing = PathBuf::from("/teleark-test-volume-does-not-exist/downloads");
        assert!(volume_space(&missing).is_err());
        assert!(probe_local_files(&vec![(missing, 0); LOCAL_FILE_PROBE_LIMIT + 1]).is_err());
    }
}
