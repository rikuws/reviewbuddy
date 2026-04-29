---
name: frontend-ui-engineering
description: Designs and refines production-quality GPUI interfaces for Remiss. Use when building or polishing native app views, panes, lists, command palettes, inspectors, and other user-facing surfaces where visual hierarchy and product feel matter.
---

# Frontend UI Engineering (GPUI)

## Overview

Build production-quality GPUI interfaces that feel soft, precise, native, and useful for long code review sessions. The target is a roomy bright workbench with an equally polished dark theme: command-driven, sans-first, blue-focused, and shaped around real review workflows.

## When To Use

- Building or revising screens in `src/views/`
- Refining layout, spacing, hierarchy, density, or product feel
- Designing panels, sidebars, lanes, tab bars, lists, command palettes, and detail views
- Improving hover, selected, loading, empty, error, disabled, or sync states
- Removing generic AI styling or stale visual direction from a GPUI surface

## GPUI View Composition

Keep view code close to the feature, and extract shared helpers only after a pattern is clearly repeated. Favor readable composition with existing primitives like `panel()`, `ghost_button()`, `badge()`, `eyebrow()`, and the theme helpers in `src/theme.rs`.

Use GPUI builders to make hierarchy obvious:

```rust
panel().child(
    div()
        .p(px(24.0))
        .flex()
        .flex_col()
        .gap(px(14.0))
        .child(eyebrow("Settings / Language Servers"))
        .child(
            div()
                .text_size(px(22.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(fg_emphasis())
                .child("Managed language servers"),
        )
        .child(
            div()
                .text_size(px(13.0))
                .line_height(px(20.0))
                .text_color(fg_muted())
                .max_w(px(760.0))
                .child("Download or repair the LSPs this app can manage itself."),
        ),
)
```

## State And Async

Keep long-running work outside rendering. Loading, error, empty, and ready states should keep the same frame so the UI does not jump while review data refreshes.

## Design System Adherence

### Use The Theme

Pull colors, radii, and sizing from `src/theme.rs` before inventing anything new.

- Use `bg_canvas()`, `bg_surface()`, `bg_overlay()`, `bg_inset()`, `bg_selected()`, and `hover_bg()`
- Use `fg_emphasis()`, `fg_default()`, `fg_muted()`, and `fg_subtle()`
- Use `focus()`, `focus_muted()`, `success()`, `warning()`, `danger()`, and `info()` sparingly and semantically
- Use `radius()`, `radius_sm()`, and `radius_lg()` instead of ad hoc corner values
- Do not hardcode raw colors in views unless you are extending the theme

### Match The App's Visual Language

Remiss now uses a bright workbench design language:

- soft neutral surfaces with thin borders and subtle depth
- blue-tinted selected and focused states
- roomy shells, palettes, panels, and queue rows
- practical density for code, diff, source, and file tree surfaces
- small semantic color signals rather than decorative color everywhere
- real product copy instead of placeholder dashboard language

### Spacing And Layout

Favor familiar spacing values like `px(4.0)`, `6.0`, `8.0`, `10.0`, `12.0`, `14.0`, `16.0`, `20.0`, `24.0`, `28.0`, `32.0`, `40.0`, and `48.0`.

Use roomy spacing where it improves scanning, especially in command palettes, overview panels, PR lists, and settings. Keep tighter rhythm in diff and source views so review work stays efficient.

Use `topbar_height()`, `sidebar_width()`, `file_tree_width()`, and `detail_side_width()` before adding new layout constants.

## Typography And Copy

- Use `ui_font_family()` for normal UI.
- Use `mono_font_family()` for code, paths, counts, shortcuts, hashes, and technical metadata.
- Use `display_serif_font_family()` only for rare brand/title moments.
- Keep copy direct and operational: repositories, queues, reviews, sync state, install failures, empty states.
- Avoid marketing tone, lorem ipsum, and generic SaaS dashboard language.

## Purpose-Built Surfaces

- Panels should clarify grouping and state.
- Lanes and sidebars should help scanning and comparison.
- Actions should sit close to the content they affect.
- Command palette rows should support keyboard-first work.
- Context panels should read like inspectors with stable labels and values.
- Prefer left-aligned text, predictable truncation, and stable rhythm.

## Avoid Generic AI Styling

| Avoid | Why It Hurts | Prefer |
|---|---|---|
| Accent-colored everything | It makes the screen compete for attention | Neutral surfaces with semantic color signals |
| Uniform card grids | They ignore how review tooling is scanned | Lists, lanes, split panes, and inspector layouts |
| Texture behind code | It hurts review readability | Plain high-contrast code surfaces |
| Raw hex colors in views | It creates visual drift | Theme helpers from `src/theme.rs` |
| Generic dashboard stats and copy | It feels templated | Content-first layouts with review language |
| Hidden keyboard focus | It weakens command-first workflows | Visible blue focus and selected states |

## Desktop-First Resilience

This is a native desktop UI, so think in terms of window pressure rather than mobile breakpoints.

- Layouts should survive narrow laptop windows, normal working widths, and wide external displays.
- Test long repository names, long PR titles, empty queues, large diffs, and scroll-heavy states.
- Prefer split panes and stable sidebars over collapsing everything into stacked cards.
- Avoid accidental horizontal overflow unless the content truly requires it.

## Interaction And Accessibility

- Every interactive element should look interactive and remain keyboard reachable.
- Hover, selected, disabled, and busy states should all read as different states.
- Icon-only actions need clear labels or obvious affordances.
- Do not rely on color alone for approval, failure, or review status.
- Empty, error, loading, and sync states are part of the design.
- Preserve orientation when panels open, selections change, or content refreshes.

## Loading And Motion

- Prefer stable layouts with subtle status text, row placeholders, or panel-level busy states.
- Avoid full-screen spinners when content can keep its structure.
- Keep animation minimal and purposeful; the app should feel calm and fast.

## See Also

- `DESIGN_LANGUAGE.md`
- `UI_IMPLEMENTATION_GUIDE.md`
- `src/theme.rs`
- `src/views/root.rs`
- `src/views/sections.rs`
- `src/views/pr_detail.rs`

## Common Rationalizations

| Rationalization | Reality |
|---|---|
| "It is just internal tooling" | Internal tools are used for long stretches; density and polish directly affect speed and trust. |
| "We can style it later" | Layout and hierarchy are structural decisions. |
| "A generic dashboard is good enough" | Review workflows need purpose-built scanning and comparison. |
| "The state work matters more than the design" | Users experience the product through the interface first. |
| "The accent color makes it feel designed" | Real polish comes from hierarchy, rhythm, and restraint. |

## Red Flags

- New raw colors inside views
- Texture or decorative imagery behind code
- Oversized whitespace that slows review work
- Uniform card grids where lists, lanes, or split views would scan better
- Multiple competing emphasis colors in the same surface
- Missing empty, error, loading, disabled, or sync states
- Long pages that should be split into sidebar/detail or lane-based layouts
- UI that could belong to any generated SaaS template

## Verification

After designing or updating a GPUI surface:

- [ ] Uses shared theme tokens and existing helpers where appropriate
- [ ] Matches the bright workbench design language
- [ ] Makes primary information immediately scannable and secondary information clearly subdued
- [ ] Works in narrow and wide desktop windows without awkward overflow or broken hierarchy
- [ ] Gives hover, selected, disabled, loading, empty, and error states intentional visual treatment
- [ ] Keeps keyboard interaction clear and usable
- [ ] Avoids generic AI styling and unnecessary decoration
