# MCP Tool Drift Detection (Exchange)

Detects MCP tool drift by comparing runtime `tools/list` responses against canonical descriptors published to **Anypoint Exchange**. Built on PDK 1.9 (Rust → WebAssembly) for MuleSoft Flex Gateway / Omni Gateway.

After the policy is applied, every `tools/list` response is validated against the descriptor set stored in an Exchange MCP asset. Drifted tools can be stripped, annotated with warnings, or logged for observability — depending on the configured enforcement mode.

## Apply this policy to an MCP-typed API instance

Create the API instance in API Manager with **type = MCP**. The policy intercepts MCP JSON-RPC traffic on the response path; upstream MCP servers continue to serve their normal endpoints.

## How it works

1. **Bootstrap**: on policy start, the canonical descriptor set is fetched from Exchange via the configured `orgId / groupId / assetId / version` coordinates. The fetch authenticates using Connected App OAuth2 or basic auth.
2. **Refresh timer**: the pin set is periodically re-fetched (default 300s). If Exchange is unreachable, the last-known-good (LKG) cache is retained.
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

- `exchange.orgId`, `exchange.groupId`, `exchange.assetId`, `exchange.version`: coordinates of the Exchange asset that holds the canonical descriptor set (at `/mcp.json`).
- `exchange.credSecretRef`: name of a Flex secrets entry containing `clientId` / `clientSecret` (for OAuth2) or `username` / `password` (for basic auth).

### Optional fields

| Field                             | Default                          | Description                                                                 |
|-----------------------------------|----------------------------------|-----------------------------------------------------------------------------|
| `exchange.baseUrl`                | `https://anypoint.mulesoft.com`  | Exchange base URL (typically the default unless private cloud).             |
| `exchange.authType`               | `oauth2_client_credentials`      | Auth method: `oauth2_client_credentials` or `basic`.                        |
| `exchange.refreshIntervalSec`     | `300`                            | Pin set refresh interval (30–86400 seconds).                                |
| `enforce.exactMatch`              | `true`                           | If `true`, any field change triggers drift (strictest).                     |
| `enforce.allowAddedTools`         | `false`                          | If `false`, runtime tools not in the pin set are considered drift.         |
| `enforce.allowRemovedTools`       | `true`                           | If `true`, pinned tools missing from runtime are logged but not blocked.   |
| `mode`                            | `enforce`                        | Decision mode: `enforce`, `warn`, or `observe`.                             |
| `failOpen.onPinUnavailable`       | `false`                          | If `false`, requests are denied when no pin set is loaded (fail closed).   |

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

1. Publish an MCP descriptor set to Exchange at `/mcp.json` (the canonical endpoint the policy fetches).
2. Create a Flex secrets entry named `exchange-creds` with `clientId` / `clientSecret` (or `username` / `password` for basic auth).
3. Apply the policy to an MCP API instance in API Manager.
4. Connect an MCP client (Claude Desktop, MCP Inspector, Postman) to the governed endpoint.
5. Observe: runtime `tools/list` responses are validated. Drifted tools are stripped (enforce mode), or logged (observe mode).

External test harness: [a2d.a2d-ai.com](https://a2d.a2d-ai.com) provides an MCP protocol tester (hosted tester + desktop builds) with scenarios for validating MCP policies end-to-end.

## Build and test

```bash
make setup           # Install cargo-anypoint 1.9.0, fetch dependencies
make build           # Build WASM + definition/implementation YAML
make test            # Run unit + integration tests
make run             # Start local Flex in Docker Compose
make publish         # Publish dev version to Exchange
make release         # Publish release version
make upload-docs     # Upload definition/home.md to Exchange
```

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
