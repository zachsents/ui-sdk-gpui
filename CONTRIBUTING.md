# Contributing

[PORTING.md](PORTING.md) has the details of how the original's DOM code maps to gpui.

## Building

You need Rust 1.98 or later and, on macOS, Xcode's Metal toolchain, which gpui compiles its shaders with (on Xcode 26 and later, `xcodebuild -downloadComponent MetalToolchain`).

```sh
git clone https://github.com/zachsents/ui-sdk-gpui
cd ui-sdk-gpui
cargo run --release -p ui-sdk-gallery
```

The first build compiles gpui and takes several minutes.

To port from the original, check out its source in `ref/ui-sdk` (ignored by git) at the commit this port tracks:

```sh
git clone https://github.com/vercel-labs/ui ref/ui-sdk
git -C ref/ui-sdk checkout 855bc7fd4df3d61aee33d9d975dc3e4f71debc6c
```

## Checking a Change

The workspace builds with no warnings:

```sh
cargo check --workspace --examples
```

Then run what you changed, as its example (`cargo run -p ui-sdk-select --example select`) or its gallery section, with the mouse, the keyboard alone, and VoiceOver when you touch roles, names or announcements. A debug build of the gallery logs what the kernel can check, like a control with no accessible name.

There's no automated test suite, so a pull request says what you ran and tried. Doc comments build without warnings too: `cargo doc --workspace --no-deps --exclude ui-sdk-gallery`.

## How the Workspace Is Laid Out

| Directory | Crate | What's in it |
| --- | --- | --- |
| `crates/kernel` | `ui-sdk-kernel` | What every component shares: parts, controlled state, change details and reasons, roles and ARIA, the agent surface. |
| `crates/foundation/<name>` | `ui-sdk-<name>` | The behaviors components are built from, one crate per module of the original's `foundation/`. |
| `crates/components/<name>` | `ui-sdk-<name>` | One crate per component, with `examples/<name>.rs`. `menu` holds Menu and Menubar. |
| `crates/page` | `ui-sdk-page` | The agent bridge, `observe`, history and undo. |
| `crates/native` | `ui-sdk-native` | A text field that stands in for `<input>` and `<textarea>`, which the original gets from the browser. |
| `crates/ui-sdk` | `ui-sdk` | Re-exports everything under the original's paths, plus `init`. |
| `crates/gallery` | `ui-sdk-gallery` | The app that shows every example. |
| `crates/sink` | `ui-sdk-sink` | Four tiles rebuilt from the original's sink page, shown in the gallery. |

Each crate depends on what its original module imports. Nothing below a component depends on a component, and the graph has no cycles.

## Conventions

The port stays close to the original, so the UI SDK docs keep describing it and upstream changes port file by file. PORTING.md has the full rules; in short:

- One Rust module per TypeScript file, in snake_case (`roving-focus.ts` becomes `roving_focus.rs`), definitions in the original's order. `index.ts` becomes the `lib.rs` re-exports.
- Every option, state field, reason, key, focus move, timing and ARIA attribute the original has. Where gpui lacks a DOM feature, the same behavior the gpui way.
- Anything left out is listed in the crate's `lib.rs` docs under "Dropped, and why", and anything that behaves differently under "Deviations".
- Names are the original's in snake_case; types keep theirs. Comments are the original's, ported with the code.
- Examples are gallery sections, so they share one shape (PORTING.md, Examples), and a new one needs an entry in `crates/gallery/build.rs`. Their UI text is labels, data and short placeholders.

## Porting Upstream Changes

To see what changed upstream since the tracked commit:

```sh
git -C ref/ui-sdk fetch origin
git -C ref/ui-sdk log --oneline 855bc7f..origin/main -- packages/core/src
git -C ref/ui-sdk diff 855bc7f..origin/main -- packages/core/src/components/select
```

Port each changed file into its module, then move the tracked commit forward in `README.md`, this file and `PORTING.md`.

## Pull Requests

One change per pull request, with what you ran and tried. A behavior change says whether the original behaves the same way, with a link to its file or commit.
