# MCP Tool Drift Detection (Exchange)

Detects MCP tool drift by comparing runtime `tools/list` responses against canonical descriptors published to **Anypoint Exchange**. Built on PDK 1.9 (Rust → WebAssembly) for MuleSoft Flex Gateway / Omni Gateway.

After the policy is applied, every `tools/list` response is validated against the descriptor set stored in an Exchange MCP asset. Drifted tools can be stripped, annotated with warnings, or logged for observability — depending on the configured enforcement mode.

## Apply this policy to an MCP-typed API instance

Create the API instance in API Manager with **type = MCP**. The policy intercepts MCP JSON-RPC traffic on the response path; upstream MCP servers continue to serve their normal endpoints.

## How it works

1. **Lazy pin load**: on the first request/response after start (and again after `refreshIntervalSec`), the canonical descriptor set is fetched from Exchange via the configured `orgId / groupId / assetId / version` coordinates. This is a **two-hop** fetch — HOP 1 gets the asset metadata (`GET exchange/api/v2/assets/{group}/{asset}/{version}`), which lists the asset's files with pre-signed `externalLink` S3 URLs; HOP 2 downloads the selected descriptor file from that pre-signed link. The fetch authenticates using Connected App OAuth2 client-credentials or basic auth on hop 1 (hop 2 uses the pre-signed URL, no `Authorization`). (Outbound calls are made from the request/response filter — not from policy bootstrap — because connected-mode Flex Gateway only connects outbound from the request path.)
2. **Last-known-good**: once loaded, the pin is cached in-process. If Exchange is unreachable on a later refresh, the LKG cache is retained.
3. **Runtime inspection**: each `tools/list` response is intercepted. Every tool descriptor is hashed (name + description + inputSchema + outputSchema + annotations) and compared to the pinned hash.
4. **Enforcement decision**: based on `mode` (enforce / warn / observe) and the drift classification (exact match / added tool / removed tool / descriptor changed), the policy strips, annotates, or logs.

## Enforcement modes

| Mode      | Behaviour when drift detected                                                       |
|-----------|-------------------------------------------------------------------------------------|
| `enforce` | Strip drifted tools from the response. Clean tools pass through.                   |
| `warn`    | Pass all tools but inject `x-mcp-drift-warning` header with drift summary.         |
| `observe` | Pass all tools; emit structured log events only (telemetry for dashboards).        |

## Configuration

Minimal config — enforce exact match against an Exchange MCP asset:

```yaml
config:
  exchange:
    orgId:             "00000000-0000-0000-0000-000000000000"
    groupId:           "your-group-id"
    assetId:           "example-mcp-server"
    version:           "1.0.0"
    baseUrl:           "https://anypoint.mulesoft.com"
    authType:          "oauth2_client_credentials"
    credSecretRef:     "exchange-creds"
    refreshIntervalSec: 300

  enforce:
    exactMatch:        true
    allowAddedTools:   false
    allowRemovedTools: true

  mode: enforce

  failOpen:
    onPinUnavailable:  false
```

### Required fields

- `exchange.orgId`, `exchange.groupId`, `exchange.assetId`, `exchange.version`: coordinates of the Exchange asset that holds the canonical descriptor set. The descriptor JSON is fetched via the two-hop metadata → pre-signed-file flow (see [How it works](#how-it-works)).
- `exchange.credSecretRef`: name of a Flex secrets entry containing `clientId` / `clientSecret` (for OAuth2) or `username` / `password` (for basic auth).

### Optional fields

| Field                             | Default                          | Description                                                                 |
|-----------------------------------|----------------------------------|-----------------------------------------------------------------------------|
| `exchange.baseUrl`                | `https://anypoint.mulesoft.com`  | Exchange base URL (`format: service`, host-only). Override for EU/GovCloud, or the loopback listener with `exchangePathPrefix`. |
| `exchange.exchangePathPrefix`     | `""`                            | HOP 1 loopback path prefix (e.g. `/exchange-pin`) → `anypoint.mulesoft.com` (metadata + token), for managed gateways that mangle the egress `Host` (see below). Empty = direct call. |
| `exchange.exchangeFilePathPrefix` | `""`                            | HOP 2 loopback path prefix (e.g. `/exchange-s3`) → the pre-signed storage/S3 host that serves the descriptor file. Shares `baseUrl` with hop 1. Empty only on a connected gateway that can reach the storage host directly. |
| `exchange.authType`               | `oauth2_client_credentials`      | Auth method: `oauth2_client_credentials` or `basic`.                        |
| `exchange.credSecretRef` value    | —                                | Colon-joined pair: `clientId:clientSecret` (OAuth2) or `username:password` (basic). |
| `exchange.refreshIntervalSec`     | `300`                            | Pin set refresh interval (30–86400 seconds), request-path driven.           |
| `enforce.exactMatch`              | `true`                           | If `true`, any field change triggers drift (strictest).                     |
| `enforce.allowAddedTools`         | `false`                          | If `false`, runtime tools not in the pin set are considered drift.         |
| `enforce.allowRemovedTools`       | `true`                           | If `true`, pinned tools missing from runtime are logged but not blocked.   |
| `mode`                            | `enforce`                        | Decision mode: `enforce`, `warn`, or `observe`.                             |
| `failOpen.onPinUnavailable`       | `false`                          | If `false`, requests are denied when no pin set is loaded (fail closed).   |

## Managed gateway deployment (egress-`Host` workaround)

On managed Anypoint Omni / Flex Gateways, policy-originated (WASM) outbound calls bypass the route-level `auto_host_rewrite` and the gateway rewrites the egress `Host` to an internal cluster identifier, which the strict upstreams reject. Because the pin fetch is **two hops** to two different `Host`-strict hosts, the managed-gateway workaround needs **two** plain HTTP passthrough loopback routes (no policy) on the **same ingress gateway**, both on the internal `http://127.0.0.1:8081`:

| Loopback path | Upstream | Hop |
|---|---|---|
| `/exchange-pin` | `https://anypoint.mulesoft.com` | Hop 1: metadata + OAuth2 token |
| `/exchange-s3`  | `https://exchange2-asset-manager-kprod.s3.amazonaws.com` | Hop 2: pre-signed descriptor file |

Set `exchange.baseUrl = http://127.0.0.1:8081`, `exchange.exchangePathPrefix = /exchange-pin`, and `exchange.exchangeFilePathPrefix = /exchange-s3`. The policy then dispatches both hops through the loopbacks, whose `auto_host_rewrite` restores the correct `Host` for each upstream. The loopbacks **must be same-pod on the ingress gateway** — cross-gateway calls to an egress internal URL still get their `Host` mangled at the internal load balancer. Since these routes are publicly reachable, harden them with a restrictive policy. See `docs/managed-omni-gateway-setup.md` for the full recipe. On self-managed / connected Flex Gateways that honor `auto_host_rewrite` and can reach both hosts directly, leave both `exchangePathPrefix` and `exchangeFilePathPrefix` empty.

## Evidence

The policy emits structured log events prefixed `mcp-drift-exchange-evt`. Fields:

- `class`: `DescriptorDrift`, `UnpinnedTool`, `RemovedTool`, or `PinUnavailable`.
- `severity`: `Critical`, `Warning`, or `Info`.
- `decision`: `Stripped`, `Blocked`, `Annotated`, or `Allowed`.
- `asset_id`, `asset_version`: Exchange asset coordinates and version fetched.
- `tool_name`, `pin_hash`, `runtime_hash`: tool name and canonical hashes (omitted for `PinUnavailable`).
- `field`: which field drifted (`description_changed`, `input_schema_changed`, etc.).
- `note`: human-readable explanation (e.g. "no pin loaded; failing closed").

These events integrate with MuleSoft Flex observability (Anypoint Monitoring, SIEM export, custom dashboards).

## Failure modes

- **Pin unavailable on bootstrap** (Exchange unreachable, credentials invalid, asset not found): if `failOpen.onPinUnavailable = false` (default), all `tools/list` requests are denied until a successful fetch. If `true`, requests pass through unvalidated.
- **Pin unavailable after successful bootstrap**: LKG cache is retained indefinitely. The policy continues enforcing against the last successful pin set.
- **Version drift**: if `exchange.version = "latest"`, the policy emits a `version_changed` event whenever the underlying asset version changes between refreshes.

## Demo

The `policy-config.json` example shows a minimal working configuration. To test:

1. Publish the descriptor set to Exchange as a `custom` (or `mcp`) asset whose JSON file is served back through the metadata → pre-signed-file flow.
2. Create a Flex secrets entry named `exchange-creds` with `clientId` / `clientSecret` (or `username` / `password` for basic auth).
3. Apply the policy to an MCP API instance in API Manager.
4. Connect an MCP client (Claude Desktop, MCP Inspector, Postman) to the governed endpoint.
5. Observe: runtime `tools/list` responses are validated. Drifted tools are stripped (enforce mode), or logged (observe mode).

External test harness: [a2d.a2d-ai.com](https://a2d.a2d-ai.com) provides an MCP protocol tester (hosted tester + desktop builds) with scenarios for validating MCP policies end-to-end.

## Verifying enforcement

Send a `tools/list` to the governed endpoint. The response is SSE, so
request the event-stream `Accept` and strip the `data: ` prefix:

```bash
curl -sS -X POST "https://<gw-host>/<mcp-basepath>/" \
  -H 'Content-Type: application/json' \
  -H 'Accept: application/json, text/event-stream' \
  -d '{"jsonrpc":"2.0","method":"tools/list","id":1}'
```

In the live reference deployment the accounts MCP server advertises three
tools — `search_accounts`, `get_account_balance`, `lookup_account` — but
the live `lookup_account` carries an extra
`properties.accountId.description` that is absent from the pinned
descriptor. In `enforce` mode `lookup_account` is therefore stripped as
`DescriptorDrift(inputSchema)`:

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "result": {
    "tools": [
      { "name": "search_accounts", "description": "Search accounts by free-text query.", "inputSchema": { "...": "..." } },
      { "name": "get_account_balance", "description": "Return the current account balance in USD.", "inputSchema": { "...": "..." } }
    ]
  }
}
```

i.e. `tools = ["search_accounts","get_account_balance"]`, with a
`descriptor_drift` evidence line logged for `lookup_account`. Switch
`mode` to `observe` (or pin a descriptor that matches every live tool) to
pass all three through while the drift event is still emitted.

## Build and test

```bash
make setup                              # Install cargo-anypoint 1.9.0, fetch deps
env -u CARGO_TARGET_DIR make build      # Build WASM + definition/implementation YAML
env -u CARGO_TARGET_DIR cargo test --lib # Run unit tests
make run                                # Start local Flex in Docker Compose
env -u CARGO_TARGET_DIR make publish    # Publish dev version to Exchange
env -u CARGO_TARGET_DIR make release    # Publish release version
make upload-docs                        # Upload definition/home.md to Exchange
```

> Always prefix cargo/make with `env -u CARGO_TARGET_DIR`. If the target
> dir is overridden, `cargo build` writes the fresh wasm elsewhere and
> `make publish` ships a stale binary from `./target`.

## Publish to Exchange

After `make release`, the policy definition and implementation are published to the configured `group_id` / `definition_asset_id` / `implementation_asset_id`. The definition is reusable across environments; the implementation binary is fetched by Flex at policy-apply time.

```bash
make release
make upload-docs     # Optional: sync this home.md to the Exchange asset page
```

## FAQ

**Q: What if the Exchange asset version changes mid-flight?**  
A: If you use `version: "latest"`, the policy emits a `version_changed` event and re-pins to the new version on the next refresh. Pinning to a semver (e.g. `1.2.3`) avoids surprise changes.

**Q: How often is the pin set refreshed?**  
A: Every `refreshIntervalSec` seconds (default 300 = 5 minutes). The refresh is background — no request blocking.

**Q: What happens if Exchange is down during a refresh?**  
A: The policy logs the failure and retains the LKG pin set. Enforcement continues uninterrupted.

**Q: Can I allow new tools at runtime but block changed tools?**  
A: Yes: set `enforce.allowAddedTools = true` and `enforce.exactMatch = true`. Added tools pass; changed tools are stripped.

**Q: What if a tool's `annotations` field changes?**  
A: Annotations are included in the canonical hash. Any change triggers drift (if `enforce.exactMatch = true`).

**Q: Can I test the policy locally before deploying?**  
A: Yes: `make run` starts a local Flex instance in Docker Compose. Edit `playground/config/api.yaml` to configure upstreams and policy parameters, then connect an MCP client to `http://localhost:8081`.
