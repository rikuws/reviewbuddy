# Remiss UI Implementation Guide

## Goal

Implement the bright workbench design language in GPUI with shared theme roles and reusable primitives. Code should make the intended hierarchy obvious through tokens, layout, and state handling.

## Theme Tokens

Use `src/theme.rs` as the only source for product colors, radii, and durable layout dimensions.

Prefer role helpers:

- Backgrounds: `bg_canvas()`, `bg_surface()`, `bg_overlay()`, `bg_inset()`, `bg_subtle()`, `bg_emphasis()`
- States: `bg_selected()`, `hover_bg()`, `focus()`, `focus_muted()`
- Borders and focus text: `border_default()`, `border_muted()`, `focus_border()`, `fg_on_focus()`
- Text: `fg_emphasis()`, `fg_default()`, `fg_muted()`, `fg_subtle()`
- Semantics: `success()`, `warning()`, `danger()`, `info()` and their muted variants
- Shape: `radius()`, `radius_sm()`, `radius_lg()`

Do not add raw hex colors in views unless you are extending the theme itself.

## Typography

- Use `ui_font_family()` for app UI by default.
- Use `mono_font_family()` for code, paths, shortcuts, counts, hashes, and structured technical labels.
- Use `display_serif_font_family()` only for rare brand/title moments.
- Keep headings compact enough for desktop work surfaces.
- Truncate or wrap long PR titles, repository names, file paths, and thread previews deliberately.

## Components

Shared primitives should express the same state language everywhere:

- Primary action: calm filled button with focus blue or selected surface treatment.
- Secondary action: white or elevated surface with a muted border.
- Ghost action: quiet surface, visible hover, no raw text-only controls unless the context is obvious.
- Badge: small semantic chip, not the main visual focus.
- List row: stable height, title plus metadata, blue selected state, subtle hover.
- Command row: grouped result with selected wash and keyboard hint.
- Inspector panel: label/value structure with clear separators and stable empty/loading/error states.

When a pattern repeats in three or more places, move the visual treatment into a helper rather than restyling each view by hand.

## Layout

Target a roomy desktop workbench:

- Side rail: stable width, clear active indicator, subdued counts.
- Main work surface: flexible, minimum widths guarded, no accidental overflow.
- Context side panels: inspector width, scroll independently when needed.
- Command palette: centered overlay with max width and max height.
- Diff/source views: preserve practical density and high contrast.

Use fixed or bounded dimensions for controls that can otherwise jitter: tabs, icon buttons, toolbar buttons, rows, badges, and popovers.

## State

Every interactive component needs:

- normal
- hover
- active or selected
- keyboard focus where supported
- disabled or unavailable
- loading
- empty
- error

Selected/focused state should be blue-tinted and readable in both themes. Do not rely on color alone for review status; pair color with labels, icons, or badges.

## Migration Checklist

For each touched screen:

- Replace raw colors with theme roles.
- Replace old compact-only spacing with roomy workbench spacing where it helps scanning.
- Remove decorative surfaces that do not support the workflow.
- Use sans for normal UI and mono only for technical text.
- Check both light and dark themes.
- Check long titles, long repository names, empty states, sync errors, and loading states.
- Check keyboard selection and hover states.

## Validation

Before finishing broad UI work:

```sh
cargo fmt --check
cargo check --all-targets --all-features
```

Then inspect the running app in both light and dark themes. At minimum verify:

- sidebar
- command palette
- overview
- pull request queues
- PR overview
- Files/diff view
- settings
- loading, error, and empty states

Use stale-term searches after documentation changes so old visual direction does not reappear.
