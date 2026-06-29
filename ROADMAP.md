# Roadmap — MCP Tool Drift Detection (Exchange)

## Now (v0.1)

- GCL schema for Exchange asset + enforcement + mode.
- Canonical-hash diff with field-level classification
  (description/inputSchema/outputSchema/annotations).
- Response-filter entrypoint that emits evidence and (in `enforce`)
  strips drifted tools from `tools/list`.
- `ExchangeRef` URL construction for descriptor + OAuth2 token
  endpoints.
- Five integration test files (drift classification, pin
  construction, Exchange URLs, config loading, passthrough).

## Short-term (v0.2)

- **HttpClient wiring** — OAuth2 client_credentials token mint, GET
  the descriptor URL with the bearer, parse into `PinSet`.
- **Timer-driven refresh** — refresh at `refreshIntervalSec`; on
  failure retain LKG and emit `pin_stale`.
- **`version=latest` resolution** — fetch the resolved version per
  refresh; emit `version_changed` on transition.
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
