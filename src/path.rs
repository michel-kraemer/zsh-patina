use std::{fs, os::unix::fs::PermissionsExt, path::Path};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PathType {
    File,
    Directory,
}

/// Get the type of the given path (file or directory). If the path is relative,
/// it is resolved against the provided `pwd`. If the path does not exist or the
/// user lacks permission to access it, the function returns `None`.
pub fn path_type(path: &str, pwd: &str) -> Option<PathType> {
    let p = Path::new(path);
    let full_path = if p.is_absolute() {
        p.to_path_buf()
    } else {
        Path::new(pwd).join(p)
    };

    let Ok(metadata) = fs::metadata(&full_path) else {
        return None;
    };

    Some(if metadata.is_dir() {
        PathType::Directory
    } else {
        PathType::File
    })
}

/// Check if the given path is an executable file. If the path is relative, it
/// is resolved against the provided `pwd`. If the path is a directory, it is
/// only considered executable if it ends with a slash.
pub fn is_path_executable(path: &str, pwd: &str) -> bool {
    let p = Path::new(path);
    let full_path = if p.is_absolute() {
        p.to_path_buf()
    } else {
        Path::new(pwd).join(p)
    };

    let Ok(metadata) = fs::metadata(&full_path) else {
        return false;
    };

    let is_executable = (metadata.permissions().mode() & 0o111) != 0;
    if metadata.is_dir() {
        is_executable && path.ends_with('/')
    } else {
        is_executable
    }
}
