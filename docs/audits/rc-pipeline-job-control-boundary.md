# Rc Pipeline, Jobs, Notes, And Rfork Boundary

Star 9 rc currently keeps pipelines deterministic inside the reusable evaluator. This is intentional for the browser-capable core and should not be confused with a full OS-concurrent process graph.

## Implemented

- `cmd1 | cmd2` with rc status aggregation.
- fd-selected pipelines such as `|[2]`.
- process substitution `<{cmd}` and `>{cmd}` through evaluator-backed temporary data.
- background `&` creates a deterministic rc job record.
- `wait` drains deterministic job records and reports job status.
- `sigexit` and `deliver_note("name")` route through rc functions.
- `rfork e` is accepted through the host boundary; unavailable flags return precise errors.

## Current Boundary

External commands still run through `RcHost::run_command`, which returns captured stdout/stderr/status. That interface is portable across native, browser, fake-host, and Star 9 namespace adapters, but it is not a streaming task graph.

Full Plan 9 process-like pipeline parity should wait until Star 9 can expose the following through task/fd files:

- evaluator jobs backed by task resources,
- explicit stdin/stdout/stderr fd graph construction,
- concurrent external stage execution,
- background task groups,
- `wait` over task/job ids,
- `rfork` namespace/env/fd/process-group sharing semantics,
- note delivery into those task groups.

## Rule For Future Work

Do not add a host-shell pipeline side channel. The eventual implementation should build pipes, task fds, task groups, and namespace state through Star 9 files and device resources, then let rc drive those surfaces.
