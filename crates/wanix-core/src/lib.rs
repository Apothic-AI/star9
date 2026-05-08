//! Shared Wanix value types.
//!
//! This crate intentionally contains no filesystem implementation. It mirrors
//! the small, widely used contracts from the Go tree: path validity, file modes,
//! metadata, operation context, open flags, and structured errors.

use std::fmt;
use std::ops::{BitAnd, BitAndAssign, BitOr, BitOrAssign, Not};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Debug, Error, Eq, PartialEq, Serialize, Deserialize)]
pub enum Error {
    #[error("{op} {path}: {kind}")]
    Path {
        op: &'static str,
        path: String,
        kind: ErrorKind,
    },
    #[error("{0}")]
    Kind(ErrorKind),
    #[error("{0}")]
    Message(String),
}

impl Error {
    pub fn path(op: &'static str, path: impl Into<String>, kind: ErrorKind) -> Self {
        Self::Path {
            op,
            path: path.into(),
            kind,
        }
    }

    pub fn kind(&self) -> ErrorKind {
        match self {
            Self::Path { kind, .. } | Self::Kind(kind) => *kind,
            Self::Message(_) => ErrorKind::Other,
        }
    }
}

impl From<ErrorKind> for Error {
    fn from(kind: ErrorKind) -> Self {
        Self::Kind(kind)
    }
}

impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Self {
        use std::io::ErrorKind as IoKind;
        match err.kind() {
            IoKind::NotFound => ErrorKind::NotFound.into(),
            IoKind::AlreadyExists => ErrorKind::AlreadyExists.into(),
            IoKind::PermissionDenied => ErrorKind::PermissionDenied.into(),
            IoKind::InvalidInput | IoKind::InvalidData => ErrorKind::Invalid.into(),
            IoKind::UnexpectedEof => ErrorKind::UnexpectedEof.into(),
            _ => Self::Message(err.to_string()),
        }
    }
}

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq, Serialize, Deserialize)]
pub enum ErrorKind {
    #[error("not found")]
    NotFound,
    #[error("already exists")]
    AlreadyExists,
    #[error("invalid argument")]
    Invalid,
    #[error("permission denied")]
    PermissionDenied,
    #[error("operation not supported")]
    NotSupported,
    #[error("directory not empty")]
    NotEmpty,
    #[error("file is closed")]
    Closed,
    #[error("not a directory")]
    NotDir,
    #[error("is a directory")]
    IsDir,
    #[error("unexpected eof")]
    UnexpectedEof,
    #[error("other error")]
    Other,
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct FileMode(u32);

impl FileMode {
    pub const DIR: Self = Self(1 << 31);
    pub const SYMLINK: Self = Self(1 << 30);
    pub const DEVICE: Self = Self(1 << 29);
    pub const NAMED_PIPE: Self = Self(1 << 28);
    pub const SOCKET: Self = Self(1 << 27);
    pub const EXECUTABLE: Self = Self(1 << 26);
    pub const PERM_MASK: Self = Self(0o777);

    pub const fn empty() -> Self {
        Self(0)
    }

    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    pub const fn from_perm(perm: u32) -> Self {
        Self(perm & Self::PERM_MASK.0)
    }

    pub const fn bits(self) -> u32 {
        self.0
    }

    pub const fn perm(self) -> u32 {
        self.0 & Self::PERM_MASK.0
    }

    pub const fn type_bits(self) -> Self {
        Self(self.0 & !Self::PERM_MASK.0)
    }

    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    pub const fn is_dir(self) -> bool {
        self.contains(Self::DIR)
    }

    pub const fn is_symlink(self) -> bool {
        self.contains(Self::SYMLINK)
    }

    pub const fn with_perm(self, perm: u32) -> Self {
        Self((self.0 & !Self::PERM_MASK.0) | (perm & Self::PERM_MASK.0))
    }

    pub fn unix_type_and_perm(self) -> u32 {
        let typ = if self.is_dir() {
            0o040000
        } else if self.is_symlink() {
            0o120000
        } else if self.contains(Self::NAMED_PIPE) {
            0o010000
        } else if self.contains(Self::SOCKET) {
            0o140000
        } else {
            0o100000
        };
        typ | self.perm()
    }
}

impl BitOr for FileMode {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for FileMode {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl BitAnd for FileMode {
    type Output = Self;

    fn bitand(self, rhs: Self) -> Self::Output {
        Self(self.0 & rhs.0)
    }
}

impl BitAndAssign for FileMode {
    fn bitand_assign(&mut self, rhs: Self) {
        self.0 &= rhs.0;
    }
}

impl Not for FileMode {
    type Output = Self;

    fn not(self) -> Self::Output {
        Self(!self.0)
    }
}

impl From<u32> for FileMode {
    fn from(value: u32) -> Self {
        Self(value)
    }
}

impl fmt::Display for FileMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = if self.is_dir() {
            'd'
        } else if self.is_symlink() {
            'l'
        } else {
            '-'
        };
        write!(f, "{}{:03o}", kind, self.perm())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Metadata {
    pub name: String,
    pub mode: FileMode,
    pub size: u64,
    pub modified: SystemTime,
    pub uid: u32,
    pub gid: u32,
}

impl Metadata {
    pub fn new(name: impl Into<String>, mode: FileMode) -> Self {
        let mode = mode.with_perm(mode.perm());
        let size = if mode.is_dir() { 2 } else { 0 };
        Self {
            name: name.into(),
            mode,
            size,
            modified: SystemTime::UNIX_EPOCH,
            uid: 0,
            gid: 0,
        }
    }

    pub fn dir(name: impl Into<String>, perm: u32) -> Self {
        Self::new(name, FileMode::DIR | FileMode::from_perm(perm))
    }

    pub fn file(name: impl Into<String>, perm: u32, size: u64) -> Self {
        let mut meta = Self::new(name, FileMode::from_perm(perm));
        meta.size = size;
        meta
    }

    pub fn symlink(name: impl Into<String>, size: u64) -> Self {
        let mut meta = Self::new(name, FileMode::SYMLINK | FileMode::from_perm(0o777));
        meta.size = size;
        meta
    }

    pub fn is_dir(&self) -> bool {
        self.mode.is_dir()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DirEntry {
    pub name: String,
    pub metadata: Metadata,
}

impl DirEntry {
    pub fn new(name: impl Into<String>, metadata: Metadata) -> Self {
        Self {
            name: name.into(),
            metadata,
        }
    }

    pub fn is_dir(&self) -> bool {
        self.metadata.is_dir()
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FsContext {
    pub follow_symlinks: bool,
    pub read_only: bool,
    pub origin_path: Option<String>,
    pub filepath: Option<String>,
    pub op: Option<&'static str>,
}

impl FsContext {
    pub fn new() -> Self {
        Self {
            follow_symlinks: true,
            ..Self::default()
        }
    }

    pub fn no_follow(mut self) -> Self {
        self.follow_symlinks = false;
        self
    }

    pub fn read_only(mut self) -> Self {
        self.read_only = true;
        self
    }

    pub fn with_origin(mut self, path: impl Into<String>, op: &'static str) -> Self {
        if self.origin_path.is_none() {
            let path = path.into();
            self.origin_path = Some(path.clone());
            self.filepath = Some(path);
            self.op = Some(op);
            if matches!(op, "open" | "stat" | "readdir" | "readlink") {
                self.read_only = true;
            }
        }
        self
    }

    pub fn with_filepath(mut self, path: impl Into<String>) -> Self {
        if self.filepath.is_none() {
            self.filepath = Some(path.into());
        }
        self
    }

    pub fn with_op(mut self, op: &'static str) -> Self {
        if self.op.is_none() {
            self.op = Some(op);
        }
        self
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct OpenFlags(u32);

impl OpenFlags {
    pub const RDONLY: Self = Self(0);
    pub const WRONLY: Self = Self(1);
    pub const RDWR: Self = Self(2);
    pub const CREATE: Self = Self(0o100);
    pub const EXCL: Self = Self(0o200);
    pub const TRUNC: Self = Self(0o1000);
    pub const APPEND: Self = Self(0o2000);

    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    pub const fn bits(self) -> u32 {
        self.0
    }

    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    pub const fn is_write(self) -> bool {
        (self.0 & (Self::WRONLY.0 | Self::RDWR.0)) != 0
    }
}

impl BitOr for OpenFlags {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for OpenFlags {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

pub fn clean_path(path: &str) -> String {
    if path.is_empty() || path == "." {
        return ".".to_string();
    }
    let absolute = path.starts_with('/');
    let mut parts = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            _ => parts.push(part),
        }
    }
    let cleaned = parts.join("/");
    match (absolute, cleaned.is_empty()) {
        (_, true) => ".".to_string(),
        (true, false) => cleaned.trim_start_matches('/').to_string(),
        (false, false) => cleaned,
    }
}

pub fn valid_path(path: &str) -> bool {
    if path == "." {
        return true;
    }
    if path.is_empty() || path.starts_with('/') || path.ends_with('/') {
        return false;
    }
    path.split('/')
        .all(|part| !part.is_empty() && part != "." && part != "..")
}

pub fn base_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

pub fn parent_path(path: &str) -> String {
    let cleaned = clean_path(path);
    if cleaned == "." {
        ".".to_string()
    } else {
        cleaned
            .rsplit_once('/')
            .map(|(parent, _)| parent.to_string())
            .unwrap_or_else(|| ".".to_string())
    }
}

pub fn relative_under(root: &str, name: &str) -> Option<String> {
    if root == name {
        Some(".".to_string())
    } else if root == "." {
        Some(clean_path(name))
    } else {
        name.strip_prefix(root)
            .and_then(|rest| rest.strip_prefix('/'))
            .map(clean_path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_validation_matches_iofs_shape() {
        assert!(valid_path("."));
        assert!(valid_path("a/b/#c"));
        assert!(!valid_path(""));
        assert!(!valid_path("/a"));
        assert!(!valid_path("a/"));
        assert!(!valid_path("a/../b"));
        assert!(!valid_path("a/./b"));
    }

    #[test]
    fn clean_path_keeps_paths_relative() {
        assert_eq!(clean_path("."), ".");
        assert_eq!(clean_path("./a//b"), "a/b");
        assert_eq!(clean_path("/a/b"), "a/b");
        assert_eq!(parent_path("a/b/c"), "a/b");
    }

    #[test]
    fn mode_reports_unix_type_and_perm() {
        assert_eq!(
            (FileMode::DIR | FileMode::from_perm(0o755)).unix_type_and_perm(),
            0o040755
        );
        assert_eq!(
            (FileMode::SYMLINK | FileMode::from_perm(0o777)).unix_type_and_perm(),
            0o120777
        );
    }
}
