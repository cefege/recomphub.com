use fs2::FileExt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};

#[cfg(windows)]
use windows_sys::Win32::Storage::FileSystem::{
    MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
};

fn reject_symlink_or_reparse_components(path: &Path) -> Result<(), DesktopError> {
    let mut current = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => {
                // A Windows prefix (for example, `C:`) is a path namespace,
                // not a filesystem entry. Inspecting it before the root is
                // attached can fail with ERROR_INVALID_FUNCTION.
                current.push(prefix.as_os_str());
                continue;
            }
            Component::RootDir => current.push(component.as_os_str()),
            Component::CurDir => continue,
            Component::ParentDir => {
                return Err(DesktopError::DataConflict(
                    "data paths may not contain parent components".into(),
                ));
            }
            Component::Normal(name) => current.push(name),
        }
        match fs::symlink_metadata(&current) {
            Ok(metadata) => {
                #[cfg(windows)]
                let reparse = {
                    use std::os::windows::fs::MetadataExt;
                    metadata.file_attributes() & 0x400 != 0
                };
                #[cfg(not(windows))]
                let reparse = false;
                if metadata.file_type().is_symlink() || reparse {
                    return Err(DesktopError::DataConflict(
                        "data paths may not pass through a symlink or reparse point".into(),
                    ));
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn atomic_replace(staged: &Path, destination: &Path) -> Result<(), DesktopError> {
    #[cfg(unix)]
    {
        fs::rename(staged, destination)?;
    }
    #[cfg(windows)]
    {
        let staged_wide: Vec<u16> = staged.as_os_str().encode_wide().chain(Some(0)).collect();
        let destination_wide: Vec<u16> = destination
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect();
        let result = unsafe {
            MoveFileExW(
                staged_wide.as_ptr(),
                destination_wide.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        };
        if result == 0 {
            return Err(io::Error::last_os_error().into());
        }
    }
    Ok(())
}
use xxhash_rust::xxh3::xxh3_64;
pub(crate) fn lock_destination(destination: &Path) -> Result<File, DesktopError> {
    let parent = destination
        .parent()
        .ok_or_else(|| DesktopError::DataConflict("data destination has no parent".into()))?;
    fs::create_dir_all(parent)?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(parent.join(".recomphub-data.lock"))?;
    lock.lock_exclusive()?;
    Ok(lock)
}

use crate::{DataStatus, DesktopError, ROM_CAP};

pub const BANJO_ROM_XXH3: u64 = 0x1B67_585D_56E0_7F8C;

pub(crate) fn stored_banjo_status(path: &Path) -> Result<DataStatus, DesktopError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(DataStatus::Missing),
        Err(error) => return Err(error.into()),
    };
    if !metadata.file_type().is_file() || metadata.len() == 0 || metadata.len() > ROM_CAP {
        return Ok(DataStatus::Conflict);
    }
    match reject_symlink_or_reparse_components(path) {
        Ok(()) => {}
        Err(DesktopError::DataConflict(_)) => return Ok(DataStatus::Conflict),
        Err(error) => return Err(error),
    }
    let digest = raw_file_digest(&mut File::open(path)?)?;
    Ok(if digest == Some(BANJO_ROM_XXH3) {
        DataStatus::Ready
    } else {
        DataStatus::Conflict
    })
}

fn raw_file_digest(file: &mut File) -> Result<Option<u64>, DesktopError> {
    let mut reader = file.take(ROM_CAP + 1);
    let mut hasher = xxhash_rust::xxh3::Xxh3::new();
    let mut buffer = [0; 8192];
    let mut total = 0_u64;
    loop {
        let length = reader.read(&mut buffer)?;
        if length == 0 {
            break;
        }
        total += length as u64;
        if total > ROM_CAP {
            return Ok(None);
        }
        hasher.update(&buffer[..length]);
    }
    Ok(Some(hasher.digest()))
}

pub(crate) fn normalize_banjo_file(path: &Path) -> Result<Vec<u8>, DesktopError> {
    reject_symlink_or_reparse_components(path)?;
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() || metadata.len() == 0 || metadata.len() > ROM_CAP {
        return Err(DesktopError::DataConflict(
            "select a regular game file no larger than 64 MiB".into(),
        ));
    }
    let file = File::open(path)?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(ROM_CAP + 1).read_to_end(&mut bytes)?;
    if bytes.len() < 4 || bytes.len() as u64 > ROM_CAP {
        return Err(DesktopError::DataConflict("game file is truncated".into()));
    }
    normalize_n64_dump(&mut bytes)?;
    if xxh3_64(&bytes) != BANJO_ROM_XXH3 {
        return Err(DesktopError::DataConflict(
            "game file is not Banjo-Kazooie North American 1.0".into(),
        ));
    }
    Ok(bytes)
}

pub fn normalize_n64_dump(bytes: &mut Vec<u8>) -> Result<(), DesktopError> {
    if bytes.len() < 4 {
        return Err(DesktopError::DataConflict("game file is truncated".into()));
    }
    match bytes[..4] {
        [0x80, 0x37, 0x12, 0x40] => {}
        [0x37, 0x80, 0x40, 0x12] => {
            for pair in bytes.as_chunks_mut::<2>().0 {
                pair.swap(0, 1);
            }
        }
        [0x40, 0x12, 0x37, 0x80] => {
            for word in bytes.as_chunks_mut::<4>().0 {
                word.reverse();
            }
        }
        _ => {
            return Err(DesktopError::DataConflict(
                "game file is not a recognized Nintendo 64 image".into(),
            ));
        }
    }
    let padded_len = bytes
        .len()
        .checked_add(3)
        .ok_or_else(|| DesktopError::DataConflict("game file size overflow".into()))?
        & !3;
    bytes.resize(padded_len, 0);
    if bytes.len() as u64 > ROM_CAP {
        return Err(DesktopError::DataConflict(
            "normalized game file exceeds 64 MiB".into(),
        ));
    }
    Ok(())
}

pub(crate) fn replace_with_durable_backup(
    destination: &Path,
    bytes: &[u8],
) -> Result<Option<PathBuf>, DesktopError> {
    #[cfg(test)]
    {
        replace_with_durable_backup_inner(destination, bytes, None)
    }
    #[cfg(not(test))]
    {
        replace_with_durable_backup_inner(destination, bytes)
    }
}

#[cfg(test)]
#[derive(PartialEq, Eq)]
enum ReplaceFault {
    AfterBackupBeforeReplace,
}

fn replace_with_durable_backup_inner(
    destination: &Path,
    bytes: &[u8],
    #[cfg(test)] fault: Option<ReplaceFault>,
) -> Result<Option<PathBuf>, DesktopError> {
    let parent = destination
        .parent()
        .ok_or_else(|| DesktopError::DataConflict("data destination has no parent".into()))?;
    reject_symlink_or_reparse_components(parent)?;
    fs::create_dir_all(parent)?;
    reject_symlink_or_reparse_components(parent)?;
    let existing = match fs::symlink_metadata(destination) {
        Ok(metadata) => {
            #[cfg(windows)]
            let reparse = {
                use std::os::windows::fs::MetadataExt;
                metadata.file_attributes() & 0x400 != 0
            };
            #[cfg(not(windows))]
            let reparse = false;
            if !metadata.file_type().is_file() || reparse || metadata.len() > ROM_CAP {
                return Err(DesktopError::DataConflict(
                    "existing original-data destination is not a bounded regular file".into(),
                ));
            }
            true
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => return Err(error.into()),
    };

    let id = uuid::Uuid::new_v4();
    let staged = parent.join(format!(".recomphub-data-{id}.part"));
    let backup = existing.then(|| {
        destination.with_file_name(format!(
            "{}.backup-{id}",
            destination
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
        ))
    });
    let result = (|| -> Result<(), DesktopError> {
        let mut new_file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staged)?;
        new_file.write_all(bytes)?;
        new_file.sync_all()?;
        if existing {
            let backup_path = backup.as_ref().expect("backup path for existing data");
            let mut backup_file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(backup_path)?;
            let mut old_file = File::open(destination)?;
            io::copy(&mut old_file, &mut backup_file)?;
            backup_file.sync_all()?;
            sync_directory(parent)?;
            #[cfg(test)]
            if fault == Some(ReplaceFault::AfterBackupBeforeReplace) {
                return Err(io::Error::other("injected failure before atomic replacement").into());
            }
        }
        atomic_replace(&staged, destination)?;
        sync_directory(parent)?;
        Ok(())
    })();
    if staged.exists() {
        let _ = fs::remove_file(&staged);
    }
    result?;
    Ok(backup)
}

fn sync_directory(path: &Path) -> Result<(), DesktopError> {
    #[cfg(unix)]
    File::open(path)?.sync_all()?;
    #[cfg(windows)]
    let _ = path;
    #[cfg(not(any(unix, windows)))]
    let _ = path;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_data_inspection_hashes_raw_bytes_and_never_normalizes_the_destination() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().canonicalize().unwrap().join("stored.z64");
        let canonical = [0x80, 0x37, 0x12, 0x40, 1, 2, 3, 4];
        let swapped = [0x37, 0x80, 0x40, 0x12, 2, 1, 4, 3];
        fs::write(&path, swapped).unwrap();
        let digest = raw_file_digest(&mut File::open(&path).unwrap()).unwrap();
        assert_eq!(digest, Some(xxh3_64(&swapped)));
        assert_ne!(digest, Some(xxh3_64(&canonical)));
        assert_eq!(stored_banjo_status(&path).unwrap(), DataStatus::Conflict);
        assert_eq!(fs::read(&path).unwrap(), swapped);
    }

    #[test]
    fn missing_data_inspection_does_not_create_an_upstream_directory() {
        let directory = tempfile::tempdir().unwrap();
        let parent = directory.path().canonicalize().unwrap().join("not-created");
        assert_eq!(
            stored_banjo_status(&parent.join("stored.z64")).unwrap(),
            DataStatus::Missing
        );
        assert!(!parent.exists());
    }

    #[test]
    fn oversized_stored_data_is_a_conflict_and_is_left_unchanged() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().canonicalize().unwrap().join("stored.z64");
        File::create(&path).unwrap().set_len(ROM_CAP + 1).unwrap();
        assert_eq!(stored_banjo_status(&path).unwrap(), DataStatus::Conflict);
        assert_eq!(fs::metadata(path).unwrap().len(), ROM_CAP + 1);
    }

    #[test]
    fn dump_byte_orders_normalize_and_padding_is_bounded() {
        let canonical = vec![0x80, 0x37, 0x12, 0x40, 1, 2, 3, 4];
        let mut word_swapped = vec![0x37, 0x80, 0x40, 0x12, 2, 1, 4, 3];
        normalize_n64_dump(&mut word_swapped).unwrap();
        assert_eq!(word_swapped, canonical);
        let mut byte_reversed = vec![0x40, 0x12, 0x37, 0x80, 4, 3, 2, 1];
        normalize_n64_dump(&mut byte_reversed).unwrap();
        assert_eq!(byte_reversed, canonical);
        let mut padded = vec![0x80, 0x37, 0x12, 0x40, 1];
        normalize_n64_dump(&mut padded).unwrap();
        assert_eq!(padded, vec![0x80, 0x37, 0x12, 0x40, 1, 0, 0, 0]);
    }

    #[test]
    fn successful_replacement_retains_durable_backup_and_contents() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().canonicalize().unwrap().join("bk.n64.us.1.0.z64");
        fs::write(&target, b"original-user-data").unwrap();
        let backup = replace_with_durable_backup(&target, b"replacement")
            .unwrap()
            .unwrap();
        assert_eq!(fs::read(&backup).unwrap(), b"original-user-data");
        assert_eq!(fs::read(&target).unwrap(), b"replacement");
        assert_eq!(dir.path().read_dir().unwrap().count(), 2);
    }

    #[test]
    fn failed_atomic_replacement_keeps_original_destination_and_backup() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().canonicalize().unwrap().join("bk.n64.us.1.0.z64");
        fs::write(&target, b"original-user-data").unwrap();
        let result = replace_with_durable_backup_inner(
            &target,
            b"replacement",
            Some(ReplaceFault::AfterBackupBeforeReplace),
        );
        assert!(result.is_err());
        assert_eq!(fs::read(&target).unwrap(), b"original-user-data");
        let entries = dir
            .path()
            .read_dir()
            .unwrap()
            .map(Result::unwrap)
            .collect::<Vec<_>>();
        assert_eq!(entries.len(), 2);
        assert!(
            entries
                .iter()
                .any(|entry| entry.file_name().to_string_lossy().contains(".backup-"))
        );
        assert!(
            entries
                .iter()
                .all(|entry| !entry.file_name().to_string_lossy().ends_with(".old"))
        );
        assert!(
            entries
                .iter()
                .all(|entry| !entry.file_name().to_string_lossy().ends_with(".part"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn source_and_destination_parent_symlinks_are_rejected() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let real = root.join("real");
        fs::create_dir(&real).unwrap();
        let link = root.join("link");
        symlink(&real, &link).unwrap();
        let source = link.join("source.rom");
        fs::write(real.join("source.rom"), b"input").unwrap();
        assert!(normalize_banjo_file(&source).is_err());
        assert!(
            replace_with_durable_backup(&link.join("destination.rom"), b"replacement").is_err()
        );
    }
}
