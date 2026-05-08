# wanix-rs Progress

## 2026-05-08

- Direction changed from wrapper-first to a full Rust port in a single sprint.
- Removed sprint-length assumptions from planning.
- Rewrote `PLAN.md` around ordered workstreams, evidence gates, target runtime surfaces, and acceptance criteria.
- Kept `../wanix` as the behavioral oracle for conformance until Rust tests cover the same semantics.

## 2026-05-07

- Inspected `../wanix` repository structure, build files, public entry points, and largest code surfaces.
- Confirmed `wanix-rs` is currently empty.
- Confirmed `../wanix` is a clean Git checkout on `main`.
- Measured first-party code shape after excluding vendored `misc/cbor` and generated worker bundles:
  - Largest areas are `fs`, `web`, `wasi`, `gojs`, `rc`, and device/resource packages.
  - Key boundary surfaces are the file/RPC API, 9P bridge, Plan 9-style namespace, task model, browser runtime, and filesystem implementations.
- Initial recommendation was wrapper-first; this has been superseded by the full-port sprint direction.
