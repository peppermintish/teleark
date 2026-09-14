//! Release only the empty admission marker of a published native download.
use super::*;

pub(super) fn release_completed(
    destination: &Path,
    expected_bytes: u64,
) -> Result<(), ApplicationError> {
    let mut reservation = destination.as_os_str().to_os_string();
    reservation.push(".partial");
    let reservation = PathBuf::from(reservation);
    // `.partial` is also the encrypted-download resume format. Never remove
    // nonempty data, symlinks, directories, or a marker without its final output.
    let Some(marker) = metadata_if_present(&reservation)? else {
        return Ok(());
    };
    if !marker.is_file() || marker.len() != 0 {
        return Ok(());
    }
    let Some(output) = metadata_if_present(destination)? else {
        return Ok(());
    };
    if !output.is_file() || output.len() != expected_bytes {
        return Ok(());
    }
    match std::fs::remove_file(&reservation) {
        Ok(()) => super::cleanup::sync_cleanup_directory(destination),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io_error(error)),
    }
}

fn metadata_if_present(path: &Path) -> Result<Option<std::fs::Metadata>, ApplicationError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(io_error(error)),
    }
}

fn io_error(error: std::io::Error) -> ApplicationError {
    ApplicationError::new(match error.kind() {
        std::io::ErrorKind::PermissionDenied => ApplicationErrorKind::PermissionDenied,
        _ => ApplicationErrorKind::Persistence,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn only_empty_markers_with_complete_regular_outputs_are_removed() {
        let root = tempfile::tempdir().expect("fixture");
        let output = root.path().join("file.bin");
        let marker = root.path().join("file.bin.partial");
        fs::write(&marker, b"").expect("marker");
        release_completed(&output, 4).expect("missing output is retained");
        assert!(marker.exists());
        fs::write(&output, b"bad").expect("changed output");
        release_completed(&output, 4).expect("changed output is retained");
        assert!(marker.exists());
        fs::write(&output, b"done").expect("complete output");
        fs::write(&marker, b"resume data").expect("encrypted resume data");
        release_completed(&output, 4).expect("nonempty partial is retained");
        assert_eq!(fs::read(&marker).expect("preserved data"), b"resume data");
        fs::remove_file(&marker).expect("remove fixture");
        fs::create_dir(&marker).expect("directory at marker name");
        release_completed(&output, 4).expect("directory is retained");
        assert!(marker.is_dir());
        fs::remove_dir(&marker).expect("remove directory fixture");
        fs::write(&marker, b"").expect("empty marker");
        release_completed(&output, 4).expect("release completed reservation");
        assert!(!marker.exists());
        assert_eq!(fs::read(&output).expect("preserved final"), b"done");
        release_completed(&output, 4).expect("idempotent repair");
        fs::write(&output, b"").expect("valid zero-byte download");
        fs::write(&marker, b"").expect("zero-byte download marker");
        release_completed(&output, 0).expect("zero-byte completion");
        assert!(!marker.exists());
        assert_eq!(
            fs::metadata(output).expect("zero-byte final remains").len(),
            0
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_markers_and_outputs_are_never_removed_or_followed() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().expect("fixture");
        let output = root.path().join("file.bin");
        let marker = root.path().join("file.bin.partial");
        let external = root.path().join("external");
        fs::write(&external, b"").expect("external file");
        fs::write(&output, b"done").expect("final");
        symlink(&external, &marker).expect("marker link");
        release_completed(&output, 4).expect("skip marker link");
        assert!(
            fs::symlink_metadata(&marker)
                .expect("link preserved")
                .is_symlink()
        );
        fs::remove_file(&marker).expect("remove marker fixture");
        fs::remove_file(&output).expect("remove output fixture");
        fs::write(&marker, b"").expect("regular marker");
        symlink(&external, &output).expect("output link");
        release_completed(&output, 0).expect("skip linked output");
        assert!(marker.exists());
        assert!(
            fs::symlink_metadata(&output)
                .expect("output link preserved")
                .is_symlink()
        );
        assert!(external.exists());
    }
}
