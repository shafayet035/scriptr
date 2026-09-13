# Contributing to Scriptr

Thanks for your interest! Scriptr is early — issues, ideas and pull requests are all welcome.

## Setup

Prerequisites: [Rust](https://rustup.rs) (stable), Node.js 20+, [pnpm](https://pnpm.io) 10+, and the
[Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) for your OS (Xcode Command Line Tools on macOS).

```bash
pnpm install
pnpm tauri dev      # full desktop app (Rust core + UI)
pnpm dev            # UI only, in a browser, against an in-memory mock backend
```

`pnpm dev` is the fastest loop for UI work: open http://localhost:1420 (add `?empty` to start with no projects).
The mock in `src/lib/mock.ts` mirrors the IPC contract, so UI changes can be built and checked without a window.

## Before opening a PR

```bash
pnpm typecheck
pnpm build
cd src-tauri && cargo fmt --check && cargo clippy --all-targets && cargo test
```

- Keep the IPC contract in sync: `src/lib/types.ts` ⇄ `src-tauri/src/model.rs` (serde `camelCase`).
- Status colour is the app's primary signal — use the `--status-*` tokens, never ad-hoc colours.
- Icons come from the design file (`src/assets/icons`); please don't hand-draw new ones in a PR.
- Process hygiene is the part that bites: if you touch `supervisor.rs` or `pty.rs`, add or extend a test in
  `src-tauri/tests/supervisor.rs` that proves the whole process tree goes away.

## Commit style

Short imperative subject (`Add HTTPS support to http gate`), with a body explaining *why* when it isn't obvious.

## Reporting bugs

Include your OS and version, how you installed Scriptr, the script command involved, and what the terminal
showed. For process-tree issues, the output of `ps -o pid,pgid,command` while the script is running helps a lot.
