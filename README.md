<h1 align="center">
  <br>
  Lathe
  <br>
</h1>

<h4 align="center">A desktop home for your coding agents.</h4>

<p align="center">
  <img alt="Tauri" src="https://img.shields.io/badge/Tauri-2-24C8DB?style=flat-square&logo=tauri&logoColor=white">
  <img alt="React" src="https://img.shields.io/badge/React-19-61DAFB?style=flat-square&logo=react&logoColor=white">
  <img alt="Vite" src="https://img.shields.io/badge/Vite-7-646CFF?style=flat-square&logo=vite&logoColor=white">
  <img alt="Version" src="https://img.shields.io/badge/version-0.8.4-blue?style=flat-square">
  <img alt="License" src="https://img.shields.io/badge/license-Apache--2.0-6B4EFF?style=flat-square">
</p>

## Overview

Lathe is a desktop app with its own Rust coding agent. Connect your ChatGPT account in Settings to use the OpenAI models available to your account. Pi and Node.js are not required to run the agent.

> This project is a fork of [monorepo-labs/dray](https://github.com/monorepo-labs/dray).

## License

This project is licensed under the Apache License 2.0. See [LICENSE](LICENSE).

## Features

- Persistent multi-session workspace with search, pinning, settling, forks, nested sessions, unread/waiting state, and desktop notifications.
- Local Sessions inside attached projects, with project and Git branch switching.
- Isolated Cloud Sessions that run Lathe's agent in Docker with their own persistent workspace volume.
- ChatGPT sign-in, account-specific model and effort controls, context usage, queued follow-ups, and generated session titles.
- Native file editing, search, shell and background commands, questions, codebase/GitHub research, and web search.
- Automatic OpenAI prompt caching with recorded token and cache-hit indicators.
- Rich chat transcripts with Markdown, syntax highlighting, reasoning, tool calls, file edits, diffs, images, and structured questions.
- File/image attachments, drag and drop, `@file` fuzzy search, `/commands`, and `$skills` discovered from `.agents/skills`.
- Git handoff actions for commit, push, and pull-request workflows.
- GitHub pull request markers and ready-to-merge notifications through `gh`.
- Themes, native window integration, keyboard shortcuts, sounds, notices, and safe quit handling while work is active.

## Agent setup

Open **Settings > Models > Continue with ChatGPT** and complete browser sign-in.
Model and reasoning choices come from the account's OpenAI catalog. Credentials
stay in the native backend and operating system credential store on Windows
and macOS. Lathe never imports Pi authentication.

Skills live in `~/.mizius/skills/<name>/SKILL.md` or a project's
`.agents/skills/<name>/SKILL.md`. Lathe's bundled skills are installed in the
global directory so they can be read like other skills; existing files are
never overwritten. Use YAML frontmatter with `name` and `description`, followed
by the skill instructions. Select a skill in the composer or invoke it with
`$name`. Project skills override global skills with the same name.

The embedded system prompt is `packages/agent/SYSTEM.md`. Usage shows ChatGPT
Codex limits when available, using a matching local Codex login if needed.
Follow the ChatGPT usage link to manage Lathe’s app allowance.

## Tech stack

- [Tauri](https://tauri.app/) 2 with a Rust backend
- [React](https://react.dev/) 19 and [Vite](https://vite.dev/) 7
- [Tailwind CSS](https://tailwindcss.com/) v4
- [pnpm](https://pnpm.io/) workspace

## Layout

| Path           | What                                                          |
| -------------- | ------------------------------------------------------------- |
| `apps/desktop` | The Tauri app. React 19 + Vite frontend, Rust backend.        |
| `packages/agent` | Standalone Rust coding agent and embedded system prompt. |
| `AGENTS.md`    | Detailed architecture, component, feature, and agent guide.   |

## Development

Install dependencies from the root. The lockfile covers the whole workspace.

```sh
pnpm install
```

Start the app from the root.

```sh
pnpm app           # desktop app (Tauri + Vite), with hot reload
pnpm app:no-watch  # desktop app without frontend or backend auto-reload
```

Windows GNU builds require MinGW with `gcc` and `windres`. The Tauri launcher
finds Scoop's MinGW installation automatically; for direct Cargo commands,
include MinGW's `bin` directory in your terminal's PATH.

Commands beyond starting an app should be run from its own directory, because
`tauri.conf.json`, `.cargo/config.toml`, and `scripts/install.ps1` resolve their
paths against it:

```sh
cd apps/desktop && pnpm tauri build
cd apps/desktop/src-tauri && cargo test
```

## Cloud sandbox

Cloud Sessions run the standalone Lathe agent in Docker without mounting or
cloning the selected project. The image includes Java 21, Java 25, Node.js 24,
GitHub CLI, Git, and Lathe. Host `~/.mizius/skills` is mounted read-only. Each
workspace has its own persistent history. Short-lived OpenAI access tokens
travel through stdin; credentials remain on the host. GitHub authentication
uses `GITHUB_TOKEN` or an authenticated host `gh`.
Build the image locally (Docker Desktop must be running):

```sh
pnpm build:sandbox
```

The build runs from the host on Windows, macOS, and Linux. On Windows it uses
the same Docker daemon (Docker Desktop's WSL 2 backend) that Cloud Sessions
run on, so no separate WSL setup is needed — the image is built where it will
be used.

Use `DRAY_CLOUD_IMAGE` to select a different image tag and `GITHUB_TOKEN` (or a
logged-in `gh`) to provide the token passed to Cloud containers.

## Releasing the app

Push a commit to `main` that bumps the matching versions in
`apps/desktop/package.json`, `apps/desktop/src-tauri/Cargo.toml`, and
`apps/desktop/src-tauri/tauri.conf.json`. The `CI` workflow builds macOS and
Windows installers with signed update bundles, publishes them in a `vX.Y.Z`
GitHub release, and makes the release available to the app's automatic updater.
Release notes are generated by GitHub from the changes since the previous release.

The repository must provide a `TAURI_SIGNING_PRIVATE_KEY` Actions secret that
matches the updater public key in `tauri.conf.json`.
