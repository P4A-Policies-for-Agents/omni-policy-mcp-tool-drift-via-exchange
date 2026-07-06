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
- **No external SaaS dependency** — the pin is fetched from Anypoint
  Exchange lazily on the request/response path, cached in-process, and
  refreshed after `refreshIntervalSec`. The hot path is a local hash
  compare; request latency overhead is ~2–5 ms even on 50-tool
  descriptor sets.
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

## How it works

This is an **inbound MCP policy**. It pins the canonical tool
descriptor set from an Anypoint Exchange asset (fetched via the two
hops documented below — metadata → pre-signed S3) and compares every
runtime JSON-RPC `tools/list` tool against that pin.

The comparison is an **exact hash** over each descriptor's `name`,
`description`, `inputSchema`, `outputSchema`, and `annotations`. From
that compare the policy classifies each runtime tool as:

- **DRIFT** — present in both the pin and the response, but the
  canonical hash differs.
- **UNPINNED** — present at runtime, absent from the pinned set.
- **REMOVED** — present in the pin, absent at runtime.

The `mode` setting decides what happens next:

- `enforce` — strip the drifted tools from the response.
- `warn` — pass them through with an `x-mcp-drift-warning` response
  header.
- `observe` — emit structured evidence only.

This policy is **drift-only**: it detects divergence from the pinned
contract and nothing more. It runs **no** prompt-injection heuristics
and **no** near-name shadowing detection — those belong to the
security-superset sibling described below.

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

## How this differs from `poisoning-detection-exchange`

`poisoning-detection-exchange` and this policy are **not** the same
thing, even though they share plumbing. Both pin from the **same**
Exchange source via the same two-hop fetch (metadata → pre-signed S3),
both detect descriptor drift, and both support `enforce` / `warn` /
`observe` on top of a last-known-good cache.

The difference is scope:

- **`poisoning-detection-exchange` is the security superset.** On top
  of drift detection it *additionally* runs **prompt-injection
  heuristics** over descriptor text and **near-name shadowing**
  detection (tools whose names impersonate a trusted tool).
- **This policy is drift-only.** It stops at contract-divergence
  detection — no injection heuristics, no shadowing.

It also relates to the two A²D siblings. `drift-via-exchange` (this
policy) is the **Exchange-sourced equivalent of `drift-via-a2d`'s cache
mode**: it pins from a versioned Exchange asset instead of the A²D
platform's `/mcp/spec` endpoint, and it drops the remote-PDP decision
axis entirely (every decision is local). If your single source of truth
is Exchange rather than the A²D platform, this is the drift policy to
use.

---

## Real-world use case

An enterprise whose API governance home is Anypoint Exchange publishes
each MCP server's tool contract as a versioned Exchange asset. The
gateway pins those descriptors and strips any production tool that has
drifted from the published contract. The result is drift enforcement
for teams whose single source of truth is Exchange — with no dependency
on the A²D platform. Approving a contract change is the same act as
bumping the Exchange asset version through the existing review workflow.

---

## Configuration

| Path | Type | Default | Notes |
|---|---|---|---|
| `exchange.orgId` | string | required | Anypoint business group. |
| `exchange.groupId` | string | required | Exchange group id. |
| `exchange.assetId` | string | required | Exchange asset id. |
| `exchange.version` | string | required | Pinned semver or `latest`. |
| `exchange.baseUrl` | string (`format: service`) | `https://anypoint.mulesoft.com` | Registered as an Envoy upstream cluster — host-only, no path. Override for EU (`https://eu1.anypoint.mulesoft.com`) / GovCloud, or point at the gateway loopback listener when using `exchangePathPrefix`. |
| `exchange.exchangePathPrefix` | string | `""` | Loopback path prefix for **HOP 1** (metadata + token), e.g. `/exchange-pin`. When set, the pin fetch is dispatched to `baseUrl` verbatim and prefixed onto every Exchange request path — the managed-gateway egress-`Host` workaround (see below). Empty = direct call to `anypoint.mulesoft.com`. |
| `exchange.exchangeFilePathPrefix` | string | `""` | Loopback path prefix for **HOP 2** (the pre-signed storage/S3 descriptor file), e.g. `/exchange-s3`. The descriptor file is served from a `Host`-strict S3 URL, so on a managed gateway the file GET is re-issued to `baseUrl` under this prefix (no `Authorization`) and the passthrough route's `auto_host_rewrite` restores the S3 `Host`. Shares `baseUrl` with hop 1. Leave empty only on a connected gateway that can reach the storage host directly. |
| `exchange.authType` | enum | `oauth2_client_credentials` | or `basic`. |
| `exchange.credSecretRef` | string | required | Flex secret holding the credentials. Resolved value is a colon-joined pair: `clientId:clientSecret` (OAuth2) or `username:password` (basic). |
| `exchange.refreshIntervalSec` | int 30–86400 | 300 | Refresh cadence (lazy, request-path driven). |
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

Evidence emission is debounced per-instance: at most one row per
`(tool_name, detection_class)` per 60 s window, bounded by a
1024-entry LRU. Enforcement decisions are NOT gated — a sustained drift
storm still triggers per-request stripping, but only one evidence row
surfaces per window.

---

## Transport

The policy handles both MCP Streamable HTTP transports:

- **`application/json`** — plain JSON-RPC envelope in the response body.
- **`text/event-stream`** — one or more SSE frames whose `data:` line
  is a JSON-RPC envelope. Un-mutated frames round-trip byte-perfectly.

`content-length` is stripped on the response headers before the body
handler mutates the payload.

---

## Authentication

The Exchange fetch authenticates with one of two schemes, selected by
`exchange.authType`:

- **`oauth2_client_credentials`** (default) — the policy POSTs
  `grant_type=client_credentials` to
  `{baseUrl}{exchangePathPrefix}/accounts/api/v2/oauth2/token`
  (Connected App), sending the client id/secret both in the form body and
  as an HTTP Basic header, then uses the returned `access_token` as
  `Authorization: Bearer <token>` on the metadata GET.
- **`basic`** — the policy builds `Authorization: Basic base64(user:pass)`
  directly for the metadata GET (no token round-trip).

`exchange.credSecretRef` points at a Flex Secrets entry whose resolved
value is a **colon-joined pair**: `clientId:clientSecret` for OAuth2, or
`username:password` for basic. If the value contains no colon, the whole
string is treated as the first component (id/username) and the second is
empty.

### The two-hop descriptor fetch

The descriptor is **not** fetched from a single fixed file path. Anypoint Exchange
serves asset metadata from the control plane but serves the asset's files
from a pre-signed storage (S3) URL on a different `Host`-strict host, so
the fetch is two hops (both dispatched through the same `baseUrl`):

1. **HOP 1 — metadata.**
   `GET {baseUrl}{exchangePathPrefix}/exchange/api/v2/assets/{groupId}/{assetId}/{version}`
   with the bearer/basic `Authorization`. The response's `files` array
   lists each file's `classifier`, `packaging`, and a pre-signed
   `externalLink` (an S3 URL). The policy selects the first file with
   `packaging == "json"` and classifier in
   `[mcp-metadata, custom, fat-mcp-metadata]` (else the first
   `packaging == "json"` file) and takes its `externalLink`.
2. **HOP 2 — file.** The `externalLink` is a pre-signed S3 URL whose
   signature binds the `Host`. The policy strips the scheme + authority
   and re-issues
   `GET {baseUrl}{exchangeFilePathPrefix}{s3_path}?{s3_query}` with **no**
   `Authorization` header; the `/exchange-s3` route's `auto_host_rewrite`
   restores the real S3 `Host` so the pre-signed signature validates.

The returned JSON is parsed into the pinned tool set, accepting
`{tools:[…]}`, `{version,tools}`, `{payload:{tools}}`, or
`{mcp:{tools}}`.

---

## Managed gateway deployment (egress-`Host` workaround)

On managed Anypoint Omni / Flex Gateways, policy-originated (WASM)
outbound calls are dispatched straight to an Envoy cluster and do **not**
traverse the route-level `auto_host_rewrite` that the main proxy relies
on. The gateway rewrites the egress `:authority`/`Host` to an internal
cluster identifier, so a direct call to `anypoint.mulesoft.com` is
rejected (the control plane routes strictly by `Host`). This is the same
class of failure documented at length in `DEPLOYMENT-NOTES.md` for the
sibling A²D policy.

Because the pin fetch is **two hops** to two different `Host`-strict
hosts (`anypoint.mulesoft.com` for metadata + token, and the pre-signed
S3 host for the file), the workaround is **two gateway loopback routes**
plus `exchangePathPrefix` / `exchangeFilePathPrefix`:

| Loopback path | Upstream | Hop |
|---|---|---|
| `/exchange-pin` | `https://anypoint.mulesoft.com` | Hop 1: metadata + OAuth2 token |
| `/exchange-s3`  | `https://exchange2-asset-manager-kprod.s3.amazonaws.com` | Hop 2: pre-signed descriptor file |

1. Create two plain HTTP passthrough API instances (no policy) on the
   **same ingress gateway**, both listening on the internal
   `http://127.0.0.1:8081`. Flex strips the base path and each route's
   `auto_host_rewrite` restores the correct `Host` for its upstream.
2. Configure this policy with:

   ```json
   {
     "exchange": {
       "baseUrl": "http://127.0.0.1:8081",
       "exchangePathPrefix": "/exchange-pin",
       "exchangeFilePathPrefix": "/exchange-s3",
       "orgId": "…", "groupId": "…", "assetId": "…", "version": "1.0.0",
       "authType": "oauth2_client_credentials",
       "credSecretRef": "exchange-creds"
     }
   }
   ```

When the prefixes are set the policy skips upstream-cluster discovery and
dispatches both hops to `baseUrl` (the loopback listener) directly. The
`wasm → 127.0.0.1:8081` call hits Envoy directly (bypassing the
`Host`-mangling load balancer), and each loopback route launders the
`Host` back to its real upstream.

> The loopbacks **must be same-pod on the ingress gateway.** An earlier
> attempt to host them on the egress gateway failed — cross-gateway calls
> to the egress internal URL still get their `Host` mangled at the
> internal load balancer. Since these routes are publicly reachable,
> harden them with a restrictive policy. See
> [`docs/managed-omni-gateway-setup.md`](docs/managed-omni-gateway-setup.md)
> for the full recipe.

On self-managed / connected Flex Gateways that honor `auto_host_rewrite`
on user clusters and can reach both hosts directly, leave both
`exchangePathPrefix` and `exchangeFilePathPrefix` empty for a direct
call.

### Build trap — always unset `CARGO_TARGET_DIR`

If `CARGO_TARGET_DIR` is overridden (e.g. by a sandbox cache), `cargo
build` writes the fresh wasm elsewhere while `make publish` reads the
stale `./target/.../*.wasm`, so you silently ship an old binary. Always
prefix build/publish commands:

```bash
env -u CARGO_TARGET_DIR make build
env -u CARGO_TARGET_DIR make publish
# verify the binary you're about to ship contains the new two-hop code:
LC_ALL=C grep -a -c "exchangeFilePathPrefix" \
  target/wasm32-wasip1/release/omni_policy_mcp_tool_drift_via_exchange.wasm  # 0 == stale
LC_ALL=C grep -a -c "exchangePathPrefix" \
  target/wasm32-wasip1/release/omni_policy_mcp_tool_drift_via_exchange.wasm  # 0 == stale
```

---

## Spec caching & refresh

The pinned descriptor set lives in an **in-memory, per-gateway-replica
cache**. It is not shared across replicas and not persisted to disk —
each replica loads and maintains its own copy.

Refresh is **lazy / request-driven**, not a background timer:

- On the first request with no pin loaded, the policy fetches the
  descriptor set inline.
- On subsequent requests it refetches once the cached pin's age reaches
  `exchange.refreshIntervalSec` (default 300 s, min 30, max 86400).
- Warming happens in the **request-headers** phase when the upstream
  cluster is already known, otherwise in the **response** phase.

On a **failed** refetch the **last-known-good** descriptor set is
retained, so enforcement survives an Exchange outage.
`failOpen.onPinUnavailable` only applies at **bootstrap** — when no
descriptor set has ever loaded. Once any pin has loaded, a later fetch
failure never downgrades the policy to fail-open; it keeps enforcing the
last-known-good set.

OAuth2 tokens are minted and refreshed as needed for the **metadata
hop**; the pre-signed **S3 file hop** carries no `Authorization`.

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
env -u CARGO_TARGET_DIR make build
env -u CARGO_TARGET_DIR cargo test --lib
make run
env -u CARGO_TARGET_DIR make publish
env -u CARGO_TARGET_DIR make release
```

`make build` runs `cargo anypoint config-gen` against
`definition/gcl.yaml`, which overwrites `src/generated/config.rs`, then
compiles the wasm and generates the definition/implementation YAML.

Always prefix cargo/make with `env -u CARGO_TARGET_DIR` (see the build
trap above). Unit tests are the source of truth; run them with
`cargo test --lib`.

---

## License

Copyright 2026 Salesforce, Inc. All rights reserved.
