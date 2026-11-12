# laudit configuration, redaction, and prompts

## Config files

- **Global:** `~/.config/laudit/config.toml`
- **Per-repo:** `<repo>/.laudit/config.toml`

`laudit build` installs a default global config on first run if the file is
missing. Existing files are never overwritten.

Per-repo values override global values for the same key. The Neovim `setup{}`
holds **only UI options** (split side/size, keymap enable, binary path); it does
not configure the engine.

API keys belong in the global config or the environment, never in the repo. A
`provider.api_key` in `<repo>/.laudit/config.toml` is rejected at load time.

## Schema

```toml
# Top-level keys (not under [provider]).
roots = ["."]                             # monorepo: multiple roots
languages = ["rust", "typescript", "javascript", "bash", "toml", "yaml", "json", "markdown", "sql"]
excludes = ["vendor/", "dist/"]           # in addition to .gitignore

[provider]
base_url = "https://api.openai.com/v1"   # any OpenAI-compatible endpoint
model = "gpt-4o-mini"                     # REQUIRED, no hidden default
api_key_env = "OPENAI_API_KEY"            # optional indirection
api_key = "sk-..."                        # global config only; rejected in repo config

[redaction]
extra_rules = []                          # additional regexes
allowlist = []                            # regexes to un-redact false positives

[budgets]
max_stops_per_chapter = 12
max_context_tokens = 12000
```

`roots`, `languages`, and `excludes` are top-level keys. Placing them under
`[provider]` is silently ignored by the loader, so keep them above the first
table header. `roots` and `languages` replace the global value when set in the
repo config; `excludes` likewise replaces rather than appends.

## API key resolution (D17)

Order, first match wins:

1. Environment: the variable named by `provider.api_key_env`, else `LAUDIT_API_KEY`.
2. Global config (if a key is stored there).

The per-repo config is never consulted for keys. Setting `provider.api_key` in
`<repo>/.laudit/config.toml` is an error, so a key cannot be committed by
accident.

`model` is required for LLM-backed commands; a missing model is a clear error.
Local endpoints (`localhost`/`127.0.0.1`/`::1`/`0.0.0.0`) require no key.
`laudit build --offline`, `laudit tour --print --offline`, and `laudit build
--offline --check` do not require a model.

## Redaction (D11)

Always on, in Rust, applied before **send**, **cache**, and **log**.

Layers:

1. **gitleaks-derived ruleset** — embedded regex rules (AWS, GCP, GitHub, Slack,
   Stripe, JWT, PEM private keys, etc.).
2. **Key-name-aware** — for env/toml/yaml/json, redact values whose key matches
   `*KEY|SECRET|TOKEN|PASSWORD|PASSWD|DSN|PRIVATE|*_URL`, keeping the key name.
3. **Session-stable placeholders** — e.g. `«aws_key#1»`; the same secret maps to
   the same placeholder within a session so the model can correlate usages.
4. **Allowlist / extra rules** — from config.

Rules:

- Never log API keys or `Authorization` headers.
- Redact remote URLs in error messages.
- Scan only the excerpt being sent, not whole files, for cost.
- The placeholder map (`redactions.json`) never contains secret values.

## Prompts (D18)

- Defaults are **embedded** in the binary.
- On first run they are installed to `~/.config/laudit/prompts/` with a version
  stamp, then loaded from there (user-editable).
- If a shipped prompt version is newer than the installed one, warn and offer a
  non-destructive update.
- `laudit prompts` lists/prints prompts.
- `prompt_version` is recorded in artifacts.

Prompt files:

| File | Purpose |
|---|---|
| `comprehension.md` | Stage 1 codebase model (strict JSON output). |
| `narration.md` | Stage 3 per-stop explanation (front-matter + markdown). |
| `repair.md` | JSON repair when structured output is malformed. |
| `ask.md` | Grounded Q&A (optional). |

## Narration output format (D10)

```
TITLE: <one line>
WHY: <one line>
---
<markdown explanation, streamed>
```

The engine parses the front-matter, then streams the body. Unknown symbol refs
in the body are dropped with a warning and logged (D24).
