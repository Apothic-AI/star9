# Conformance

The Rust tests are organized around the behavioral gates from `PLAN.md`. Fixtures and specs are derived from the `../wanix` reference implementation as behavior is ported, then validated independently in Rust without invoking Go code.

## Covered

- Core path validation and path cleaning.
- File mode type bits, permission bits, and Unix mode projection.
- `MemFs` create, read, write, stat, directory synthesis, directory rename, symlink, lstat, and readlink behavior.
- `MemFs`/`Node` xattr set, get, list, remove, and missing-attribute behavior.
- `MetaCacheFs` cached success/error behavior, TTL expiry, refresh-ahead, and mutation invalidation.
- `SyncFs` local-first dirty tracking plus explicit push/pull/sync behavior over tar patch payloads.
- `SyncFs` pull conflict behavior for default keep-local semantics, explicit prefer-remote overwrites, retained conflict reporting, and dirty descendant protection.
- `SyncFs` debounced scheduling hooks for deterministic pending state, due checks, immediate flush, last-error reporting, and retry-after-failure behavior.
- `TarFs` archive reads, directory listings, symlink lstat/readlink/follow behavior, archive round-tripping, and read-only mutation failures.
- `MapFs` mount exposure and synthetic parent directories.
- `HttpFs` GET/HEAD reads, directory listing parsing, PUT writes, mkdir, symlink, MOVE rename, DELETE remove, metadata parsing, and protocol header formatting through a Rust recording transport.
- `HttpFs` PATCH tar payload transport behavior and mutating request `Change-Timestamp` headers through a Rust recording transport.
- `R2Fs` object key scoping, directory listing objects, metadata fields, files, directories, symlinks, base path scoping, rename, remove, and parent listing updates through a Rust object store trait.
- `R2Fs` parent listing compare-and-swap retry behavior and deterministic conflict exhaustion through Rust in-memory object stores.
- Pipe bidirectional reads and writes with file close preserving the underlying pipe.
- Namespace file and directory binds.
- Namespace root unions and overlapping directory reads.
- Namespace synthesized parent directories.
- Namespace hidden `#` listings with direct hidden-path access.
- Namespace create routing into writable bindings.
- Task allocation through `TaskFs`.
- Task field reads and alias updates.
- Task fd open, read, close, and invalid-fd behavior.
- Task command, environment, cwd, exit-state setters, explicit standard fd installation, and fd listing behavior.
- Child task namespace cloning.
- Typed protocol dispatch for JS-handle file operations.
- CBOR request/response round-tripping for the typed protocol boundary.
- CBOR round-tripping for typed runtime worker spawn/start, execution, port handoff, task messages, stdio/fd descriptors, and exit status.
- Runtime protocol host handling for worker spawn/start, stdio/fd setup, port open/handoff, task messages, and exit-state updates.
- Browser worker/message-port adapter coverage for typed runtime request/response dispatch, task message delivery, and lossless 9P frame transfer through a host-neutral port.
- Host-neutral browser binding source registry coverage for file byte sources, tar archive mounts, and 9P import transports.
- Native execution registry coverage for missing-handler behavior plus deterministic WASI and JS-WASM handlers that exercise task namespace files, stdio/fd descriptors, args/env/cwd, and exit status.
- Fixture coverage for all public Wanix file API operation names.
- Protocol EOF mapping to `null`-style optional bytes.
- Rust-native 9P2000.L frame encode/decode coverage for core import/export messages.
- 9P server attach, walk, and getattr behavior against a `MemFs` export.
- 9P client filesystem behavior over loopback transport for read, write, create, readdir, rename, remove, mkdir, and rmdir operations.
- 9P partial-walk edge behavior, duplicate `newfid` rejection, and client not-found mapping for partial remote walks.
- Runtime 9P namespace export and loopback import behavior.
- Browser smoke coverage for reading files through a Rust 9P imported mount.
- Typed browser binding/storage descriptor validation for namespace, file, archive, import, OPFS, File System Access, Cache API, JS value, download, worker, and DOM plans.
- Host-neutral browser storage registry resolution for writable registered handles, persistent descriptor identities, and subpath-rooted mounts.
- Runtime root bindings for core and device surfaces.
- Device allocator resource creation.
- Terminal device program/data queues, winch signal path, ctl clear/reset/noop behavior, state, and size files.
- VM device ctl start/stop/reset behavior, state, alias/config fields, console log, id, and kind files.
- Network placeholder allocator with per-resource ctl/data/status files and deterministic connect/listen/close/reset behavior.
- WASI and Go-compatible JS execution adapter task starts.
- Native `WanixSystem` smoke operations.
- Browser wasm smoke operations through `tests/browser-smoke.html`.

## Explicit Replacement Fixtures

`tests/browser-smoke.html` replaces the representative browser examples as a Rust-backed acceptance path. It initializes the wasm package, binds a ramfs, mounts descriptor-backed storage, writes and reads files through the public API and 9P loopback import, lists directories, verifies task fields, and starts WASI/Go JS adapter tasks.

`tests/fixtures/api-operations.json` lists the public operation names used by the typed protocol boundary.

`tests/fixtures/runtime-requests.json` lists the typed runtime protocol method names for worker and port dispatch.

`tests/fixtures/browser-bindings.json` captures representative validated browser binding/storage plans for namespace, file, archive, import, and browser storage backends.

## Remaining Oracle Areas

These surfaces are represented in Rust but should continue to be expanded with differential or fixture-backed tests as behavior becomes more specific:

- HTTP filesystem caching and remote metadata semantics.
- `SyncFs` host timer integration and backend-specific patch/application semantics beyond the in-memory test backend.
- HTTP filesystem caching, multipart parsing, PATCH archive application semantics, and real network transport.
- Cloudflare/S3 adapter wiring for the Rust `ObjectStore` contract.
- Cross-document/browser MessagePort transport wiring for remote 9P import/export beyond loopback smoke.
- Additional 9P edge cases such as flush cancellation, xattr messages, and remote conflict/error parity.
- Browser JS glue for real fetch/archive/import sources and OPFS, File System Access, Cache API, JS value, DOM, download, and worker-backed storage handles.
- Browser JS glue for real Worker and MessagePort objects on top of the host-neutral typed runtime adapter.
- Terminal screen protocol details beyond the host-neutral file protocol.
- Real VM execution and network/TCP transport behavior beyond deterministic placeholder resources.
- Full WASI syscall execution.
- Full Go-compatible JS/WASM worker execution.
