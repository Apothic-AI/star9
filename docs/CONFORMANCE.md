# Conformance

The Rust tests are organized around the behavioral gates from `PLAN.md`. This repository's specs, fixtures, and conformance tests are the authoritative behavior source.

## Covered

- Core path validation and path cleaning.
- File mode type bits, permission bits, and Unix mode projection.
- `MemFs` create, read, write, stat, directory synthesis, directory rename, symlink, lstat, and readlink behavior.
- `MapFs` mount exposure and synthetic parent directories.
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
- 9P import/export bridge details.
- Browser storage backends such as OPFS and file-system-access handles.
- Terminal screen protocol details.
- VM/network device behavior.
- Full WASI syscall execution.
- Full Go-compatible JS/WASM worker execution.
