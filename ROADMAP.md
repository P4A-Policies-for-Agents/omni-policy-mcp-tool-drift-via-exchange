# Roadmap — MCP Tool Drift Detection (Exchange)

## Now (v0.1) — shipped

- GCL schema for Exchange asset + enforcement + mode.
- Canonical-hash diff with field-level classification
  (description/inputSchema/outputSchema/annotations).
- Dual-phase filter (`on_request` + `on_response`) that lazily loads
  the pin, emits evidence, and (in `enforce`) strips drifted tools from
  `tools/list` on both `application/json` and `text/event-stream`
  transports.
- **Live Exchange fetch wired** — real PDK `HttpClient` + `Service`
  outbound: OAuth2 client-credentials token mint (or Basic header),
  descriptor GET, parse into `PinSet`. LKG retained on failure.
- **`baseUrl` as `format: service`** — registered as an Envoy upstream
  cluster; `Option<pdk::hl::Service>` bound in typed config.
- **`exchangePathPrefix` loopback mode** — managed-gateway egress-`Host`
  workaround (dispatch to `baseUrl` verbatim through a loopback route).
- `ExchangeRef` URL/path construction for descriptor + OAuth2 token
  endpoints (prefix-aware).
- Five integration test files + expanded `exchange` unit tests
  (token parse, descriptor parse shapes, path-prefix, credentials).

## Short-term (v0.2)

- **Cross-replica pin cache** — memoize the pin in the PDK `Cache`
  (from `CacheBuilder`) so warm replicas skip the cold fetch.
- **`version=latest` resolution** — fetch the resolved version per
  refresh; emit `version_changed` on transition.
- **`pin_stale` evidence** — fire on every failed refresh once an LKG
  pin exists.
- **`x-mcp-drift-warning` header** for `warn` mode.

## Medium-term (v0.3)

- **Stream-aware `tools/list`** for MCP servers that emit SSE chunks
  for large tool sets.
- **Per-environment override** — allow `prod` to use `enforce` while
  `staging` uses `warn`, all from a single asset.
- **Credential rotation** — re-read `credSecretRef` on hot reload.
- **Schema-level diff** — surface which field of `inputSchema`
  changed (today the diff is hash-level).

## Long-term (v1.0)

- **Multi-version support** — keep two Exchange versions warm so a
  blue/green rollout doesn't trip every request as drift.
- **Drift summary export** — periodic rollup of drift events posted
  back to Exchange asset metadata for design-time visibility.

## Risk register

| Risk | Mitigation |
|---|---|
| Exchange outage on cold start blocks all `tools/list`. | `failOpen.onPinUnavailable=true` for non-prod; evidence event always fires; LKG is retained across hot restarts. |
| Aggressive `refreshIntervalSec` exhausts Anypoint API quota. | Default 300 s; minimum 30 s; tuning guidance in README. |
| `version=latest` introduces silent drift on Exchange-side bumps. | `version_changed` evidence fires on every resolution change; recommend pinned semver for `enforce` mode. |
| `enforce.exactMatch=false` opens a hole. | Default is `true`; flipping to `false` still emits `descriptor_drift` evidence for visibility. |
| Stale credentials after rotation. | Hot-reload of `credSecretRef` planned for v0.3; today a policy reload picks up new secrets. |
