//! FAT-safe replacement: retain the previous save until the new file is installed.
use std::{fs, io, path::Path};

pub fn read(path: &Path) -> io::Result<Vec<u8>> {
    fs::read(path).or_else(|e| {
        if e.kind() == io::ErrorKind::NotFound {
            fs::read(format!("{}.bak", path.display()))
        } else {
            Err(e)
        }
    })
}

/// The previous save kept beside `path` by [`write`].
pub fn read_backup(path: &Path) -> io::Result<Vec<u8>> {
    fs::read(path.with_extension("sav.bak"))
}

/// Puts the previous save back in place of a damaged one, moving the damaged
/// file to `.sav.bad`. The backup is never touched, so if this stops part way
/// [`read`] still finds a good copy: first the backup (once the damaged file
/// is aside, the slot reads as missing and falls back), then the restored file.
pub fn restore_backup(path: &Path) -> io::Result<()> {
    restore(path, |from, to| fs::rename(from, to))
}

fn restore(path: &Path, mut rename: impl FnMut(&Path, &Path) -> io::Result<()>) -> io::Result<()> {
    let data = read_backup(path)?;
    let bad = path.with_extension("sav.bad");
    if path.exists() {
        match fs::remove_file(&bad) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        rename(path, &bad)?;
    }
    // The slot is missing now, so this installs without moving the backup.
    replace(path, &data, rename)
}

pub fn write(path: &Path, data: &[u8]) -> io::Result<()> {
    replace(path, data, |from, to| fs::rename(from, to))
}

fn replace(
    path: &Path,
    data: &[u8],
    mut rename: impl FnMut(&Path, &Path) -> io::Result<()>,
) -> io::Result<()> {
    use io::Write;
    let tmp = path.with_extension("sav.tmp");
    let backup = path.with_extension("sav.bak");
    let mut file = fs::File::create(&tmp)?;
    file.write_all(data)?;
    file.sync_all()?;
    drop(file);
    // POSIX hosts can replace directly. libctru's SD rename rejects an existing target.
    if rename(&tmp, path).is_ok() {
        return Ok(());
    }
    if path.exists() {
        match fs::remove_file(&backup) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        rename(path, &backup)?;
    }
    if let Err(e) = rename(&tmp, path) {
        // Even if rollback fails, read() can recover the previous save from .bak.
        if backup.exists() {
            let _ = rename(&backup, path);
        }
        return Err(e);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fat_overwrite_and_interrupted_replacement_preserve_saves() {
        let dir = std::env::temp_dir().join(format!("kingdom-save-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("game.sav");
        write(&path, b"old").unwrap();
        let fat_rename = |a: &Path, b: &Path| {
            if b.exists() {
                Err(io::Error::from(io::ErrorKind::AlreadyExists))
            } else {
                fs::rename(a, b)
            }
        };
        replace(&path, b"new", fat_rename).unwrap();
        assert_eq!(read(&path).unwrap(), b"new");
        let fail_install = |a: &Path, b: &Path| {
            if a.extension().is_some_and(|e| e == "tmp") {
                Err(io::Error::from(io::ErrorKind::PermissionDenied))
            } else {
                fs::rename(a, b)
            }
        };
        assert!(replace(&path, b"failed", fail_install).is_err());
        assert_eq!(read(&path).unwrap(), b"new");
        fs::rename(&path, path.with_extension("sav.bak")).unwrap();
        assert_eq!(read(&path).unwrap(), b"new");
        replace(&path, b"recovered", fat_rename).unwrap();
        assert_eq!(read(&path).unwrap(), b"recovered");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn damaged_save_restores_from_backup_and_keeps_a_copy_at_every_step() {
        let dir = std::env::temp_dir().join(format!("kingdom-restore-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("game.sav");
        let fat_rename = |a: &Path, b: &Path| {
            if b.exists() {
                Err(io::Error::from(io::ErrorKind::AlreadyExists))
            } else {
                fs::rename(a, b)
            }
        };
        replace(&path, b"good", fat_rename).unwrap();
        replace(&path, b"newer", fat_rename).unwrap();
        // FAT keeps the previous save as the backup.
        assert_eq!(read_backup(&path).unwrap(), b"good");
        fs::write(&path, b"damaged").unwrap();

        // Stopped once the damaged file is aside: read finds the backup.
        let mut n = 0;
        let stop_second = |a: &Path, b: &Path| {
            n += 1;
            if n >= 2 {
                Err(io::Error::from(io::ErrorKind::PermissionDenied))
            } else {
                fat_rename(a, b)
            }
        };
        assert!(restore(&path, stop_second).is_err());
        assert_eq!(read(&path).unwrap(), b"good");
        assert_eq!(read_backup(&path).unwrap(), b"good");
        assert_eq!(
            fs::read(path.with_extension("sav.bad")).unwrap(),
            b"damaged"
        );

        // A full restore, with an older damaged copy already aside.
        fs::write(&path, b"damaged again").unwrap();
        restore(&path, fat_rename).unwrap();
        assert_eq!(read(&path).unwrap(), b"good");
        assert_eq!(read_backup(&path).unwrap(), b"good");
        assert_eq!(
            fs::read(path.with_extension("sav.bad")).unwrap(),
            b"damaged again"
        );

        // Without a backup nothing moves.
        fs::remove_file(path.with_extension("sav.bak")).unwrap();
        fs::write(&path, b"damaged").unwrap();
        assert!(restore(&path, fat_rename).is_err());
        assert_eq!(read(&path).unwrap(), b"damaged");
        fs::remove_dir_all(dir).unwrap();
    }
}
