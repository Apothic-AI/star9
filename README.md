# wanix-rs

Rust-native Wanix runtime.

`../wanix` is the reference implementation for porting behavior into Rust. This project does not wrap, execute, link, shell out to, or test against Go code; runtime behavior is delivered by this repository's Rust code, fixtures, and tests.

This workspace implements the main Wanix runtime surfaces in Rust:

- Plan 9-style namespaces and bind semantics.
- Filesystem traits, helpers, metadata, paths, file handles, and open flags.
- In-memory, local, map, union, pipe, signal, tar-compatible, metadata-cache, and initial sync filesystem surfaces.
- HTTP filesystem protocol semantics through a Rust transport abstraction.
- R2-style object storage semantics through a Rust object-store abstraction.
- Explicit metadata caching through `MetaCacheFs` and local-first sync orchestration through `SyncFs`.
- Task/resource filesystem with per-task namespaces, aliases, drivers, and file descriptors.
- Typed public file API for the Wanix JS handle operation set.
- CBOR encode/decode helpers for the typed public API boundary.
- Runtime root construction with built-in `#wanix`, `#task`, `#pipe`, `#signal`, `#ramfs`, `#term`, `#vm`, `#worker`, `#web`, `#js`, `#cache`, and `#download` surfaces.
- Browser/WASM facade, custom elements, and CLI smoke paths.

## Workspace

- `wanix-core`: shared errors, file modes, metadata, paths, contexts, open flags.
- `wanix-fs`: filesystem traits, helpers, backends, nodes, field/control files, pipes, signals.
- `wanix-vfs`: namespace binding, union behavior, synthesized directories, write routing.
- `wanix-task`: task/resource filesystem, task fields, aliases, fd table, drivers.
- `wanix-protocol`: typed request/response API for file operations.
- `wanix-runtime`: root composition and built-in device/resource surfaces.
- `wanix-web`: `wasm-bindgen` browser facade plus plain JS custom elements.
- `wanix-cli`: native CLI entry point.

## Verification

```sh
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build -p wanix-web --target wasm32-unknown-unknown
wasm-pack build crates/wanix-web --target web --out-dir ../../target/wanix-web-pkg --dev
python3 -m http.server 4177 --bind 127.0.0.1
```

Open `http://127.0.0.1:4177/tests/browser-smoke.html` after the `wasm-pack build` command. The page sets `document.body.dataset.status` to `ok` after it initializes `wanix-system`, applies `wanix-bind` children, binds a ramfs, performs file API and 9P loopback operations, and starts representative WASI and Go-compatible JS adapter tasks.
