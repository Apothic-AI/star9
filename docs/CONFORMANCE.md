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
- `TarFs` archive reads, directory listings, symlink lstat/readlink/follow behavior, archive round-tripping, and read-only mutation failures.
- `MapFs` mount exposure and synthetic parent directories.
- `HttpFs` GET/HEAD reads, directory listing parsing, PUT writes, mkdir, symlink, MOVE rename, DELETE remove, metadata parsing, and protocol header formatting through a Rust recording transport.
- `R2Fs` object key scoping, directory listing objects, metadata fields, files, directories, symlinks, base path scoping, rename, remove, and parent listing updates through a Rust object store trait.
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
- Fixture coverage for all public Wanix file API operation names.
- Protocol EOF mapping to `null`-style optional bytes.
- Rust-native 9P2000.L frame encode/decode coverage for core import/export messages.
- 9P server attach, walk, and getattr behavior against a `MemFs` export.
- 9P client filesystem behavior over loopback transport for read, write, create, readdir, rename, remove, mkdir, and rmdir operations.
- 9P partial-walk edge behavior, duplicate `newfid` rejection, and client not-found mapping for partial remote walks.
- Runtime 9P namespace export and loopback import behavior.
- Browser smoke coverage for reading files through a Rust 9P imported mount.
- Typed browser binding/storage descriptor validation for namespace, file, archive, import, OPFS, File System Access, Cache API, JS value, download, worker, and DOM plans.
- Runtime root bindings for core and device surfaces.
- Device allocator resource creation.
- WASI and Go-compatible JS execution adapter task starts.
- Native `WanixSystem` smoke operations.
- Browser wasm smoke operations through `tests/browser-smoke.html`.

## Explicit Replacement Fixtures

`tests/browser-smoke.html` replaces the representative browser examples as a Rust-backed acceptance path. It initializes the wasm package, binds a ramfs, writes and reads a file through the public API, lists the directory, and starts WASI/Go JS adapter tasks.

`tests/fixtures/api-operations.json` lists the public operation names used by the typed protocol boundary.

## Remaining Oracle Areas

These surfaces are represented in Rust but should continue to be expanded with differential or fixture-backed tests as behavior becomes more specific:

- HTTP filesystem caching and remote metadata semantics.
- `SyncFs` background/debounced scheduling and backend-specific patch/application semantics beyond the in-memory test backend.
- HTTP filesystem caching, multipart parsing, PATCH archive updates, and real network transport.
- R2 compare-and-swap conflict handling and Cloudflare/S3 adapter wiring.
- Cross-document/browser MessagePort transport wiring for remote 9P import/export beyond loopback smoke.
- Additional 9P edge cases such as flush cancellation, xattr messages, and remote conflict/error parity.
- Browser storage backends such as OPFS and file-system-access handles.
- Browser worker adapter wiring for the typed runtime protocol.
- Terminal screen protocol details.
- VM/network device behavior.
- Full WASI syscall execution.
- Full Go-compatible JS/WASM worker execution.
