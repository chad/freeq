# Contributing to freeq

Thanks for your interest in contributing to freeq! This project treats IRC as
infrastructure — contributions should be clear, auditable, and avoid cleverness.

## Getting Started

```bash
git clone https://github.com/chad/freeq
cd freeq
cargo build
cargo test
```

The web client:
```bash
cd freeq-app
npm install
npm run dev
```

## Development Setup

- **Rust** (stable, 2024 edition)
- **Node.js** 20+ (for freeq-app)
- **SQLite** (bundled via rusqlite)

## How to Contribute

### Bug Reports

Open a GitHub issue with:
- What you expected
- What happened
- Steps to reproduce
- Server/client version and transport (TCP/WS/iroh)

### Feature Requests

Open a GitHub issue. Describe the use case, not just the solution. Features
that align with the project philosophy (decentralized identity, open protocol,
no lock-in) are most likely to be accepted.

### Pull Requests

1. Fork the repo and create a branch from `main`
2. Write clear commit messages
3. Add tests for new functionality
4. Run `cargo test` and `cargo clippy` before submitting
5. Update docs if you change behavior
6. Keep PRs focused — one feature or fix per PR

### Code Style

- Follow existing patterns in the codebase
- Use `tracing` for logging (not `println!` or `eprintln!`)
- Prefer explicit error handling over `.unwrap()` in library code
- Comment non-obvious logic, especially protocol behavior

### What We're Looking For

Check the [TODO list](CLAUDE.md) for current priorities. High-impact areas:

- S2S federation improvements
- Search (FTS5)
- Auto-reconnection
- Documentation improvements
- Test coverage

## Architecture

```
freeq-server/    Rust IRC server (async tokio, SQLite, iroh)
freeq-sdk/       Rust client SDK (connect, auth, events, E2EE)
freeq-app/       React web client (Vite + Tailwind)
freeq-tui/       Terminal client (ratatui)
freeq-bots/      Example bots using the SDK
freeq-auth-broker/ OAuth broker for AT Protocol
freeq-site/      Marketing site (freeq.at)
```

## License

By contributing, you agree that your contributions will be licensed under
the [MIT License](LICENSE).
