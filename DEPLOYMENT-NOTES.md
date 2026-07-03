# Deployment notes — Anypoint Omni Gateway

Gotchas that took an entire debugging session to find while bringing
this policy up on `agent-network-ingress-gw` (org
`82a0453b-22e6-430d-bbf4-35b989d043dc`, env `Sandbox`). Read before
deploying or recreating instances.

---

## Trailing slash is required on the proxy path

The Flex Gateway routes the proxy path **as exact prefix including a
trailing slash**. An instance configured with path `/foo` will only
answer at `/foo/`. Without the trailing slash the gateway returns
`HTTP 404` with `server: Anypoint Flex Gateway` and an empty body.

Always set:

- `endpointUri` → `https://<host>/<basepath>/`
- `--path` → `/<basepath>/`
- curl URL in demos → `<host>/<basepath>/`

## Do NOT set `routing[0].rules.path` for MCP-type instances

MCP-type instances use only `routing[].upstreams`; there is no
`label` or `rules.path`. Setting `routing[].rules.path` corrupts the
runtime route silently — every request returns 404. Basepath belongs
in `endpoint.proxyUri` / `--path`.

**Correct edit command:**

```bash
anypoint-cli-v4 api-mgr api edit <id> \
  -f \
  --path "/<basepath>/" \
  --endpointUri "https://<host>/<basepath>/" \
  --routing '[{"upstreams":[{"id":"<upstream-uuid>","weight":100}]}]'
```

After every edit, run `anypoint-cli-v4 api-mgr api redeploy <id>`.

## Exchange OAuth: `groupId` must be the root org id

`exchange.groupId` is the root org id, not any sub-BG id. Passing a BG
id makes the asset lookup 404, which surfaces as `pin_unavailable` at
runtime with no clue that the id was structurally wrong.

## `version: latest` defeats the point of drift detection

`exchange.version: latest` re-pins whenever Exchange publishes a new
version — that is, whenever any developer promotes an updated
descriptor. Drift detection needs a stable pin; use an explicit semver.

## Placeholder secrets do NOT block the proxy

Placeholder Exchange OAuth secrets don't block bootstrap — the policy
still attaches and `tools/list` still flows. It just can't fetch the
pin, so `pin_unavailable` fires and `failOpen.onPinUnavailable` decides
allow/block. Swap for real Anypoint Secret Manager refs before
enforcement is meaningful.

## SSE responses are now enforced

Older revisions passed SSE (`text/event-stream`) responses through
untouched. The current revision parses SSE frames, enforces
`tools/list` inside each frame's `data:` payload, and re-serializes
only when a frame was actually mutated. Round-trip is byte-perfect
otherwise.

## `api-mgr api list` may return empty even when instances exist

In Sandbox with API Manager v2, `list` intermittently returns zero rows
even when `describe <id>` succeeds. Fall back to `describe` per-id.

## ANSI color codes break JSON parsing

`anypoint-cli-v4 ... -o json` emits ANSI color escapes in some output
paths. Strip with `sed -e 's/\x1b\[[0-9;]*m//g'`.

## Gateway-runtime registration is invisible from CLI

`anypoint-cli-v4 runtime-mgr application list` does **not** show the
per-API-instance gateway applications. Use the Anypoint Console:

> Runtime Manager → Omni Gateway → `<gateway-name>` → Applications tab.

---

## Order of operations for a clean recreate

1. Create API instance via Anypoint Console UI (API Manager → Add API
   → From scratch → MCP type). UI sets up `routing` correctly.
2. Set Implementation URI to the upstream MCP server.
3. Set Consumer endpoint with trailing slash.
4. Pick the existing managed gateway target.
5. Apply this policy via `api-mgr policy apply <api-id> <policy-asset-id>
   --policyVersion <version> --configFile policy-config.yaml`.
6. Verify with the curl in `README.md` → "Live Demo" → "Try it".
