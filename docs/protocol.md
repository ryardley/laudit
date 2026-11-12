# laudit protocol

Two protocols exist:

1. **JSON-RPC 2.0 over stdio** — the editor-agnostic control channel used by the
   Neovim plugin and any other frontend (`laudit serve`).
2. **MCP over stdio** — exposes engine tools to AI agents (`laudit mcp`).

Both are thin layers over the same `laudit-core` handlers.

## JSON-RPC 2.0 (laudit serve)

### Framing

- Newline-delimited JSON (one JSON object per line) over stdin/stdout.
- Standard JSON-RPC 2.0 request/response/notification objects.
- Requests carry `id`; responses echo it. Notifications have no `id`.
- Logs go to **stderr**, never stdout (stdout is the protocol channel).

### Methods (client → engine)

| Method | Params | Result |
|---|---|---|
| `ping` | `{}` | `{ "pong": true, "version": "0.1.0" }` |
| `status` | `{ "tour_id"?: string, "repo"?: string }` | `{ "engine": "ok", "tour"?: {…counts…} }` |
| `build_tour` | `{ "repo": string, "offline"?: bool, "source"?: {…} }` | `{ "tour_id": string }` |
| `resume_tour` | `{ "repo": string, "tour_id"?: string }` | `{ "tour_id": string, "position": { "chapter": number, "stop": number } }` |
| `explain_stop` | `{ "tour_id": string, "stop_id": string }` | `{ "stop_id": string }` (prose streams via notifications) |
| `get_outline` | `{ "tour_id": string }` | `{ "chapters": [ {id,title,stops:[{id,title,why_here,file,start_line,end_line,explanation,status}]} ] }` |
| `set_position` | `{ "tour_id": string, "chapter": number, "stop": number }` | `{ "position": { "chapter": number, "stop": number } }` |
| `cancel` | `{ "tour_id": string, "stop_id"?: string }` | `{ "cancelled": true }` |

### Notifications (engine → client)

| Notification | Payload |
|---|---|
| `progress` | `{ "tour_id", "stage": "recon"\|"comprehension"\|"plan"\|"narration", "message", "pct"? }` |
| `comprehension_done` | `{ "tour_id", "model_path" }` |
| `stop_ready` | `{ "tour_id", "stop_id", "file", "start_line", "end_line", "title", "why_here" }` |
| `explanation_chunk` | `{ "tour_id", "stop_id", "delta" }` |
| `error` | `{ "tour_id"?, "stop_id"?, "code", "message" }` |

### Error codes

Use JSON-RPC standard codes where applicable (`-32700` parse error, `-32600`
invalid request, `-32601` method not found, `-32602` invalid params, `-32603`
internal error). Application errors use the range `-32000..-32099`:

| Code | Meaning |
|---|---|
| `-32001` | no model configured |
| `-32002` | tour not found |
| `-32003` | stop not found |
| `-32004` | cancelled |
| `-32005` | LLM failure (after retries) |

### Lifecycle

- Lazily started by the frontend on first use; one child per Neovim instance.
- `ping` for health; bounded auto-restart on crash (D15).
- Clean shutdown on stdin EOF or `VimLeave`.
- One active tour per instance; `build_tour` cancels/replaces the current tour.

## MCP (laudit mcp)

MCP over stdio: `initialize` handshake, capabilities, `tools/list`, `tools/call`.

| Tool | Input | Output |
|---|---|---|
| `build_tour` | `{ repo, offline? }` | `{ tour_id, outline }` |
| `explain` | `{ repo, symbol }` | `{ title, why_here, explanation, refs }` |
| `outline` | `{ repo }` | `{ chapters: [...] }` |
| `status` | `{ repo }` | `{ model_present, tour_present, counts }` |

All returned text is redacted (D11). The MCP transport is separate from the
engine; it reuses `laudit-core` handlers.
