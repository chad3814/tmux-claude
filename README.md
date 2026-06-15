# tmux-claude

## nrun PreToolUse hook

`hooks/route-to-nrun.sh` is a Claude Code `PreToolUse` hook that transparently
routes allowlisted long-lived commands (dev servers, docker, build tools)
through `nrun`, so each runs in a dedicated tmux window while Claude still reads
its output. Unrecognized commands run unchanged. The hook is **fail-open**: any
error, unrecognized input, or missing `nrun` passes the command through verbatim.

### Requirements

- `nrun` on `PATH`: `cargo install --path .`
- `jq` on `PATH`.

### Enable it (opt-in)

Add to `~/.claude/settings.json` (use the absolute path to this checkout):

```json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Bash",
        "hooks": [
          { "type": "command", "command": "/ABSOLUTE/PATH/TO/tmux-claude/main/hooks/route-to-nrun.sh" }
        ]
      }
    ]
  }
}
```

### Allowlist

`pnpm dev`, `npm run dev`, `yarn dev`, `next dev`, `vite` (and `npx next dev` /
`npx vite`), `docker run`, `docker compose up` / `docker-compose up`, `make`,
`cmake`, `configure`. A leading `cd <dir> &&` and leading `VAR=value` env
assignments are folded into the run; commands with pipes, redirects, `;`,
additional `&&`, subshells, or command substitution pass through unchanged.
