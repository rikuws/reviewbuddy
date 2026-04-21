# PLAN

## Product Direction

Build a native desktop IDE for code review, not code editing.

The product should optimize for the part of review that costs the most time:

- understanding what changed
- navigating to the right context quickly
- judging impact and risk
- keeping orientation across a review session

This means the center of the product is not an editor, chat box, or browser-style PR page. It is a read-only review workspace built around semantic diffs, impact navigation, and structured context.

## Non-Goals

- No code editing, staging, or merge/conflict workflows
- No voice controls
- No generic chat-first review flow
- No vector-RAG-first architecture for core code understanding
- No attempt to support every language equally in v1

## Product Thesis

The research direction is clear:

- Program comprehension and navigation dominate engineering time.
- Review quality materially affects defect rates.
- Reviewers struggle most with finding the important parts of a change and following impact into unfamiliar code.
- Structural context outperforms raw line diffs and noisy search when the task is review.

So the product should feel like IntelliJ for reviewing change impact:

- diff-first
- symbol-aware
- graph-backed
- spatially stable
- read-only

## What We Already Have

The current app is a strong starting point for this pivot. The key pieces already exist:

- A native PR workspace shell in `src/views/pr_detail.rs`
- A changed-files surface in `src/views/diff_view.rs`
- A guided-review surface in `src/views/tour_view.rs`
- Local checkout and document plumbing in `src/local_repo.rs` and `src/local_documents.rs`
- Read-only LSP support in `src/lsp.rs`
- Token-aware code display in `src/code_display.rs`
- Existing review/feedback summaries in `src/views/pr_detail.rs`

We should pivot these into a tighter review IDE instead of rebuilding from scratch.

## Target Workspace

The target UI should converge on a 3-pane review workspace:

- Left pane: review queue, symbol/file outline, pinned waymarks, saved route
- Center pane: semantic diff and read-only source browser
- Right pane: impact map, symbol context, docs, ownership, review status

Supporting UI:

- Compact PR header that shrinks once the reviewer is inside the review flow
- Fast jump actions for definitions, callers, callees, usages, threads, and tests
- Stable layout and saved position so the review session feels navigable, not ephemeral

## Core Product Pillars

### 1. Semantic diff by default

Replace the raw line-diff mental model with a symbol-aware one.

Deliverables:

- AST-based diff pipeline for supported languages
- Whitespace and formatting noise hidden by default
- Explicit labels for rename, move, extract, inline, and likely-refactor changes
- Symbol-grouped diff sections for functions, methods, types, and fields
- Fallback to line diff for unsupported languages or oversized files

### 2. Graph-based impact navigation

Treat changed code as an entry point into a local symbol graph.

Deliverables:

- Symbol graph for changed entities and nearby impacted entities
- Side-panel impact map with changed vs impacted nodes
- Jump to definition, callers, callees, type references, and hierarchy
- Read-only source browser for unchanged but impacted code
- Multi-commit symbol history for stacked changes later

### 3. Review wayfinding

Make orientation and re-finding core product features.

Deliverables:

- Stable file and hunk ordering across refreshes and sessions
- Pins, bookmarks, named waymarks, and "come back later" markers
- Persistent fold state and last-read position
- Review history and back/forward navigation
- A route view that feels like walking a review path, not scanning a flat patch

### 4. Prioritized review order

Help reviewers start where scrutiny matters most.

Deliverables:

- Review queue scored by complexity, churn, ownership, bug history, and comment history
- Symbol or hunk-level risk markers when possible
- Suggested review order with "start here" guidance
- Trivial bucket for likely pure refactors or renames

### 5. Inline context

Pull the surrounding context into the review workspace instead of forcing tab-switching.

Deliverables:

- Inline docs, ADRs, tickets, and API references for the selected symbol/file
- Ownership and team metadata
- Review state summary in the side panel
- Thread digest and "feedback on my PR" summary with one-click navigation

### 6. AI grounded in structure

Use AI as an overlay on the graph-backed review model, not as the primary navigation primitive.

Deliverables:

- Impact summaries grounded in the diff plus symbol graph
- Natural-language queries over structural context
- Suggested review routes and likely hotspot explanations
- Answers tied to concrete symbols and files, not vague repo-wide retrieval

### 7. Behavior-aware review later

Only after the navigation core is strong.

Deliverables:

- Changed tests linked to relevant symbols and hunks
- Failing-test context near the affected code
- Optional behavior or trace diff for critical paths

## Roadmap

### Phase 1: Reframe the existing app into a review IDE

Goal: turn the current PR viewer into a more intentional review workspace without changing the backend model yet.

Scope:

- Keep `Files`, `Tour`, and PR detail, but reshape them around one review flow
- Make the header compact while reviewing
- Move reviewer state, approvals, change requests, and open feedback into a persistent side area
- Stop showing internal storage paths in normal UI
- Improve LSP hover layout so markdown and long responses fit cleanly
- Reposition the current Tour feature as a review route rather than a separate AI novelty

Exit criteria:

- The app already feels like a purpose-built review workspace before semantic diff lands
- Reviewers can see status, route, and open feedback without hunting through the page

### Phase 2: Ship semantic diff as the primary review surface

Goal: make structural review the default experience.

Scope:

- Introduce AST-backed diff classification for the first supported languages
- Add symbol-level grouping inside the diff surface
- Label move/rename/refactor changes explicitly
- Add low-noise fallback behavior for unsupported files

Implementation notes:

- Start with the repo's most important language set first, not broad coverage
- Reuse local checkout data as the source of truth
- Keep raw line diff available as a fallback or compare mode

Exit criteria:

- The reviewer sees structural change units first, not raw hunk fragments

### Phase 3: Add the graph-backed navigation model

Goal: let the reviewer follow impact outside the changed lines.

Scope:

- Build a repo index for symbols and relationships from the local checkout
- Add changed/impacted graph view beside the diff
- Support definition and caller navigation into unchanged code
- Introduce a read-only source browser for definition targets and impacted neighbors

Implementation notes:

- The symbol graph becomes the primary context engine for navigation and later AI
- Existing LSP support remains useful, but it should sit beside a repo-owned index, not define the whole architecture

Exit criteria:

- A changed symbol can be explored through callers, callees, and related types without leaving the review workspace

### Phase 4: Make review sessions spatial and resumable

Goal: make long or unfamiliar reviews tractable.

Scope:

- Save review position, folds, pinned waymarks, and route state per PR
- Add history navigation and quick-return to pinned hotspots
- Preserve ordering so reloading the PR does not destroy the user's mental map

Exit criteria:

- A reviewer can stop and resume later without reconstructing the review path from scratch

### Phase 5: Add prioritization and queueing

Goal: guide reviewer attention toward likely hotspots.

Scope:

- Score changed files and symbols by risk
- Add a queue view with "start here", "needs scrutiny", and "likely trivial" sections
- Use current review data plus repo history to improve ordering over time

Exit criteria:

- The product actively helps the reviewer choose where to spend time first

### Phase 6: Layer in context and AI

Goal: reduce context switching and make assistance structurally trustworthy.

Scope:

- Right-hand context panel for docs, ownership, tickets, and related threads
- Graph-grounded impact summaries
- Natural-language structural queries such as "show all usages of this changed enum" or "where do we check permissions for this endpoint?"

Exit criteria:

- The reviewer gets useful assistance without leaving the review surface or relying on generic repo search

### Phase 7: Add behavior-aware review for critical changes

Goal: connect code changes to observed behavior where it matters.

Scope:

- Attach test results to symbols and hunks
- Highlight changed tests and execution paths
- Explore richer trace diff only after basic behavior linking proves useful

Exit criteria:

- For risky logic changes, the reviewer can inspect both structural and behavioral impact in one workspace

## Architecture Decisions

These should be treated as fixed direction unless we learn something stronger:

- Local checkout is the source of truth for review intelligence.
- The diff is the organizing unit of the workspace.
- The symbol/call/dependency graph is the primary retrieval layer for navigation and AI.
- LSP is a supporting capability for read-only symbol intelligence, not the whole product strategy.
- Every major feature must work in a read-only model.
- Extra context should be suppressible; noisy context is worse than no context.

## Proposed Module Direction

Expected codebase direction from the current structure:

- Keep `src/views/diff_view.rs` as the center review canvas, but evolve it from file tree + line diff into review queue + semantic diff
- Keep `src/views/tour_view.rs`, but turn it into the route/wayfinding layer
- Keep `src/lsp.rs` for hover/definition/signature features where it helps
- Add a repo intelligence layer for AST parsing, symbol indexing, and graph queries
- Add session-state support for pins, saved paths, and stable review ordering
- Add a context-panel layer that aggregates docs, ownership, review state, and artifact links

Likely new modules:

- `src/review_graph.rs`
- `src/review_queue.rs`
- `src/review_session.rs`
- `src/review_context.rs`
- `src/source_browser.rs`

## Near-Term Product Decisions

These questions should be resolved early, not mid-implementation:

- Which languages get semantic diff first?
- Does the app keep separate `Files` and `Tour` tabs, or collapse them into one review workspace with modes?
- How much of the impact graph is computed eagerly versus on-demand?
- What is the persistence model for pins, route state, and last position?
- Which external artifacts are worth integrating in v1: tickets, ADRs, docs, ownership, test data?

## Success Metrics

We should measure the pivot with workflow metrics, not vanity usage:

- Time to first meaningful review comment
- Time to reach the first high-risk file or symbol
- Share of review session time spent inside this app instead of browser/tab hopping
- Re-find speed for previously seen hotspots
- Precision of suggested review order
- Adoption of pins, route state, and impact navigation

## Immediate Build Order

The implementation order should be:

1. Reframe the UI around review workflow
2. Ship semantic diff for a narrow language set
3. Add graph-backed navigation and read-only source browsing
4. Add review-session persistence and wayfinding
5. Add prioritization and queueing
6. Add inline context and graph-grounded AI
7. Add behavior-aware review for critical paths

## Summary

The pivot should be decisive:

- from PR viewer to review IDE
- from line diff to semantic diff
- from flat file lists to guided review routes
- from tab switching to inline structural context
- from generic AI assistance to graph-grounded review intelligence

If we execute this well, the app becomes the place where reviewers understand a change, trace its impact, and complete review work quickly, without ever needing to edit code.
