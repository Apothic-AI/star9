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

`wanix-fs` includes native loopback coverage under the `native-http` feature. Broader live HTTP checks use an environment-gated test and should point at a temporary server that supports conditional GET/HEAD, validators, range responses, redirects, and auth failures. These checks are outside default test runs unless explicitly configured.

Run:

```sh
WANIX_LIVE_HTTP=1 \
WANIX_LIVE_HTTP_BASE_URL=https://example.test/wanix/readme.txt \
cargo test -p wanix-fs --features native-http --test live_backends live_http -- --nocapture
```

Supported environment:

```sh
WANIX_LIVE_HTTP=1
WANIX_LIVE_HTTP_BASE_URL=https://example.test/wanix
WANIX_LIVE_HTTP_AUTH=optional-token
WANIX_LIVE_HTTP_RANGE_URL=https://example.test/wanix/range.bin
WANIX_LIVE_HTTP_REDIRECT_URL=https://example.test/wanix/redirect
WANIX_LIVE_HTTP_AUTH_FAILURE_URL=https://example.test/wanix/private
```

## S3 And R2

The default suite validates `S3ObjectStore` and `AwsSigV4Signer` with deterministic fake transports. Live bucket checks must be explicitly enabled and should use disposable prefixes. The live test creates and deletes objects under the configured prefix and exercises `GET`, `PUT`, `DELETE`, prefix listing, metadata, compare-and-swap conflicts, and optional auth failure.

Run:

```sh
WANIX_LIVE_S3=1 \
WANIX_S3_ENDPOINT=https://s3.example.com \
WANIX_S3_REGION=auto \
WANIX_S3_BUCKET=wanix-live \
WANIX_S3_ACCESS_KEY_ID=... \
WANIX_S3_SECRET_ACCESS_KEY=... \
cargo test -p wanix-fs --features native-http --test live_backends live_s3 -- --nocapture
```

Supported environment:

```sh
WANIX_LIVE_S3=1
WANIX_S3_ENDPOINT=https://s3.example.com
WANIX_S3_REGION=auto
WANIX_S3_BUCKET=wanix-live
WANIX_S3_ACCESS_KEY_ID=...
WANIX_S3_SECRET_ACCESS_KEY=...
WANIX_S3_PREFIX=wanix-live-${USER}
WANIX_S3_SERVICE=s3
WANIX_LIVE_S3_AUTH_FAILURE=1
```

For Cloudflare R2, use the R2 S3-compatible endpoint, `WANIX_S3_REGION=auto`, and keep `WANIX_S3_SERVICE=s3`.

## Browser Storage

`tests/browser-smoke.html` performs capability-detected browser checks. OPFS and Cache API run when available. File System Access and download behavior depend on browser permissions and automation support, so they should be enabled only in host-capability runs.

Required browser storage behaviors: read, write, list, stat, mkdir, remove, error reporting, explicit flush/close where available, and cleanup of smoke data.

Raw OPFS is the preferred simple persistent browser filesystem. `wanix-system.mountStorage(...)` mounts OPFS directly into the browser async mount table. `wanix-system.mountStorageExport(...)` and `wanix-system.mountTaskStorage(...)` export the async adapter through a `MessagePort` 9P server, then mount the imported namespace at a normal browser Wanix path such as `#task/<id>/ns/storage/opfs`. This is the real browser storage boundary; synchronous Rust Wasmi task namespaces still use descriptor-backed stand-ins unless a browser worker proxy is mounted over 9P.

StarFS is an additional optional mount backend, not a replacement for raw OPFS or the other browser storage mounts. The current lightweight adapter is StarFS-compatible and OPFS-backed by default:

```js
await system.mountStarFs("workspaces/starfs/agent-a", {
  id: "agent-a",
  storage: { backend: "opfs", root: "starfs/agent-a" }
});
```

It exposes normal filesystem entries plus xattr helpers, `.starfs/kv`, `.starfs/toolcalls`, and restorable `.starfs/snapshots`. A separate `backend: "starfs-sdk"` adapter hook can wrap a real external StarFS SDK/PrimaDB/Turso worker/wasm adapter:

```js
await createBrowserStorageAdapter({ backend: "starfs-sdk", id: "agent-a" }, {
  starfsSdk: {
    factory: async (descriptor) => createExternalStarFsAdapter(descriptor)
  }
});
```

The SDK-backed adapter is optional and must not replace raw OPFS, existing storage adapters, or the lightweight StarFS-compatible adapter.

## Native And Browser Network

The default `#net` device is deterministic and offline. Real native TCP and browser transport adapters must be opt-in and should not replace deterministic tests as the default conformance oracle.

Run the native TCP loopback host-capability check with:

```sh
cargo run -p wanix-cli -- accept native-tcp
```

This opens a loopback `TcpListener`, connects a `TcpStream`, exchanges request/response bytes, and exits. It is separate from `accept all` so default verification remains deterministic and does not open sockets beyond explicit host opt-in.

Run the native 9P stream import/serve check with:

```sh
cargo run -p wanix-cli -- accept native-p9
```

This serves a `MemFs` export over a loopback `TcpListener`, imports it through `TcpStreamTransport`, performs read/write operations, and closes cleanly.

Browser network transport coverage is WebSocket/WebTransport-style and file-model-shaped. Raw TCP is not a browser API. The deterministic Node test uses a fake WebSocket transport; external browser network services should remain opt-in host checks.

## Native PTY Execution

Host process execution is opt-in. Run it only on native hosts where spawning `/bin/sh` through a pseudo terminal is acceptable:

```sh
cargo run -p wanix-cli -- accept native
```

This validates the Rust native PTY handler's stdout routing and exit-state propagation without making host process execution part of the default offline gate.
