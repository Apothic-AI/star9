//! Plan 9-style namespace and bind semantics.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, RwLock};

use star9_core::{
    base_name, clean_path, parent_path, valid_path, DirEntry, Error, ErrorKind, FileMode,
    FsContext, Metadata, Result,
};
use star9_fs::{directory_file, FileSystem, FsRef};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BindMode {
    After,
    Replace,
    Before,
}

#[derive(Clone)]
struct BindTarget {
    fs: FsRef,
    path: String,
    metadata: Metadata,
}

#[derive(Clone, Default)]
pub struct Namespace {
    bindings: Arc<RwLock<BTreeMap<String, Vec<BindTarget>>>>,
}

impl Namespace {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn clone_namespace(&self) -> Self {
        Self {
            bindings: Arc::new(RwLock::new(self.bindings.read().unwrap().clone())),
        }
    }

    pub fn bind(&self, src: FsRef, src_path: &str, dst_path: &str, mode: BindMode) -> Result<()> {
        if !valid_path(src_path) {
            return Err(Error::path("bind", src_path, ErrorKind::NotFound));
        }
        if !valid_path(dst_path) {
            return Err(Error::path("bind", dst_path, ErrorKind::NotFound));
        }
        let src_path = clean_path(src_path);
        let dst_path = clean_path(dst_path);
        let metadata = src.stat(&FsContext::new().with_origin(&src_path, "stat"), &src_path)?;
        let target = BindTarget {
            fs: src,
            path: src_path,
            metadata,
        };
        let mut bindings = self.bindings.write().unwrap();
        match mode {
            BindMode::After => bindings.entry(dst_path).or_default().insert(0, target),
            BindMode::Before => bindings.entry(dst_path).or_default().push(target),
            BindMode::Replace => {
                bindings.insert(dst_path, vec![target]);
            }
        }
        Ok(())
    }

    pub fn unbind(&self, src: &FsRef, src_path: &str, dst_path: &str) -> Result<()> {
        if !valid_path(src_path) {
            return Err(Error::path("unbind", src_path, ErrorKind::NotFound));
        }
        if !valid_path(dst_path) {
            return Err(Error::path("unbind", dst_path, ErrorKind::NotFound));
        }
        let src_path = clean_path(src_path);
        let dst_path = clean_path(dst_path);
        let mut bindings = self.bindings.write().unwrap();
        if let Some(targets) = bindings.get_mut(&dst_path) {
            targets.retain(|target| !(Arc::ptr_eq(&target.fs, src) && target.path == src_path));
            if targets.is_empty() {
                bindings.remove(&dst_path);
            }
        }
        Ok(())
    }

    pub fn unbind_path(&self, dst_path: &str) -> Result<()> {
        if !valid_path(dst_path) {
            return Err(Error::path("unmount", dst_path, ErrorKind::NotFound));
        }
        let dst_path = clean_path(dst_path);
        let removed = self.bindings.write().unwrap().remove(&dst_path);
        if removed.is_some() {
            Ok(())
        } else {
            Err(Error::path("unmount", dst_path, ErrorKind::NotFound))
        }
    }

    pub fn unbind_all(&self) {
        self.bindings
            .write()
            .unwrap()
            .retain(|path, _| path.starts_with('#'));
    }

    pub fn binding_paths(&self) -> Vec<String> {
        self.bindings.read().unwrap().keys().cloned().collect()
    }

    pub fn format_bindings(&self) -> String {
        let bindings = self.bindings.read().unwrap();
        let mut lines = Vec::new();
        for (dst, targets) in bindings.iter() {
            for target in targets {
                lines.push(format!("{dst} -> fs:{}", target.path));
            }
        }
        lines.join("\n")
    }

    fn direct_targets(&self, name: &str) -> Option<Vec<BindTarget>> {
        self.bindings.read().unwrap().get(name).cloned()
    }

    fn matching_targets(&self, name: &str) -> Vec<(String, Vec<BindTarget>)> {
        let bindings = self.bindings.read().unwrap();
        let mut out: Vec<_> = bindings
            .iter()
            .filter_map(|(bind_path, targets)| {
                if bind_path == "."
                    || bind_path == name
                    || name.starts_with(&format!("{bind_path}/"))
                {
                    Some((bind_path.clone(), targets.clone()))
                } else {
                    None
                }
            })
            .collect();
        out.sort_by_key(|entry| std::cmp::Reverse(entry.0.len()));
        out
    }

    fn target_path(bind_path: &str, target_path: &str, name: &str) -> String {
        if bind_path == name {
            clean_path(target_path)
        } else if bind_path == "." {
            if target_path == "." {
                clean_path(name)
            } else {
                clean_path(&format!("{target_path}/{name}"))
            }
        } else {
            let rel = name
                .strip_prefix(bind_path)
                .and_then(|rest| rest.strip_prefix('/'))
                .unwrap_or(".");
            if target_path == "." {
                clean_path(rel)
            } else {
                clean_path(&format!("{target_path}/{rel}"))
            }
        }
    }

    fn synthesize_entries(&self, ctx: &FsContext, name: &str) -> Vec<DirEntry> {
        let mut entries = Vec::new();
        let mut need = BTreeSet::new();
        let bindings = self.bindings.read().unwrap();
        if name == "." {
            for (path, targets) in bindings.iter() {
                if let Some((head, _)) = path.split_once('/') {
                    need.insert(head.to_string());
                } else if path != "." {
                    for target in targets {
                        let mut meta = target
                            .fs
                            .stat(ctx, &target.path)
                            .unwrap_or_else(|_| target.metadata.clone());
                        meta.name = path.clone();
                        entries.push(DirEntry::new(path.clone(), meta));
                    }
                }
            }
        } else {
            let prefix = format!("{name}/");
            for (path, targets) in bindings.iter() {
                if let Some(rest) = path.strip_prefix(&prefix) {
                    if let Some((head, _)) = rest.split_once('/') {
                        need.insert(head.to_string());
                    } else {
                        for target in targets {
                            let mut meta = target
                                .fs
                                .stat(ctx, &target.path)
                                .unwrap_or_else(|_| target.metadata.clone());
                            meta.name = rest.to_string();
                            entries.push(DirEntry::new(rest.to_string(), meta));
                        }
                    }
                }
            }
        }
        for entry in &entries {
            need.remove(&entry.name);
        }
        for dir in need {
            entries.push(DirEntry::new(dir.clone(), Metadata::dir(dir, 0o755)));
        }
        entries
    }

    fn route_write<F, T>(&self, name: &str, op: &'static str, mut f: F) -> Result<T>
    where
        F: FnMut(&BindTarget, String) -> Result<T>,
    {
        let name = clean_path(name);
        if let Some(targets) = self.direct_targets(&name) {
            let mut last = ErrorKind::NotSupported.into();
            for target in targets {
                match f(&target, target.path.clone()) {
                    Ok(value) => return Ok(value),
                    Err(err) if err.kind() == ErrorKind::NotSupported => last = err,
                    Err(err) => return Err(err),
                }
            }
            return Err(last);
        }
        for (bind_path, targets) in self.matching_targets(&name) {
            for target in targets {
                let full = Self::target_path(&bind_path, &target.path, &name);
                match f(&target, full) {
                    Ok(value) => return Ok(value),
                    Err(err)
                        if matches!(
                            err.kind(),
                            ErrorKind::NotFound | ErrorKind::NotSupported | ErrorKind::NotDir
                        ) => {}
                    Err(err) => return Err(err),
                }
            }
        }
        Err(Error::path(op, name, ErrorKind::NotFound))
    }

    fn target_matches(bind_path: &str, name: &str) -> bool {
        bind_path == "."
            || bind_path == name
            || name
                .strip_prefix(bind_path)
                .is_some_and(|rest| rest.starts_with('/'))
    }
}

impl FileSystem for Namespace {
    fn open(&self, ctx: &FsContext, name: &str) -> Result<star9_fs::BoxFile> {
        if !valid_path(name) {
            return Err(Error::path("open", name, ErrorKind::NotFound));
        }
        let ctx = ctx.clone().with_origin(name, "open");
        let name = clean_path(name);
        let mut dir_meta: Option<Metadata> = None;
        let mut entries = Vec::new();

        if let Some(targets) = self.direct_targets(&name) {
            for target in targets {
                let meta = target
                    .fs
                    .stat(&ctx, &target.path)
                    .unwrap_or_else(|_| target.metadata.clone());
                if meta.is_dir() {
                    if dir_meta.is_none() {
                        dir_meta = Some(meta.clone());
                    }
                    entries.extend(target.fs.read_dir(&ctx, &target.path)?);
                } else {
                    return target.fs.open(&ctx, &target.path);
                }
            }
        }

        for (bind_path, targets) in self.matching_targets(&name) {
            if bind_path == name {
                continue;
            }
            for target in targets {
                let full = Self::target_path(&bind_path, &target.path, &name);
                match target.fs.stat(&ctx, &full) {
                    Ok(meta) if meta.is_dir() => {
                        if dir_meta.is_none() {
                            dir_meta = Some(meta.clone());
                        }
                        entries.extend(target.fs.read_dir(&ctx, &full)?);
                    }
                    Ok(_) => return target.fs.open(&ctx, &full),
                    Err(err) if err.kind() == ErrorKind::NotFound => {}
                    Err(err) => return Err(err),
                }
            }
        }

        entries.extend(self.synthesize_entries(&ctx, &name));
        if !entries.is_empty() || dir_meta.is_some() || name == "." {
            let meta = dir_meta.unwrap_or_else(|| Metadata::dir(base_name(&name), 0o755));
            return Ok(directory_file(meta, entries));
        }
        Err(Error::path("open", name, ErrorKind::NotFound))
    }

    fn stat(&self, ctx: &FsContext, name: &str) -> Result<Metadata> {
        if !valid_path(name) {
            return Err(Error::path("stat", name, ErrorKind::NotFound));
        }
        let ctx = ctx.clone().with_origin(name, "stat");
        let name = clean_path(name);
        if name == "." {
            return Ok(Metadata::dir(".", 0o755));
        }
        if let Some(targets) = self.direct_targets(&name) {
            for target in targets {
                if let Ok(mut meta) = target.fs.stat(&ctx, &target.path) {
                    meta.name = base_name(&name).to_string();
                    return Ok(meta);
                }
            }
        }
        for (bind_path, targets) in self.matching_targets(&name) {
            if bind_path == name {
                continue;
            }
            for target in targets {
                let full = Self::target_path(&bind_path, &target.path, &name);
                match target.fs.stat(&ctx, &full) {
                    Ok(mut meta) => {
                        meta.name = base_name(&name).to_string();
                        return Ok(meta);
                    }
                    Err(err) if err.kind() == ErrorKind::NotFound => {}
                    Err(err) => return Err(err),
                }
            }
        }
        if !self.synthesize_entries(&ctx, &name).is_empty() {
            return Ok(Metadata::dir(base_name(&name), 0o755));
        }
        Err(Error::path("stat", name, ErrorKind::NotFound))
    }

    fn create(&self, name: &str) -> Result<star9_fs::BoxFile> {
        self.route_write(name, "create", |target, full| {
            if target
                .fs
                .stat(&FsContext::new(), &parent_path(&full))
                .is_ok()
            {
                target.fs.create(&full)
            } else {
                Err(Error::path("create", full, ErrorKind::NotFound))
            }
        })
    }

    fn mkdir(&self, name: &str, perm: FileMode) -> Result<()> {
        self.route_write(name, "mkdir", |target, full| {
            if target
                .fs
                .stat(&FsContext::new(), &parent_path(&full))
                .is_ok()
            {
                target.fs.mkdir(&full, perm)
            } else {
                Err(Error::path("mkdir", full, ErrorKind::NotFound))
            }
        })
    }

    fn remove(&self, name: &str) -> Result<()> {
        self.route_write(name, "remove", |target, full| target.fs.remove(&full))
    }

    fn rename(&self, old: &str, new: &str) -> Result<()> {
        self.route_write(old, "rename", |target, full| {
            let bind_path = self
                .matching_targets(old)
                .into_iter()
                .find(|(_, targets)| {
                    targets
                        .iter()
                        .any(|candidate| Arc::ptr_eq(&candidate.fs, &target.fs))
                })
                .map(|(path, _)| path)
                .unwrap_or_else(|| ".".to_string());
            let new_full = Self::target_path(&bind_path, &target.path, new);
            target.fs.rename(&full, &new_full)
        })
    }

    fn link(&self, old: &str, new: &str) -> Result<()> {
        let old = clean_path(old);
        let new = clean_path(new);
        if !valid_path(&old) || !valid_path(&new) {
            return Err(Error::path("link", old, ErrorKind::NotFound));
        }
        let old_targets = self.matching_targets(&old);
        let new_targets = self.matching_targets(&new);
        let mut last: Error = Error::path("link", &new, ErrorKind::NotFound);
        for (new_bind_path, new_bind_targets) in &new_targets {
            if !Self::target_matches(new_bind_path, &new) {
                continue;
            }
            for new_target in new_bind_targets {
                let new_full = Self::target_path(new_bind_path, &new_target.path, &new);
                for (old_bind_path, old_bind_targets) in &old_targets {
                    if !Self::target_matches(old_bind_path, &old) {
                        continue;
                    }
                    for old_target in old_bind_targets {
                        if !Arc::ptr_eq(&old_target.fs, &new_target.fs) {
                            continue;
                        }
                        let old_full = Self::target_path(old_bind_path, &old_target.path, &old);
                        match new_target.fs.link(&old_full, &new_full) {
                            Ok(()) => return Ok(()),
                            Err(err)
                                if matches!(
                                    err.kind(),
                                    ErrorKind::NotFound
                                        | ErrorKind::NotSupported
                                        | ErrorKind::NotDir
                                ) =>
                            {
                                last = err
                            }
                            Err(err) => return Err(err),
                        }
                    }
                }
            }
        }
        Err(last)
    }

    fn chmod(&self, name: &str, mode: FileMode) -> Result<()> {
        self.route_write(name, "chmod", |target, full| target.fs.chmod(&full, mode))
    }

    fn chown(&self, name: &str, uid: u32, gid: u32) -> Result<()> {
        self.route_write(name, "chown", |target, full| {
            target.fs.chown(&full, uid, gid)
        })
    }

    fn chtimes(&self, name: &str, mtime: std::time::SystemTime) -> Result<()> {
        self.route_write(name, "chtimes", |target, full| {
            target.fs.chtimes(&full, mtime)
        })
    }

    fn truncate(&self, name: &str, size: u64) -> Result<()> {
        self.route_write(name, "truncate", |target, full| {
            target.fs.truncate(&full, size)
        })
    }

    fn symlink(&self, old: &str, new: &str) -> Result<()> {
        self.route_write(new, "symlink", |target, full| target.fs.symlink(old, &full))
    }

    fn readlink(&self, name: &str) -> Result<String> {
        self.route_write(name, "readlink", |target, full| target.fs.readlink(&full))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use star9_fs::{fs_ref, open, read_dir, read_file, write_file, MemFs};

    #[test]
    fn binds_file_and_directory() {
        let fs = fs_ref(MemFs::from_entries([
            ("file1.txt", b"content1".to_vec()),
            ("dir/file2.txt", b"content2".to_vec()),
        ]));
        let ns = Namespace::new();
        ns.bind(fs.clone(), "file1.txt", "bound-file.txt", BindMode::Replace)
            .unwrap();
        assert_eq!(read_file(&ns, "bound-file.txt").unwrap(), b"content1");
        ns.bind(fs, ".", "bound-dir", BindMode::Replace).unwrap();
        assert_eq!(
            read_file(&ns, "bound-dir/dir/file2.txt").unwrap(),
            b"content2"
        );
    }

    #[test]
    fn unions_overlapping_directories() {
        let fs1 = fs_ref(MemFs::from_entries([("a", b"a".to_vec())]));
        let fs2 = fs_ref(MemFs::from_entries([("b", b"b".to_vec())]));
        let ns = Namespace::new();
        ns.bind(fs1, ".", ".", BindMode::After).unwrap();
        ns.bind(fs2, ".", ".", BindMode::After).unwrap();
        let names: Vec<_> = read_dir(&ns, ".")
            .unwrap()
            .into_iter()
            .map(|entry| entry.name)
            .collect();
        assert_eq!(names, vec!["a", "b"]);
    }

    #[test]
    fn synthesizes_parent_directories_and_hides_hash_entries() {
        let fs = fs_ref(MemFs::from_entries([("a", b"a".to_vec())]));
        let ns = Namespace::new();
        ns.bind(fs.clone(), ".", "#hidden", BindMode::Replace)
            .unwrap();
        ns.bind(fs, ".", "visible/sub", BindMode::Replace).unwrap();
        assert!(read_dir(&ns, ".")
            .unwrap()
            .iter()
            .all(|entry| entry.name != "#hidden"));
        assert_eq!(
            read_dir(&ns, "visible")
                .unwrap()
                .into_iter()
                .map(|entry| entry.name)
                .collect::<Vec<_>>(),
            vec!["sub"]
        );
    }

    #[test]
    fn create_routes_to_writable_binding_parent() {
        let fs = fs_ref(MemFs::new());
        let ns = Namespace::new();
        ns.bind(fs.clone(), ".", "tmp", BindMode::Replace).unwrap();
        write_file(&ns, "tmp/file", b"value", FileMode::from_perm(0o644)).unwrap();
        assert_eq!(read_file(fs.as_ref(), "file").unwrap(), b"value");
    }

    #[test]
    fn unbind_path_removes_exact_mountpoint() {
        let fs = fs_ref(MemFs::from_entries([("file", b"value".to_vec())]));
        let ns = Namespace::new();
        ns.bind(fs, ".", "mnt/export", BindMode::Replace).unwrap();

        assert_eq!(read_file(&ns, "mnt/export/file").unwrap(), b"value");
        ns.unbind_path("mnt/export").unwrap();
        assert_eq!(
            read_file(&ns, "mnt/export/file").unwrap_err().kind(),
            ErrorKind::NotFound
        );
        assert_eq!(
            ns.unbind_path("mnt/export").unwrap_err().kind(),
            ErrorKind::NotFound
        );
    }

    #[test]
    fn link_routes_within_same_bound_filesystem() {
        let fs = fs_ref(MemFs::from_entries([("file", b"before".to_vec())]));
        let ns = Namespace::new();
        ns.bind(fs.clone(), ".", "workspace", BindMode::Replace)
            .unwrap();

        ns.link("workspace/file", "workspace/linked").unwrap();
        let mut linked = open(&ns, "workspace/linked").unwrap();
        linked.write(b"shared").unwrap();
        linked.close().unwrap();

        assert_eq!(read_file(fs.as_ref(), "file").unwrap(), b"shared");
        assert_eq!(read_file(&ns, "workspace/linked").unwrap(), b"shared");
    }

    #[test]
    fn link_rejects_cross_filesystem_targets() {
        let left = fs_ref(MemFs::from_entries([("file", b"left".to_vec())]));
        let right = fs_ref(MemFs::new());
        let ns = Namespace::new();
        ns.bind(left, ".", "left", BindMode::Replace).unwrap();
        ns.bind(right, ".", "right", BindMode::Replace).unwrap();

        assert_eq!(
            ns.link("left/file", "right/linked").unwrap_err().kind(),
            ErrorKind::NotFound
        );
    }
}
