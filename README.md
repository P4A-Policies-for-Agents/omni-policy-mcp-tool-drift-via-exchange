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

## Purpose & business need

### The problem

When an MCP server is exposed to an LLM agent, the `tools/list`
response is the contract that determines agent behavior. That
contract is also a moving target:

- A platform team redeploys an MCP server with a "minor" description
  edit. The agent's behavior changes because the LLM reads
  descriptions when picking tools.
- An input schema gains a field. The agent populates it because LLMs
  trust schemas they see.
- An output type silently flips from `string` to `object`. Downstream
  consumers break, but the gateway's metrics still show `200 OK`.

Most organizations don't have any runtime check that the descriptors
served by an MCP server match the descriptors they actually approved
and published.

### Why this policy

Anypoint Exchange already is the governance surface for API and MCP
assets in Mulesoft customers — it owns versioning, approvals, and
audit trail. This policy makes the Exchange artifact load-bearing at
runtime:

- **One source of truth** — the descriptor set in the Exchange asset
  is the contract. The policy hashes each tool descriptor against the
  pin and reports field-level drift.
- **Tenant-sovereign** — every fetch, decision, and evidence event
  stays inside the customer's Anypoint tenant. No external SaaS, no
  cross-tenant data movement, no SOC2/HIPAA scope creep.
- **No external request-path dependency** — the pin is fetched on a
  refresh timer and cached. The hot path is local hash compare;
  request latency overhead is ~2–5 ms even on 50-tool descriptor
  sets.
- **Reuses existing governance** — approving a pin update means
  bumping the Exchange asset version through the customer's existing
  approval workflow. No separate truth source to drift out of sync.

### Who needs this

- Anypoint customers who already publish MCP assets to Exchange and
  want runtime enforcement without an external dependency.
- Regulated environments (FSI, healthcare, public sector) where every
  decision must stay inside the tenant.
- Operators who want a single PR review for "did we approve this
  descriptor change?" — by reviewing the Exchange asset version bump.

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

### Real-world scenario

A healthcare ISV runs an "Accounts" MCP server inside its Anypoint
tenant for an internal copilot that helps care coordinators schedule
follow-ups. The current Exchange asset `accounts-mcp/2.4.0` declares
three tools: `lookup_account`, `search_accounts`, `get_account_balance`.
Each was reviewed by privacy and approved on the Exchange asset PR.

A new sprint adds a `get_account_history` tool — but a backend
engineer ships the MCP server change before bumping the Exchange asset
version. The agent has no idea the new tool exists in the asset
catalog because it doesn't — only in the running container.

Compliance impact: a tool that returns historical transactions is now
callable by the LLM, and there's no privacy review on file.

With this policy attached and the Exchange pin at `2.4.0`:

1. The runtime `tools/list` returns four tools; the pin contains
   three.
2. `get_account_history` triggers an `unpinned_tool` event.
3. In `enforce` mode the new tool is stripped before the agent sees
   it. The copilot continues working with the three approved tools.
4. The `pin_hash` of each remaining tool matches the runtime hash —
   no false positives.
5. The event lands in Anypoint Analytics with `asset_id`,
   `asset_version`, `tool_name`. The on-call platform engineer pages
   the backend team to either bump the Exchange asset (with privacy
   approval) or roll back the deploy.

Throughout this, no data left the tenant. The contract was the
Exchange asset version that compliance had already approved.

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
