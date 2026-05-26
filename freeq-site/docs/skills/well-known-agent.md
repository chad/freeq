---
name: well-known-agent
description: Probe an A2A-style agent discovery endpoint at /.well-known/agent.json — fetch, validate shape, smoke-test advertised capabilities, and report findings
allowed-tools: Bash Read
---

# Well-Known Agent Probe

Inspect a remote agent server's `.well-known/agent.json` discovery document, then smoke-test each advertised capability. Works against freeq-shape agents (`service` / `assistance_endpoint` / flat `capabilities`) and Google A2A-shape AgentCards (`name` / `url` / `skills`). Other shapes get a plain dump.

## Inputs

- `BASE_URL` — server origin to probe. Defaults to `https://irc.freeq.at` if no argument was given.
- `--no-smoke` — fetch + validate only, skip capability calls.
- `--verbose` — include full request/response bodies in the report.

## Steps

### Step 0: Resolve target

If the user passed a bare hostname (`irc.freeq.at`), prepend `https://`. Strip any trailing slash. Reject anything that doesn't parse as `http(s)://...`.

### Step 1: Fetch the discovery document

```
curl -sS -m 10 -w '\n---HTTP %{http_code} CT %{content_type}---\n' \
  "$BASE_URL/.well-known/agent.json"
```

Bail with a clear error if:
- HTTP status is not 200 — surface status, body excerpt, and `Content-Type`.
- Body isn't JSON — show first 200 chars, suggest the server doesn't expose A2A discovery.
- TLS / DNS / timeout — print the curl exit code and stderr.

### Step 2: Detect shape

Run the body through `jq` and pick a shape:

| Shape       | Trigger keys                                  |
|-------------|-----------------------------------------------|
| `freeq`     | `service` + `assistance_endpoint` + `capabilities` array of strings |
| `a2a`       | `name` + `url` + (`skills` array OR `capabilities` object)          |
| `unknown`   | anything else                                 |

If both could match, prefer `freeq` (it's a proper subset for our codebase).

### Step 3: Validate required fields

**freeq shape** — required: `service`, `version`, `description`, `assistance_endpoint`, `capabilities` (array of strings), `auth.required` (bool), `auth.methods` (array). Flag any missing.

**a2a shape** — required: `name`, `url`, `version`, `capabilities` (object). Recommended: `description`, `defaultInputModes`, `defaultOutputModes`, `skills`.

Note any extra fields too — they're informational, not errors.

### Step 4: Smoke-test capabilities

Skip if `--no-smoke` was passed.

**freeq:** for each capability in the `capabilities` array, build the URL `<BASE_URL><assistance_endpoint>/<capability>` and POST an empty JSON object:

```
curl -sS -m 10 -w '\n---HTTP %{http_code}---\n' \
  -X POST -H 'Content-Type: application/json' \
  -d '{}' \
  "$BASE_URL$assistance_endpoint/$capability"
```

Score each call:
- `200` + body has `ok`, `request_id`, `diagnosis.code` → ✓ healthy
- `4xx` with structured error → ⚠ rejects empty body (still indicates the endpoint exists)
- `5xx` or non-JSON or missing `request_id` → ✗ broken
- `404` → ✗ advertised but not routable

For `free_form_session` (or any capability whose name suggests an LLM/streaming session), POST `{"messages":[{"role":"user","content":"ping"}]}` instead of `{}`.

**a2a:** for each entry in `skills[]`, just print `name`, `description`, `tags`, and `examples` — there's no standardized smoke endpoint per skill in the A2A AgentCard spec, so don't fabricate one.

If `auth.required` is `true` (freeq) or any skill declares auth, note that smoke tests will likely 401; don't treat that as a probe failure.

### Step 5: Report

One concise block per section:

```
Target: <BASE_URL>
Shape:  <freeq|a2a|unknown>
Service: <service or name> v<version>
Description: <one-line>
Auth: <required | optional> via <methods>

Capabilities (N):
  ✓ validate_client_config       — diagnosis: CONFIG_OK
  ✓ diagnose_message_ordering    — diagnosis: NEED_INPUT
  ⚠ free_form_session            — 400, body: {"error":"messages required"}
  ✗ diagnose_sync                — 500, body: "internal error"

Schema issues: none | list of missing/unexpected fields
```

In `--verbose`, follow the summary with the full discovery JSON and each smoke-test request/response. Otherwise keep it under 30 lines.

### Step 6: Suggest follow-ups

If everything is ✓ and the target was the freeq production server (`irc.freeq.at`), offer to update `docs/agent-assist-test-session.md` with the fresh transcript. If anything failed, offer to file the failure under `docs/agent-assist-failures/<date>-<host>.md` for later triage. Don't write either without confirmation.

## Notes

- `curl` and `jq` are required — the skill assumes both are on PATH (they ship with macOS / standard Linux).
- This is a probe, not a load test — one request per capability, 10s timeout each.
- The skill never sends auth credentials. If the target requires auth, expect 401s and report them as "auth-gated" rather than failures.
