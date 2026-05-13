# Live And Host-Capability Tests

Default verification is offline and deterministic. Live or host-capability checks are opt-in so the workspace can pass in CI without credentials, cloud buckets, or browser permissions.

## Default Offline Verification

```sh
cargo test --workspace
cargo test -p wanix-fs --features native-http
cargo clippy --workspace --all-targets -- -D warnings
node --test tests/*.test.mjs
cargo build -p wanix-web --target wasm32-unknown-unknown
wasm-pack build crates/wanix-web --target web --out-dir ../../target/wanix-web-pkg --dev
cargo run -p wanix-cli -- accept all
```

Run the browser smoke after `wasm-pack build`:

```sh
python3 -m http.server 4177 --bind 127.0.0.1
```

Then open `http://127.0.0.1:4177/tests/browser-smoke.html` or drive it with Playwright and verify `document.body.dataset.status === "ok"`.

## HTTP

`wanix-fs` includes native loopback coverage under the `native-http` feature. Broader live HTTP checks should use a temporary server that supports conditional GET/HEAD, validators, range responses, redirects, and auth failures. Keep those checks outside default test runs unless the server endpoint is explicitly configured.

Expected environment for future live HTTP tests:

```sh
WANIX_LIVE_HTTP_BASE_URL=https://example.test/wanix
WANIX_LIVE_HTTP_AUTH=optional-token
```

## S3 And R2

The default suite validates `S3ObjectStore` and `AwsSigV4Signer` with deterministic fake transports. Live bucket checks must be explicitly enabled and should use disposable prefixes.

Expected environment for future live S3/R2 tests:

```sh
WANIX_LIVE_S3=1
WANIX_S3_ENDPOINT=https://s3.example.com
WANIX_S3_REGION=auto
WANIX_S3_BUCKET=wanix-live
WANIX_S3_ACCESS_KEY_ID=...
WANIX_S3_SECRET_ACCESS_KEY=...
WANIX_S3_PREFIX=wanix-live-${USER}
```

Required live behaviors: `GET`, `PUT`, `DELETE`, prefix listing, pagination, metadata preservation, compare-and-swap conflict handling, auth failure reporting, and cleanup of the configured prefix.

## Browser Storage

`tests/browser-smoke.html` performs capability-detected browser checks. OPFS and Cache API run when available. File System Access and download behavior depend on browser permissions and automation support, so they should be enabled only in host-capability runs.

Required browser storage behaviors: read, write, list, stat, mkdir, remove, error reporting, explicit flush/close where available, and cleanup of smoke data.

Raw OPFS is the preferred simple persistent browser filesystem. `wanix-system.mountStorage(...)` mounts OPFS directly into the browser async mount table. `wanix-system.mountStorageExport(...)` and `wanix-system.mountTaskStorage(...)` export the async adapter through a `MessagePort` 9P server, then mount the imported namespace at a normal browser Wanix path such as `#task/<id>/ns/storage/opfs`. This is the real browser storage boundary; synchronous Rust Wasmi task namespaces still use descriptor-backed stand-ins unless a browser worker proxy is mounted over 9P.

StarFS is an additional optional mount backend, not a replacement for raw OPFS or the other browser storage mounts. The current adapter is StarFS-compatible and OPFS-backed by default:

```js
await system.mountStarFs("workspaces/starfs/agent-a", {
  id: "agent-a",
  storage: { backend: "opfs", root: "starfs/agent-a" }
});
```

It exposes normal filesystem entries plus `.starfs/kv`, `.starfs/toolcalls`, and `.starfs/snapshots`. Full external StarFS SDK/PrimaDB inode semantics remain an opt-in worker/wasm integration boundary.

## Native And Browser Network

The default `#net` device is deterministic and offline. Real native TCP and browser transport adapters must be opt-in and should not replace deterministic tests as the default conformance oracle.

Run the native TCP loopback host-capability check with:

```sh
cargo run -p wanix-cli -- accept native-tcp
```

This opens a loopback `TcpListener`, connects a `TcpStream`, exchanges request/response bytes, and exits. It is separate from `accept all` so default verification remains deterministic and does not open sockets beyond explicit host opt-in.

## Native PTY Execution

Host process execution is opt-in. Run it only on native hosts where spawning `/bin/sh` through a pseudo terminal is acceptable:

```sh
cargo run -p wanix-cli -- accept native
```

This validates the Rust native PTY handler's stdout routing and exit-state propagation without making host process execution part of the default offline gate.
