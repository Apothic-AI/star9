# Star 9 Rc Execution Provider Parity Sprint

Status: implemented for the current provider scope on 2026-05-14.

## Goal

Move rc from task/fd graph records plus deterministic evaluator execution to real provider-backed execution where Star 9 can install streaming fds, while preserving the portable evaluator for pure built-ins, fake hosts, and browser-limited paths.

Plan 9 rule:

> Pipelines, jobs, signals, `rfork`, browser transports, and service providers must stay visible through Star 9 tasks, fds, namespaces, device files, `#srv`, `#net`, and 9P-style composition. No host-shell side channels.

## Completed

1. Execution graph contract
   - `star9-rc` now exposes host-neutral executable graph specs for stages, argv/env/cwd, fd mappings, graph kind, and job ids.
   - `RcHost` includes optional hooks for executing foreground graphs and starting provider-backed background jobs.
   - Pure built-ins and unsupported providers still use deterministic evaluator fallback.

2. Streaming fd graph runtime
   - `star9-shell::rc` builds `.rc/graphs/...` task/fd/pipe graph surfaces.
   - Provider graph stages install stdin/stdout/stderr and mapped fds through `ExecutionSpec` descriptors.
   - `PipeFs` now has non-destructive `stat` behavior so metadata probes do not close live pipe endpoints.

3. Concurrent external rc pipelines
   - Native-host two-stage and linear multi-stage WASI stages run concurrently through Star 9 task fds and pipe resources.
   - Opt-in native process stages are routed through the same provider graph when native execution is enabled.
   - Pipeline status aggregation preserves rc `a|b|c` status strings.

4. Background task groups and `wait`
   - External WASI/native graph jobs can start through provider threads.
   - `wait` and `wait <job>` can block on provider-backed job receivers and return stdout/stderr/status.
   - Pure built-in background jobs keep deterministic evaluator job records.

5. Notes/signals and `rfork`
   - Existing rc note hooks and `sigexit` behavior remain intact.
   - Star 9 note broadcast still routes through `#signal/data` where present.
   - `rfork e` remains supported. Unsupported flags continue to fail precisely until Star 9 has matching task scope controls.

6. Browser service providers
   - Browser shell `srv`/`mount` supports `import!url#system`, `ws!host!path`, `wss!host!path`, and configured `webtransport!host!path`.
   - `SystemElement.mountWebSocket9p` wraps a browser `WebSocket` as the binary 9P frame endpoint and mounts it through the existing async 9P namespace mount client.
   - Browser raw `tcp!` remains a precise capability error.

7. Docs and conformance
   - Updated `PLAN.md`, `PROGRESS.md`, `docs/CONFORMANCE.md`, `docs/audits/rc-process-graph-matrix.json`, `docs/audits/rc-pipeline-job-control-boundary.md`, and `docs/audits/browser-service-provider-parity.md`.

## Remaining Provider Depth

- JS/WASM worker stages in rc provider graphs.
- Browser worker-backed process graph execution through runtime fd/port streams.
- Active task-group note/signal delivery for provider-backed jobs.
- Persistent task-group/job cleanup files.
- Exact `rfork` namespace/env/fd/process-group sharing beyond `rfork e`.
- Concrete default WebTransport 9P provider.

## Evidence

- `cargo test -p star9-rc -p star9-shell`
- `node --test tests/browser-shell.test.mjs tests/browser-network-adapter.test.mjs`
