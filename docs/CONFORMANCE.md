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
- `SyncFs` native background scheduling for debounce preservation across repeated requests, clean shutdown with pending work, failure state publication, and retry only after a new request.
- `SyncFs` reusable tar patch application semantics for file/directory/symlink upserts, parent creation, mode preservation, and PAX delete/recursive delete markers over mutable filesystem backends.
- `TarFs` archive reads, directory listings, symlink lstat/readlink/follow behavior, archive round-tripping, and read-only mutation failures.
- `MapFs` mount exposure and synthetic parent directories.
- `HttpFs` GET/HEAD reads, directory listing parsing, PUT writes, mkdir, symlink, MOVE rename, DELETE remove, metadata parsing, and protocol header formatting through a Rust recording transport.
- `HttpFs` PATCH tar payload transport behavior and mutating request `Change-Timestamp` headers through a Rust recording transport.
- `HttpFs` opt-in metadata/node caching behavior for success and not-found responses, deterministic TTL expiry, and mutation-driven invalidation after write, mkdir/symlink, remove, rename, and PATCH tar operations.
- `HttpFs` multipart/mixed directory listing parsing and fake-transport PATCH tar application behavior.
- `HttpFs` opt-in native blocking transport behavior through loopback `TcpListener` tests for request/response round trips, response header/body preservation, and 404 status mapping without live external services.
- `HttpFsHandler` server-side protocol behavior over in-memory filesystems for GET/HEAD metadata, plain and multipart directory listings, PUT file/directory/symlink, DELETE, MOVE, metadata PATCH, tar PATCH application through `apply_sync_patch`, and unsupported methods.
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
- Task allocation through `TaskFs`.
- Task field reads and alias updates.
- Task fd open, read, close, and invalid-fd behavior.
- Task command, environment, cwd, exit-state setters, explicit standard fd installation, and fd listing behavior.
- Child task namespace cloning.
- Typed protocol dispatch for JS-handle file operations.
- CBOR request/response round-tripping for the typed protocol boundary.
- CBOR round-tripping for typed runtime worker spawn/start, execution, port handoff, task messages, stdio/fd descriptors, and exit status.
- Runtime protocol host handling for worker spawn/start, stdio/fd setup, port open/handoff, task messages, and exit-state updates.
- Runtime protocol immutable snapshot APIs for workers, ports, handoff targets, and task messages.
- Browser worker/message-port adapter coverage for typed runtime request/response dispatch, task message delivery, and lossless 9P frame transfer through a host-neutral port.
- Browser Worker/MessagePort JS glue for tagged binary runtime envelopes, endpoint wrappers, port transfer helpers, system facade resolution, and import-port requests.
- Browser 9P `MessagePort` serving helpers for complete binary frame request/response handling, transferable served ports, `wanix-import` responder handoff, and non-binary frame error reporting with deterministic fake-port tests.
- Browser Worker host facade coverage for fake Worker-like startup, transferred runtime ports, binary request/response/task-message routing, stop/restart, and cleanup.
- Browser JS/WASM Worker host bootstrap coverage for normalized execution messages, runtime descriptor transfer, task-message observation, existing-host wrapping, and cleanup.
- Browser JS/WASM execution-worker coverage for runtime/bootstrap message ordering, injected runner context, default dynamic JS runner import, explicit direct-WASM unsupported errors, task-message emission, exit/error reporting, and cleanup.
- Browser storage JS adapter coverage for OPFS, File System Access, Cache API, DOM, download, JS value, and worker-backed handles using deterministic fake host APIs.
- Host-neutral browser binding source registry coverage for file byte sources, tar archive mounts, and 9P import transports.
- Native execution registry coverage for missing-handler behavior plus deterministic WASI and JS-WASM handlers that exercise task namespace files, stdio/fd descriptors, args/env/cwd, and exit status.
- Wasmi-backed WASI preview1 execution coverage for task namespace module loading, args/env/cwd propagation, preopened cwd, fd read/write/pread/pwrite/seek/tell/allocate/close/stat/advice/flags/rights/timestamps/sync/datasync/truncate/readdir behavior, path open/stat/timestamps/create-directory/unlink/remove-directory/rename/symlink/readlink behavior, stdio fd writes, deterministic random/clock imports, preview1 errno mapping, and proc-exit mapping.
- Fixture coverage for all public Wanix file API operation names.
- Protocol EOF mapping to `null`-style optional bytes.
- Rust-native 9P2000.L frame encode/decode coverage for core import/export messages.
- 9P server attach, walk, and getattr behavior against a `MemFs` export.
- 9P client filesystem behavior over loopback transport for read, write, create, readdir, rename, remove, mkdir, and rmdir operations.
- 9P partial-walk edge behavior, duplicate `newfid` rejection, and client not-found mapping for partial remote walks.
- 9P flush acknowledgement behavior and fid-state preservation for synchronous server handling.
- 9P xattr walk/create message codec coverage plus server/client xattr read, list, and write commit behavior over MemFs xattrs.
- 9P client invalid path rejection before normalization for empty, absolute, and parent-traversal inputs.
- Runtime 9P namespace export and loopback import behavior.
- Browser smoke coverage for reading files through a Rust 9P imported mount.
- Browser custom element smoke coverage for `wanix-system` and `wanix-bind` initialization, root ramfs binding, inline file binding, fetched file binding, descriptor-backed storage, 9P loopback reads, and task startup state.
- Typed browser binding/storage descriptor validation for namespace, file, archive, import, OPFS, File System Access, Cache API, JS value, download, worker, and DOM plans.
- Host-neutral browser storage registry resolution for writable registered handles, persistent descriptor identities, and subpath-rooted mounts.
- Runtime root bindings for core and device surfaces.
- Device allocator resource creation.
- Terminal device program/data queues, program LF-to-CRLF normalization, winch signal path, ctl clear/reset/noop behavior, state, and size files.
- VM device `new/<kind>` allocation, ctl start/stop/reset/alias/config behavior, alias lookup, state fields, console log, id, and kind files.
- Network deterministic Plan 9-style connection resources for dial, bind, announce, listen accept, hangup/reset, status/local/remote, data flow, and invalid transitions.
- WASI and Go-compatible JS execution adapter task starts.
- Native `WanixSystem` smoke operations.
- Native CLI acceptance smoke for 9P loopback, deterministic device surfaces, and runtime worker protocol flows.
- Browser wasm smoke operations through `tests/browser-smoke.html`.

## Explicit Replacement Fixtures

`tests/browser-smoke.html` replaces the representative browser examples as a Rust-backed acceptance path. It imports the browser custom element module, initializes `wanix-system`, applies `wanix-bind` children, binds a ramfs, mounts descriptor-backed storage, writes and reads files through the public API and 9P loopback import, lists directories, verifies task fields, and starts WASI/Go JS adapter tasks.

`tests/fixtures/api-operations.json` lists the public operation names used by the typed protocol boundary.

`tests/fixtures/runtime-requests.json` lists the typed runtime protocol method names for worker and port dispatch.

`tests/fixtures/browser-bindings.json` captures representative validated browser binding/storage plans for namespace, file, archive, import, and browser storage backends.

`tests/browser-storage-adapters.test.mjs` exercises browser storage adapter behavior with fake host APIs so OPFS/File System Access, Cache API, DOM, download, JS value, and worker request/response semantics are tested without live browser storage.

`tests/browser-worker-host.test.mjs` exercises the browser Worker host facade with fake Worker and MessagePort targets so runtime port transfer and tagged binary message routing are covered without a live browser worker.

`tests/browser-js-wasm-worker-host.test.mjs` exercises JS/WASM execution bootstrap messages over the browser Worker host facade with fake Worker and MessagePort targets, covering runtime port transfer and task-message observation without running a real JS/WASM payload.

`tests/browser-js-wasm-execution-worker.test.mjs` exercises the worker-side JS/WASM bootstrap acceptor with fake worker scopes and message ports, covering deterministic injected-runner execution, default dynamic JS runner import, and explicit direct-WASM rejection without a real browser worker or Go shim.

`tests/browser-p9-port.test.mjs` exercises browser-side 9P frame serving over fake MessagePorts, including complete binary request/response frames, import responder port handoff, and error reporting for non-binary requests.

## Remaining Oracle Areas

These surfaces are represented in Rust but should continue to be expanded with differential or fixture-backed tests as behavior becomes more specific:

- HTTP filesystem remote metadata semantics beyond the currently covered cache headers and Wanix metadata fields.
- `SyncFs` browser timer integration and backend-specific transport scheduling semantics beyond reusable tar patch application.
- HTTP filesystem broader live-service behavior against real servers.
- Cloudflare/S3 live-service coverage such as bucket-specific behavior and opt-in live tests over `S3ObjectStore` plus `AwsSigV4Signer`.
- Cross-document/browser MessagePort namespace mounting for remote 9P imports using the JS port helpers beyond whole-frame serving and loopback smoke.
- Additional 9P edge cases such as flush cancellation and remote conflict/error parity.
- Browser namespace mounting for real OPFS, File System Access, Cache API, JS value, DOM, download, and worker-backed storage adapters where async browser APIs meet the synchronous Rust filesystem trait boundary.
- Broader browser Worker startup and real execution orchestration on top of the JS Worker host facade, JS/WASM bootstrap host, and host-neutral typed runtime adapter.
- Terminal screen protocol details beyond the host-neutral file protocol.
- Real VM execution and native/browser TCP transport adapters beyond deterministic state-machine resources.
- WASI syscall coverage beyond the current preview1 fd directory, fd positional I/O/allocation, fd advice/flags/timestamps, fd sync/truncate, path mutation, args/env, clock, and random baseline.
- Full Go-compatible JS/WASM worker execution.
