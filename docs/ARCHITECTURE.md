# Architecture

`wanix-rs` is the primary Rust implementation of the Wanix runtime. `../wanix` is the reference implementation for cloning and porting behavior, but runtime behavior is delivered entirely by this repository's Rust code, specs, fixtures, and conformance tests.

## Core Values

`wanix-core` owns stable value types that are shared by every other crate:

- `Error` and `ErrorKind` preserve operation/path-aware failures.
- `FileMode`, `Metadata`, and `DirEntry` model Wanix filesystem metadata and type bits.
- `FsContext` carries follow-symlink, read-only, origin path, filepath, and operation flags.
- `OpenFlags` preserves the public API shape used by the JS handle.
- Path helpers validate and normalize relative Wanix paths.

## Filesystems

`wanix-fs` defines `FileSystem` and `FileHandle`. Backends implement the trait directly while shared helpers provide package-level behavior for common Wanix filesystem operations:

- `open`, `stat`, `lstat`, `read_dir`, `read_file`, `write_file`, `append_file`.
- `mkdir_all`, `remove_all`, `copy_all`, `copy_fs`, `exists`, `is_dir`, `is_empty`.
- Open-file fallback behavior for create, truncate, append, and chmod-like mode application.

The crate also ports the key `fskit` building blocks:

- `Node` and `NodeFile`.
- `MapFs` with synthetic parent directories.
- `UnionFs` with directory merge behavior.
- `FieldFile` and `ControlFile`.
- `MemFs`, `LocalFs`, `TarFs`, `HttpFs`, `R2Fs`, `PipeFs`, `SignalFs`, `MetaCacheFs`, and an initial `SyncFs` surface.

`MetaCacheFs` wraps any `FileSystem` and caches `stat`, `lstat`, and `read_dir` metadata with TTL expiry, half-TTL refresh-ahead, cached errors, explicit invalidation, and close-time invalidation after writes.

`SyncFs` is currently a contained local-first wrapper over a local `FileSystem` plus a `RemoteSyncBackend` trait. It tracks dirty upserts/removes and exposes explicit `push`, `pull`, and `sync` entry points using tar-based patch payloads. Pull conflict handling is explicit through `PullConflictPolicy`, with keep-local default behavior, prefer-remote overwrite behavior, and optional retained conflict reporting for callers that need to surface skipped dirty paths.

`R2Fs` is implemented over a Rust `ObjectStore` trait. The trait includes compare-and-swap for parent directory listing updates, allowing deterministic retry and conflict behavior to be tested with `InMemoryObjectStore` while keeping remote Cloudflare/S3 adapter wiring separate from the storage-format semantics.

`HttpFs` is implemented over a Rust `HttpTransport` trait. Mutating requests carry Wanix metadata headers plus a `Change-Timestamp`, and `patch_tar` sends complete tar patch payloads through `PATCH` without requiring a live network transport in tests.

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
- Runtime drivers can set task command/env/cwd/exit state and install explicit descriptors, including standard fds, through public task APIs.
- Drivers implement `TaskDriver`; function drivers cover auto-selection and adapter use cases.

## Protocol

`wanix-protocol` defines typed requests and responses for the public Wanix file API:

`Open`, `OpenFile`, `Create`, `Close`, `Sync`, `Read`, `Write`, `WriteAt`, `ReadDir`, `Mkdir`, `MkdirAll`, `Bind`, `Unbind`, `Stat`, `Truncate`, `WaitFor`, `Rename`, `Copy`, `Remove`, `RemoveAll`, `ReadFile`, `WriteFile`, `AppendFile`, `Fstat`, `Lstat`, `Chmod`, `Chown`, `Fchmod`, `Fchown`, `Ftruncate`, `Readlink`, `Symlink`, and `Chtimes`.

`WanixApi` executes those requests against a `Task`.

Typed requests and responses can be encoded and decoded as CBOR through `wanix-protocol`, keeping the wire boundary Rust-owned while matching the public Wanix operation set.

`wanix-protocol::p9` provides the Rust-native 9P bridge baseline. It owns a 9P2000.L-style frame codec, a `NinePServer` that exports any `wanix-fs::FileSystem`, a synchronous `NinePTransport` trait, and a `NinePClientFs` that imports a remote 9P export back into the normal filesystem trait surface. Browser MessagePort/WebSocket adapters can wrap the frame transport without changing the core protocol implementation.

`wanix-protocol::runtime` owns typed worker/task messages for spawn/start requests, execution specs, stdio/fd descriptors, port open and handoff, task message payloads, and exit status. The protocol has Rust-owned CBOR round-trip tests and is the shared contract for runtime drivers and browser worker adapters.

`wanix-runtime` exposes helpers to export a task namespace as 9P and import a `NinePTransport` into the root namespace. `wanix-web` layers browser-facing frame helpers and smoke facade methods over those hooks while keeping the core bridge host-neutral.

`wanix-runtime::RuntimeProtocolHost` handles the typed runtime protocol in a host-neutral way. It allocates worker tasks, applies execution specs to task state and descriptors, tracks in-memory port open/handoff records, records task messages, and updates task exit state from exit messages. Real browser/native execution drivers attach beyond that contract.

## Runtime And Web

`wanix-runtime` builds the root task and binds the built-in surfaces:

- `#wanix` for version metadata.
- `#task` for task allocation and lookup.
- `#pipe`, `#signal`, `#ramfs`, `#term`, `#vm`, `#worker`, `#web`, `#js`, `#cache`, and `#download`.

`wanix-web` exposes a `wasm-bindgen` `WanixSystem` facade. Browser-specific logic stays in this crate; core runtime state remains Rust-native and host-neutral.

Browser binding and storage setup is represented by typed descriptors in `wanix-web` before touching browser APIs. Namespace/file/archive/import binds and OPFS, File System Access, Cache API, JS value, download, worker, and DOM storage plans validate independently of JS host objects so runtime behavior can be tested natively first.

`BrowserStorageRegistry` maps those storage descriptors to `FileSystem` instances. It can use registered host handles or deterministic in-memory stand-ins, preserves descriptor identity for repeated mounts, and can expose a descriptor subpath as the mounted root through the existing namespace machinery.

`wanix-web` also provides a host-neutral `MessagePort` trait, an in-memory port for native tests, and worker-runtime adapters that move typed runtime requests, responses, and task messages over complete message payloads. Browser-specific `Worker` and `MessagePort` glue can attach to that surface without changing protocol encoding.

## Generated And Vendored Code

Generated worker bundles and vendored/patched reference support code are not ported line-for-line. Rust equivalents live behind task drivers, device allocators, typed protocols, and browser facade APIs.
