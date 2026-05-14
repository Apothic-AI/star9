# Star 9 Rc Process-Graph Parity Sprint Plan

Status: implemented for task/fd graph records, with true streaming/concurrent external execution still classified as provider-depth work.

## Goal

Move Star 9 rc beyond purely opaque in-process pipeline/job behavior by introducing host-neutral process-graph contracts and a Star 9 adapter implementation that exposes the intended graph through Star 9 tasks, fds, and generated pipe resources.

Plan 9 rule:

> Pipelines, jobs, rfork, notes, and wait must be represented through Star 9 tasks, fd tables, pipes, namespaces, env files, and device files. Do not add host-shell side channels.

## Completed

- Added host-neutral process graph types to `star9-rc`:
  - `RcProcessGraphSpec`
  - `RcProcessStageSpec`
  - `RcFdBindingSpec`
  - `RcProcessGraphRecord`
  - `RcProcessStageOutcome`
- Extended `RcHost` with:
  - `prepare_process_graph`
  - `finish_process_graph`
  - `wait_process_job`
  - `send_note_to_processes`
- Kept default host behavior as a no-op fallback so `star9-rc` remains reusable without Star 9 runtime dependencies.
- Updated rc evaluation so pipelines, background jobs, and process substitution prepare and finish graph records when the host supports them.
- Added `wait <job>` support while preserving `wait` over all pending deterministic jobs.
- Implemented Star 9 process graph preparation in `star9-shell::rc`:
  - generated graph roots under `.rc/graphs/rcgraphN`
  - generated pipe endpoints under `.rc/graphs/rcgraphN/pipe0`
  - one Star 9 task per pipeline/process-substitution/background stage
  - standard fd installation
  - selected fd bindings to generated pipe endpoints
  - task exit updates from rc stage status
- Routed note delivery through the host boundary and Star 9 `#signal/data` where present.
- Added tests proving:
  - pipelines create Star 9 task/fd graph state
  - process substitution creates Star 9 task/fd graph state
  - background jobs create task records and `wait <job>` drains the selected job
  - reusable rc still passes standalone fake-host tests
- Updated planning, progress, README, architecture, conformance, and audit docs.

## Current Boundary

The portable evaluator still executes pipeline stages deterministically and passes captured stdout/stderr between stages. The Star 9 adapter now exposes task/fd graph records for inspection and future provider execution, but it does not yet run external pipeline stages concurrently through a streaming task scheduler.

That remaining boundary is deliberate: native/browser execution providers need to support streaming stdio, task groups, and job lifecycle before rc should claim full Plan 9 process graph parity.

## Remaining Provider-Depth Work

- Concurrent external pipeline stage execution.
- Streaming fd graph execution for WASI, JS/WASM worker, and opt-in native process providers.
- Background task groups with provider-backed lifecycle instead of completed deterministic job records.
- Full `wait` over live provider task groups.
- More exact `rfork` namespace/env/fd/process-group sharing semantics.
- Note delivery to running provider-backed task groups.

## Verification

- `cargo fmt --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- `cargo test -p star9-fs --features native-http`
- `node --test tests/*.test.mjs`
- `cargo run -p star9-cli -- accept all`
- `cargo build -p star9-web --target wasm32-unknown-unknown`
- `wasm-pack build crates/star9-web --target web --out-dir ../../target/star9-web-pkg --dev`
- Playwright smoke against `http://127.0.0.1:4177/tests/browser-smoke.html`
