# Rc Pipeline, Jobs, Notes, And Rfork Boundary

Star 9 rc has three process-graph layers:

- The reusable `star9-rc` evaluator remains deterministic and host-neutral. This keeps browser and fake-host behavior portable.
- The Star 9 adapter now receives process-graph prepare/finish hooks and materializes pipeline, background-job, and process-substitution graph records as Star 9 tasks with fd entries pointing at generated pipe resources under `.rc/graphs/...`.
- Native-host Star 9 rc can execute external WASI stages and opt-in native stages through provider-backed executable graph hooks. Those hooks build task/fd/pipe graphs, start stages concurrently, close pipe writer endpoints for EOF, read final stdout/stderr through task fds, and aggregate rc pipeline status.

This is still provider-scoped, not bit-perfect Plan 9 process semantics. Pure built-ins and browser-limited hosts keep the deterministic evaluator path. JS/WASM worker graph execution, exact task-group cleanup files, active group signal delivery, and complete `rfork` sharing semantics remain explicit boundaries.

## Implemented

- `cmd1 | cmd2` with rc status aggregation.
- fd-selected pipelines such as `|[2]`.
- process substitution `<{cmd}` and `>{cmd}` through evaluator-backed temporary data.
- background `&` creates an rc job record and, in the Star 9 adapter, a task-backed graph record.
- `wait` drains deterministic job records, supports `wait <job>`, and reports job status.
- `sigexit` and `deliver_note("name")` route through rc functions.
- Host note delivery is exposed through `RcHost::send_note_to_processes`; the Star 9 adapter broadcasts through `#signal/data` where available.
- `rfork e` is accepted through the host boundary; unavailable flags return precise errors.
- Star 9 adapter graph preparation creates observable `#task` entries for pipeline stages, background jobs, and process substitution, installs standard fds, binds generated pipe endpoints to selected fds, and records stage exit status.
- Provider-backed `wasi a.wasm | wasi b.wasm` runs stages concurrently through Star 9 task fd descriptors and pipe resources.
- Provider-backed `wasi a.wasm & wait` starts a live provider job and waits on the provider receiver through the rc job id.

## Current Boundary

External commands that cannot be mapped to a provider still run through `RcHost::run_command`, which returns captured stdout/stderr/status. That interface is portable across native, browser, fake-host, and Star 9 namespace adapters, but it is not a streaming task graph. Native-host WASI and opt-in native stage commands now use the streaming task/fd provider path. JS/WASM worker graph execution remains a future provider because it needs browser/runtime port fd streams.

Full Plan 9 process-like pipeline parity should wait until Star 9 can expose the following through task/fd files:

- explicit stdin/stdout/stderr fd graph construction,
- background task groups,
- `wait` over task/job ids,
- `rfork` namespace/env/fd/process-group sharing semantics,
- note delivery into those task groups.

## Rule For Future Work

Do not add a host-shell pipeline side channel. The eventual implementation should build pipes, task fds, task groups, and namespace state through Star 9 files and device resources, then let rc drive those surfaces.
