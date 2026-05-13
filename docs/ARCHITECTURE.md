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

`SyncFs` is currently a contained local-first wrapper over a local `FileSystem` plus a `RemoteSyncBackend` trait. It tracks dirty upserts/removes and exposes explicit `push`, `pull`, and `sync` entry points using tar-based patch payloads. The reusable `apply_sync_patch` helper applies those payloads back onto any mutable `FileSystem`, including file/directory/symlink upserts and PAX delete markers. Pull conflict handling is explicit through `PullConflictPolicy`, with keep-local default behavior, prefer-remote overwrite behavior, and optional retained conflict reporting for callers that need to surface skipped dirty paths. `DebouncedSyncScheduler` adds host-neutral scheduling hooks for pending state, due checks, immediate flush, and retry-after-error behavior. Native builds can also start a background scheduler handle that wakes on requests, preserves the debounce window, avoids retry spin after a failed request, and shuts down cleanly.

`R2Fs` is implemented over a Rust `ObjectStore` trait. The trait includes compare-and-swap for parent directory listing updates, allowing deterministic retry and conflict behavior to be tested with `InMemoryObjectStore`. `S3ObjectStore` is the S3/R2-compatible HTTP adapter for that trait: it maps object keys to bucket URLs, preserves object metadata as headers, supports GET/PUT/DELETE/list-prefix, exposes a signing hook for host credentials, and keeps live cloud behavior behind opt-in callers and fake-transport tests. `AwsSigV4Signer` provides deterministic AWS-compatible request signing at the same transport boundary without loading credentials or opening sockets itself.

`HttpFs` is implemented over a Rust `HttpTransport` trait. Mutating requests carry Wanix metadata headers plus a `Change-Timestamp`, and `patch_tar` sends complete tar patch payloads through `PATCH` without requiring a live network transport in tests. Directory reads request multipart listings when available and fall back to plain directory bodies. Caching is opt-in through `with_cache_ttl`, stores stat/node successes and not-found responses, and invalidates affected cached entries after mutations. `HttpFsHandler` is the host-neutral server-side adapter for the same protocol, mapping `HttpRequest` values onto any `FileSystem` and returning `HttpResponse` values without opening sockets; tar `PATCH` requests reuse `apply_sync_patch` so the client and server adapters share patch semantics. The non-default `native-http` feature exposes `NativeHttpTransport`, a blocking native transport over `ureq` that is tested with loopback servers and is excluded from `wasm32` builds.

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

`wanix-protocol::p9` provides the Rust-native 9P bridge baseline. It owns a 9P2000.L-style frame codec, a `NinePServer` that exports any `wanix-fs::FileSystem`, a synchronous `NinePTransport` trait, and a `NinePClientFs` that imports a remote 9P export back into the normal filesystem trait surface. The bridge includes xattr walk/create support over the existing filesystem xattr trait methods. Browser MessagePort/WebSocket adapters can wrap the frame transport without changing the core protocol implementation.

`wanix-protocol::runtime` owns typed worker/task messages for spawn/start requests, execution specs, stdio/fd descriptors, port open and handoff, task message payloads, and exit status. The protocol has Rust-owned CBOR round-trip tests and is the shared contract for runtime drivers and browser worker adapters.

`wanix-runtime` exposes helpers to export a task namespace as 9P and import a `NinePTransport` into the root namespace. `wanix-web` layers browser-facing frame helpers and smoke facade methods over those hooks while keeping the core bridge host-neutral.

`wanix-runtime::RuntimeProtocolHost` handles the typed runtime protocol in a host-neutral way. It allocates worker tasks, applies execution specs to task state and descriptors, tracks in-memory port open/handoff records, records task messages, and updates task exit state from exit messages. It exposes immutable snapshots for workers, ports, handoff targets, and task messages so browser glue, CLI acceptance, and tests can inspect lifecycle state without reaching into locks. Real browser/native execution drivers attach beyond that contract.

## Runtime And Web

`wanix-runtime` builds the root task and binds the built-in surfaces:

- `#wanix` for version metadata.
- `#task` for task allocation and lookup.
- `#pipe`, `#signal`, `#ramfs`, `#term`, `#vm`, `#worker`, `#web`, `#js`, `#cache`, `#download`, and `#net`.

Runtime device implementations live in a focused `devices` module. The terminal surface exposes deterministic program/data queues, reference-style LF-to-CRLF program write normalization, winch signaling, ctl, state, and size files. The VM surface exposes `new/<kind>` allocation, ctl-driven state, alias lookup/update, config, console, id, and kind files. `#net` is a deterministic Plan 9-style TCP state machine with dial, bind, announce, listen accept, hangup/reset, status/local/remote, and in-memory data flow; it does not open real sockets yet.

`wanix-web` exposes a `wasm-bindgen` `WanixSystem` facade. Browser-specific logic stays in this crate; core runtime state remains Rust-native and host-neutral. The plain ES module at `crates/wanix-web/js/wanix-elements.js` defines `wanix-system`, `wanix-bind`, and `wanix-task` without a bundler, lazy-loads the wasm package, and delegates file, namespace, 9P, and task operations to the Rust facade.

Browser binding and storage setup is represented by typed descriptors in `wanix-web` before touching browser APIs. Namespace/file/archive/import binds and OPFS, File System Access, Cache API, JS value, download, worker, and DOM storage plans validate independently of JS host objects so runtime behavior can be tested natively first.

`BrowserStorageRegistry` maps those storage descriptors to `FileSystem` instances. It can use registered host handles or deterministic in-memory stand-ins, preserves descriptor identity for repeated mounts, and can expose a descriptor subpath as the mounted root through the existing namespace machinery.

The JS storage modules under `crates/wanix-web/js/` provide browser-native host adapters for OPFS/File System Access, Cache API, DOM, download, JS value, and worker-backed storage. They are async host adapters with deterministic fake-host tests; mounting them into Rust namespaces is a separate integration layer because the core `FileSystem` trait is synchronous.

`wanix-web` also provides a host-neutral `MessagePort` trait, an in-memory port for native tests, and worker-runtime adapters that move typed runtime requests, responses, and task messages over complete message payloads. The JS `worker-runtime.js` module adds browser-native Worker/MessagePort helpers for tagged binary envelopes, endpoint listeners, transferred ports, system facade resolution, and import-port requests without changing protocol encoding. The JS `p9-port.js` module serves complete 9P frames over browser MessagePorts and answers `wanix-import` requests by transferring a served port backed by the Rust `WanixSystem::handle9pFrame` facade. The JS `worker-host.js` module builds on the runtime layer to spawn or attach Worker-like targets, transfer a runtime port, surface request/response/task-message listeners, and manage stop/restart/cleanup with fake-host coverage. The JS `js-wasm-worker-host.js` module layers a stable JS/WASM execution bootstrap over the same worker host so real execution shims can receive module, args, env, cwd, fd, stdio, port, task, worker, and runtime descriptor data through one message shape. The JS `js-wasm-execution-worker.js` module is the worker-side acceptor for that shape: it waits for the runtime port and execution bootstrap, builds a normalized runner context, supports dynamic JS runner imports, and reports deterministic task exit/error messages while direct `.wasm` execution remains a clear unsupported path.

`BrowserBindingRegistry` is the host-neutral source registry for `<wanix-bind>`-style file, archive, and import bindings. It maps source identifiers to bytes or 9P transports so native tests can validate file writes, tar mounts, and 9P imports. Browser custom elements register fetched file/archive bytes through the wasm facade before applying typed namespace descriptors.

`wanix-runtime::ExecutionRegistry` is the host-neutral execution contract for WASI and JS-WASM modules. Callers register native handlers by execution kind or module name; the registry applies the typed execution spec to task state and descriptors, invokes the handler, and records the returned exit status. `WasmiWasiHandler` is the first real engine-backed handler: it loads a WASI module from the task namespace, preopens the task cwd, maps args/env and fd descriptors into a preview1 syscall subset, supports fd read/write/pread/pwrite/seek/tell/allocate/renumber/stat/advice/flags/rights/timestamps/sync/truncate/readdir, poll/yield/signal imports, and path open/stat/timestamps/create/remove/rename/symlink/readlink operations over the task namespace, and returns `proc_exit` status through the normal task lifecycle.

## Generated And Vendored Code

Generated worker bundles and vendored/patched reference support code are not ported line-for-line. Rust equivalents live behind task drivers, device allocators, typed protocols, and browser facade APIs.
