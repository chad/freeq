# Security Policy

## Reporting a Vulnerability

If you discover a security vulnerability in freeq, please report it responsibly.

**Do not open a public GitHub issue for security vulnerabilities.**

### How to Report

Email: **security@freeq.at**

Include:
- Description of the vulnerability
- Steps to reproduce
- Impact assessment (what an attacker could do)
- Any suggested fixes

### What to Expect

- **Acknowledgment** within 48 hours
- **Assessment** within 7 days
- **Fix or mitigation** as fast as possible, coordinated with you
- **Credit** in the release notes (unless you prefer anonymity)

### Scope

The following are in scope:

- freeq-server (IRC server, WebSocket, REST API, S2S federation)
- freeq-sdk (client SDK, authentication, E2EE)
- freeq-auth-broker (OAuth broker)
- freeq-app (web client — XSS, CSRF, etc.)
- Infrastructure at irc.freeq.at

The following are out of scope:

- AT Protocol / Bluesky PDS vulnerabilities (report to Bluesky)
- iroh transport vulnerabilities (report to n0.computer)
- Theoretical attacks requiring physical access

### Safe Harbor

We will not take legal action against researchers who:
- Make a good-faith effort to avoid privacy violations and data destruction
- Report vulnerabilities to us before public disclosure
- Give us reasonable time to fix issues before disclosing

## Security Hardening

For production deployment security guidance, see [docs/SECURITY.md](docs/SECURITY.md).
