# Star 9 Rc Feature Completion Sprint Plan

Updated status: 2026-05-14  
Implementation branch: `rc-features`  
Latest implementation state: rc language parity, Plan 9 service command compatibility, and rc-first shell entry points are included in this branch.

## Executive Status

The rc language sprint is complete for the current Star 9 host model, and the first recommended Plan 9 command/service compatibility tranche has now landed.

Current evidence:

- `star9-rc` is a reusable standalone rc crate with no Star 9 runtime, CLI, web, browser, or native process dependency.
- `star9-shell::rc` adapts the reusable rc host traits to Star 9 namespace/file/device/shell operations.
- `star9 shell`, `star9 rc`, `Star9System.createRcShell()`, `<star9-shell>`, and `examples/shell.html` are rc-first.
- The smaller Star 9 admin parser remains explicitly available as `star9 shell --simple` and `<star9-shell simple>`.
- `docs/audits/rc-compatibility-matrix.json` records current rc compatibility status.
- `docs/audits/plan9-command-compatibility-matrix.json` records Plan 9 command/service compatibility status.
- The current 9front `rc/bin/9fs` script parses and runs its no-argument usage path under `star9 rc`.
- Unquoted Plan 9 service addresses such as `tcp!9p.io` now remain single rc words.
- `bind`, `unmount`, `srv`, and `mount` are available as Star 9 shell commands and therefore as rc external commands.
- `srv` supports loopback Star 9 namespace exports by default through `#srv`/`srv`.
- `mount` mounts registered services through ordinary Star 9 namespace binds, including `/n`-style paths represented as `n/...`.
- `dossrv`, `vacfs`, and network service sources such as `tcp!host` now return precise provider-missing errors rather than unknown-command behavior.

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

- Add a mounted `#env` or `/env` Star 9 device surface if scripts need direct filesystem access to environment entries.

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

Host-limited placeholders:

- `rfork`
- `flag`
- `builtin`

Remaining work:

- Exact `rfork` namespace/process semantics should wait for matching Star 9 provider behavior.
- `flag` and exact `builtin` behavior can be tightened later if real scripts require their exact 9front/plan9port edge behavior.

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

Complete for destination-based unmount.

Supported:

- `unmount dst`
- `unmount src dst`, currently implemented as destination mountpoint removal

Implementation surface:

- Added `star9-vfs::Namespace::unbind_path`
- `ShellHost::unmount_path`
- Shell command available to rc external command dispatch

Remaining precision work:

- Source-specific unmount by filesystem identity can be added later if scripts need exact Plan 9 two-argument semantics.

### 4. `#srv` / `srv`

Complete for loopback Star 9 services.

Implemented:

- Runtime `ServiceRegistry`
- Hidden `#srv`
- Visible `srv`
- Service descriptor files
- Service unregister via remove
- Loopback root namespace exports

### 5. `srv`

Complete with provider limits.

Supported:

- `srv`
- `srv name`
- `srv root name`
- `srv self name`
- `srv loopback name`
- `srv -m root name mountpoint`

Provider-limited:

- `srv tcp!host name`
- `srv -nqC tcp!host name /n/name`

Those return precise provider-missing errors until a native/browser network service provider is configured.

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
cargo run -q -p star9-cli -- rc -c 'srv -nqC tcp!9p.io sources /n/sources'
cargo run -q -p star9-cli -- shell -c 'srv root rootsrv; mount rootsrv n/root; ls #srv; ls n/root'
cargo run -q -p star9-cli -- shell --simple -c 'mkdir demo; write demo/hello ok; cat demo/hello'
```

Expected provider-boundary behavior:

```text
srv: tcp!9p.io: provider not configured for this Star 9 runtime
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

2. Native/browser network service providers:
   - Add configured providers for `srv tcp!host name` style flows.
   - Route them through `#net`, 9P transports, service files, and ordinary namespace mounts.

3. Provider-backed commands:
   - Implement `dossrv` only after a disk/partition provider exists.
   - Implement `vacfs` only after a vac/Venti/archive provider exists.

4. Exact edge semantics:
   - Source-specific `unmount src dst` behavior.
   - Exact `rfork`, `flag`, `builtin`, and `whatis` edge parity where real script coverage requires it.
   - A mounted `#env` or `/env` surface if scripts need direct environment-file access.

## Recommendation

The recommended immediate command tranche has been implemented. The next work should be provider-driven: add real service providers only when the underlying Star 9 device/backend exists, and keep every provider visible through file/service/namespace surfaces instead of shell-only side APIs.
