# Demo — MCP Tool Drift Detection (Exchange)

This walks through a full end-to-end demo of the policy: publish a
canonical descriptor set to Anypoint Exchange, attach the policy to an
MCP-type API instance, and prove that a healthy `tools/list` passes
through while a drifted/unpinned tool is stripped.

> [!IMPORTANT]
> **⚠️ MANDATORY on a managed Omni Gateway — Exchange + pre-signed S3 are `Host`-strict.**
>
> This policy calls Anypoint Exchange and follows its redirect to a pre-signed storage (S3) URL. Both are `Host`-strict: routing and the URL signature depend on the exact `Host` / TLS SNI — exactly like the multi-tenant edge platforms **Vercel, Railway, Render, Heroku, Cloudflare Pages/Workers, Fly.io, Netlify**. On a **managed** Omni Gateway (e.g. Anypoint CloudHub 2.0), policy-originated (WASM) outbound calls have their egress `Host` rewritten to an internal Envoy cluster name, so Exchange returns `404`/`403` and the S3 signature fails.
>
> **You MUST route BOTH hops through same-gateway loopback "pins":**
> 1. Set **`exchange.baseUrl`** = `http://127.0.0.1:8081` (the gateway's own internal listener).
> 2. Set **`exchange.exchangePathPrefix`** = `/exchange-pin` (hop 1: Exchange API) and **`exchange.exchangeFilePathPrefix`** = `/exchange-s3` (hop 2: the pre-signed S3 descriptor).
> 3. Create two plain passthrough routes (no policy) on the **same** gateway — `/exchange-pin` → `https://anypoint.mulesoft.com`, and `/exchange-s3` → the S3 asset host — each with **`auto_host_rewrite`** so the correct `Host` is restored on egress.
>
> Without the pins the policy **cannot reach Exchange/S3** on a managed gateway. Full recipe: [`docs/managed-omni-gateway-setup.md`](docs/managed-omni-gateway-setup.md). The same pin applies to any custom upstream you self-host on one of the edge platforms listed above.
>
> **Self-managed / connected Flex Gateway** (reaches both hosts directly and honors route `auto_host_rewrite`): leave both prefixes empty for direct calls.

The demo mirrors the deployment pattern proven for the sibling A²D
policy (see `DEPLOYMENT-NOTES.md`), including the managed-gateway
loopback workaround.

> The parent process publishes to Exchange with a connected-app client id
> (`<CLIENT_ID>`). Do **not** publish or deploy from
> this guide — the commands below are the reference recipe. Supply your own
> Exchange connected-app credentials; never commit real ones.

---

## What this demo proves (at a glance)

- **HAPPY** — every tool whose live descriptor matches the
  Exchange-pinned descriptor passes through untouched. Here
  `search_accounts` and `get_account_balance` match the pin exactly and
  are returned as-is.
- **FAILURE (enforce)** — a tool whose live `inputSchema` /
  `description` has drifted from the pin is stripped from the
  `tools/list` response. Here `lookup_account` drifts because the live
  server adds `properties.accountId.description` (`"Account UUID"`) that
  the pinned descriptor does not carry, so it is removed and a
  `descriptor_drift` evidence event is emitted.

The rest of this guide reproduces both paths end-to-end against the live
demo route.

---

## 0. Prerequisites

- `anypoint-cli-v4` logged in to the target org/environment.
- A Connected App (client id + secret) with Exchange **read** scope, or
  a basic-auth user, stored as a Flex Secrets entry whose value is a
  colon-joined pair: `clientId:clientSecret` (OAuth2) or
  `username:password` (basic).
- Rust `wasm32-wasip1` target + `cargo-anypoint@1.9.0` (`make setup`).

---

## 1. Build the policy

```bash
env -u CARGO_TARGET_DIR make build
env -u CARGO_TARGET_DIR cargo test --lib
# Sanity: the wasm you're about to publish must contain the new two-hop code
LC_ALL=C grep -a -c "exchangeFilePathPrefix" \
  target/wasm32-wasip1/release/omni_policy_mcp_tool_drift_via_exchange.wasm   # expect >=1
LC_ALL=C grep -a -c "exchangePathPrefix" \
  target/wasm32-wasip1/release/omni_policy_mcp_tool_drift_via_exchange.wasm   # expect >=1
```

---

## 2. How the pin is fetched — the two-hop Exchange flow

The policy does **not** fetch a single descriptor file from a fixed
path. Anypoint Exchange serves asset **metadata** from the control plane but
serves the asset's **files** from a pre-signed storage (S3) URL on a
different, `Host`-strict host. So the pin fetch is **two hops**, both
dispatched through the **same** gateway loopback service
(`baseUrl=http://127.0.0.1:8081`):

0. **Mint a token** (only when `authType=oauth2_client_credentials`):
   `POST {baseUrl}{exchangePathPrefix}/accounts/api/v2/oauth2/token`
   with `grant_type=client_credentials`. For `basic` auth the policy
   builds an `Authorization: Basic …` header instead — no token hop.

1. **HOP 1 — metadata.**
   `GET {baseUrl}{exchangePathPrefix}/exchange/api/v2/assets/{groupId}/{assetId}/{version}`
   with the bearer/basic `Authorization`. The response JSON has a
   `files` array; each file carries a `classifier`, a `packaging`, and a
   pre-signed `externalLink` (an S3 URL).

2. **Descriptor selection.** The policy picks the first file with
   `packaging == "json"` **and** classifier in
   `[mcp-metadata, custom, fat-mcp-metadata]`; failing that, the first
   `packaging == "json"` file. It takes that file's `externalLink`.

3. **HOP 2 — file.** The `externalLink` is a pre-signed S3 URL whose
   signature binds the `Host`. On a managed gateway the policy strips the
   scheme + authority and re-issues
   `GET {baseUrl}{exchangeFilePathPrefix}{s3_path}?{s3_query}` with **no**
   `Authorization` header. The `/exchange-s3` passthrough route's
   `auto_host_rewrite` restores the real S3 `Host` so the pre-signed
   signature validates.

4. **Parse.** The returned JSON is parsed into the pinned tool set. Any
   of these shapes is accepted: `{tools:[…]}`, `{version,tools}`,
   `{payload:{tools}}`, `{mcp:{tools}}`.

The descriptor JSON is the contract. Example shape:

```json
{
  "assetVersion": "1.0.0",
  "tools": [
    {
      "name": "lookup_account",
      "description": "Look up an account by id. Returns the account record.",
      "inputSchema": {
        "type": "object",
        "properties": { "accountId": { "type": "string" } },
        "required": ["accountId"],
        "additionalProperties": false,
        "$schema": "http://json-schema.org/draft-07/schema#"
      }
    }
  ]
}
```

If no `version`/`assetVersion` field is present, the configured
`exchange.version` is used as the pin's `asset_version`.

> The canonical hash covers only `name` / `description` / `inputSchema`
> / `outputSchema` / `annotations` (**not** `execution`), and object
> keys are sorted before hashing — so key order and any `execution`
> block do not affect matching.

### Publish the pinned descriptor as an Exchange asset (parent performs this)

The descriptor JSON is uploaded as a `custom` Exchange asset; its file is
served back through the metadata → S3 flow above. This is the exact
recipe used for the live demo (the accounts-server pin):

```bash
cat > /tmp/pin_accounts.json <<'JSON'
{ "assetVersion":"1.0.0", "tools":[
  {"name":"lookup_account","description":"Look up an account by id. Returns the account record.","inputSchema":{"type":"object","properties":{"accountId":{"type":"string"}},"required":["accountId"],"additionalProperties":false,"$schema":"http://json-schema.org/draft-07/schema#"}},
  {"name":"search_accounts","description":"Search accounts by free-text query.","inputSchema":{"type":"object","properties":{"query":{"type":"string"}},"required":["query"],"additionalProperties":false,"$schema":"http://json-schema.org/draft-07/schema#"}},
  {"name":"get_account_balance","description":"Return the current account balance in USD.","inputSchema":{"type":"object","properties":{"accountId":{"type":"string"}},"required":["accountId"],"additionalProperties":false,"$schema":"http://json-schema.org/draft-07/schema#"}}
]}
JSON
anypoint-cli-v4 exchange asset upload mcp-pin-accounts/1.0.0 \
  --name mcp-pin-accounts --type custom \
  --properties '{"mainFile":"custom.json"}' \
  --files '{"custom.json":"/tmp/pin_accounts.json"}'
```

The `custom.json` file lands as a `packaging=="json"` file with the
`custom` classifier — exactly what HOP 1's selection step looks for.

---

## 3. Create the two gateway loopback routes

Because the fetch is two hops to two different `Host`-strict hosts, a
managed Omni / Flex Gateway needs **two** plain passthrough routes (no
policy) on the **same ingress gateway**, both reachable on the internal
listener `http://127.0.0.1:8081`:

| Loopback path | Upstream | Used for |
|---|---|---|
| `/exchange-pin` | `https://anypoint.mulesoft.com` | HOP 1 metadata + OAuth2 token |
| `/exchange-s3`  | `https://exchange2-asset-manager-kprod.s3.amazonaws.com` | HOP 2 pre-signed descriptor file |

Each route's `auto_host_rewrite` launders the mangled egress `Host` back
to the real upstream so the control plane (hop 1) and the pre-signed
signature (hop 2) both validate. See
[`docs/managed-omni-gateway-setup.md`](docs/managed-omni-gateway-setup.md)
for the full create-and-deploy recipe.

> These loopbacks **must be same-pod on the ingress gateway.** An earlier
> attempt to host them on the egress gateway failed: cross-gateway calls
> to the egress internal URL still get their `Host` mangled at the
> internal load balancer. Because they are publicly reachable, harden
> them with a restrictive policy.

### Verified DEMO config (enforce mode, applied to the live instance)

```json
{
  "exchange": {
    "orgId": "82a0453b-22e6-430d-bbf4-35b989d043dc",
    "groupId": "82a0453b-22e6-430d-bbf4-35b989d043dc",
    "assetId": "mcp-pin-accounts",
    "version": "1.0.0",
    "baseUrl": "http://127.0.0.1:8081",
    "exchangePathPrefix": "/exchange-pin",
    "exchangeFilePathPrefix": "/exchange-s3",
    "authType": "oauth2_client_credentials",
    "credSecretRef": "<CLIENT_ID>:<CLIENT_SECRET>",
    "refreshIntervalSec": 300
  },
  "enforce": { "exactMatch": true, "allowAddedTools": false, "allowRemovedTools": true },
  "mode": "enforce",
  "failOpen": { "onPinUnavailable": true }
}
```

On a **connected** gateway that can reach both hosts directly with the
correct `Host`, set `baseUrl=https://anypoint.mulesoft.com` and leave
both `exchangePathPrefix` and `exchangeFilePathPrefix` empty.

---

## 4. Attach the policy to an MCP API instance

Create an **MCP-type** API instance in API Manager whose upstream is the
MCP server under test, then apply this policy (parent performs the
publish + apply). Remember the trailing-slash rule from
`DEPLOYMENT-NOTES.md`: the proxy path must end in `/`.

---

## 5. Exercise it — the observed FAILURE path (the headline demo)

Send a `tools/list` to the live demo route. The response is **SSE**, so
ask for the event-stream `Accept` and strip the `data: ` prefix:

```bash
curl -sS -X POST \
  https://agent-network-ingress-gw-zovwbn.jeg62f.usa-e2.cloudhub.io/mcp-drift-via-exchange-demo/ \
  -H 'Content-Type: application/json' \
  -H 'Accept: application/json, text/event-stream' \
  -d '{"jsonrpc":"2.0","method":"tools/list","id":1}'
```

The live upstream (accounts MCP server) advertises **three** tools:
`search_accounts`, `get_account_balance`, `lookup_account`. But the live
`lookup_account` carries an extra
`properties.accountId.description: "Account UUID"` that is **not** in the
pinned Exchange descriptor.

**Observed result in `enforce` mode** — `lookup_account` is stripped:

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

i.e. `tools = ["search_accounts","get_account_balance"]`.
`lookup_account` is removed because its `inputSchema` drifted from the
pin (the added field description) → `DescriptorDrift(inputSchema)`. This
is genuine input-schema drift detection between the Exchange-pinned
contract and the live server.

A JSON evidence line appears in the gateway logs:

```
mcp-drift-exchange-evt {"class":"descriptor_drift","severity":"critical","decision":"stripped","asset_id":"mcp-pin-accounts","asset_version":"1.0.0","tool_name":"lookup_account","pin_hash":"…","runtime_hash":"…","field":"input_schema_changed"}
```

---

## 6. The HAPPY path (clean passthrough)

There are two ways to show all three tools passing through:

**A. `mode: observe` (report-only).** Set `"mode": "observe"` and re-run
the same request. Nothing is stripped — all three tools (including
`lookup_account`) are returned — while the same drift event is still
emitted for `lookup_account`:

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "result": {
    "tools": [
      { "name": "search_accounts", "description": "Search accounts by free-text query.", "inputSchema": { "...": "..." } },
      { "name": "get_account_balance", "description": "Return the current account balance in USD.", "inputSchema": { "...": "..." } },
      { "name": "lookup_account", "description": "Look up an account by id. Returns the account record.", "inputSchema": { "...": "..." } }
    ]
  }
}
```

**B. Pin a matching descriptor.** Re-upload the `mcp-pin-accounts`
descriptor so `lookup_account`'s `inputSchema` includes the same
`properties.accountId.description: "Account UUID"` the live server
serves. With the pin matching every live tool exactly, `enforce` mode
passes all three through and emits no drift event.

---

## 7. Confirm the pin actually loaded

Look for the one-shot load line in the gateway logs:

```
mcp-drift-exchange: pin loaded (first_load=true asset_version=1.0.0 tools=3)
```

If instead you see `mcp-drift-exchange: pin fetch failed: …`, the most
likely cause on a managed gateway is the egress-`Host` mangling on one of
the two hops — confirm **both** the `/exchange-pin` (metadata) and
`/exchange-s3` (pre-signed file) loopback routes exist and that
`exchangePathPrefix` / `exchangeFilePathPrefix` are set (step 3). With
`failOpen.onPinUnavailable=true` the proxy still serves traffic
(unstripped) while the pin is missing.

---

## Troubleshooting quick table

| Symptom | Likely cause | Fix |
|---|---|---|
| `pin fetch failed: … HTTP 404 DEPLOYMENT_NOT_FOUND` / host errors on hop 1 | Managed gateway mangles egress `Host` | Add `/exchange-pin` loopback + set `exchangePathPrefix` |
| `pin fetch failed: …` on hop 2 / S3 `SignatureDoesNotMatch` / 403 | Pre-signed file host `Host` mangled or missing S3 loopback | Add `/exchange-s3` loopback + set `exchangeFilePathPrefix` |
| `pin fetch failed: … token: …` / 401 | Bad/rotated `credSecretRef` or wrong `authType` | Fix secret (colon-joined pair) / auth type |
| Drift not stripped but logged | `mode` is `observe`/`warn` | Set `mode: enforce` |
| Nothing happens, 404 on curl | Missing trailing slash on proxy path | See `DEPLOYMENT-NOTES.md` |
| New code not taking effect after publish | `CARGO_TARGET_DIR` override → stale wasm | `env -u CARGO_TARGET_DIR make build/publish` |
