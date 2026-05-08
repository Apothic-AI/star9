# Architecture

`wanix-rs` is the primary Rust implementation of the Wanix runtime. The Go repository in `../wanix` remains the behavioral oracle for any semantics not yet covered by Rust conformance tests.

## Core Values

`wanix-core` owns stable value types that are shared by every other crate:

- `Error` and `ErrorKind` preserve operation/path-aware failures.
- `FileMode`, `Metadata`, and `DirEntry` model Go `io/fs`-style metadata with Wanix type bits.
- `FsContext` carries follow-symlink, read-only, origin path, filepath, and operation flags.
- `OpenFlags` preserves the public API shape used by the JS handle.
- Path helpers validate and normalize relative Wanix paths.

## Filesystems

`wanix-fs` defines `FileSystem` and `FileHandle`. Backends implement the trait directly while shared helpers provide package-level behavior equivalent to the Go `fs` helpers:

- `open`, `stat`, `lstat`, `read_dir`, `read_file`, `write_file`, `append_file`.
- `mkdir_all`, `remove_all`, `copy_all`, `copy_fs`, `exists`, `is_dir`, `is_empty`.
- Open-file fallback behavior for create, truncate, append, and chmod-like mode application.

The crate also ports the key `fskit` building blocks:

- `Node` and `NodeFile`.
- `MapFs` with synthetic parent directories.
- `UnionFs` with directory merge behavior.
- `FieldFile` and `ControlFile`.
- `MemFs`, `LocalFs`, `PipeFs`, `SignalFs`, `CacheFs`, and `TarFs` surfaces.

## Namespace

`wanix-vfs::Namespace` stores ordered bind targets keyed by destination path. It supports:

- `BindMode::After`, `Replace`, and `Before`.
- Direct file and directory bindings.
- Subpath binding resolution.
- Directory unions over multiple bindings.
- Synthesized parent directories for bind paths.
- Hidden `#` entries in directory listings while preserving direct access.
- Write routing for create, mkdir, remove, rename, chmod, chown, chtimes, truncate, symlink, and readlink.

## Tasks

`wanix-task` ports the task/resource filesystem:

- `TaskFs` allocates tasks through `new/<kind>` and exposes resources by id and alias.
- `Task` exposes `ctl`, `id`, `kind`, `cmd`, `alias`, `env`, `dir`, `exit`, `fd`, and `ns`.
- Child tasks clone the parent namespace.
- File descriptors are task-local and accessed through fd helpers or `fd/<n>` proxy files.
- Drivers implement `TaskDriver`; function drivers cover auto-selection and adapter use cases.

## Protocol

`wanix-protocol` defines typed requests and responses for the public API exposed by the Go `api.Responder` and `api/handle.js`:

`Open`, `OpenFile`, `Create`, `Close`, `Sync`, `Read`, `Write`, `WriteAt`, `ReadDir`, `Mkdir`, `MkdirAll`, `Bind`, `Unbind`, `Stat`, `Truncate`, `WaitFor`, `Rename`, `Copy`, `Remove`, `RemoveAll`, `ReadFile`, `WriteFile`, `AppendFile`, `Fstat`, `Lstat`, `Chmod`, `Chown`, `Fchmod`, `Fchown`, `Ftruncate`, `Readlink`, `Symlink`, and `Chtimes`.

`WanixApi` executes those requests against a `Task`.

## Runtime And Web

`wanix-runtime` builds the root task and binds the built-in surfaces:

- `#wanix` for version metadata.
- `#task` for task allocation and lookup.
- `#pipe`, `#signal`, `#ramfs`, `#term`, `#vm`, `#worker`, `#web`, `#js`, `#cache`, and `#download`.

`wanix-web` exposes a `wasm-bindgen` `WanixSystem` facade. Browser-specific logic stays in this crate; core runtime state remains Rust-native and host-neutral.

## Generated And Vendored Code

The Go repository contains generated worker bundles and vendored/patched support code. These are not ported line-for-line. Rust equivalents live behind task drivers, device allocators, typed protocols, and browser facade APIs.
