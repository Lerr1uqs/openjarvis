---
name: acpx
description: Use acpx as a headless ACP CLI for agent-to-agent communication, including prompt/exec/sessions workflows, session scoping, queueing, permissions, and output formats.
---

# acpx

## When to use this skill

Use this skill when you need to run coding agents through `acpx`, manage persistent ACP sessions, queue prompts, or consume structured agent output from scripts.

## Install

```bash
npm i -g acpx
```

For stable multi-turn work, prefer a global install over `npx`.

## Core commands

```bash
acpx codex exec "reply with one word: pong"
acpx codex prompt "continue the current task"
acpx codex sessions new --name backend
acpx codex status
acpx codex cancel
```

## Guidance

- Prefer `--format json` when the caller needs structured output.
- Use a stable session name derived from the current OpenJarvis thread when multi-turn continuity matters.
- Prefer `exec` for one-shot work and `prompt` for long-lived sessions.
- If the caller asks for a different agent, pass the agent name positionally, for example `acpx claude exec "summarize this repo"`.
- Do not assume `acpx` is installed; verify with `acpx --help` or `acpx codex status` before using it.
