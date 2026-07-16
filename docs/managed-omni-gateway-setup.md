# Deploying on a managed Omni / Flex Gateway (CloudHub 2.0)

This guide covers deploying the **MCP Tool Drift Detection (Exchange)**
policy on a **managed** Anypoint Flex / Omni Gateway (CloudHub 2.0), and
the one non-obvious piece of infrastructure it needs there: the
**Exchange pin-fetch loopback routes** — note the plural.

The same `Host`-mangling failure — and the same loopback-pin fix — applies to **any** `Host`-routed multi-tenant edge platform: Vercel, Railway, Render, Heroku, Cloudflare Pages/Workers, Fly.io, Netlify.

If you run a self-managed (connected) Flex Gateway whose pod can reach
both `anypoint.mulesoft.com` **and** the Exchange pre-signed storage host
directly with the correct `Host`, you do **not** need the loopbacks — set
`exchange.baseUrl` to `https://anypoint.mulesoft.com`, leave both
`exchange.exchangePathPrefix` and `exchange.exchangeFilePathPrefix`
empty, and skip to [Configuration](#configuration).

---

## Why two loopbacks are required on a managed gateway

The pin fetch is a **two-hop** flow, and the two hops target two
different `Host`-strict hosts:

1. **HOP 1 — metadata (and, for OAuth2, the token).**
   `GET {baseUrl}{exchangePathPrefix}/exchange/api/v2/assets/{groupId}/{assetId}/{version}`
   (and `POST {baseUrl}{exchangePathPrefix}/accounts/api/v2/oauth2/token`)
   against `anypoint.mulesoft.com`. The response's `files` array carries,
   for each file, a `classifier`, a `packaging`, and a pre-signed
   `externalLink` — an S3 URL. The policy selects the first
   `packaging == "json"` file whose classifier is one of
   `mcp-metadata` / `custom` / `fat-mcp-metadata` (else the first
   `packaging == "json"` file) and takes its `externalLink`.

2. **HOP 2 — the descriptor file.**
   `GET {baseUrl}{exchangeFilePathPrefix}{s3_path}?{s3_query}` against the
   pre-signed storage host
   (`exchange2-asset-manager-kprod.s3.amazonaws.com`), with **no**
   `Authorization` header — the pre-signed query string *is* the
   credential, and it binds the `Host`.

On a **managed** Omni gateway, the runtime rewrites the outbound `Host` /
`:authority` of any policy-originated (WASM) call to an internal Envoy
cluster identifier. `exchange.baseUrl` is declared `format: service`, so
cargo-anypoint binds a `Service` handle — but on a managed gateway that
handle is dispatched to a synthetic cluster whose authority is mangled.
The control plane (hop 1) routes strictly by `Host` and rejects it; the
pre-signed S3 URL (hop 2) fails its signature check because the signed
`Host` no longer matches. Both hops therefore need a loopback.

### The loopback pattern

Call a **plain passthrough route on the same gateway** whose upstream is
the real host. That route is normal proxied traffic, so its
`auto_host_rewrite` sets the correct `Host`. The policy reaches it via
the gateway's **internal listener** (`http://127.0.0.1:8081`), which
bypasses the CloudHub load balancer that enforces `Host` at the public
edge. Envoy routes by **path**, so each loopback's base path is matched
regardless of the mangled `Host`.

| Loopback base path | Upstream | Serves |
|---|---|---|
| `/exchange-pin` | `https://anypoint.mulesoft.com` | Hop 1: asset metadata + OAuth2 token |
| `/exchange-s3`  | `https://exchange2-asset-manager-kprod.s3.amazonaws.com` | Hop 2: the pre-signed descriptor file |

```
        policy (WASM)                         two loopback routes                real upstreams
 ┌──────────────────────────────┐  http://127.0.0.1:8081/exchange-pin/...  ┌────────────────────────┐
 │ baseUrl=127.0.0.1:8081         │ ───────────────────────────────────────▶ │ /exchange-pin/ → auto_ │→ anypoint.mulesoft.com
 │ exchangePathPrefix=/exchange-  │  Host mangled here, but Envoy routes      │ host_rewrite           │   (metadata + token)
 │ pin                            │  by PATH not Host                         ├────────────────────────┤
 │ exchangeFilePathPrefix=        │  http://127.0.0.1:8081/exchange-s3/...    │ /exchange-s3/ → auto_  │→ exchange2-asset-manager
 │ /exchange-s3                   │ ───────────────────────────────────────▶ │ host_rewrite           │   -kprod.s3.amazonaws.com
 └──────────────────────────────┘                                           └────────────────────────┘   (pre-signed file)
```

### Both loopbacks must live on the ingress gateway (same pod)

An earlier attempt to move these routes to the **egress** gateway
**failed**: cross-gateway calls to the egress internal URL still get
their `Host` mangled at the internal load balancer, exactly the failure
the loopback is meant to avoid. The loopbacks **must be same-pod on the
ingress gateway** the policy runs on.

Because both routes are publicly reachable (they proxy straight to
Anypoint and S3), **harden them with a restrictive policy** (IP allow
list, client-id enforcement, or similar) so they cannot be abused as an
open proxy.

---

## Step 1 — Create the two loopback passthrough routes

Each loopback is an ordinary HTTP proxy API on the **same** managed
ingress gateway, carrying **no** enforcement policy.

MCP-type Exchange assets reject `--type http`, so first publish throwaway
`http-api` assets to back them:

```bash
anypoint-cli-v4 exchange asset upload exchange-pin-loopback-api/1.0.0 \
  --name exchange-pin-loopback-api --type http-api \
  --properties '{"apiVersion":"v1"}'

anypoint-cli-v4 exchange asset upload exchange-s3-loopback-api/1.0.0 \
  --name exchange-s3-loopback-api --type http-api \
  --properties '{"apiVersion":"v1"}'
```

Create the instances. On managed CloudHub 2.0 the **proxy** scheme must
be HTTP on port `8081` (TLS terminates at the LB) even though the public
endpoint is HTTPS:

```bash
GW_HOST="agent-network-ingress-gw-<suffix>.<region>.cloudhub.io"

# Hop 1 loopback -> anypoint.mulesoft.com
anypoint-cli-v4 api-mgr api manage exchange-pin-loopback-api 1.0.0 \
  --environment "Sandbox" \
  --isFlex -p \
  --type http \
  --uri "https://anypoint.mulesoft.com" \
  --scheme http --port 8081 \
  --path "/exchange-pin/" \
  --endpointUri "https://$GW_HOST/exchange-pin/" \
  --apiInstanceLabel "exchange-pin-loopback" \
  --deploymentType hybrid

# Hop 2 loopback -> pre-signed S3 storage host
anypoint-cli-v4 api-mgr api manage exchange-s3-loopback-api 1.0.0 \
  --environment "Sandbox" \
  --isFlex -p \
  --type http \
  --uri "https://exchange2-asset-manager-kprod.s3.amazonaws.com" \
  --scheme http --port 8081 \
  --path "/exchange-s3/" \
  --endpointUri "https://$GW_HOST/exchange-s3/" \
  --apiInstanceLabel "exchange-s3-loopback" \
  --deploymentType hybrid
```

Deploy both to the managed gateway target:

```bash
anypoint-cli-v4 api-mgr api deploy <pin-instance-id> \
  --environment "Sandbox" --target <gateway-target-id> \
  --gatewayVersion "1.13.2"

anypoint-cli-v4 api-mgr api deploy <s3-instance-id> \
  --environment "Sandbox" --target <gateway-target-id> \
  --gatewayVersion "1.13.2"
```

**Verify** hop 1 reaches Exchange (Flex strips the `/exchange-pin/` base
path, so `…/exchange-pin/exchange/api/v2/assets/<g>/<a>/<v>` proxies to
`anypoint.mulesoft.com/exchange/api/v2/assets/...`). A 401/403 here
(rather than 404) already proves routing works — auth is applied by the
policy, not the loopback:

```bash
curl -s -o /dev/null -w "%{http_code}\n" \
  "https://$GW_HOST/exchange-pin/exchange/api/v2/assets/<groupId>/<assetId>/<version>"
# expect: 200 (public asset) or 401/403 (auth required) — NOT 404
```

The `/exchange-s3` route is exercised only with a live pre-signed URL, so
it is easiest to validate end-to-end via a real pin load (Step 3).

---

## Step 2 — Configure the policy for two-hop loopback mode

Set `exchange.baseUrl` to the internal listener, `exchangePathPrefix` to
the hop-1 route, and `exchangeFilePathPrefix` to the hop-2 route. When
these prefixes are non-empty the policy runs in **loopback mode**: it
dispatches straight to `baseUrl` (skipping upstream-cluster discovery)
and prefixes each hop's request path accordingly.

```json
{
  "exchange": {
    "baseUrl": "http://127.0.0.1:8081",
    "exchangePathPrefix": "/exchange-pin",
    "exchangeFilePathPrefix": "/exchange-s3",
    "orgId": "<root-org-id>",
    "groupId": "<root-org-id>",
    "assetId": "<mcp-pin-asset-id>",
    "version": "1.0.0",
    "authType": "oauth2_client_credentials",
    "credSecretRef": "<secret-ref-resolving-to clientId:clientSecret>",
    "refreshIntervalSec": 300
  },
  "enforce": { "exactMatch": true, "allowAddedTools": false, "allowRemovedTools": true },
  "mode": "enforce",
  "failOpen": { "onPinUnavailable": true }
}
```

Apply it:

```bash
anypoint-cli-v4 api-mgr policy apply <mcp-api-instance-id> \
  omni-policy-mcp-tool-drift-via-exchange \
  --environment "Sandbox" \
  --groupId <org-id> \
  --policyVersion <version> \
  --configFile policy-config.json

anypoint-cli-v4 api-mgr api redeploy <mcp-api-instance-id> --environment "Sandbox"
```

---

## Step 3 — Verify the pin loads

After redeploy + a warmup request or two (the pin loads lazily on live
traffic), send a `tools/list`. The response is SSE, so ask for the
event-stream `Accept`:

```bash
curl -s -X POST "https://$GW_HOST/<mcp-basepath>/" \
  -H 'Content-Type: application/json' \
  -H 'Accept: application/json, text/event-stream' \
  -d '{"jsonrpc":"2.0","method":"tools/list","id":1}'
```

In `enforce` mode, any tool whose runtime descriptor has drifted from the
pin is stripped. In the accounts reference deployment, `lookup_account`
is removed (its `inputSchema` gained a field description not present in
the pin) and only `search_accounts` + `get_account_balance` come back.

Look for the successful load line in Runtime Manager logs:

```
mcp-drift-exchange: pin loaded (first_load=true asset_version=1.0.0 tools=3)
```

If tools pass through unchanged in `enforce` mode, the pin did **not**
load and the policy failed open (`failOpen.onPinUnavailable=true`). Check
the logs for `mcp-drift-exchange: pin fetch failed`. A failure on hop 1
points at the `/exchange-pin` route or credentials; a failure on hop 2
(e.g. an S3 `SignatureDoesNotMatch` / 403) points at the `/exchange-s3`
route or a missing `exchangeFilePathPrefix`.

---

## Configuration

All standard fields are documented in the [README](../README.md). The
loopback-specific and service-binding fields:

| Path | Type | Default | Description |
|---|---|---|---|
| `exchange.baseUrl` | string (`format: service`) | `https://anypoint.mulesoft.com` | Control-plane URL; cargo-anypoint binds an outbound `Service` from it. Point at `http://127.0.0.1:8081` for loopback mode. Shared by both hops. |
| `exchange.exchangePathPrefix` | string | `""` | HOP 1 loopback prefix (e.g. `/exchange-pin`) → `anypoint.mulesoft.com`. When set, enables loopback mode for the metadata + token hop. Leave empty on gateways that don't mangle the egress `Host`. |
| `exchange.exchangeFilePathPrefix` | string | `""` | HOP 2 loopback prefix (e.g. `/exchange-s3`) → the pre-signed storage/S3 host. Required whenever the file host's `Host` is mangled on a managed gateway. Leave empty only on a connected gateway that can reach the storage host directly. |

---

## Operational notes (managed gateway)

### The `CARGO_TARGET_DIR` build trap (read before publishing)

If a "new" policy version behaves exactly like the old one no matter how
many times you redeploy, you are probably publishing a **stale wasm**.
Some sandboxes override `CARGO_TARGET_DIR`, so `cargo build` writes the
fresh wasm to a cache dir while `make publish` uploads the old binary in
`./target`. Always build + publish with a consistent target dir and grep
the binary for a marker only the new two-hop code has **before**
publishing:

```bash
env -u CARGO_TARGET_DIR make publish
LC_ALL=C grep -a -c "exchangeFilePathPrefix" \
  target/wasm32-wasip1/release/omni_policy_mcp_tool_drift_via_exchange.wasm   # 0 == stale
```

### `groupId` must be the root org id

`exchange.groupId` is the **root org** id, not a nested business-group
id. A BG id 404s the asset lookup on hop 1 and surfaces as
`pin_unavailable` with no hint that the id was structurally wrong. See
`DEPLOYMENT-NOTES.md`.

See [`DEPLOYMENT-NOTES.md`](../DEPLOYMENT-NOTES.md) for the full set of
managed-gateway gotchas (trailing-slash routing, MCP `routing.path`
corruption, ANSI-in-JSON, etc.).
