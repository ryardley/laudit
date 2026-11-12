#!/usr/bin/env bash
# Fake laudit engine for headless Neovim tests.
#
# Speaks newline-delimited JSON-RPC 2.0 on stdin/stdout. It is intentionally
# tiny: enough to exercise the Lua client's request/response, notification, and
# crash/restart paths. No network.

set -u

if [ "${FAKE_ENGINE_CRASH_ON_START:-}" = "1" ]; then
  exit 1
fi

emit() {
  printf '%s\n' "$1"
}

while IFS= read -r line; do
  [ -z "$line" ] && continue

  id=$(printf '%s' "$line" | sed -n 's/.*"id"[[:space:]]*:[[:space:]]*\([^,}]*\).*/\1/p')
  method=$(printf '%s' "$line" | sed -n 's/.*"method"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')

  case "$method" in
    ping)
      emit "{\"jsonrpc\":\"2.0\",\"id\":${id:-null},\"result\":{\"pong\":true,\"version\":\"fake\"}}"
      ;;
    notify)
      emit '{"jsonrpc":"2.0","method":"progress","params":{"stage":"recon","message":"fake","pct":50}}'
      emit "{\"jsonrpc\":\"2.0\",\"id\":${id:-null},\"result\":{\"ok\":true}}"
      ;;
    build_tour)
      emit "{\"jsonrpc\":\"2.0\",\"id\":${id:-null},\"result\":{\"tour_id\":\"tour-fake\"}}"
      emit '{"jsonrpc":"2.0","method":"progress","params":{"tour_id":"tour-fake","stage":"plan","message":"Tour planned","pct":100}}'
      emit '{"jsonrpc":"2.0","method":"stop_ready","params":{"tour_id":"tour-fake","stop_id":"stop-1","file":"src/main.rs","start_line":1,"end_line":2,"title":"Main","why_here":"entrypoint"}}'
      emit '{"jsonrpc":"2.0","method":"stop_ready","params":{"tour_id":"tour-fake","stop_id":"stop-2","file":"src/lib.rs","start_line":1,"end_line":1,"title":"Helper","why_here":"utility"}}'
      emit '{"jsonrpc":"2.0","method":"explanation_chunk","params":{"tour_id":"tour-fake","stop_id":"stop-1","delta":"Hello "}}'
      emit '{"jsonrpc":"2.0","method":"explanation_chunk","params":{"tour_id":"tour-fake","stop_id":"stop-1","delta":"world"}}'
      ;;
    get_outline)
      emit "{\"jsonrpc\":\"2.0\",\"id\":${id:-null},\"result\":{\"chapters\":[{\"id\":\"ch-1\",\"title\":\"On-ramp\",\"stops\":[{\"id\":\"stop-1\",\"title\":\"Main\",\"file\":\"src/main.rs\",\"start_line\":1,\"end_line\":2},{\"id\":\"stop-2\",\"title\":\"Helper\",\"file\":\"src/lib.rs\",\"start_line\":1,\"end_line\":1}]}]}}"
      ;;
    explain_stop)
      emit "{\"jsonrpc\":\"2.0\",\"id\":${id:-null},\"result\":{\"stop_id\":\"stop-1\"}}"
      emit '{"jsonrpc":"2.0","method":"explanation_chunk","params":{"tour_id":"tour-fake","stop_id":"stop-1","delta":" More"}}'
      ;;
    cancel)
      emit "{\"jsonrpc\":\"2.0\",\"id\":${id:-null},\"result\":{\"cancelled\":true}}"
      ;;
    status)
      emit "{\"jsonrpc\":\"2.0\",\"id\":${id:-null},\"result\":{\"engine\":\"ok\"}}"
      ;;
    crash)
      exit 1
      ;;
    *)
      emit "{\"jsonrpc\":\"2.0\",\"id\":${id:-null},\"error\":{\"code\":-32601,\"message\":\"method not found: ${method}\"}}"
      ;;
  esac
done
