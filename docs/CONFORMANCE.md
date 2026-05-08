# Conformance

The Rust tests are organized around the behavioral gates from `PLAN.md`. Fixtures and specs are derived from the `../wanix` reference implementation as behavior is ported, then validated independently in Rust without invoking Go code.

## Covered

- Core path validation and path cleaning.
- File mode type bits, permission bits, and Unix mode projection.
- `MemFs` create, read, write, stat, directory synthesis, directory rename, symlink, lstat, and readlink behavior.
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
- Child task namespace cloning.
- Typed protocol dispatch for JS-handle file operations.
- CBOR request/response round-tripping for the typed protocol boundary.
- Fixture coverage for all public Wanix file API operation names.
- Protocol EOF mapping to `null`-style optional bytes.
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
- HTTP filesystem caching, multipart parsing, PATCH archive updates, and real network transport.
- R2 compare-and-swap conflict handling and Cloudflare/S3 adapter wiring.
- 9P import/export bridge details.
- Browser storage backends such as OPFS and file-system-access handles.
- Terminal screen protocol details.
- VM/network device behavior.
- Full WASI syscall execution.
- Full Go-compatible JS/WASM worker execution.
