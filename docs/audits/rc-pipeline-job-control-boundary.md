# Rc Pipeline, Jobs, Notes, And Rfork Boundary

Star 9 rc has two process-graph layers:

- The reusable `star9-rc` evaluator remains deterministic and host-neutral. This keeps browser and fake-host behavior portable.
- The Star 9 adapter now receives process-graph prepare/finish hooks and materializes pipeline, background-job, and process-substitution graph records as Star 9 tasks with fd entries pointing at generated pipe resources under `.rc/graphs/...`.

This is task/fd graph visibility, not yet a full OS-concurrent external process graph. External commands still return captured stdout/stderr/status through the host command boundary unless a concrete execution provider handles them.

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

## Current Boundary

External commands still run through `RcHost::run_command`, which returns captured stdout/stderr/status. That interface is portable across native, browser, fake-host, and Star 9 namespace adapters, but it is not a streaming task graph. The Star 9 graph records prove the intended fd/task shape and make it inspectable, but they do not yet drive concurrent external stage execution.

Full Plan 9 process-like pipeline parity should wait until Star 9 can expose the following through task/fd files:

- explicit stdin/stdout/stderr fd graph construction,
- concurrent external stage execution,
- background task groups,
- `wait` over task/job ids,
- `rfork` namespace/env/fd/process-group sharing semantics,
- note delivery into those task groups.

## Rule For Future Work

Do not add a host-shell pipeline side channel. The eventual implementation should build pipes, task fds, task groups, and namespace state through Star 9 files and device resources, then let rc drive those surfaces.
