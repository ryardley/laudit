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
    crash)
      exit 1
      ;;
    *)
      emit "{\"jsonrpc\":\"2.0\",\"id\":${id:-null},\"error\":{\"code\":-32601,\"message\":\"method not found: ${method}\"}}"
      ;;
  esac
done
