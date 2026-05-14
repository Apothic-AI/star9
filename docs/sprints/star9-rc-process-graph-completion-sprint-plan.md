# Star 9 Rc Process Graph Completion Sprint

Status: implemented for the current provider model.

## Goal

Finish the remaining rc process-graph depth while preserving the Plan 9 shape of Star 9: rc execution flows through tasks, fd tables, pipes, namespaces, env files, graph/job files, notes, and provider contracts.

## Completed

- Browser rc process graph provider:
  - `Star9ShellController` now has a browser worker graph provider for graph-compatible worker stages.
  - `worker module.mjs | worker other.mjs` and `worker module.mjs & wait` run through `SystemElement.startBrowserWorker`.
  - Browser worker tasks expose completion, collected stdout/stderr, cancellation, and task exit observation.
  - Browser smoke covers a worker-backed rc pipeline.
- Provider cancellation:
  - Native Star 9 rc graph jobs can be marked through note delivery or `.rc/graphs/<id>/ctl` with `cancel <note>`.
  - Active jobs receive deterministic `signal:<note>` state/status, note records, task exits, and wait results.
  - Browser worker graph controllers can terminate owned workers.
  - WASI/native/registered in-process handlers remain precise hard-cancel boundaries unless a provider supplies an interrupt primitive.
- Graph cleanup controls:
  - Provider graphs expose `kind`, `job`, `tasks`, `notes`, `state`, `status`, and `ctl`.
  - `ctl` accepts `status`, `hold`, `release`, `cancel <note>`, `cleanup`, and `close`.
  - Completed graph cleanup unbinds graph resources and is covered by tests.
- Fd graph depth:
  - Runtime fd descriptors are append-aware.
  - Provider-backed executable stages carry representable file read/write/append redirects, fd close, selected pipe fds, and executable process substitution into task fd descriptors.
  - Pipe descriptors keep live pipe behavior instead of being reopened as truncating files.
- Documentation and evidence:
  - `PLAN.md`, `PROGRESS.md`, `docs/ARCHITECTURE.md`, `docs/CONFORMANCE.md`, `docs/audits/rc-process-graph-matrix.json`, and `docs/audits/rc-pipeline-job-control-boundary.md` describe the current state.

## Remaining Boundaries

- Exact `rfork` namespace/env/fd/process-group sharing remains limited to `rfork e` until Star 9 exposes matching task scope controls.
- Provider hard cancellation remains provider-specific. Browser workers can be terminated, but WASI/native/registered in-process handlers need explicit interrupt primitives before Star 9 can forcibly stop them.
- Browser rc graph execution uses bounded stdin/stdout handoff. True async fd/MessagePort streaming can be added once worker stages consume runtime fd streams directly.
- Arbitrary nested fd graphs and every Plan 9 rc fd edge form are not claimed bit-perfect until broader conformance proves them.

## Verification

Default verification remains offline and deterministic:

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
node --test tests/*.test.mjs
cargo run -p star9-cli -- accept all
cargo build -p star9-web --target wasm32-unknown-unknown
wasm-pack build crates/star9-web --target web --out-dir ../../target/star9-web-pkg --dev
```

Browser smoke:

```sh
python3 -m http.server 4177 --bind 127.0.0.1
# http://127.0.0.1:4177/tests/browser-smoke.html
```
