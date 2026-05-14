# Browser Service Provider Parity

Browser service providers must be honest about browser capabilities while preserving the Star 9 `#srv`/`mount` composition model.

## Implemented

- `srv import!url#system name` in the browser shell controller registers a cross-document Star 9 import service.
- `mount name path` mounts that service through the existing `star9-import` MessagePort/9P boundary.
- `srv -m import!url#system name path` registers and mounts in one step.
- `srv tcp!host!port name` returns an explicit browser raw-TCP error because raw TCP is not a browser API.

This path is covered by `tests/browser-shell.test.mjs` with fake browser facade methods. Full Playwright cross-document mutation/error coverage continues through the existing browser import and 9P tests.

## Provider Hooks

Future browser network service providers should use explicit address families rather than pretending to support raw TCP:

- `ws!url` or `wss!url` for WebSocket-backed 9P.
- `webtransport!url` for WebTransport-backed 9P where available.
- `import!url#system` for cross-document MessagePort-backed 9P.

Each provider must register a service entry and mount through the same user-facing `mount` flow. Provider-specific state should be visible through service descriptor files, `#net`-style resources, or documented control/status files.

## Non-Goals

- No browser raw TCP compatibility claim.
- No direct JS object handles as filesystem capabilities.
- No service mount side channel that bypasses Star 9 namespace paths.
