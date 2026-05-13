# wanix-rs Progress

## 2026-05-13

- Added `wanix_protocol::runtime`, a focused typed protocol module for worker spawn/start, execution specs, port open/handoff, task message payloads, stdio/fd descriptors, and exit status messages.
- Added CBOR encode/decode helpers plus round-trip tests for runtime requests, runtime responses, and task messages in `wanix-protocol`.
- Added a typed `wanix-web` descriptor module for browser binding planning with explicit `ns`, `file`, `archive`, and `import` binding kinds plus host-neutral storage backend descriptors for `opfs`, `file-system-access`, `cache`, `js-value`, `download`, `worker`, and `dom`.
- Added deterministic validation rules and Rust-native unit tests for binding path hygiene, archive/import source requirements, and backend-specific descriptor fields without invoking browser APIs.
- Switched `WanixSystem::setup_namespace_native` to validate descriptor plans up front, apply typed `ns`/`file` bindings, and reject unimplemented `archive`/`import` execution with `not supported`.
- Added task-state setters and explicit fd installation APIs so runtime execution drivers can set command, env, cwd, exit state, and standard descriptors without reaching through task internals.
- Extended the WASI/Go-compatible execution adapters to install standard fds, mark task start/finish state, and record the command used for driver startup.
- Hardened `SyncFs::pull` with an explicit `PullConflictPolicy`, retained conflict reporting, keep-local default behavior, prefer-remote overwrite behavior, and deterministic descendant-conflict tests.
- Added runtime 9P import/export hooks: a `Runtime` can export a task namespace through `NinePServer`, import any `NinePTransport` as `NinePClientFs`, and mount a loopback 9P export into its namespace.
- Added a browser-facing 9P frame buffer helper plus a `WanixSystem::mountSelf9p` smoke path that mounts the current system through the Rust 9P bridge.
- Extended the browser smoke fixture to verify 9P import/export by reading `remote/tmp/hello` through the imported mount.
- Hardened 9P walk behavior so raw server partial walks return the successful qid prefix, duplicate `newfid` walks are rejected, and the client maps partial walks to not-found instead of using a partially resolved fid.
- Added a host-neutral `BrowserStorageRegistry` in `wanix-web` that resolves typed OPFS, File System Access, Cache API, JS value, download, worker, and DOM descriptors into registered filesystems or deterministic in-memory stand-ins.
- Wired descriptor-backed storage mounts into `WanixSystem::setup_namespace_native`, with native tests for writable registered handles, persistent descriptor identities, and subpath-rooted mounts.
- Added a `RuntimeProtocolHost` and `WorkerHost` in `wanix-runtime` that handle typed runtime worker requests for spawn, start, port open/handoff, task messages, stdio/fd setup, and exit-state updates without browser or Go dependencies.
- Hardened `R2Fs` parent directory listings with an `ObjectStore::compare_and_swap` contract, bounded retry behavior, deterministic conflict errors, and in-memory object-store tests for retry success and conflict exhaustion.
- Added Rust-owned conformance fixtures for runtime protocol method coverage and representative browser binding/storage descriptor plans.
- Added `HttpFs::patch_tar` for Rust-native HTTP PATCH tar payloads, with deterministic transport-boundary tests for method, URL, headers, body, and error mapping.
- Added `Change-Timestamp` mutation headers to HTTP PUT, MOVE, DELETE, and PATCH requests.
- Added a host-neutral `MessagePort` trait and in-memory message-port channel in `wanix-web`, with tests for message-boundary preservation and lossless 9P frame transfer.
- Added `WebWorkerAdapter` and `BrowserWorkerRuntime` scaffolding for typed runtime worker requests, responses, and task messages over the message-port abstraction.
- Moved runtime device internals into a focused `devices` module and added deterministic deeper `#term`, `#vm`, and `#net` surfaces with resource allocator tests.
- Verified `cargo fmt`, `cargo test -p wanix-runtime`, `cargo test -p wanix-web`, and `cargo build -p wanix-web --target wasm32-unknown-unknown`.
- Added a Rust-native `wanix_protocol::p9` module implementing a 9P2000.L-style frame codec for version, attach, walk, open/create, getattr/setattr, read/write, clunk/remove, mkdir/readdir, renameat/unlinkat, fsync, symlink, and readlink messages.
- Added `NinePServer` over `wanix_fs::FileSystem`, with fid tracking, stable FNV-1a qids, Wanix metadata-to-9P attribute mapping, directory-entry encoding, and errno-based `Rlerror` responses.
- Added `NinePClientFs` over a synchronous frame transport trait plus `LoopbackTransport`, allowing Rust callers to mount a 9P export as a normal `FileSystem` without Go dependencies.
- Added MemFs-backed 9P tests for codec round trips, raw version/attach/walk/getattr behavior, client read/write/create/list behavior, rename/remove behavior, and directory create/remove behavior.
- Verified `cargo fmt`, `cargo test -p wanix-fs`, `cargo test -p wanix-protocol`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, and `cargo build -p wanix-web --target wasm32-unknown-unknown`.

## 2026-05-12

- Replaced the `CacheFs = MemFs` placeholder with a real Rust-native `MetaCacheFs` wrapper in `wanix-fs`.
- Ported metadata cache behavior from `../wanix/fs/metacache` into Rust: cached `stat`/`lstat`/`read_dir` results, cached errors, TTL expiry, refresh-ahead after roughly half-TTL, explicit invalidation methods, and file-handle close invalidation after writes.
- Added focused `MetaCacheFs` tests covering cached success, cached errors, TTL expiry, refresh-ahead with transient refresh failure extension, and mutation-driven invalidation of file and parent directory listings.
- Added an initial Rust-native `SyncFs` implementation in `wanix-fs` using `../wanix/fs/syncfs` as the behavior reference for a local-first model.
- Added a `RemoteSyncBackend` trait plus explicit `SyncFs::push`, `pull`, and `sync` operations, with dirty upsert/remove tracking and tar-based patch generation.
- Added focused `SyncFs` tests covering push of local writes/removes, pull of remote files and symlinks, protection of locally dirty paths during pull, and combined sync behavior.
- Review pass tightened `SyncFs::sync_fs` so it runs the wrapper sync operation, prevented no-op writable opens from marking paths dirty, and aligned delete patch entries with the reference tar PAX delete marker shape.
- Documented the current `SyncFs` scope as an initial baseline; remaining work includes richer conflict policy, background/debounced sync scheduling, and backend-specific patch semantics beyond the in-memory test backend.
- Verified `cargo fmt`, `cargo test -p wanix-fs`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, and `cargo build -p wanix-web --target wasm32-unknown-unknown`.

## 2026-05-08

- Direction changed from wrapper-first to a full Rust port in a single sprint.
- Removed sprint-length assumptions from planning.
- Rewrote `PLAN.md` around ordered workstreams, evidence gates, target runtime surfaces, and acceptance criteria.
- Clarified porting policy: use `../wanix` as the reference implementation to clone/port from, while keeping runtime, build, tests, browser smoke, and conformance free of Go dependencies.
- Created the Rust workspace with crates for core values, filesystem/backends, namespace, tasks, protocol, runtime, web, and CLI.
- Ported core path, mode, metadata, context, open flag, and error contracts.
- Ported the filesystem trait layer, helper operations, `Node`/`MapFs`, `MemFs`, `LocalFs`, union directories, field/control files, pipe, signal, cache, and tar aliases.
- Ported namespace bind/unbind, bind ordering, union directory reads, synthesized parent directories, hidden `#` listing behavior, and routed write operations.
- Ported task allocation, task fields, control file commands, aliases, per-task namespace cloning, driver registration, and file descriptor lifecycle.
- Ported the public API operation set into typed Rust request/response structures covering the Wanix file API methods.
- Added runtime root construction with `#wanix`, `#task`, pipe, signal, ramfs, terminal, VM, worker, web, JS, cache, and download surfaces.
- Added browser/WASM facade and native CLI entry points for smoke operations.
- Added `README.md`, `docs/ARCHITECTURE.md`, `docs/CONFORMANCE.md`, `tests/browser-smoke.html`, and `tests/fixtures/api-operations.json`.
- Verified `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo build -p wanix-web --target wasm32-unknown-unknown`.
- Verified the generated wasm web package with Playwright at `tests/browser-smoke.html`; the page reached `document.body.dataset.status = "ok"` after binding ramfs, running file API operations, and starting WASI/Go-compatible JS adapter tasks.
- Observed `wasm-pack test --headless --chrome crates/wanix-web` compile the wasm test target, but the ChromeDriver runner exited before executing the harness. Replaced that harness path with the direct `wasm-pack build` plus Playwright browser smoke.
- Clarified project policy in docs: `../wanix` is the reference implementation for porting, but runtime, build, test, browser smoke, and conformance paths must not wrap, execute, link, shell out to, or test against Go code.
- Replaced the `TarFs = MemFs` placeholder with a real read-only tar-backed filesystem and tar archive writer using the Rust `tar` crate.
- Added tar conformance tests for directory listings, file reads, symlink lstat/readlink/follow behavior, archive round-tripping, and read-only mutation failures.
- Added a transport-driven `HttpFs` implementation covering HTTP GET/HEAD reads, directory listing parsing, PUT writes, mkdir, symlink, MOVE rename, DELETE remove, metadata parsing, and protocol header formatting.
- Added HTTP filesystem tests with a recording transport so protocol behavior is validated without network or Go dependencies.
- Added CBOR encode/decode helpers for typed API requests/responses using `ciborium`.
- Added protocol fixture coverage to ensure all public Wanix file API operations have typed request variants.
- Verified `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, and `cargo build -p wanix-web --target wasm32-unknown-unknown`; workspace tests now cover 28 passing cases.
- Added an R2-style object storage filesystem over a Rust `ObjectStore` trait, plus an in-memory object store for conformance tests.
- Ported R2 storage-format behavior for object keys, directory listing objects, metadata fields, files, directories, symlinks, base path scoping, rename, remove, and parent listing updates.
- Verified `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, and `cargo build -p wanix-web --target wasm32-unknown-unknown`; workspace tests now cover 31 passing cases.
- Added Rust-owned xattr helper functions and `MemFs`/`Node` xattr storage for set/get/list/remove behavior.
- Verified `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, and `cargo build -p wanix-web --target wasm32-unknown-unknown`; workspace tests now cover 32 passing cases.

## 2026-05-07

- Inspected `../wanix` repository structure, build files, public entry points, and largest code surfaces.
- Confirmed `wanix-rs` is currently empty.
- Confirmed `../wanix` is a clean Git checkout on `main`.
- Measured first-party code shape after excluding vendored `misc/cbor` and generated worker bundles:
  - Largest areas are `fs`, `web`, `wasi`, `gojs`, `rc`, and device/resource packages.
  - Key boundary surfaces are the file/RPC API, 9P bridge, Plan 9-style namespace, task model, browser runtime, and filesystem implementations.
- Initial recommendation was wrapper-first; this has been superseded by the Rust-native full-port direction with no runtime dependency on legacy code.
