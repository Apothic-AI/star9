# wanix-rs Plan

## Current Focus

Build a Rust-native Wanix runtime in a single sprint, using `../wanix` as the reference implementation to clone and port from without wrapping or depending on Go code.

Immediate protocol/runtime tranche:

- Expand browser system and binding parity on top of the Rust-native runtime, typed binding descriptors, and 9P loopback hooks.
- Wire the typed worker/task execution protocol from the runtime host into browser worker adapters and real execution drivers.
- Add cross-document MessagePort/WebSocket transport wiring for real remote 9P imports.
- Add JS/browser glue for the host-neutral browser storage registry and validated `<wanix-bind>` descriptors.
- Continue backend hardening with deterministic conflict/error behavior for `HttpFs`, plus real transport boundaries for browser/native adapters.
- Deepen placeholder device surfaces for terminal, VM, worker, and network behavior using Rust-owned tests.
- Keep expanding Rust-owned conformance fixtures for browser bindings, storage backends, worker messaging, execution, devices, and backend hardening.

## Planning Constraints

- Do not attach scheduling/time estimate assumptions to planning docs.
- Use `../wanix` as the source reference while porting behavior into Rust.
- Do not invoke Go code from runtime, build, test, browser smoke, or conformance paths.
- Treat Rust specs, fixtures, and conformance tests as the authoritative behavior source once behavior is ported.
- Prefer generalized Rust designs over one-off translations of individual files.
- Do not add compatibility shims for unpublished intermediate APIs unless they are needed for conformance testing or migration of real examples.
- Validate assumptions with Rust tests, fixtures, browser smoke checks, and source inspection.

## Target Outcome

The sprint is complete when `wanix-rs` can build and run a Rust-native implementation of the main Wanix runtime surfaces:

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

- `wanix-core`: errors, paths, file modes, metadata, context/origin flags, utility types.
- `wanix-fs`: filesystem and file traits, helper operations, `fskit` equivalents, `memfs`, `localfs`, `tarfs`, pipes, signals, caches.
- `wanix-vfs`: namespace, bind/unbind, union behavior, path resolution, symlink traversal.
- `wanix-task`: tasks, task filesystem, drivers, file descriptors, task-local namespace cloning.
- `wanix-protocol`: API method schemas, CBOR/RPC message types, HTTP filesystem metadata, 9P bridge types where needed.
- `wanix-runtime`: root construction, built-in device registration, CLI/runtime composition.
- `wanix-web`: `wasm-bindgen`/web-sys integration, web components, workers, JS value filesystem, browser storage backends.
- `wanix-cli`: native command entry point and serving/build tooling.

These crate names are working names. They can be collapsed if implementation evidence shows that fewer crates reduce coupling without blurring ownership.

## Evidence Gates

Proceed through these gates in order. A gate is done only when the Rust implementation has tests or differential evidence proving the targeted behavior.

1. Source map and contracts
   - Inventory exported APIs, filesystem extension traits, resource/device roots, worker message schemas, and example entry points from `../wanix`.
   - Convert required runtime behavior from the reference implementation into Rust-owned conformance fixtures and checklists.
   - Mark generated, vendored, or reference-only code that should not be ported directly.

2. Workspace foundation
   - Create the Rust workspace, crate boundaries, build commands, lint/test commands, and basic documentation.
   - Add fixtures and a test harness capable of validating Rust behavior without invoking Go code.

3. Core filesystem model
   - Port file modes, file info, directory entries, errors, path validation, open flags, and context/origin flags.
   - Define Rust traits for file, directory, filesystem, stat, create, remove, rename, chmod/chown/chtimes, truncate, symlink/readlink, xattrs, watch, and sync.
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
   - For device surfaces that depend heavily on browser APIs, keep Rust traits host-neutral and place JS bindings in `wanix-web`.

8. Protocol and API surfaces
   - Port the public file API currently exposed by `api.Responder` and `api/handle.js`.
   - Implement typed request/response structures for open, read, write, stat, readdir, mkdir, bind, unbind, rename, copy, remove, chmod, chown, truncate, symlink, readlink, chtimes, wait-for, and fd operations.
   - Preserve protocol semantics, not necessarily internal names.
   - Add protocol fixtures so JavaScript clients can exercise the Rust implementation without depending on Go internals.

9. WASM and browser runtime
   - Port root runtime initialization and built-in bindings from the current `wasm` package.
   - Recreate `wanix-system` behavior with Rust-backed WASM exports and minimal JS glue.
   - Port worker startup, task worker messaging, port opening, 9P import/export, namespace setup from `<wanix-bind>`, and browser filesystem bindings.

10. WASI and Go-compatible JS execution
    - Port or replace the current worker shims for WASI and Go-compatible JS/WASM workloads.
    - Keep execution adapters isolated behind task drivers.
    - Validate with Rust-backed WASI and Go-compatible JS/WASM tests/examples.

11. CLI, build, and examples
    - Build the native CLI and web distribution artifacts from the Rust workspace.
    - Port representative examples so they run against Rust-backed Wanix.
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
- Browser smoke tests prove that `wanix-system` can initialize, bind filesystems, open a handle, run file API operations, and start representative WASI/Go-compatible JS tasks.
- Existing examples have Rust-backed equivalents or explicit replacement tests.
- The public Rust API is documented enough to guide continued development from this repository alone.
