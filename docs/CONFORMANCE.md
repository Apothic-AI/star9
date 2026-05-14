# Conformance

The Rust tests are organized around the behavioral gates from `PLAN.md`. Fixtures and specs are derived from the the legacy upstream reference checkout reference implementation as behavior is ported, then validated independently in Rust without invoking Go code.

## Covered

- Core path validation and path cleaning.
- File mode type bits, permission bits, and Unix mode projection.
- `MemFs` create, read, write, stat, directory synthesis, directory rename, symlink, lstat, and readlink behavior.
- `MemFs`/`Node` xattr set, get, list, remove, and missing-attribute behavior.
- `MetaCacheFs` cached success/error behavior, TTL expiry, refresh-ahead, and mutation invalidation.
- `SyncFs` local-first dirty tracking plus explicit push/pull/sync behavior over tar patch payloads.
- `SyncFs` pull conflict behavior for default keep-local semantics, explicit prefer-remote overwrites, retained conflict reporting, and dirty descendant protection.
- `SyncFs` debounced scheduling hooks for deterministic pending state, due checks, immediate flush, stable status text, last-error reporting, and retry-after-failure behavior.
- `SyncFs` native background scheduling for debounce preservation across repeated requests, clean shutdown with pending work, failure state publication, and retry only after a new request.
- `SyncFs` reusable tar patch application semantics for file/directory/symlink upserts, parent creation, mode preservation, and PAX delete/recursive delete markers over mutable filesystem backends.
- `TarFs` archive reads, directory listings, symlink lstat/readlink/follow behavior, archive round-tripping, and read-only mutation failures.
- `MapFs` mount exposure and synthetic parent directories.
- `HttpFs` GET/HEAD reads, directory listing parsing, PUT writes, mkdir, symlink, MOVE rename, DELETE remove, metadata parsing, and protocol header formatting through a Rust recording transport.
- `HttpFs` PATCH tar payload transport behavior and mutating request `Change-Timestamp` headers through a Rust recording transport.
- `HttpFs` opt-in metadata/node caching behavior for success and not-found responses, deterministic TTL expiry, and mutation-driven invalidation after write, mkdir/symlink, remove, rename, and PATCH tar operations.
- `HttpFs` multipart/mixed directory listing parsing and fake-transport PATCH tar application behavior.
- `HttpFs` opt-in native blocking transport behavior through loopback `TcpListener` tests for request/response round trips, response header/body preservation, and 404 status mapping without live external services.
- `HttpFs` cache behavior for TTL reuse, cached not-found responses, validator-driven stale revalidation with `If-None-Match`/`If-Modified-Since`, `304 Not Modified` reuse, and mutation invalidation.
- `HttpFsHandler` server-side protocol behavior over in-memory filesystems for GET/HEAD metadata, conditional GET/HEAD validators, plain and multipart directory listings, PUT file/directory/symlink, DELETE, MOVE, metadata PATCH, tar PATCH application through `apply_sync_patch`, and unsupported methods.
- `R2Fs` object key scoping, directory listing objects, metadata fields, files, directories, symlinks, base path scoping, rename, remove, and parent listing updates through a Rust object store trait.
- `R2Fs` parent listing compare-and-swap retry behavior and deterministic conflict exhaustion through Rust in-memory object stores.
- S3/R2-compatible `S3ObjectStore` adapter behavior over the Rust `HttpTransport` trait, including GET/PUT/DELETE/list-prefix requests, XML key parsing, request-signing hooks, ETag-based compare-and-swap headers, and fake-transport coverage without live cloud services.
- AWS SigV4-compatible request signing for S3/R2 transports through deterministic `AwsSigV4Signer` tests covering canonical path/query/header signing, payload hashing, fixed signing timestamps, and stable authorization signatures without live credentials.
- Pipe bidirectional reads and writes with file close preserving the underlying pipe.
- Namespace file and directory binds.
- Namespace root unions and overlapping directory reads.
- Namespace synthesized parent directories.
- Namespace hidden `#` listings with direct hidden-path access.
- Namespace create routing into writable bindings.
- Namespace destination unbind behavior for shell-style unmount operations.
- Namespace source-specific unmount behavior for removing one matching bind layer from a union stack without relying on pointer identity.
- Namespace bind rendering for task-facing `binds` introspection.
- Task allocation through `TaskFs`.
- Task field reads and alias updates.
- Task parent/root defaulting for rootless allocation after root creation.
- Task export filesystem exposure through `#task/<id>/export`.
- Task fd open, read, close, and invalid-fd behavior.
- Task command, environment, cwd, exit-state setters, explicit standard fd installation, and fd listing behavior.
- Child task namespace cloning.
- Typed protocol dispatch for JS-handle file operations.
- CBOR request/response round-tripping for the typed protocol boundary.
- CBOR round-tripping for typed runtime worker spawn/start, execution, port handoff, task messages, stdio/fd descriptors, and exit status.
- Runtime protocol host handling for worker spawn/start, stdio/fd setup, port open/handoff, task messages, worker path/fd namespace requests, and exit-state updates.
- Runtime protocol immutable snapshot APIs for workers, ports, handoff targets, and task messages.
- Browser worker/message-port adapter coverage for typed runtime request/response dispatch, task message delivery, and lossless 9P frame transfer through a host-neutral port.
- Browser Worker/MessagePort JS glue for tagged binary runtime envelopes, endpoint wrappers, port transfer helpers, system facade resolution, import-port requests, worker-side namespace/fd request helpers, and CBOR runtime request/task-message bridging into a Star9System facade.
- Browser 9P `MessagePort` helpers for complete binary frame request/response handling, tag-matched async client requests, async namespace read/write/list mounts, transferable served ports, origin-gated `star9-import` responder handoff, concurrent imports from one exporter, cross-document import iframe lifetime/retry behavior, unknown-tag/error reporting, malformed-frame handling, and non-binary frame error reporting with deterministic fake-port tests plus real Playwright smoke import over `MessagePort`.
- Browser Worker host facade coverage for fake Worker-like startup, transferred runtime ports, binary request/response/task-message routing, stop/restart, and cleanup.
- Browser JS/WASM Worker host bootstrap coverage for normalized execution messages, runtime descriptor transfer, task-message observation, existing-host wrapping, and cleanup.
- Browser JS/WASM execution-worker coverage for runtime/bootstrap message ordering, injected runner context, default dynamic JS runner import, direct WASI-style `.wasm` instantiation, Go-compatible JS/WASM runner fixture execution, task-message emission, exit/error reporting, and cleanup.
- Browser async mount resolution and debounced sync scheduling over async storage/sync targets with deterministic fake timers.
- Browser storage JS adapter coverage for OPFS, File System Access, File System Access permission helper flow, Cache API, DOM, download, JS value, and worker-backed handles using deterministic fake host APIs, plus browser-side async mount routing for real host adapters through `star9-system`.
- Browser storage-to-9P export coverage for async storage adapters, including create/read/write/list/mkdir/remove/stat behavior through `Star9P9NamespaceMount`, large file writes, large directory listings with complete dirent chunks, storage `Tflush` cancellation, malformed frame rejection, and late reply suppression.
- Browser StarFS-compatible optional mount coverage over an OPFS-style backing adapter, including ordinary filesystem entries, xattrs, explicit hard-link unsupported behavior on non-link-capable backing stores, `.starfs/kv`, `.starfs/toolcalls`, and restorable `.starfs/snapshots` surfaces mounted independently beside raw OPFS.
- Browser StarFS SDK optional backend coverage through `backend: "starfs-sdk"` with a fake SDK adapter proving the external SDK path is additional and does not replace raw OPFS or the lightweight StarFS-compatible adapter.
- Browser network adapter coverage for WebSocket-style transports surfaced through `new`, `<id>/ctl`, `<id>/data`, `<id>/status`, `<id>/local`, and `<id>/remote` file-like operations with explicit browser raw-listen rejection.
- Star 9 shell parser/session/command coverage for quotes, escapes, comment handling, `;` sequencing, cwd tracking, status behavior, file commands, device path access, deterministic VM control through `#vm` files, and Plan 9-style `bind`/`unmount`/`srv`/`mount` service commands.
- Reusable rc crate coverage for parser/AST control flow, 9front-style `if not` layout, parenthesized groups, service-address words containing `!`, list variables, quotes, positional expansion, `$#`, command substitution, caret concatenation, globbing, pattern matching, functions, `for`, `if`, `while`, `switch`, fd dup/close redirection, `/dev/null`, fd-selected pipeline behavior, process substitution, parsed here documents, source scripts, `exit` propagation, `sigexit`/note hooks, zero-byte environment export/import, `$path` rc script execution, fake-host execution without depending on Star 9 runtime crates, and optional `STAR9_RC_ORACLE` comparison against a configured plan9port or 9front `rc` binary.
- Star 9 rc adapter coverage through `star9-shell::rc`, CLI `star9 rc`, rc-first CLI `star9 shell`, rc script args, wasm `createRcShell`, browser controller tests, browser smoke, adapter dispatch of `.wasm`/`.wat` commands to `wasi` plus `.js`/`.mjs` commands to `worker`, loopback `srv`/`mount` service workflows under `n`, and native-host provider-backed WASI rc pipelines/background jobs through Star 9 task fds and pipe resources.
- Native CLI shell command coverage through `star9 shell -c ...`, `star9 shell --simple -c ...`, script/stdin handling, precise provider-missing errors for unavailable service providers, and opt-in native process execution routing through the existing native PTY handler when enabled.
- Browser shell controller coverage for rc-first facade command delegation, explicit simple-shell facade selection, browser storage/import helper commands, browser `srv import!url#system` service registration/mounting, configured `ws!`/`wss!`/`webtransport!` service-provider routing through `srv`/`mount`, and explicit raw-browser-TCP rejection, plus browser smoke coverage for `Star9System.createShell()` running file commands through the wasm runtime namespace.
- Host-neutral browser binding source registry coverage for file byte sources, tar archive mounts, and 9P import transports.
- Native execution registry coverage for missing-handler behavior plus deterministic WASI and JS-WASM handlers that exercise task namespace files, stdio/fd descriptors, args/env/cwd, and exit status.
- Wasmi-backed WASI preview1 execution coverage for task namespace module loading from inline WAT and checked-in compiled WASM fixtures, args/env/cwd propagation, preopened cwd, fd read/write/pread/pwrite/seek/tell/allocate/renumber/close/stat/advice/flags/rights/timestamps/sync/datasync/truncate/readdir behavior, path open/stat/timestamps/create-directory/unlink/remove-directory/rename/link/symlink/readlink behavior, stdio fd writes, deterministic random/clock resolution/time imports, poll/yield/signal imports, socket listener accept through `#net/<id>/listen`, socket send/recv/shutdown over installed task fds, unsupported socket accept on non-listener fds, preview1 errno mapping, and proc-exit mapping.
- Fixture coverage for all public Star 9 file API operation names.
- Protocol EOF mapping to `null`-style optional bytes.
- Rust-native 9P2000.L frame encode/decode coverage for core import/export messages.
- 9P server attach, walk, and getattr behavior against a `MemFs` export.
- 9P client filesystem behavior over loopback transport for read, write, create, hard link, readdir, rename, remove, mkdir, and rmdir operations.
- 9P partial-walk edge behavior, duplicate `newfid` rejection, and client not-found mapping for partial remote walks.
- 9P flush acknowledgement behavior, Rust/native async server cancellation by tag, async browser server cancellation for Promise-backed work, late reply suppression, duplicate-tag cancellation, and fid-state preservation for synchronous server handling.
- 9P native stream framing helpers and `TcpStreamTransport` for consecutive length-prefixed frames, invalid frame size rejection, TCP stream import, and multi-request server dispatch over `Read`/`Write` boundaries.
- 9P hard-link and xattr walk/create message codec coverage plus server/client xattr read, list, and write commit behavior over MemFs xattrs.
- 9P client invalid path rejection before normalization for empty, absolute, and parent-traversal inputs.
- Runtime 9P namespace export and loopback import behavior.
- Browser smoke coverage for reading files through a Rust 9P imported mount.
- Browser custom element smoke coverage for `star9-system` and `star9-bind` initialization, root ramfs binding, inline file binding, fetched file binding, descriptor-backed storage, 9P loopback reads, and task startup state.
- Typed browser binding/storage descriptor validation for namespace, file, archive, import, OPFS, File System Access, Cache API, JS value, download, worker, and DOM plans.
- Host-neutral browser storage registry resolution for writable registered handles, persistent descriptor identities, and subpath-rooted mounts.
- Runtime root bindings for core, environment, service, compatibility, and device surfaces, including hidden `#env`/`#srv` plus visible `env`, `srv`, `n`, and `mnt` paths.
- Runtime environment registry behavior for listing, reading, writing, replacing, and removing NUL-separated rc environment entries through normal filesystem operations.
- Runtime service registry behavior for listing service descriptor files, registering loopback 9P namespace exports, registering opt-in native `tcp!host!port` 9P services, and mounting registered services through ordinary namespace binds.
- Device allocator resource creation.
- Terminal device program/data queues, retained screen file, program LF-to-CRLF normalization, winch signal path, ctl clear/reset/noop behavior, state, size files, and browser `star9-terminal` element integration.
- Terminal raw program queue behavior for browser/workbench callers that need byte-preserving input.
- VM provider contract behavior behind `#vm/<id>/ctl`, `state`, `console`, `config`, `alias`, and `guest`, with deterministic default provider and test provider coverage.
- CLI opt-in native 9P stream acceptance through `cargo run -p star9-cli -- accept native-p9`.
- CLI opt-in native service acceptance through `cargo run -p star9-cli -- accept native-srv`, covering `srv tcp!127.0.0.1!port`, `mount`, read-through, unmount, and service unregister over a loopback 9P TCP export.
- Live HTTP and S3/R2 environment-gated tests under `cargo test -p star9-fs --features native-http --test live_backends`, which skip unless explicit live-service environment variables are present.
- VM device `new/<kind>` allocation, ctl start/stop/reset/alias/config behavior, alias lookup, state fields, console log, id, kind files, and attached guest filesystem exposure through `#vm/<id>/guest`.
- Network deterministic Plan 9-style connection resources for dial, bind, announce, listen accept, hangup/reset, status/local/remote, data flow, and invalid transitions.
- WASI and Go-compatible JS execution adapter task starts.
- Opt-in native PTY execution handler coverage for host process stdout, nonzero exit state, and missing-binary errors.
- Opt-in native TCP loopback acceptance for real host bind/connect/accept/read/write behavior through `cargo run -p star9-cli -- accept native-tcp`.
- Native `Star9System` smoke operations.
- Native CLI acceptance smoke for 9P loopback, deterministic device surfaces, compiled WASI preview1 fixtures, runtime worker protocol flows, and fd-backed worker stdout routing, plus a native stdin/stdout `serve-p9` export command built on Rust 9P stream framing.
- Browser wasm smoke operations through `tests/browser-smoke.html`.

## Explicit Replacement Fixtures

`tests/browser-smoke.html` replaces the representative browser examples as a Rust-backed acceptance path. It imports the browser custom element module, initializes `star9-system`, applies `star9-bind` children, binds a ramfs, runs a `Star9System.createShell()` session through file and cwd commands, mounts descriptor-backed storage, writes and reads files through the public API and 9P loopback import, lists directories, verifies task fields, drives normalized and raw `star9-terminal` paths, starts WASI/Go JS adapter tasks, mounts OPFS and StarFS through browser capability-gated storage paths, runs a real module-worker JS task, runs a direct WASI-style `.wasm` task through the Star 9 runtime path, runs the Go-compatible JS/WASM runner fixture through the same worker path, and mounts a worker-exported 9P filesystem as both `#task/<id>/export` and `#vm/<id>/guest`.

`tests/fixtures/api-operations.json` lists the public operation names used by the typed protocol boundary.

`tests/fixtures/runtime-requests.json` lists the typed runtime protocol method names for worker and port dispatch.

`tests/fixtures/wasi-hard-link.wasm` is a checked-in compiled WASI preview1 module, with source in `tests/fixtures/wasi-hard-link.wat`, used to prove `WasmiWasiHandler` executes precompiled fixture bytes loaded from the Star 9 namespace.

`tests/fixtures/wasi-preview1-smoke.wasm` is a checked-in compiled WASI preview1 module, with source in `tests/fixtures/wasi-preview1-smoke.wat`, used by Rust, CLI, Node, and browser smoke coverage for clock resolution/time, args/env sizing, random bytes, stdout, and direct browser `.wasm` execution.

`docs/audits/completion-gap-matrix.json` classifies the remaining host capability boundaries and preview1 import coverage. `docs/audits/real-host-depth-matrix.json` records the OPFS, StarFS, cross-document 9P, live backend, native TCP, VM-provider, and Go-compatible execution classifications for the current sprint. `tests/audit-matrix.test.mjs` validates that code-level unsupported markers have an audit classification.

`docs/audits/upstream-catch-up-matrix.json` records the accepted and deliberately unported upstream changes from `b753801..2feaf3f`, including task exports, worker export handoff, VM guest mounts, native PTY execution, raw terminal mode, the logger hook, and Rust-backed example replacements.

`docs/audits/shell-dependency-matrix.json` records the shell sprint dependency/license decisions, including the rejection of GPL shell dependencies and the decision to keep Star 9's own browser-aware 9P stack primary.

`docs/audits/rc-compatibility-matrix.json` records the current rc feature compatibility state across parser, expansion, evaluation, host integration, browser support, and optional oracle work.

`docs/audits/plan9-command-compatibility-matrix.json` records shell/rc-visible Plan 9 command compatibility for `bind`, `unmount`, `srv`, and `mount`, plus provider-missing boundaries for `dossrv`, `vacfs`, and network service sources.

`docs/audits/rc-process-graph-matrix.json` and `docs/audits/rc-pipeline-job-control-boundary.md` record the rc process-graph boundary. The portable rc evaluator remains deterministic, while the Star 9 adapter creates task/fd/pipe graph records for pipelines, background jobs, and process substitution. Native-host WASI, registered JS/WASM, and opt-in native rc pipelines/background jobs now use provider-backed task/fd execution with persistent `.rc/graphs/<id>` lifecycle and `ctl` files, deterministic active-job note status, and file redirect descriptors. Browser worker graph-compatible pipelines/jobs use a browser provider with bounded stdin/stdout handoff. Exact `rfork` sharing, provider-specific hard cancellation for handlers without interrupts, and arbitrary nested fd graph concurrency remain documented provider boundaries.

`tests/fixtures/browser-bindings.json` captures representative validated browser binding/storage plans for namespace, file, archive, import, and browser storage backends.

`tests/browser-mounts.test.mjs` exercises browser async mount resolution and debounced sync scheduling with fake adapters and fake timers.

`tests/browser-storage-adapters.test.mjs` exercises browser storage adapter behavior with fake host APIs so OPFS/File System Access, Cache API, DOM, download, JS value, and worker request/response semantics are tested without live browser storage.

`tests/browser-worker-host.test.mjs` exercises the browser Worker host facade with fake Worker and MessagePort targets so runtime port transfer and tagged binary message routing are covered without a live browser worker.

`tests/browser-js-wasm-worker-host.test.mjs` exercises JS/WASM execution bootstrap messages over the browser Worker host facade with fake Worker and MessagePort targets, covering runtime port transfer and task-message observation without running a real JS/WASM payload.

`tests/browser-js-wasm-execution-worker.test.mjs` exercises the worker-side JS/WASM bootstrap acceptor with fake worker scopes and message ports, covering deterministic injected-runner execution, default dynamic JS runner import, direct WASI-style `.wasm` execution with bounded stdin bytes, and Go-compatible runner fixture execution without a Go shim.

`tests/browser-p9-port.test.mjs` exercises browser-side 9P frame serving over fake MessagePorts, including complete binary request/response frames, tag-matched async client requests, AbortSignal-to-`Tflush` cancellation, facade-error-to-`Rlerror` replies, import responder port handoff, storage-backed 9P exports, complete large directory chunks, malformed frames, and error reporting for unknown tags and non-binary requests.

`tests/browser-shell.test.mjs` exercises the browser shell controller with fake Star 9 facades so command delegation, StarFS mount helpers, OPFS capability errors, and word parsing are covered without live browser APIs.

## Remaining Oracle Areas

These surfaces are represented in Rust but should continue to be expanded with differential or fixture-backed tests as behavior becomes more specific:

- The completion sprint in `PLAN.md` is the authoritative checklist for closing these remaining oracle areas. Each item below should either gain direct Rust/Node/Playwright/CLI/live conformance or be reclassified as an explicit host capability boundary with tests and docs.
- HTTP filesystem remote metadata semantics beyond the currently covered cache validators, Star 9 metadata fields, and conditional GET/HEAD behavior.
- `SyncFs` backend-specific transport scheduling semantics beyond reusable tar patch application, stable status text, and the browser async-target debounce scheduler.
- HTTP filesystem broader live-service behavior against real servers is live-service opt-in and is covered by the environment-gated live test harness documented in `docs/LIVE_TESTS.md`.
- Cloudflare/S3/R2 live-service coverage over `S3ObjectStore` plus `AwsSigV4Signer` is credential-gated, documented in `docs/LIVE_TESTS.md`, and covered by the env-gated live test harness when credentials are present.
- Browser task/WASI visibility for real OPFS and other async browser storage uses the browser async mount table and storage 9P proxy. Direct insertion into the synchronous Rust `FileSystem` trait remains an explicit sync/async host boundary.
- Browser Worker execution orchestration covers real JS, direct WASI-style `.wasm`, Go-compatible runner fixtures, port handoff, stdout, exit, worker export mounts, and typed runtime path/fd requests. Representative workloads should continue to expand over that protocol.
- Real VM execution providers beyond the deterministic provider contract remain native/browser host integrations. Guest storage attachment is covered through `#vm/<id>/guest`.
- Real browser raw TCP is not a browser capability. Native TCP has opt-in loopback acceptance; browser transport adapters use WebSocket/WebTransport-style file-model resources.
- WASI listener-backed `sock_accept` is implemented for `#net/<id>/listen` fds and remains unsupported on generic non-listener fds.
