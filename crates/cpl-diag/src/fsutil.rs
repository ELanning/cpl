//! Writing into directories that a project's own files can shape.
//!
//! The toolchain writes under paths a checked-out repository controls (`.cpl/build/<target>`,
//! `cpl.lock`, a package's `.cpl-generated` list). A repository can plant symbolic links there,
//! so a write that follows one would create, overwrite, or delete a file anywhere the user can
//! write. Every such write goes through these functions: a path below a root may not pass
//! through a symbolic link, and a file is written by renaming a fresh temporary file over it,
//! which replaces a link at the destination instead of writing through it.

use std::io;
use std::path::{Component, Path, PathBuf};

/// `root` joined with `relative`, refused when `relative` is not a plain relative path (`..`,
/// an absolute path) or when an existing component below `root` is a symbolic link. A
/// component that does not exist yet is fine: the caller creates it.
pub fn beneath(root: &Path, relative: &Path) -> io::Result<PathBuf> {
    let mut path = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(c) = component else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{} is not a plain relative path", relative.display()),
            ));
        };
        path.push(c);
        refuse_link(&path)?;
    }
    Ok(path)
}

/// An error when `path` is a symbolic link; nothing when it is anything else or missing.
pub fn refuse_link(path: &Path) -> io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "{} is a symbolic link; the toolchain does not write through links",
                path.display()
            ),
        )),
        Ok(_) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// The directories of `relative` below `root`, created as needed, none of them a link.
pub fn create_dirs_beneath(root: &Path, relative: &Path) -> io::Result<PathBuf> {
    let dir = beneath(root, relative)?;
    std::fs::create_dir_all(&dir)?;
    // a directory created concurrently as a link is caught here
    beneath(root, relative)?;
    Ok(dir)
}

/// Write `bytes` to `path` atomically: a temporary file beside it, then renamed over it. A
/// symbolic link at `path` is replaced, never written through; a reader sees the old file or
/// the new one, never a part.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut attempt = 0u32;
    loop {
        let tmp = dir.join(format!(
            ".{name}.tmp-{}-{attempt}",
            std::process::id()
        ));
        // create_new: never a file (or link) that is already there
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
        {
            Ok(mut f) => {
                use std::io::Write;
                let written = f.write_all(bytes).and_then(|()| f.sync_all());
                drop(f);
                if let Err(e) = written.and_then(|()| std::fs::rename(&tmp, path)) {
                    let _ = std::fs::remove_file(&tmp);
                    return Err(e);
                }
                return Ok(());
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists && attempt < 64 => attempt += 1,
            Err(e) => return Err(e),
        }
    }
}

/// Remove the file `relative` below `root`: refused through a link, and a link at the end is
/// removed itself (as `remove_file` does), never its target.
pub fn remove_beneath(root: &Path, relative: &Path) -> io::Result<()> {
    let path = beneath_parent(root, relative)?;
    std::fs::remove_file(path)
}

/// Remove the directory tree `relative` below `root`, refused when it or a component on the way
/// is a link (`remove_dir_all` does not follow links inside the tree).
pub fn remove_dir_beneath(root: &Path, relative: &Path) -> io::Result<()> {
    let path = beneath(root, relative)?;
    match std::fs::remove_dir_all(&path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// `beneath` for every component but the last, which may itself be a link (to be replaced or
/// removed, not followed).
fn beneath_parent(root: &Path, relative: &Path) -> io::Result<PathBuf> {
    let parent = relative.parent().unwrap_or_else(|| Path::new(""));
    let dir = beneath(root, parent)?;
    let Some(name) = relative.file_name() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} names no file", relative.display()),
        ));
    };
    if !matches!(
        Path::new(name).components().next(),
        Some(Component::Normal(_))
    ) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} is not a plain relative path", relative.display()),
        ));
    }
    Ok(dir.join(name))
}

/// A new directory in the temporary directory, private to this user (mode `0700` on Unix) and
/// created here (never one that already exists), named `<prefix>-<pid>-<time>-<n>`. A fixed or
/// guessable name in a directory every user can write is one another user can create first, as
/// a directory of theirs or a link, and then read or replace what is written into it.
pub fn private_temp_dir(prefix: &str) -> io::Result<PathBuf> {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let base = std::env::temp_dir();
    let mut last = None;
    for _ in 0..64 {
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = base.join(format!("{prefix}-{}-{nanos:x}-{n}", std::process::id()));
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
        match builder.create(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => last = Some(e),
            Err(e) => return Err(e),
        }
    }
    Err(last.unwrap_or_else(|| io::Error::other("no fresh temporary directory")))
}

/// Write `bytes` to the file `relative` below `root` atomically, creating its directories, none
/// of them a link.
pub fn write_beneath(root: &Path, relative: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = relative.parent() {
        create_dirs_beneath(root, parent)?;
    }
    let path = beneath_parent(root, relative)?;
    write_atomic(&path, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("cpl-fsutil-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[cfg(unix)]
    #[test]
    fn links_below_the_root_are_refused_and_never_written_through() {
        let root = scratch("links");
        let outside = scratch("outside");
        std::fs::write(outside.join("secret"), "keep").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("dir")).unwrap();
        std::os::unix::fs::symlink(outside.join("secret"), root.join("file")).unwrap();
        // through a linked directory: refused
        assert!(write_beneath(&root, Path::new("dir/x"), b"x").is_err());
        assert!(remove_beneath(&root, Path::new("dir/secret")).is_err());
        assert!(remove_dir_beneath(&root, Path::new("dir")).is_err());
        // a link as the file itself: replaced, not followed
        write_beneath(&root, Path::new("file"), b"new").unwrap();
        assert_eq!(std::fs::read_to_string(outside.join("secret")).unwrap(), "keep");
        assert_eq!(std::fs::read_to_string(root.join("file")).unwrap(), "new");
        assert!(!std::fs::symlink_metadata(root.join("file")).unwrap().file_type().is_symlink());
        // .. and absolute paths: refused
        assert!(beneath(&root, Path::new("../x")).is_err());
        assert!(beneath(&root, Path::new("/etc/passwd")).is_err());
        // plain paths work
        write_beneath(&root, Path::new("a/b/c.txt"), b"ok").unwrap();
        assert_eq!(std::fs::read_to_string(root.join("a/b/c.txt")).unwrap(), "ok");
        remove_beneath(&root, Path::new("a/b/c.txt")).unwrap();
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&outside);
    }

    #[test]
    fn private_temp_dirs_are_fresh() {
        let a = private_temp_dir("cpl-fsutil-private").unwrap();
        let b = private_temp_dir("cpl-fsutil-private").unwrap();
        assert_ne!(a, b);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&a).unwrap().permissions().mode();
            assert_eq!(mode & 0o077, 0, "{mode:o}");
        }
        let _ = std::fs::remove_dir(&a);
        let _ = std::fs::remove_dir(&b);
    }
}
