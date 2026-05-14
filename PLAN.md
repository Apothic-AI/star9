# Star 9 Plan

## Current Focus

Finish the Star 9 shell sprint while keeping the Rust tree primary, Plan 9-aligned, deterministic by default, and free of GPL or Go runtime/build dependencies.

Current shell sprint contract:

- Shell behavior routes through Star 9 namespaces, files, task fds, device files, runtime requests, and 9P-shaped mounts.
- `star9-shell` is a host-neutral shell core, not a POSIX/Bash/Plan 9 `rc` compatibility claim.
- Native shell UX uses `reedline`; GPL shell dependencies are rejected.
- Native host process execution remains an explicit `star9 shell --native` / `native <cmd...>` opt-in.
- Browser shell UX is a plain custom element and JS controller over the `Star9System` wasm facade.
- VM demo behavior remains deterministic by default through `#vm` files; real x86 providers stay future provider work behind the same device surface.
- `docs/audits/shell-dependency-matrix.json`, planning, progress, architecture, conformance, and live-test docs track the dependency/license and behavior decisions.

### Star 9 Shell Sprint Plan

1. Evidence and license gate
   - Reject GPL shell dependencies such as `rcshell`.
   - Record BSD-licensed `rs9p`/`unpfs` as future native interop references only.
   - Accept `reedline` for native interactive input.

2. Shell core
   - Add `crates/star9-shell`.
   - Implement `ShellSession`, `ShellHost`, parser, command registry, status/stdout/stderr results, cwd, prompt, and sequencing.
   - Keep parsing deliberately small: words, quotes, escapes, comments, and `;`.

3. Runtime host adapter
   - Route file, task, terminal, VM, network, WASI, worker, and opt-in native execution through Star 9 runtime APIs and file/device protocols.
   - Use a writable ramfs workspace for native CLI shell sessions while browser shells use their existing `star9-system` namespace setup.

4. Core commands
   - Provide `pwd`, `cd`, `ls`, `cat`, `write`, `append`, `mkdir`, `rm`, `mv`, `cp`, `stat`, `version`, `binds`, `tasks`, `fds`, `term`, `vm`, `net`, `wasi`, `worker`, `native`, and `help`.

5. Native CLI shell
   - Add `star9 shell`, `star9 shell -c '<command>'`, `star9 shell <script-file>`, and stdin script mode.
   - Use `reedline` for interactive mode.

6. Browser shell
   - Expose `Star9System.createShell()` via wasm-bindgen.
   - Add `crates/star9-web/js/shell.js`, `<star9-shell>`, and `examples/shell.html`.
   - Keep browser-only mount/download helpers routed into normal mounted namespaces where they touch storage.

7. VM/provider direction
   - Keep the deterministic VM provider as the demo target.
   - Document the future real-provider path behind `#vm/<id>/ctl`, `state`, `console`, `config`, `term`, `guest`, and `error`.

8. Verification
   - Cover parser/core commands in Rust.
   - Cover browser shell controller behavior in Node.
   - Cover wasm facade shell execution in browser smoke.
   - Keep all checks offline by default.

Previous host-depth completion contract:

- Host capabilities must appear as mounted namespaces or device files controlled by small file protocols; they must not bypass task namespaces, fd tables, or 9P-style composition.
- Rust/native 9P cancellation uses tag-keyed async operation tracking where native work can run concurrently. `Tflush` cancels by old tag, returns `Rflush`, and suppresses late replies.
- WASI socket behavior routes listener-backed accept and stream send/recv/shutdown through deterministic `#net` resources and task fds by default.
- Browser workers access task files through typed runtime path/fd requests, not privileged JS object handles.
- Live HTTP and S3/R2 checks are implemented as environment-gated opt-in tests. Default verification remains offline.
- Browser networking uses WebSocket/WebTransport-style adapters surfaced through a `#net`-shaped file model; raw TCP remains unavailable in browsers.
- SyncFs scheduling state is exposed through structured snapshots and stable status text.
- Real VM providers sit behind a `VmProvider` contract; deterministic `#vm` remains the default offline provider.
- Native 9P import/serve includes stdio plus opt-in TCP stream acceptance through Rust-owned frame framing.
- Preserve the existing browser storage adapters: OPFS, File System Access, Cache API, JS value, DOM, download, and worker-backed storage.
- Prefer raw OPFS for simple persistent browser workspaces.
- Keep StarFS as separate optional mount backends: the lightweight OPFS-backed compatible adapter and the additional `starfs-sdk` adapter hook for an external StarFS SDK/worker/wasm integration.
- Route browser async storage into Star 9 task-facing paths through a 9P proxy/mount boundary instead of pretending browser async APIs satisfy the synchronous Rust `FileSystem` trait.
- Expand browser 9P import/export parity for mutation, errors, large payloads, cancellation, malformed frames, concurrent imports, and teardown.
- Keep deterministic terminal/VM/network devices as default conformance. Native TCP, native PTY, real VM providers, and live HTTP/S3/R2 checks remain explicit opt-ins.
- Keep Go-compatible JS/WASM execution represented by Rust/browser worker lifecycle fixtures without reintroducing Go build dependencies.
- Keep `docs/audits/completion-gap-matrix.json`, `docs/audits/real-host-depth-matrix.json`, conformance docs, live-test docs, and progress notes current as the sprint lands.

### Plan 9 Host-Depth Completion Sprint Plan

1. Rust/native 9P true async cancellation
   - Maintain an async-capable operation registry keyed by 9P tag.
   - Cancel pending native work by `Tflush.oldtag`, return `Rflush`, and suppress late replies.
   - Preserve fid/session state and keep Rust-owned length-prefixed stream framing as the native boundary.

2. Real network resources and WASI listener-backed sockets
   - Keep deterministic `#net` as the default provider.
   - Route WASI `sock_accept`, `sock_recv`, `sock_send`, and `sock_shutdown` through `#net` listener/data files and task fds.
   - Keep native/browser real transports opt-in and file-protocol-shaped.

3. Browser worker runtime namespace/fd request protocol
   - Provide typed runtime requests for path and fd operations.
   - Route browser worker access through owning task namespaces and fd tables.

4. Live backend depth
   - Keep fake/loopback transports as default.
   - Provide opt-in HTTP and S3/R2 live tests with documented env vars and cleanup expectations.

5. Browser network transport adapters
   - Provide browser WebSocket/WebTransport-style boundaries as `#net`-shaped resources.
   - Keep browser raw TCP explicitly unavailable.

6. SyncFs backend scheduling visibility
   - Keep deterministic scheduler tests.
   - Expose scheduler pending/due/error state through stable status text and structured snapshots.

7. StarFS optional adapters and semantic depth
   - Keep raw OPFS and existing storage mounts.
   - Keep lightweight StarFS-compatible storage.
   - Add the separate `starfs-sdk` adapter hook for the external SDK.
   - Expose xattrs, snapshots, KV, and audit/tool-call data through filesystem/control surfaces.

8. Real VM provider integration
   - Keep deterministic VM provider as default.
   - Route optional real providers through `VmProvider` behind `#vm/<id>/ctl`, `state`, `console`, `config`, `alias`, and `guest`.

9. Native 9P import/serve ergonomics
   - Keep stdio serving.
   - Add native TCP stream import acceptance through `TcpStreamTransport`.

10. Browser File System Access/download automation
   - Keep fake-host tests default.
   - Add permission helper coverage and document real browser permission gates.

11. Documentation, examples, and verification
   - Keep planning, progress, conformance, live-test, architecture, and audit docs current.
   - Default verification remains offline; host/live checks are explicit opt-ins.

### Prior Real-Host-Depth Sprint Plan

1. Evidence refresh and storage direction
   - Refresh the gap/audit matrix and classify default offline, browser capability-gated, native opt-in, live-service opt-in, and explicit unsupported boundaries.
   - Evaluate `/home/bitnom/Code/apothic-monorepo/libs/rust/starfs` and document the chosen integration shape.
   - Gate: docs state the final storage hierarchy and adapter responsibilities.

2. Existing browser storage mount preservation
   - Keep OPFS, File System Access, Cache API, JS value, DOM, download, and worker storage adapters working independently.
   - Gate: Node/browser tests prove read/write/list/stat/mkdir/remove behavior where supported.

3. Raw OPFS task-facing namespace mounts
   - Export async browser storage adapters over MessagePort 9P and mount them into browser task paths such as `#task/<id>/ns/storage/opfs`.
   - Cover read/write/list/stat/mkdir/remove, large I/O, cancellation, and persistence across remount.
   - Gate: Playwright smoke covers OPFS when browser capability exists.

4. StarFS optional mount backend
   - Add a StarFS-compatible adapter over an explicit backing adapter, OPFS by default.
   - Expose filesystem, `.starfs/kv`, `.starfs/toolcalls`, and `.starfs/snapshots` surfaces.
   - Gate: raw OPFS and StarFS mount side by side with independent state.

5. Cross-document 9P import parity
   - Expand mutation, error, large read/write, large directory, concurrent import, cancellation, malformed frame, and teardown coverage.
   - Gate: browser/Node 9P tests prove success, failure, cancellation, and cleanup behavior.

6. Live backend depth
   - Keep offline fake transports as default.
   - Document exact live HTTP/S3/R2/SyncFs environment variables and commands.
   - Gate: live checks are opt-in and never required by default verification.

7. Network, VM, and execution host depth
   - Keep deterministic `#net` and `#vm` as default.
   - Add opt-in native TCP acceptance separate from default `accept all`.
   - Continue Go-compatible JS/WASM execution through the browser worker lifecycle fixture.
   - Gate: deterministic defaults pass; opt-in host checks have exact commands.

8. Final cleanup and verification
   - Re-run unsupported-marker audit, formatting, linting, Rust tests, Node tests, wasm builds, browser smoke, and CLI acceptance.
   - Gate: default verification passes offline and every remaining real-host boundary is documented.

## Completion Sprint Plan

This sprint is the final port-completion run. The work should proceed continuously through all gates below until the Rust implementation is either complete or every intentionally unported surface is classified in documentation with a test proving the user-facing behavior.

### 1. Evidence Inventory And Gap Matrix

- Build a machine-readable audit of WASI preview1 imports implemented by `WasmiWasiHandler`, imports still missing, imports intentionally unsupported, and the exact errno behavior for unsupported host capabilities.
- Audit direct JS/WASM execution paths, browser worker bootstrap paths, task fd/namespace handoff, browser custom elements, and current smoke fixtures against the reference the legacy upstream reference checkout behavior that still matters.
- Audit device roots for terminal, VM, and network files against the expected Plan 9-style file protocol: stable files, control commands, state transitions, blocking or async behavior, close semantics, resize/winch behavior, and error behavior.
- Audit backend surfaces for HTTP, S3/R2, browser storage, SyncFs scheduling, cache validators, conditional requests, auth errors, pagination, object metadata, conflict handling, and capability detection.
- Audit all `unsupported`, `not supported`, `placeholder`, `TODO`, `FIXME`, `todo!`, and `unimplemented!` sites. Each site must be assigned to one of: remove by implementation, keep as a host capability boundary, or keep as an explicit spec-level unsupported behavior with direct tests.
- Gate: add or update a checked-in audit fixture or docs table mapping every remaining gap to a tranche below, with no unclassified placeholders.

### 2. WASI Preview1 Completion

- Complete the preview1 import matrix for fd, path, clock, random, args/env, process, polling, advisory, sync, file metadata, and socket-like imports.
- Replace broad unsupported behavior with precise host behavior where Star 9 can model it over task namespaces and fd handles.
- Implement socket/listener behavior once the network device adapter contract is in place: create/listen/accept/connect/read/write/shutdown should route through deterministic Star 9 network resources by default and native/browser adapters when explicitly enabled.
- Add representative compiled fixtures, not only inline WAT, covering file tree traversal, stdio, args/env, clocks/random, fd renumbering, links/symlinks/xattrs where applicable, polling, network sockets, nonzero exits, traps, and invalid syscall/error paths.
- Ensure WASI workloads run through the normal Star 9 task lifecycle: namespace clone, cwd/preopens, fd table installation, stdio descriptors, task messages, exit state, cleanup, and deterministic status reporting.
- Gate: Rust tests and CLI acceptance run compiled WASI workloads through Star 9 task paths without Go shims, and the syscall matrix has no unclassified preview1 imports.

### 3. Direct Browser JS/WASM Execution

- Move browser execution beyond bootstrap-only fixtures by running real module workers that instantiate representative JS and WebAssembly workloads through Star 9 runtime ports.
- Provide a browser-side WASI import object backed by Star 9 runtime requests for namespace and fd operations, with stdio and task messages routed through the same task fd table used by native execution.
- Support namespace/fd/stdio handoff into browser workers, explicit task start/exit/error messages, port handoff, structured worker cleanup, and deterministic propagation of worker failures into `#task/<id>/exit`.
- Keep direct `.wasm` behavior explicit: either implement direct browser WASM execution through the Star 9 WASI import object or retain a tested, documented unsupported boundary if only JS-runner execution is meant to be public.
- Expand browser smoke and Node tests to cover successful JS workload, successful WASM workload, stderr/stdout routing, namespace read/write, worker error, port transfer, cancellation/termination, and cleanup.
- Gate: Playwright smoke starts real browser JS and WASM workloads through Star 9 task paths and observes fd/stdout/task/exit/port behavior without test-only shortcuts.

### 4. Terminal, VM, And Network Device Parity

- Terminal: finish browser element protocol parity on top of the retained `data`, `program`, `screen`, `resize`, `winch`, `state`, and `size` files. Cover CRLF normalization, resize event delivery, screen retention, clear/reset, close behavior, and browser element integration.
- VM: introduce a provider contract for VM lifecycle operations and keep the deterministic provider as the default offline implementation. Implement enough real provider plumbing for examples to start, observe state, exchange console/control data, stop, reset, and report errors through stable device files.
- Network: split the deterministic Plan 9-style model from optional real adapters. Implement native TCP listener/dialer behavior and a browser transport boundary where browser capability exists, while preserving offline deterministic tests as the default.
- Add CLI examples and acceptance paths for terminal, VM, and TCP behavior that exercise the same files users interact with.
- Gate: terminal browser smoke, VM lifecycle acceptance, deterministic network tests, native opt-in TCP tests, and browser transport smoke all pass.

### 5. Live Backend And Browser Storage Hardening

- HTTP: deepen remote metadata semantics, validators, range/conditional behavior where needed, multipart metadata, mutation preconditions, cache invalidation, transport errors, redirect/auth handling where supported, and opt-in live-server tests.
- SyncFs: connect browser timer scheduling and backend-specific scheduling semantics to real async storage targets, including explicit flush, close, retry-after-error, and no-retry-spin behavior.
- S3/R2: add opt-in live bucket coverage for GET/PUT/DELETE/list, pagination, metadata, CAS/conflict behavior, auth failure, SigV4 edge cases, and R2/S3 service differences. Keep default tests offline.
- Browser storage: expand real capability-detected Playwright smoke for OPFS, File System Access where automatable, Cache API, JS value, DOM, download, and worker-backed adapters. Cover read/write/list/stat/mkdir/remove, failure modes, and cleanup.
- Gate: offline default tests remain deterministic, live tests are environment-gated and documented, and browser smoke covers real host storage capabilities without test-only storage shortcuts.

### 6. True Async 9P Cancellation And Transport Parity

- Replace synchronous-only `Tflush` acknowledgement with an async operation registry for 9P server work that can cancel in-flight reads, writes, walks, directory reads, backend calls, and browser transport requests where the underlying host supports cancellation.
- Ensure `Tflush` removes or aborts pending work by old tag, returns `Rflush`, prevents late replies from completing cancelled client requests, and preserves fid/session state exactly where the protocol requires it.
- Add stress coverage for partial frames, malformed sizes, response ordering, duplicate tags, unknown tags, concurrent operations, cancellation races, remote errors, close/unmount while pending, and browser `MessagePort` transfer edge cases.
- Extend native import/serve hooks beyond stdin/stdout where examples need it, while keeping Rust-owned stream framing as the common boundary.
- Gate: Rust, Node, and Playwright conformance prove cancellation, late replies, remote errors, and browser/native transport stress behavior.

### 7. Examples, CLI, Distribution, And Docs

- Port or replace representative namespace, bind, import/export, terminal, VM, TCP, WASI REPL, JS/WASM REPL, and workbench-style examples with Rust-backed artifacts.
- Expand CLI acceptance around native 9P import/serve, device protocols, worker protocol, browser artifact generation, live-test opt-ins, and distribution outputs.
- Update README, architecture, conformance, and any site/example docs to describe Rust as the primary implementation, the supported host capability boundaries, and how to run offline, browser, and live backend verification.
- Gate: every major public claim has a test, smoke, fixture, or documented opt-in verification path.

### 8. Final Cleanup And Release Gate

- Remove obsolete scaffolding and temporary adapters that were only useful during porting.
- Resolve the unsupported audit: implemented surfaces should no longer say unsupported, host capability boundaries should produce precise errors, and spec-level unsupported behavior should be covered by tests and docs.
- Run final formatting, linting, Rust tests, Node tests, wasm build, wasm-pack build, Playwright smoke, CLI acceptance, native opt-in tests, and live opt-in tests when credentials/capabilities are present.
- Gate: the working tree is clean, docs and progress are current, all default verification passes offline, and opt-in live/browser/native coverage is documented with exact commands.

## Planning Constraints

- Do not attach scheduling/time estimate assumptions to planning docs.
- Use the legacy upstream reference checkout as the source reference while porting behavior into Rust.
- Do not invoke Go code from runtime, build, test, browser smoke, or conformance paths.
- Treat Rust specs, fixtures, and conformance tests as the authoritative behavior source once behavior is ported.
- Prefer generalized Rust designs over one-off translations of individual files.
- Do not add compatibility shims for unpublished intermediate APIs unless they are needed for conformance testing or migration of real examples.
- Validate assumptions with Rust tests, fixtures, browser smoke checks, and source inspection.

## Target Outcome

The sprint is complete when `star9` can build and run a Rust-native implementation of the main Star 9 runtime surfaces:

- Plan 9-style namespace and bind semantics.
- Core filesystem traits, metadata, file handles, and path resolution.
- In-memory and local filesystem backends.
- Task/resource model with per-task namespaces and file descriptors.
- Browser/WASM runtime entry point equivalent to the current `wasm` package.
- Public API surface equivalent to the current JS handle/RPC file operations.
- Worker execution paths for WASI and Go-compatible JS/WASM workloads, with existing examples ported or replaced by Rust-backed equivalents.
- Conformance tests covering the Rust-owned behavior specs and representative browser examples.

## Port Shape

Use a Rust workspace with crates split by semantic boundary:

- `star9-core`: errors, paths, file modes, metadata, context/origin flags, utility types.
- `star9-fs`: filesystem and file traits, helper operations, `fskit` equivalents, `memfs`, `localfs`, `tarfs`, pipes, signals, caches.
- `star9-vfs`: namespace, bind/unbind, union behavior, path resolution, symlink traversal.
- `star9-task`: tasks, task filesystem, drivers, file descriptors, task-local namespace cloning.
- `star9-protocol`: API method schemas, CBOR/RPC message types, HTTP filesystem metadata, 9P bridge types where needed.
- `star9-runtime`: root construction, built-in device registration, CLI/runtime composition.
- `star9-web`: `wasm-bindgen`/web-sys integration, web components, workers, JS value filesystem, browser storage backends.
- `star9-cli`: native command entry point and serving/build tooling.

These crate names are working names. They can be collapsed if implementation evidence shows that fewer crates reduce coupling without blurring ownership.

## Evidence Gates

Proceed through these gates in order. A gate is done only when the Rust implementation has tests or differential evidence proving the targeted behavior.

1. Source map and contracts
   - Inventory exported APIs, filesystem extension traits, resource/device roots, worker message schemas, and example entry points from the legacy upstream reference checkout.
   - Convert required runtime behavior from the reference implementation into Rust-owned conformance fixtures and checklists.
   - Mark generated, vendored, or reference-only code that should not be ported directly.

2. Workspace foundation
   - Create the Rust workspace, crate boundaries, build commands, lint/test commands, and basic documentation.
   - Add fixtures and a test harness capable of validating Rust behavior without invoking Go code.

3. Core filesystem model
   - Port file modes, file info, directory entries, errors, path validation, open flags, and context/origin flags.
   - Define Rust traits for file, directory, filesystem, stat, create, remove, rename, hard link, chmod/chown/chtimes, truncate, symlink/readlink, xattrs, watch, and sync.
   - Port helper operations before backend implementations so all backends share one behavior layer.

4. Basic backends and `fskit`
   - Port node/file abstractions, map filesystem, union filesystem, function/control/field files, stream files, and directory iterators.
   - Port `memfs` and `localfs` as the first complete read/write backends.
   - Use Rust conformance fixtures for permissions, symlinks, directory synthesis, file sizes, and open flags.

5. Namespace and bind semantics
   - Port `vfs::NS`, binding order, direct binding resolution, subpath binding resolution, synthesized parent directories, and writable binding selection.
   - Add tests for overlapping binds, union directories, file-vs-directory binds, unbind behavior, symlink behavior through namespace origins, and create/mkdir target selection.

6. Task/resource model
   - Port root creation, `TaskFS`, task allocation, task drivers, aliases, task fields, control files, fd allocation, standard fd lookup, and namespace cloning.
   - Add tests for task creation, self lookup, aliases, fd lifecycle, and driver selection.

7. Device filesystems
   - Port pipe and signal first because they are core communication primitives.
   - Port terminal, VM, worker, DOM, cache, download, JS value filesystem, browser storage, and network devices according to dependency order.
   - For device surfaces that depend heavily on browser APIs, keep Rust traits host-neutral and place JS bindings in `star9-web`.

8. Protocol and API surfaces
   - Port the public file API currently exposed by `api.Responder` and `api/handle.js`.
   - Implement typed request/response structures for open, read, write, stat, readdir, mkdir, bind, unbind, rename, copy, remove, chmod, chown, truncate, symlink, readlink, chtimes, wait-for, and fd operations.
   - Preserve protocol semantics, not necessarily internal names.
   - Add protocol fixtures so JavaScript clients can exercise the Rust implementation without depending on Go internals.

9. WASM and browser runtime
   - Port root runtime initialization and built-in bindings from the current `wasm` package.
   - Recreate `star9-system` behavior with Rust-backed WASM exports and minimal JS glue.
- Port worker startup, task worker messaging, port opening, 9P import/export, namespace setup from `<star9-bind>`, and mount integration for browser filesystem bindings. Keep the JS Worker host facade covered by deterministic fake-worker tests while real browser execution is integrated.

10. WASI and Go-compatible JS execution
- Extend the Rust Wasmi WASI preview1 handler, including fd positional I/O/allocation and remaining preview1 syscall coverage, and port or replace the current worker shims for Go-compatible JS/WASM workloads.
    - Keep execution adapters isolated behind task drivers.
    - Validate with Rust-backed WASI and Go-compatible JS/WASM tests/examples.

11. CLI, build, and examples
    - Build the native CLI and web distribution artifacts from the Rust workspace.
    - Port representative examples so they run against Rust-backed Star 9.
    - Keep examples as acceptance tests where possible.

12. Hardening and cleanup
    - Run full Rust tests, browser smoke tests, and conformance checks.
    - Remove temporary scaffolding that is not needed for ongoing conformance.
    - Update docs to describe the Rust architecture as the primary implementation.

## Largest Port Surfaces

- `fs`: core traits, helpers, namespace-sensitive operations, and most backend behavior.
- `web`/`wasm`/`elements`: browser runtime, web components, JS/WASM host integration.
- `wasi` and `gojs`: worker shims and execution adapters.
- `task.go`: small but central task/resource/fd model.
- `api`: public file-operation RPC boundary.
- Storage/protocol backends: `httpfs`, `r2fs`, `p9kit`, `idbfs`, `jsfs`, `fsa`, `caches`.

## Acceptance Criteria

- `cargo test --workspace` passes.
- Rust conformance tests cover the core filesystem, namespace, task, and API semantics.
- Native CLI acceptance commands cover 9P loopback, deterministic devices, runtime worker protocol smoke paths, and fd-backed stdout routing.
- Browser smoke tests prove that `star9-system` can initialize, bind filesystems, open a handle, run file API operations, mount browser storage/9P imports, start representative WASI/Go-compatible JS tasks, and run a real module-worker JS/WASM task through Star 9 worker paths.
- Existing examples have Rust-backed equivalents or explicit replacement tests.
- The public Rust API is documented enough to guide continued development from this repository alone.
