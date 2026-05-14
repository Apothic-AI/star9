# Rc Pipeline, Jobs, Notes, And Rfork Boundary

Star 9 rc has three process-graph layers:

- The reusable `star9-rc` evaluator remains deterministic and host-neutral. This keeps browser and fake-host behavior portable.
- The Star 9 adapter now receives process-graph prepare/finish hooks and materializes pipeline, background-job, and process-substitution graph records as Star 9 tasks with fd entries pointing at generated pipe resources under `.rc/graphs/...`.
- Native-host Star 9 rc can execute external WASI stages, registered JS/WASM stages, and opt-in native stages through provider-backed executable graph hooks. Those hooks build task/fd/pipe graphs, start stages concurrently where the provider supports streaming, close pipe writer endpoints for EOF, read final stdout/stderr through task fds, and aggregate rc pipeline status.

This is still provider-scoped, not bit-perfect Plan 9 process semantics. Pure built-ins and browser-limited hosts keep the deterministic evaluator path. Browser worker graph execution, hard provider cancellation, explicit cleanup control files, and complete `rfork` sharing semantics remain explicit boundaries.

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
- Provider-backed `wasi a.wasm & wait` starts a live provider job and waits on the provider receiver through the rc job id.
- Provider-backed graphs write persistent lifecycle files under `.rc/graphs/<id>/`: `kind`, `job`, `tasks`, `notes`, `state`, and `status`.

## Current Boundary

External commands that cannot be mapped to a provider still run through `RcHost::run_command`, which returns captured stdout/stderr/status. That interface is portable across native, browser, fake-host, and Star 9 namespace adapters, but it is not a streaming task graph. Native-host WASI, registered JS/WASM, and opt-in native stage commands now use the streaming task/fd provider path. Browser worker graph execution remains a runtime-port provider boundary because the wasm rc host trait is synchronous and browser fd streams are async.

Full Plan 9 process-like pipeline parity should wait until Star 9 can expose the following through task/fd files:

- `rfork` namespace/env/fd/process-group sharing semantics,
- provider-specific hard cancellation for running stages,
- explicit cleanup controls for provider graph resources,
- async browser worker fd/port streams integrated with the rc host boundary.

## Rule For Future Work

Do not add a host-shell pipeline side channel. The eventual implementation should build pipes, task fds, task groups, and namespace state through Star 9 files and device resources, then let rc drive those surfaces.
