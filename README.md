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

## Live Demo

A reference deployment of this policy is running on the
`agent-network-ingress-gw` Flex Gateway in the Anypoint Sandbox
environment (org `anypoint-cbp-1780648272`).

| Field | Value |
|---|---|
| Gateway | `agent-network-ingress-gw` (id `35755bec-3177-4d32-a8c9-c9705f5b1c0b`, gw `1.13.2`) |
| Public base URL | `https://agent-network-ingress-gw-zovwbn.jeg62f.usa-e2.cloudhub.io` |
| Proxy path | `/mcp-drift-via-exchange-demo` |
| API instance | `20999090` (Exchange asset `drift-demo-a2d-mcp/1.0.0`) |
| Pin source (Exchange asset) | `82a0453b-22e6-430d-bbf4-35b989d043dc/drift-demo-a2d-mcp/1.0.0` |
| Upstream (a2d mock) | `https://www.a2d-ai.com/api/platform/7b26e0d0-dfcf-4c6a-8484-8c907724366d/mcp` |
| Policy version (dev) | `omni-policy-mcp-tool-drift-via-exchange-dev/0.1.0-20260629203732` |

The Exchange asset and the runtime upstream are derived from the same
A²D MCP server, so a healthy `tools/list` matches the pinned set
exactly. The contract lives in Exchange; the gateway enforces it.

### Try it

```bash
curl -sS -X POST \
  https://agent-network-ingress-gw-zovwbn.jeg62f.usa-e2.cloudhub.io/mcp-drift-via-exchange-demo \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"tools/list","id":1}'
```

To exercise drift, mutate the upstream A²D mock's tool descriptors
without re-publishing the Exchange asset. The runtime hash diverges
from the pinned Exchange hash, the policy strips the drifted tool, and
the gateway log line carries a `descriptor_drift` JSON record indexed
by Anypoint Analytics.

Note: the policy config currently uses placeholder secrets
(`REPLACE_WITH_EXCHANGE_CRED_SECRET_REF`). Swap in real Flex secret
refs containing the Connected App `clientId`/`clientSecret` for the
target Anypoint org and re-apply the policy before the Exchange fetch
will authenticate.

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
