# Rc Pipeline, Jobs, Notes, And Rfork Boundary

Star 9 rc has three process-graph layers:

- The reusable `star9-rc` evaluator remains deterministic and host-neutral. This keeps browser and fake-host behavior portable.
- The Star 9 adapter now receives process-graph prepare/finish hooks and materializes pipeline, background-job, and process-substitution graph records as Star 9 tasks with fd entries pointing at generated pipe resources under `.rc/graphs/...`.
- Native-host Star 9 rc can execute external WASI stages, registered JS/WASM stages, and opt-in native stages through provider-backed executable graph hooks. Those hooks build task/fd/pipe graphs, start stages concurrently where the provider supports streaming, close pipe writer endpoints for EOF, honor provider-stage file read/write/append redirects and fd-close forms where representable, read final stdout/stderr through task fds, and aggregate rc pipeline status.
- Browser Star 9 rc now has a worker graph provider for graph-compatible worker stages. It preserves the deterministic evaluator for pure rc scripts and unsupported graph shapes, but can run `worker module.mjs | worker other.mjs` and `worker module.mjs & wait` through browser Worker tasks with bounded stdin/stdout handoff and task exit observation.

This is still provider-scoped, not bit-perfect Plan 9 process semantics. Pure built-ins and unsupported browser graph shapes keep the deterministic evaluator path. Hard provider cancellation and complete `rfork` sharing semantics remain explicit boundaries.

## Implemented

- `cmd1 | cmd2` with rc status aggregation.
- fd-selected pipelines such as `|[2]`.
- process substitution `<{cmd}` and `>{cmd}` through provider-backed graph execution for executable stages, with evaluator-backed temporary data preserved for pure built-ins and unsupported hosts.
- background `&` creates an rc job record and, in the Star 9 adapter, a task-backed graph record.
- `wait` drains deterministic job records, supports `wait <job>`, and reports job status.
- `sigexit` and `deliver_note("name")` route through rc functions.
- Host note delivery is exposed through `RcHost::send_note_to_processes`; the Star 9 adapter broadcasts through `#signal/data` where available and marks active provider jobs with `signal:<note>` state/status.
- `rfork e` is accepted through the host boundary; unavailable flags return precise errors.
- Star 9 adapter graph preparation creates observable `#task` entries for pipeline stages, background jobs, and process substitution, installs standard fds, binds generated pipe endpoints to selected fds, and records stage exit status.
- Provider-backed `wasi a.wasm | wasi b.wasm` runs stages concurrently through Star 9 task fd descriptors and pipe resources.
- Provider-backed mixed WASI/registered JS-WASM pipelines run through the same task fd descriptors when a JS/WASM handler is registered in `ExecutionRegistry`.
- Provider-backed executable stages can carry file read/write/append redirects and fd close forms into task fd descriptors where the graph can be represented safely.
- Provider-backed `wasi a.wasm & wait` starts a live provider job and waits on the provider receiver through the rc job id.
- Provider-backed graphs write persistent lifecycle files under `.rc/graphs/<id>/`: `kind`, `job`, `tasks`, `notes`, `state`, `status`, and `ctl`.
- Writing `cleanup` or `close` to `.rc/graphs/<id>/ctl` removes completed graph resources; writing `cancel <note>` marks active provider jobs with deterministic `signal:<note>` state/status.
- Browser rc graph-compatible worker stages can run through `Star9ShellController` and `SystemElement.startBrowserWorker` with bounded stdin/stdout handoff and `wait` support.

## Current Boundary

External commands that cannot be mapped to a provider still run through `RcHost::run_command`, which returns captured stdout/stderr/status. That interface is portable across native, browser, fake-host, and Star 9 namespace adapters, but it is not a streaming task graph. Native-host WASI, registered JS/WASM, and opt-in native stage commands now use the streaming task/fd provider path. Browser worker graph execution is intentionally a JS-side provider boundary because the wasm rc host trait is synchronous and browser fd streams are async; it uses bounded buffering unless a worker/provider supplies a true stream primitive.

Full Plan 9 process-like pipeline parity should wait until Star 9 can expose the following through task/fd files:

- `rfork` namespace/env/fd/process-group sharing semantics,
- provider-specific hard cancellation for running stages,
- arbitrary nested fd graphs and process substitutions that require true concurrent OS-style process graphs.

## Rule For Future Work

Do not add a host-shell pipeline side channel. The eventual implementation should build pipes, task fds, task groups, and namespace state through Star 9 files and device resources, then let rc drive those surfaces.
