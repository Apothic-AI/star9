# wanix-rs

Rust-native Wanix runtime.

`../wanix` is the reference implementation for porting behavior into Rust. This project does not wrap, execute, link, shell out to, or test against Go code; runtime behavior is delivered by this repository's Rust code, fixtures, and tests.

This workspace implements the main Wanix runtime surfaces in Rust:

- Plan 9-style namespaces and bind semantics.
- Filesystem traits, helpers, metadata, paths, file handles, and open flags.
- In-memory, local, map, union, pipe, signal, tar-compatible, metadata-cache, and local-first sync filesystem surfaces.
- HTTP filesystem protocol semantics through Rust client/server transport abstractions, with an opt-in native blocking transport.
- Multipart HTTP directory listing parsing, PATCH tar behavior, conditional GET/HEAD validators, and server-side request handling tested through fake transports.
- Opt-in HTTP filesystem caching with deterministic TTL tests, validator-driven stale revalidation, `304 Not Modified` reuse, and mutation invalidation.
- R2-style object storage semantics through a Rust object-store abstraction plus an S3/R2-compatible HTTP object-store adapter and deterministic SigV4 signer with fake-transport tests.
- Explicit metadata caching through `MetaCacheFs` and local-first sync orchestration through `SyncFs`, including reusable tar patch application and a native background debounce scheduler.
- Task/resource filesystem with per-task namespaces, aliases, drivers, and file descriptors.
- Task export filesystems exposed at `#task/<id>/export`, plus bind introspection through `#task/<id>/binds`.
- Typed public file API for the Wanix JS handle operation set.
- CBOR encode/decode helpers for the typed public API boundary.
- Runtime root construction with built-in `#wanix`, `#task`, `#pipe`, `#signal`, `#ramfs`, `#term`, `#vm`, `#worker`, `#web`, `#js`, `#cache`, `#download`, and `#net` surfaces.
- Deterministic terminal, VM, and Plan 9-style network device state surfaces, including a retained terminal screen file, raw terminal input queue, and attached VM guest filesystem surface.
- Rust-native 9P import/export with hard-link and xattr read/list/write support plus browser MessagePort frame-serving/client helpers, an async browser namespace mount client, and a browser storage-to-9P export bridge for async storage adapters.
- Browser/WASM facade, custom elements, and CLI smoke paths.
- Browser Worker/MessagePort JS glue for runtime message envelopes, transferred ports, and CBOR request/task-message bridging into the Rust runtime host.
- Browser Worker host facade and execution-worker helper for real module-worker startup, runtime port transfer, message routing, JS/WASM execution bootstrap, dynamic JS runner import, direct WASI-style `.wasm` instantiation, Go-compatible runner fixture execution, runner context, port handoff, exit/error reporting, and cleanup.
- Browser worker export handoff for `{ export: MessagePort }` 9P exports, mounted into the task export namespace and optional VM guest namespace.
- Browser storage host adapters for OPFS/File System Access, Cache API, DOM, download, JS value, and worker-backed handles with deterministic fake-host tests, JS-side async namespace mount routing for real browser hosts, task-facing 9P proxy mounts, and browser timer-backed debounced sync scheduling for async targets.
- StarFS as a separate optional browser storage mount backend, OPFS-backed by default, exposing ordinary files plus `.starfs/kv`, `.starfs/toolcalls`, and `.starfs/snapshots` without replacing raw OPFS.
- Wasmi-backed WASI preview1 execution over Wanix task namespaces, fd tables, fd directory/positional I/O/allocation/renumber/advice/flags/timestamps/sync/truncate syscalls, clock resolution/time, poll/yield/signal imports, hard-link and other path mutation syscalls, deterministic socket send/recv/shutdown over task fds, and explicit unsupported socket accept on non-listener fds.
- Rust-owned WASI fixtures include checked-in compiled `.wasm` modules as well as focused WAT unit fixtures.
- Native CLI acceptance commands for 9P loopback, deterministic devices, compiled WASI preview1 fixtures, runtime worker protocol flows, and fd-backed worker stdout routing, plus a Rust-native `serve-p9` stdin/stdout stream hook for local filesystem export.
- Opt-in native PTY execution acceptance on native hosts with `cargo run -p wanix-cli -- accept native`.
- Opt-in native TCP loopback acceptance on native hosts with `cargo run -p wanix-cli -- accept native-tcp`.

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
cargo test -p wanix-fs --features native-http
node --test tests/*.test.mjs
cargo run -p wanix-cli -- accept all
cargo build -p wanix-web --target wasm32-unknown-unknown
wasm-pack build crates/wanix-web --target web --out-dir ../../target/wanix-web-pkg --dev
python3 -m http.server 4177 --bind 127.0.0.1
```

Open `http://127.0.0.1:4177/tests/browser-smoke.html` after the `wasm-pack build` command. The page sets `document.body.dataset.status` to `ok` after it initializes `wanix-system`, applies `wanix-bind` children, binds a ramfs, performs file API, mounts a 9P export over a real `MessagePort`, exercises Rust 9P loopback operations, exercises real browser storage adapters through the async JS mount table where available, mounts OPFS and StarFS through task-facing browser paths where supported, drives normalized and raw browser terminal paths, starts representative WASI and Go-compatible JS adapter tasks, runs a real module-worker JS workload, runs a direct WASI-style `.wasm` workload through the Wanix worker/runtime path, runs a Go-compatible JS/WASM runner fixture, and mounts a worker-exported 9P filesystem as both `#task/<id>/export` and `#vm/<id>/guest`.

Rust-backed browser examples live under `examples/` for basic VM, VM workbench, import/iframe workbench, and worker-export behavior.

Live and host-capability checks, including `accept native`, `accept native-tcp`, browser OPFS/StarFS storage, and live HTTP/S3/R2 runs, are documented in `docs/LIVE_TESTS.md`; default tests remain offline.
