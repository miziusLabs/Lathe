---
name: github
description: Use the GitHub CLI and Git to inspect repositories, pull requests, issues, workflow runs, branches, and remotes.
---
# GitHub and Git workflows

Use the `bash` tool to run `gh` and `git` in the current workspace. Lathe does not provide the main agent with a dedicated GitHub API tool. Prefer the official `gh` CLI for GitHub-hosted information and `git` for local repository state and history.

## Check the repository and authentication

- Start by checking `git status --short --branch` and `git remote -v` to understand the current checkout and remotes.
- Run `gh auth status` before authenticated GitHub work. If the CLI is missing or unauthenticated, report that clearly; do not ask the user to paste a token.
- Use `gh repo view` to identify the current GitHub repository. Do not assume the selected local project is the intended remote when the checkout or request is ambiguous.
- Keep output focused. Prefer `--json` with selected fields and `--jq` for large results when supported.

## Common read operations

- Pull requests: `gh pr status`, `gh pr list`, `gh pr view <number> --comments`, `gh pr diff <number>`, and `gh pr checks <number>`.
- Issues: `gh issue list` and `gh issue view <number>`.
- Actions: `gh run list` and `gh run view <run-id>`.
- Local Git state and history: `git status`, `git diff`, `git log`, `git branch`, and `git remote`.
- If `gh` has no suitable command, use `gh api` for the narrowest relevant endpoint. Prefer GET requests for inspection and avoid dumping large responses.

## Changes and safety

- Treat reads as safe, but only create, edit, close, approve, merge, push, or otherwise change remote state when the user explicitly asks.
- Before a requested remote mutation, inspect the target and summarize consequential effects. Never force-push, delete a branch, or merge a pull request without explicit authorization for that action.
- Do not commit or push unrelated working-tree changes. Preserve existing user changes and report blockers such as a dirty worktree or missing permissions.
- Never print, copy, or request authentication tokens or other credentials.
