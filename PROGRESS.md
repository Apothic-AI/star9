# wanix-rs Progress

## 2026-05-08

- Direction changed from wrapper-first to a full Rust port in a single sprint.
- Removed sprint-length assumptions from planning.
- Rewrote `PLAN.md` around ordered workstreams, evidence gates, target runtime surfaces, and acceptance criteria.
- Clarified porting policy: use `../wanix` as the reference implementation to clone/port from, while keeping runtime, build, tests, browser smoke, and conformance free of Go dependencies.
- Created the Rust workspace with crates for core values, filesystem/backends, namespace, tasks, protocol, runtime, web, and CLI.
- Ported core path, mode, metadata, context, open flag, and error contracts.
- Ported the filesystem trait layer, helper operations, `Node`/`MapFs`, `MemFs`, `LocalFs`, union directories, field/control files, pipe, signal, cache, and tar aliases.
- Ported namespace bind/unbind, bind ordering, union directory reads, synthesized parent directories, hidden `#` listing behavior, and routed write operations.
- Ported task allocation, task fields, control file commands, aliases, per-task namespace cloning, driver registration, and file descriptor lifecycle.
- Ported the public API operation set into typed Rust request/response structures covering the Wanix file API methods.
- Added runtime root construction with `#wanix`, `#task`, pipe, signal, ramfs, terminal, VM, worker, web, JS, cache, and download surfaces.
- Added browser/WASM facade and native CLI entry points for smoke operations.
- Added `README.md`, `docs/ARCHITECTURE.md`, `docs/CONFORMANCE.md`, `tests/browser-smoke.html`, and `tests/fixtures/api-operations.json`.
- Verified `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo build -p wanix-web --target wasm32-unknown-unknown`.
- Verified the generated wasm web package with Playwright at `tests/browser-smoke.html`; the page reached `document.body.dataset.status = "ok"` after binding ramfs, running file API operations, and starting WASI/Go-compatible JS adapter tasks.
- Observed `wasm-pack test --headless --chrome crates/wanix-web` compile the wasm test target, but the ChromeDriver runner exited before executing the harness. Replaced that harness path with the direct `wasm-pack build` plus Playwright browser smoke.
- Clarified project policy in docs: `../wanix` is the reference implementation for porting, but runtime, build, test, browser smoke, and conformance paths must not wrap, execute, link, shell out to, or test against Go code.

## 2026-05-07

- Inspected `../wanix` repository structure, build files, public entry points, and largest code surfaces.
- Confirmed `wanix-rs` is currently empty.
- Confirmed `../wanix` is a clean Git checkout on `main`.
- Measured first-party code shape after excluding vendored `misc/cbor` and generated worker bundles:
  - Largest areas are `fs`, `web`, `wasi`, `gojs`, `rc`, and device/resource packages.
  - Key boundary surfaces are the file/RPC API, 9P bridge, Plan 9-style namespace, task model, browser runtime, and filesystem implementations.
- Initial recommendation was wrapper-first; this has been superseded by the Rust-native full-port direction with no runtime dependency on legacy code.
