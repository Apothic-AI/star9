# Star 9 Rc Feature Completion Sprint Plan

Updated status: 2026-05-14  
Implementation branch: `rc-userland-depth`  
Latest implementation state: rc language parity, Plan 9 service command compatibility, rc-first shell entry points, environment-device integration, source-specific unmount, native 9P service import, and browser import service commands are included in this branch.

## Executive Status

The rc language sprint is complete for the current Star 9 host model, and the recommended rc/userland depth tranche has now landed for offline and opt-in native/browser service paths.

Current evidence:

- `star9-rc` is a reusable standalone rc crate with no Star 9 runtime, CLI, web, browser, or native process dependency.
- `star9-shell::rc` adapts the reusable rc host traits to Star 9 namespace/file/device/shell operations.
- `star9 shell`, `star9 rc`, `Star9System.createRcShell()`, `<star9-shell>`, and `examples/shell.html` are rc-first.
- The smaller Star 9 admin parser remains explicitly available as `star9 shell --simple` and `<star9-shell simple>`.
- `docs/audits/rc-compatibility-matrix.json` records current rc compatibility status.
- `docs/audits/plan9-command-compatibility-matrix.json` records Plan 9 command/service compatibility status.
- Checked-in Star 9-owned fixtures cover rc functions/control flow, environment/jobs, redirection/pipelines, and userland namespace/service flows.
- The current 9front `rc/bin/9fs` script parses and runs its no-argument usage path under `star9 rc`.
- Unquoted Plan 9 service addresses such as `tcp!9p.io` now remain single rc words.
- `bind`, `unmount`, `srv`, and `mount` are available as Star 9 shell commands and therefore as rc external commands.
- `srv` supports loopback Star 9 namespace exports by default through `#srv`/`srv`.
- `mount` mounts registered services through ordinary Star 9 namespace binds, including `/n`-style paths represented as `n/...`.
- `#env` and visible `env` expose rc environment entries as files; rc sessions import/export variables and functions through this surface.
- Source-specific `unmount src dst` removes one matching bind layer rather than dropping the whole destination.
- Native `srv tcp!host!port name` registers an imported 9P filesystem in `#srv` when explicitly used.
- Browser shells support `srv import!url#system name` and `srv -m import!url#system name mountpoint` over the existing cross-document MessagePort/9P import boundary.
- `dossrv` and `vacfs` now return precise provider-missing errors rather than unknown-command behavior.

## Current Compatibility Claim

Star 9 has documented partial-to-strong rc compatibility for the implemented language subset and a useful Plan 9 service/namespace command layer for offline workflows.

It should still avoid claiming full 9front userland script compatibility until Star 9 provides the remaining provider-backed services that those scripts can call, especially disk/partition providers, vac/Venti/archive providers, and configured native/browser network service providers.

## Plan 9 Rule

Host capabilities must appear as mounted namespaces, service files, task fds, or device files. Commands like `srv`, `mount`, `bind`, and `unmount` are thin userland frontends over those surfaces, not privileged side APIs.

## Completed Rc Language Work

### 1. Rc Core And Integration

Complete.

- Added reusable `crates/star9-rc`.
- Added Star 9 adapter in `crates/star9-shell/src/rc.rs`.
- Added CLI/browser entry points.
- Kept the old Star 9 admin shell separate from rc mode.

### 2. Parser And AST

Complete for current compatibility level.

- Simple commands.
- Assignments.
- Blocks.
- Parenthesized groups.
- Functions.
- `if`, `if not`, `for`, `while`, `switch`, `case`.
- `&&`, `||`, `!`, background `&`.
- Pipelines.
- Redirections.
- Command substitution.
- Here-document preprocessing.
- 9front-style `if not` layout across command separators.
- Service-address words containing `!`, such as `tcp!9p.io`.

### 3. Expansion And Values

Complete for current compatibility level.

- List variables.
- `name=value`.
- `name=(a b c)`.
- `$name`.
- `$name(n)`.
- `$#name`.
- `$"name`.
- `$*`, `$0`, `$1`, `$2`, and positional args.
- Local command assignment restoration.
- Rc apostrophe quoting and doubled apostrophes.
- `ifs` splitting.
- Caret concatenation.
- Globbing and pattern matching.
- `~` built-in behavior.

### 4. Redirection, Pipes, Process Substitution, Here Docs

Complete within Star 9's current in-process evaluator model.

- `<`
- `>`
- `>>`
- fd-specific output redirects
- fd dup/close for captured stdout/stderr:
  - `>[1=2]`
  - `>[2=1]`
  - `>[2=]`
- `/dev/null` reads/writes
- fd-selected pipeline input, including stderr piping
- process substitution:
  - `<{cmd}`
  - `>{cmd}`
- parsed here documents
- quoted here-doc delimiter suppression
- expanded unquoted here-doc bodies

Remaining host-model limitation:

- Pipelines are deterministic evaluator pipelines, not OS-concurrent process graphs. That is acceptable for the current Star 9/browser-capable model, but should be documented if exact process timing matters.

### 5. Environment And Notes

Complete at reusable rc-core level.

- Zero-byte-separated variable export/import.
- `fn#name` function export/import.
- `sigexit` execution on `exit`.
- `deliver_note("name")` dispatch to `sig<name>`.

Remaining host/device work:

- Star 9 now mounts hidden `#env` plus visible `env`; the rc adapter synchronizes variables/functions with this file surface.

### 6. Built-ins

Improved.

Implemented or improved:

- `.`
- `basename`
- `cd`
- `echo`
- `eval`
- `exec`
- `exit`
- `false`
- `pwd`
- `shift`
- `status`
- `test`
- `true`
- `wait`
- `whatis`
- `~`

Host-limited but stable:

- `rfork` supports host-routed supported flags and returns precise unsupported errors for unavailable namespace/process behavior.
- `flag` returns stable no-op success for supported current shell flags and precise failure for unknown flags.
- `builtin` explicitly dispatches to rc built-ins.

Remaining work:

- Exact `rfork` namespace/env/fd/process sharing semantics should wait for matching Star 9 task/process behavior.
- Exact 9front/plan9port edge formatting for `flag`, `builtin`, and `whatis` can be tightened later if real scripts require it.

### 7. `$path` Command Search And Dispatch

Complete for current model.

- `$path` rc script search.
- `$0` and `$*` for path-executed scripts.
- CLI rc script args.
- Star 9 adapter dispatch:
  - `.wasm` / `.wat` through `wasi`
  - `.js` / `.mjs` through `worker`
- Other commands go through the Star 9 shell/runtime command surface.

## Completed Plan 9 Command And Service Compatibility Tranche

### 1. Evidence Matrix

Complete.

- Added `docs/audits/plan9-command-compatibility-matrix.json`.
- Classified `bind`, `unmount`, `srv`, `mount`, `dossrv`, `vacfs`, `#srv`, `srv`, `n`, `mnt`, and `tmp`.

### 2. `bind`

Complete.

Supported:

- `bind src dst`
- `bind -a src dst`
- `bind -b src dst`
- `bind -c src dst`

Implementation surface:

- `ShellHost::bind_path`
- `RuntimeShellHost` binding over `star9-vfs::Namespace::bind`
- Shell command available to rc external command dispatch

### 3. `unmount`

Complete for destination-based and source-specific unmount.

Supported:

- `unmount dst`
- `unmount src dst`

Implementation surface:

- Added `star9-vfs::Namespace::unbind_path`
- Added `star9-vfs::Namespace::unbind_source_path`
- `ShellHost::unmount_path`
- `ShellHost::unmount_binding`
- Shell command available to rc external command dispatch

The two-argument form removes only bind layers whose recorded source path matches `src`, which avoids pointer-identity surprises for namespace self-binds.

### 4. `#srv` / `srv`

Complete for loopback Star 9 services and opt-in native TCP 9P services.

Implemented:

- Runtime `ServiceRegistry`
- Hidden `#srv`
- Visible `srv`
- Service descriptor files
- Service unregister via remove
- Loopback root namespace exports
- Native `tcp!host!port` 9P imports through `TcpStreamTransport`

### 5. `srv`

Complete with provider limits.

Supported:

- `srv`
- `srv name`
- `srv root name`
- `srv self name`
- `srv loopback name`
- `srv -m root name mountpoint`
- `srv tcp!host!port name` on native hosts
- browser shell `srv import!url#system name`
- browser shell `srv -m import!url#system name mountpoint`

Provider-limited:

- `srv -nqC tcp!host name /n/name`
- browser `ws!`/`wss!`/`webtransport!` service addresses until configured providers exist

Native `tcp!host!port` is implemented as a 9P service import. Browser raw TCP returns an explicit capability error because raw TCP is not a browser API.

### 6. `mount`

Complete with provider limits.

Supported:

- `mount service mountpoint`
- `mount -a service mountpoint`
- `mount -b service mountpoint`
- `mount -c service mountpoint [aname]`
- `mount -n service mountpoint [aname]`
- `mount -C service mountpoint [aname]`

Implementation surface:

- `Runtime::mount_service`
- `ServiceRegistry::get`
- `Namespace::bind`

### 7. Compatibility Directories

Complete for current defaults.

Installed in the runtime root namespace:

- `#env`
- `env`
- `#srv`
- `srv`
- `n`
- `mnt`

Not forced:

- `tmp`

Rationale:

- CLI shell sessions already get a writable workspace at `.`, and forcing `tmp` as a direct bind would make scripts that create `tmp` fail unnecessarily.

### 8. Provider-Heavy Commands

Complete as precise provider boundaries.

Provider-missing commands:

- `dossrv`
- `vacfs`

Do not fully port these yet. They should be implemented when Star 9 has corresponding disk/partition and vac/Venti/archive providers that can be exposed through files/services.

## Verification Passed For This Tranche

```sh
cargo fmt
cargo test -p star9-rc -p star9-vfs -p star9-runtime -p star9-shell -p star9-cli --tests
cargo test -p star9-cli --tests rc_runs_userland_namespace_fixture
cargo run -q -p star9-cli -- rc -c 'shape=(circle square); cat #env/shape'
cargo run -q -p star9-cli -- accept native-srv
cargo run -q -p star9-cli -- shell -c 'srv root rootsrv; mount rootsrv n/root; ls #srv; ls n/root'
cargo run -q -p star9-cli -- shell --simple -c 'mkdir demo; write demo/hello ok; cat demo/hello'
node --test tests/browser-shell.test.mjs
```

Expected browser raw-TCP boundary behavior:

```text
srv: tcp!host!564: raw TCP is not available in browsers
```

Expected loopback service behavior:

```text
rootsrv
mnt/
n/
srv/
```

## Remaining Work

1. Broader script corpus:
   - Build an independently authored fixture suite for common rc scripts that use `bind`, `unmount`, `srv`, and `mount`.
   - Keep 9front scripts as external references unless license/attribution is handled.

2. Browser network service providers:
   - Add configured providers for `ws!`, `wss!`, and `webtransport!` service flows.
   - Route them through `#net`-shaped resources, 9P transports, service files, and ordinary namespace mounts.

3. Provider-backed commands:
   - Implement `dossrv` only after a disk/partition provider exists.
   - Implement `vacfs` only after a vac/Venti/archive provider exists.

4. Exact edge semantics:
   - Exact `rfork`, `flag`, `builtin`, and `whatis` edge parity where real script coverage requires it.
   - Full OS-concurrent process/task pipeline graph behavior.

## Recommendation

The recommended immediate command tranche has been implemented. The next work should be provider-driven: add real service providers only when the underlying Star 9 device/backend exists, and keep every provider visible through file/service/namespace surfaces instead of shell-only side APIs.
