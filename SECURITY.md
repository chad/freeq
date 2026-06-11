# Security Policy

freeq is an open-source, federated chat platform combining IRC with AT Protocol
identity. It is MIT-licensed, maintained by Chad Fowler, and operates a public
instance at `irc.freeq.at`. We take security reports seriously and appreciate
the work of researchers who report issues responsibly.

## Supported Versions

freeq is pre-1.0. There are no long-term support branches. Security fixes land
on `main` and in the most recent beta tag. Reports are accepted against:

| Version              | Supported |
| -------------------- | --------- |
| Latest `main`        | Yes       |
| Most recent beta tag | Yes       |
| Anything older       | No        |

If you find an issue in an older revision, please confirm it still reproduces
on current `main` before reporting.

## Reporting a Vulnerability

Email **security@freeq.at**. A PGP key for encrypted reports will be published;
until then, avoid putting working exploits for critical issues in plaintext
email — send a description first and we will arrange a secure channel.

Please do **not** open public GitHub issues for security vulnerabilities.

### What to include

- Affected component (e.g. `freeq-server`, `freeq-sdk`, `freeq-auth-broker`)
  and commit hash or tag
- A clear description of the vulnerability and its impact
- Step-by-step reproduction instructions (proof-of-concept code welcome)
- Any relevant configuration (federation peers, enabled features, client used)
- Whether the issue is exploitable against the public instance at `irc.freeq.at`
- How you would like to be credited, if at all

## Response Expectations

- **Acknowledgement:** within 48 hours of receipt
- **Triage and initial assessment:** within 7 days
- We will keep you informed while we work on a fix and coordinate the
  disclosure timeline with you. Credit is given in release notes unless you
  prefer anonymity.

## Coordinated Disclosure

We ask for a standard **90-day** disclosure window from the date of report.
If a fix ships sooner, you are welcome to publish once the fix is released and
deployed to `irc.freeq.at`. If an unusually complex issue needs more than 90
days, we will say so explicitly and explain why.

## Scope

In scope:

- `freeq-server` — the IRC + AT Protocol server, including the REST API and
  WebSocket transport
- `freeq-sdk` (Rust) and `freeq-sdk-js` (TypeScript)
- `freeq-auth-broker` — the OAuth/session broker
- Official clients: web (`freeq-app`), TUI, iOS, Android, macOS, Windows

Areas of particular interest:

- **Federation (S2S):** authorization bypass, state-sync spoofing, protocol
  injection across server boundaries
- **SASL `ATPROTO-CHALLENGE` authentication:** challenge replay, signature
  verification flaws, DID/identity confusion, nick/account binding bugs
- E2EE key exchange and message signing
- OAuth scope handling and step-up flows in the auth broker

Out of scope:

- Volumetric denial of service (flooding, bandwidth exhaustion). Logic-level
  DoS — e.g. a single message that crashes the server — **is** in scope.
- Social engineering of maintainers, operators, or users
- Vulnerabilities in third-party dependencies (AT Protocol PDS, iroh, etc.)
  without a demonstrated freeq-specific exploit path — report those upstream;
  we track advisories via Dependabot

## Safe Harbor

We will not pursue legal action against researchers who:

- Act in good faith and within the scope above
- Make a reasonable effort to avoid privacy violations, data destruction, and
  service degradation (test against your own instance where practical;
  freeq is easy to self-host)
- Do not access, modify, or retain data belonging to others beyond what is
  necessary to demonstrate the issue
- Give us a reasonable opportunity to fix the issue before public disclosure

Good-faith research conducted under this policy is considered authorized, and
we will not report it to law enforcement.

## Prior Audits

A comprehensive internal security audit was completed in March 2026, covering
the server, SDKs, web client, broker, federation, and authentication paths.
See [AUDIT-REPORT.md](AUDIT-REPORT.md) for findings and resolutions.
