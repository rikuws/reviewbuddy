# Remiss

Remiss is a native Rust/GPUI desktop app for read-only pull request review. It combines GitHub pull request metadata, local checkouts, semantic diff navigation, LSP-backed source context, and AI-generated code tours without trying to become a general editor.

## Status

Remiss is an early alpha. The core workflow is usable for local development, but packaging, onboarding, and provider disclosure are still being hardened.

## Requirements

- macOS is the primary development target today.
- Rust toolchain from `rust-toolchain.toml`.
- Git.
- GitHub CLI (`gh`) authenticated with `gh auth login`.

## Development

```sh
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo run
```

## Data Model

Remiss uses GitHub CLI for live pull request data and caches snapshots locally. Local code intelligence prefers a checked-out repository at the pull request head, and committed file reads are cached by exact Git blob object. Worktree reads are not cached.

Large GitHub collections are paged until complete where possible. If GitHub reports more queue or pull request data than Remiss can load, the app records and displays an explicit completeness warning.

## AI Providers

Code tours may send pull request metadata, changed file lists, review comments, snippets, and a local checkout path to the selected provider. Codex tours run with a read-only sandbox and no network access. Copilot tours are constrained to read/search/glob tools.

## Managed Language Servers

Remiss can install managed language servers for supported languages. These installs may download toolchains or packages from upstream registries, so release builds should expose explicit consent, versions, and uninstall controls before broad distribution.

## Roadmap

See `PLAN.md` for the product direction and review-IDE constraints.
