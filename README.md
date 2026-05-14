# Star 9

Rust-native Star 9 runtime.

The legacy upstream reference checkout remains the behavior reference while this project moves forward as Star 9. This project does not wrap, execute, link, shell out to, or test against Go code; runtime behavior is delivered by this repository's Rust code, fixtures, and tests.

This workspace implements the main Star 9 runtime surfaces in Rust:

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
- Runtime environment registry exposed at hidden `#env` and visible `env`, with rc list variables represented as NUL-separated files and exported functions represented as `fn#name` entries.
- Runtime service registry exposed at hidden `#srv` and visible `srv`, with `n`/`mnt` compatibility mount points and shell/rc-visible `bind`, source-specific `unmount`, `srv`, and `mount` commands.
- Typed public file API for the Star 9 JS handle operation set.
- CBOR encode/decode helpers for the typed public API boundary.
- Runtime root construction with built-in `#star9`, `#task`, `#pipe`, `#signal`, `#ramfs`, `#term`, `#vm`, `#worker`, `#web`, `#js`, `#cache`, `#download`, and `#net` surfaces.
- Deterministic terminal, VM, and Plan 9-style network device state surfaces, including a retained terminal screen file, raw terminal input queue, a `VmProvider` lifecycle contract behind `#vm`, and attached VM guest filesystem surface.
- Rust-native 9P import/export with hard-link and xattr read/list/write support, native stream/TCP transport helpers, async native `Tflush` cancellation, browser MessagePort frame-serving/client helpers, an async browser namespace mount client, and a browser storage-to-9P export bridge for async storage adapters.
- Browser/WASM facade, custom elements, and CLI smoke paths.
- Host-neutral Star 9 shell core plus native/browser shell surfaces that route through namespaces, files, task fds, and device protocols rather than host side channels.
- Reusable Plan 9 rc language core with a Star 9 host adapter for native/browser rc sessions, including task/fd graph records for pipelines, background jobs, process substitution, Star 9-backed `rfork nNeEsfFm` scope controls, native-host provider-backed WASI/registered-JS/opt-in-native pipeline execution, browser worker graph-compatible pipelines, and `.rc/graphs/<id>/ctl` cleanup/cancel controls.
- Browser Worker/MessagePort JS glue for runtime message envelopes, transferred ports, typed namespace/fd request helpers, and CBOR request/task-message bridging into the Rust runtime host.
- Browser Worker host facade and execution-worker helper for real module-worker startup, runtime port transfer, message routing, JS/WASM execution bootstrap, dynamic JS runner import, direct WASI-style `.wasm` instantiation, Go-compatible runner fixture execution, runner context, port handoff, exit/error reporting, and cleanup.
- Browser worker export handoff for `{ export: MessagePort }` 9P exports, mounted into the task export namespace and optional VM guest namespace.
- Browser storage host adapters for OPFS/File System Access, Cache API, DOM, download, JS value, and worker-backed handles with deterministic fake-host tests, JS-side async namespace mount routing for real browser hosts, task-facing 9P proxy mounts, and browser timer-backed debounced sync scheduling for async targets.
- StarFS as separate optional browser storage mount backends: a lightweight OPFS-backed compatible adapter with xattrs, KV, tool-call audit logs, and restorable snapshots, plus an additional `starfs-sdk` adapter hook for an external SDK/worker/wasm backend. Neither replaces raw OPFS.
- Browser network transport adapters and service providers shaped like `#net`/`#srv` resources over `import!`, WebSocket, and WebTransport-style capabilities; browser raw TCP remains explicitly unavailable.
- Wasmi-backed WASI preview1 execution over Star 9 task namespaces, fd tables, fd directory/positional I/O/allocation/renumber/advice/flags/timestamps/sync/truncate syscalls, clock resolution/time, poll/yield/signal imports, hard-link and other path mutation syscalls, deterministic socket listener accept plus send/recv/shutdown over `#net` task fds, and explicit unsupported socket accept on non-listener fds.
- Rust-owned WASI fixtures include checked-in compiled `.wasm` modules as well as focused WAT unit fixtures.
- Native CLI acceptance commands for 9P loopback, deterministic devices, compiled WASI preview1 fixtures, runtime worker protocol flows, and fd-backed worker stdout routing, plus Rust-native `serve-p9` stdin/stdout export and opt-in native `srv tcp!host!port` service import checks.
- Opt-in native PTY execution acceptance on native hosts with `cargo run -p star9-cli -- accept native`.
- Opt-in native TCP loopback acceptance on native hosts with `cargo run -p star9-cli -- accept native-tcp`.
- Opt-in native 9P TCP stream acceptance on native hosts with `cargo run -p star9-cli -- accept native-p9`.

## Workspace

- `star9-core`: shared errors, file modes, metadata, paths, contexts, open flags.
- `star9-fs`: filesystem traits, helpers, backends, nodes, field/control files, pipes, signals.
- `star9-vfs`: namespace binding, union behavior, synthesized directories, write routing.
- `star9-task`: task/resource filesystem, task fields, aliases, fd table, drivers.
- `star9-protocol`: typed request/response API for file operations.
- `star9-rc`: reusable Plan 9 rc lexer/parser/AST/evaluator core with host traits.
- `star9-runtime`: root composition and built-in device/resource surfaces.
- `star9-shell`: host-neutral shell parser, session, command registry, and runtime host adapter.
- `star9-web`: `wasm-bindgen` browser facade plus plain JS custom elements.
- `star9-cli`: native CLI entry point.

## Shell

Run one command:

```sh
cargo run -p star9-cli -- shell -c 'mkdir demo; write demo/hello hello; cat demo/hello'
```

Run interactively:

```sh
cargo run -p star9-cli -- shell
```

`star9 shell` is rc-first. It uses the reusable rc language core by default while exposing Star 9-native commands such as `ls`, `cat`, `write`, `bind`, `unmount`, `srv`, `mount`, `tasks`, `fds`, `term`, `vm`, `net`, `wasi`, and `worker` through Star 9 namespaces and device files. The smaller admin parser is still available as `star9 shell --simple`. Host process execution is opt-in with `star9 shell --native` and the `native <cmd...>` shell command.

Run rc mode:

```sh
cargo run -p star9-cli -- rc -c 'x=(one two); fn twice { echo $1 $1 }; for(i in $x) twice $i'
cargo run -p star9-cli -- shell -c 'echo hello | cat'
cargo run -p star9-cli -- rc ./script.rc arg1 arg2
```

The reusable `star9-rc` crate owns the rc language core and can be embedded without depending on Star 9 runtime or browser crates. It covers rc lists, expansion, functions/control flow, globbing, fd redirection/duplication, process substitution, here documents, environment import/export, notes, `$path` rc script dispatch, and optional oracle checks. Star 9 integrates it through a host adapter that routes files, commands, devices, process-graph records, and native-host provider-backed WASI/registered-JS/native pipelines through namespaces, task fds, generated pipe resources, and runtime surfaces. Browser rc keeps the same language surface and adds a worker graph provider for graph-compatible worker stages using Star 9 browser Worker tasks and bounded stdin/stdout handoff.

Plan 9-style service composition works through the same shell/rc path:

```sh
cargo run -p star9-cli -- rc -c 'mkdir exported; write exported/hello ok; srv root rootsrv; mount rootsrv n/root; cat n/root/exported/hello'
cargo run -p star9-cli -- accept native-srv
```

Native hosts can register 9P services from `tcp!host!port` addresses when explicitly used. Browser shells can register cross-document imports with `srv import!url#system name`, configured WebSocket services with `srv ws!host!path name` or `srv wss!host!path name`, and configured WebTransport services with `srv webtransport!host!path name`; all mount with `mount name path`. Browser raw TCP remains unavailable. Provider-heavy commands such as `dossrv` and `vacfs` are explicit provider-missing boundaries until matching Star 9 disk/vac/archive providers are configured.

## Verification

```sh
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo test -p star9-fs --features native-http
node --test tests/*.test.mjs
cargo run -p star9-cli -- accept all
cargo build -p star9-web --target wasm32-unknown-unknown
wasm-pack build crates/star9-web --target web --out-dir ../../target/star9-web-pkg --dev
python3 -m http.server 4177 --bind 127.0.0.1
```

Open `http://127.0.0.1:4177/tests/browser-smoke.html` after the `wasm-pack build` command. The page sets `document.body.dataset.status` to `ok` after it initializes `star9-system`, applies `star9-bind` children, binds a ramfs, performs file API, mounts a 9P export over a real `MessagePort`, exercises Rust 9P loopback operations, exercises real browser storage adapters through the async JS mount table where available, mounts OPFS and StarFS through task-facing browser paths where supported, drives normalized and raw browser terminal paths, starts representative WASI and Go-compatible JS adapter tasks, runs a real module-worker JS workload, runs a direct WASI-style `.wasm` workload through the Star 9 worker/runtime path, runs a Go-compatible JS/WASM runner fixture, and mounts a worker-exported 9P filesystem as both `#task/<id>/export` and `#vm/<id>/guest`.

Rust-backed browser examples live under `examples/` for the shell, rc shell, basic VM, VM workbench, import/iframe workbench, and worker-export behavior. After the wasm package is built, open `http://127.0.0.1:4177/examples/shell.html` for the default rc-backed browser shell or `http://127.0.0.1:4177/examples/rc.html` for the explicit rc example.

Live and host-capability checks, including `accept native`, `accept native-tcp`, browser OPFS/StarFS storage, and live HTTP/S3/R2 runs, are documented in `docs/LIVE_TESTS.md`; default tests remain offline.
