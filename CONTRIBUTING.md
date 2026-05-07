# Contributing to Bastion

Thanks for your interest in contributing! Bastion follows Domain-Driven Design (DDD) and Clean Architecture principles.

## Getting Started

1. Fork the repository
2. Clone your fork
3. Create a feature branch: `git checkout -b feat/my-feature`
4. Make your changes
5. Run tests: `cargo test --workspace`
6. Run lints: `cargo clippy --workspace`
7. Commit with descriptive messages
8. Push and create a Pull Request

## Architecture Guidelines

- **Domain crate** (`bastion-domain`): Pure domain logic. No infrastructure dependencies. Defines traits.
- **Application crate** (`bastion-application`): Use cases. Orchestrates domain entities via ports.
- **Infrastructure crate** (`bastion-infrastructure`): Implements domain ports. External adapters.
- **Gateway crate** (`bastion-gateway`): Composition root. MCP server. Wires everything together.

## Code Style

- Follow Rust standard conventions
- Use `tracing` for logging, not `println!`
- Document public APIs with doc comments
- Keep domain types free of framework dependencies

## Commit Convention

- `feat: description` — new features
- `fix: description` — bug fixes
- `docs: description` — documentation
- `refactor: description` — code changes (no behavior change)
- `test: description` — test additions/changes

## PR Checklist

- [ ] `cargo check --workspace` passes
- [ ] `cargo clippy --workspace` has 0 warnings
- [ ] `cargo test --workspace` passes
- [ ] New code has appropriate tests
- [ ] Public APIs are documented
- [ ] `cargo check --target wasm32-wasip1 -p enrichment-engine` passes (if changing enrichment crates)
