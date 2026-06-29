# MCP Tool Drift Detection (Exchange)

A Mulesoft Flex / Omni Gateway custom policy that detects MCP **tool
drift** — runtime `tools/list` responses that have diverged from the
descriptor set published to an Anypoint Exchange MCP asset — and
(optionally) strips the drifted tools from the response before it
reaches the LLM client.

The Exchange asset is the contract. The gateway is the enforcement
point. Decisions are local; evidence flows into the customer's
existing Anypoint analytics pipeline.

---

## What it catches

- **Descriptor drift** — runtime descriptor hash ≠ published hash.
  Reported as `description_changed`, `input_schema_changed`,
  `output_schema_changed`, or `annotation_changed`.
- **Unpinned tools** — present at runtime, absent from the Exchange
  asset.
- **Removed tools** — present in the asset, absent at runtime.
- **Version changes** — when `exchange.version=latest`, a change in
  the underlying version between refreshes emits `version_changed`.

---

## Modes

- `enforce` — strip drifted tools from `tools/list` responses.
- `warn` — pass through with `x-mcp-drift-warning` header.
- `observe` — emit structured evidence only.

---

## Configuration

| Path | Type | Default | Notes |
|---|---|---|---|
| `exchange.orgId` | string | required | Anypoint business group. |
| `exchange.groupId` | string | required | Exchange group id. |
| `exchange.assetId` | string | required | Exchange asset id. |
| `exchange.version` | string | required | Pinned semver or `latest`. |
| `exchange.baseUrl` | string | `https://anypoint.mulesoft.com` | Override for sovereign / EU. |
| `exchange.authType` | enum | `oauth2_client_credentials` | or `basic`. |
| `exchange.credSecretRef` | string | required | Flex secret holding the credentials. |
| `exchange.refreshIntervalSec` | int 30–86400 | 300 | Refresh cadence. |
| `enforce.exactMatch` | bool | `true` | Strict hash equality. |
| `enforce.allowAddedTools` | bool | `false` | Unpinned tools blocked by default. |
| `enforce.allowRemovedTools` | bool | `true` | Deprecation allowed. |
| `mode` | enum | `enforce` | `enforce` / `observe` / `warn`. |
| `failOpen.onPinUnavailable` | bool | `false` | Allow traffic on bootstrap when LKG is empty. |

---

## Evidence

Every decision lands as a JSON log line through the PDK logger.

```json
{
  "class": "descriptor_drift",
  "severity": "critical",
  "decision": "stripped",
  "asset_id": "demo-mcp-asset",
  "asset_version": "1.4.2",
  "tool_name": "get_user",
  "pin_hash": "ab12…",
  "runtime_hash": "cd34…",
  "field": "description_changed"
}
```

`class` ∈ `descriptor_drift | unpinned_tool | removed_tool |
version_changed | pin_stale | pin_unavailable`.

---

## Failure modes

- **Exchange unreachable on bootstrap.** `failOpen.onPinUnavailable`
  controls allow/block. Default is closed (block + `pin_unavailable`
  evidence).
- **Exchange unreachable after bootstrap.** Last-known-good cache is
  retained; `pin_stale` evidence fires at every failed refresh.
- **Asset version moved.** When `version=latest`, the underlying
  resolution change emits `version_changed`. Pin a semver to opt out.

---

## FAQ

**Why an Exchange-only variant?** Some customers run entirely on
Anypoint and want a single source of truth they already understand —
the Exchange asset. No external dependencies, no remote calls on
the request path, no extra signing keys to rotate.

**What if I want real-time policy decisions instead of a local cache?**
That is the role of the platform-managed drift-detection variant
(separate policy).

---

## Build, test, run

```bash
make setup
make build
make test
make run
make publish
make release
```

`make build` runs `cargo anypoint config-gen` against
`definition/gcl.yaml`, which overwrites `src/generated/config.rs`.

---

## License

Copyright 2026 Salesforce, Inc. All rights reserved.
