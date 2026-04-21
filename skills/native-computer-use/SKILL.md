---
name: native-computer-use
description: Inspect and verify native macOS app UI with the Computer Use plugin. Use when a locally built app needs visual validation, especially when the process starts as a raw debug binary instead of a normal `.app` bundle.
---

# Native Computer Use

Use this skill when you need to drive a local macOS app with the Computer Use plugin and inspect the real UI, not just the code.

## When To Use

- Verifying screens in a native desktop app
- Navigating flows visually after local code changes
- Checking whether a GPUI view actually renders the intended hierarchy
- Working around Computer Use discovery problems for raw binaries

## Workflow

1. Start the app locally.
   - Prefer the repo's normal dev command first.
   - Keep the process alive in a long-running exec session so you can poll logs while using Computer Use.

2. Confirm the app is visible to the OS.
   - Use `osascript` or `System Events` to inspect foreground process names if needed.
   - Use `mcp__computer_use__.list_apps` to see whether Computer Use can target it.

3. If the app is a raw debug binary and `get_app_state` returns `appNotFound(...)`, wrap it in a minimal `.app` bundle.
   - Create `target/debug/<app>.app/Contents/MacOS/<app>`.
   - Copy the built binary into that bundle.
   - Write a minimal `Info.plist` with `CFBundleName`, `CFBundleIdentifier`, and `CFBundleExecutable`.
   - Launch it with `open /absolute/path/to/<app>.app`.
   - Re-run `list_apps`; once the bundle appears, target that app name with Computer Use.

4. Use Computer Use against the real window.
   - Call `get_app_state` once each assistant turn before clicking or typing.
   - If the accessibility tree is sparse, drive the app with screenshot coordinates instead of element indices.
   - Re-check `get_app_state` after major navigation changes so the screenshot stays current.

5. Keep terminal and UI inspection paired.
   - Poll the long-running app session for runtime errors or warnings while interacting with the UI.
   - If the screen looks wrong but the app did not crash, capture both the visible symptom and any log clues.

## Repo-Specific Notes For `gh-ui`

- The app is a native GPUI desktop app, not a web app.
- `cargo run` starts the binary, but Computer Use may not discover it until it is wrapped and launched as `target/debug/gh-ui.app`.
- If the `Reviews` queue is empty, open a PR through `Pull Requests` and use `Authored` or `Involved` to reach a real review surface.
- `gh auth status` is a quick sanity check before assuming the UI is empty because of rendering bugs.

## Tactics

- Favor direct UI inspection over code assumptions.
- If a screen has limited accessibility metadata, document that separately as an AX issue rather than confusing it with a visual bug.
- When checking a dense review surface, inspect three things explicitly:
  - information hierarchy
  - diff readability
  - whether side panels compete with the primary task

## Done Criteria

- The app is running and targetable by Computer Use.
- You navigated to the intended screen, not just the landing page.
- You verified the live UI visually.
- You recorded any runtime, layout, or accessibility issues discovered during navigation.
