# LSP support plan

## Goal
Use the existing local PR checkout as the source of truth for IDE-like language features in ReviewBuddy: hover details, parameter info, and jump-to-definition from code shown in Files, code tours, and other read-only code surfaces.

## Scope assumptions
- Keep the feature read-only. No editing, diagnostics fixing, or code actions in the first implementation.
- Support the PR surfaces first: changed-file diffs, code tour snippets/callsites, and a new read-only source browser for definition targets.
- Use standard language servers where available instead of custom parsers.
- Treat the local checkout as canonical for symbol lookup; GitHub API snapshots remain useful for diff metadata, but not for LSP queries.

## Minimal progress track
Update only the `Status` and `Notes` columns during future sessions.

| Workstream | Status | Notes |
| --- | --- | --- |
| 1. Checkout source-of-truth | done | Files and Tour now share PR-head checkout readiness. Linked checkouts must match the PR head and stay clean before local-code features use them. |
| 2. Local document service | done | Files now prefer checkout-backed document reads, syntax spans carry column offsets, prepared local documents keep source line numbers, and Tour callsites can preload checkout-backed source excerpts even for unchanged files. |
| 3. LSP session manager | done | The repo-scoped manager now syncs `didOpen`/`didChange` documents, converts positions to LSP coordinates, auto-acks server-side requests, exposes hover/signatureHelp/definition requests on top of capability detection, and can resolve app-managed Rust, TypeScript/JavaScript, Python/Pyright, Go/gopls, Kotlin, Java/JDTLS, and C#/Roslyn servers before falling back to PATH binaries. |
| 4. Hover and parameter UI | in progress | Files diff lines, Tour changeset previews, and Tour callsites now use token-aware LSP tooltips with cached hover docs and signature details, and hover hit-testing now tokenizes raw code text instead of relying on syntax-highlight spans. Markdown code fences and other remaining code surfaces still need the same interaction model. |
| 5. Definition navigation | pending | Definition requests are available in the manager, but the read-only source browser and jump flow are still ahead. |
| 6. Hardening and fallback UX | in progress | Files now surface blocked/missing LSP status text, managed installs cover every currently wired language server, Node-hosted servers use an app-managed Node runtime, Go uses an app-managed `gopls` install, concurrent managed installs are serialized so cleanup cannot race another download, and Settings now exposes per-server download/repair actions plus install-error visibility. Broader checkout drift, cancellation, eviction, and cross-surface fallback handling are still ahead. |

## Proposed implementation

### 1. Unify the checkout contract
- Promote local checkout preparation from a Tour-only concern into a shared service used by Files, Tour, and future LSP features.
- Store the resolved checkout root and the matched PR head OID in app state/detail state so code views can trust the path they are querying.
- Tighten linked-checkout behavior: today linked repos are validated but not synced to the PR head. Decide whether to:
  - require the linked checkout to already be at the PR head, or
  - create a separate app-managed worktree/detached checkout for interactive features.
- Keep the app-managed checkout path as the safest default for deterministic LSP results.

### 2. Build a local document service
- Add a read-only service that loads file contents from the local checkout, caches them per repo + commit + path, and invalidates them when the PR head changes.
- Keep using GitHub data for diff structure and review threads, but add mapping metadata so rendered code can resolve back to `(repo_root, path, line, column)`.
- Extend code rendering paths so interactive surfaces know which displayed spans correspond to which source coordinates.
- Reuse the existing syntax/highlighting pipeline where possible, but stop treating displayed code as anonymous `StyledText`.

### 3. Add a persistent LSP session manager
- Create a repo-scoped background service that:
  - detects the file language,
  - chooses the correct server binary/config,
  - starts or reuses one server per repo/language,
  - speaks JSON-RPC over stdio,
  - tracks `initialize`, `didOpen`, `didChange`, `didClose`, and request/response lifecycles.
- Expose a Rust API that UI code can call for:
  - `hover`
  - `signatureHelp`
  - `definition`
- Cache server capabilities and show explicit unsupported/missing-server states instead of failing silently.

### 4. Add token-aware hover interaction
- Replace plain code-block hit areas with token-aware regions or equivalent coordinate hit-testing.
- Add hover triggers for visible code in:
  - Files diff lines
  - code tour changesets
  - code tour callsite snippets
  - markdown code fences if we want the experience consistent everywhere code appears
- Show a lightweight read-only popover with symbol info, docs, and parameters/signatures when available.
- Start with hover on changed-file code before expanding to every code surface.

### 5. Add definition navigation
- Add a read-only source browser capable of opening arbitrary repository files, including unchanged files.
- Allow definition results to:
  - jump within the current changed file/diff when possible,
  - open the new source browser when the target file is unchanged or outside the visible diff.
- Preserve the existing diff/tour navigation model; definition navigation should complement it, not replace it.

### 6. Harden the feature
- Handle checkout drift, PR refreshes, missing binaries, unsupported languages, and cancelled hover requests.
- Decide how long servers stay warm and when to evict them.
- Add logging around server startup, request latency, and failure cases.
- Add tests around checkout resolution, document mapping, request routing, and UI fallback states.

## Risks and decisions to resolve
- **Linked checkout safety:** syncing a user-linked repo directly could disrupt their branch/worktree. A separate worktree is safer.
- **UI model gap:** current code rendering is optimized for display, not per-token interaction, so hover/goto requires new mapping infrastructure.
- **Definition target UX:** unchanged-file navigation needs a new read-only source browser, not just the current diff view.
- **Server availability:** this feature needs a strategy for missing or multiple language servers per machine.

## Learnings from Zed
- **Make the checkout/worktree a first-class dependency.** Zed builds `worktree_store` first, then `buffer_store`, then `lsp_store` (`crates/project/src/project.rs`). That ordering reinforces the right source of truth: LSP works against local worktrees and real buffers, not API snapshots.
- **Keep LSP state in a long-lived service.** Zed’s `LspStore::new_local(...)` depends on the worktree store, buffer store, toolchains, environment, language registry, filesystem, and manifest tree (`crates/project/src/lsp_store.rs`). The lesson is to build one persistent repo-scoped service, not ad hoc request helpers.
- **Separate protocol work from UI work.** Zed keeps hover docs in `crates/editor/src/hover_popover.rs`, signature help in `crates/editor/src/signature_help.rs`, and definition-link handling in `crates/editor/src/hover_links.rs`. We should mirror that split instead of mixing hover, goto-definition, and process management into one feature module.
- **Use an intermediate semantics/navigation layer.** Zed’s editor code talks to a provider (`provider.hover`, `provider.definitions`) or to `project.lsp_store()` rather than embedding LSP protocol knowledge directly in the view layer. We should add a ReviewBuddy-facing interface on top of the raw LSP client.
- **Treat hover info and definition navigation as different interactions.** Zed’s hover popover shows docs, while `hover_links.rs` handles token hit-testing, modifier-based highlighting, and click navigation. For us, the useful takeaway is that hover popovers should not also be the only definition-navigation mechanism.
- **Support multiple-capable servers and dedupe results.** Zed’s `definitions` and `hover` paths aggregate results from multiple local servers and deduplicate them (`request_multiple_lsp_locally(...)` in `crates/project/src/lsp_store.rs`). Even if we start simpler, our service boundary should not assume exactly one server forever.
- **Invest in interaction testing.** Zed has GPUI interaction tests around hovered links and modifier/click behavior in `hover_links.rs`. Once we add token-aware hover/click handling, we should test mouse movement, modifier keys, and navigation the same way.
- **Do not copy the whole architecture blindly.** Zed’s local/remote split is powerful but much broader than what ReviewBuddy needs today. The right thing to borrow first is the service boundary (`checkout/worktree -> buffer/document -> lsp -> UI`), not the full collaboration/remote stack.

## Acceptance criteria
- Hovering a symbol in supported visible code shows LSP hover details when a server is available.
- Hovering a call expression can show parameter/signature info when the server supports it.
- Go-to-definition works from changed-file code and can open definition targets in unchanged files.
- The app clearly explains when the checkout is stale, a server is missing, or a language is unsupported.

## SQL todo mapping
- `lsp-checkout-contract`
- `lsp-document-service`
- `lsp-session-manager`
- `lsp-hover-ui`
- `lsp-definition-navigation`
- `lsp-hardening`
