#!/usr/bin/env bash
# Fake laudit engine for headless Neovim tests.
#
# Speaks newline-delimited JSON-RPC 2.0 on stdin/stdout. It is intentionally
# tiny: enough to exercise the Lua client's request/response, notification,
# crash/restart, outline, resume, and error paths. No network.
#
# Error simulation (set in the child environment):
#   FAKE_ENGINE_BUILD_ERROR=1    build_tour returns -32001
#   FAKE_ENGINE_EXPLAIN_ERROR=1  explain_stop returns -32005
#   FAKE_ENGINE_OUTLINE_ERROR=1  get_outline returns -32002
#   FAKE_ENGINE_INVALID_JSON=1   ping emits a malformed line first
#   FAKE_ENGINE_CRASH_ON_START=1 exit immediately

set -u

if [ "${1:-}" = "--version" ]; then
  printf 'laudit %s\n' "${FAKE_ENGINE_VERSION:-0.1.0-fake}"
  exit 0
fi

if [ "${FAKE_ENGINE_CRASH_ON_START:-}" = "1" ]; then
  exit 1
fi

state_file="${FAKE_ENGINE_STATE:-/tmp/laudit-fake-state}"
build_error="${FAKE_ENGINE_BUILD_ERROR:-}"
explain_error="${FAKE_ENGINE_EXPLAIN_ERROR:-}"
outline_error="${FAKE_ENGINE_OUTLINE_ERROR:-}"
invalid_json="${FAKE_ENGINE_INVALID_JSON:-}"

emit() {
  printf '%s\n' "$1"
}

while IFS= read -r line; do
  [ -z "$line" ] && continue

  id=$(printf '%s' "$line" | sed -n 's/.*"id"[[:space:]]*:[[:space:]]*\([^,}]*\).*/\1/p')
  method=$(printf '%s' "$line" | sed -n 's/.*"method"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')

  case "$method" in
    ping)
      if [ "$invalid_json" = "1" ]; then
        emit 'this is not json'
      fi
      emit "{\"jsonrpc\":\"2.0\",\"id\":${id:-null},\"result\":{\"pong\":true,\"version\":\"fake\"}}"
      ;;
    notify)
      emit '{"jsonrpc":"2.0","method":"progress","params":{"stage":"recon","message":"fake","pct":50}}'
      emit "{\"jsonrpc\":\"2.0\",\"id\":${id:-null},\"result\":{\"ok\":true}}"
      ;;
    build_tour)
      if [ -n "$build_error" ]; then
        emit "{\"jsonrpc\":\"2.0\",\"id\":${id:-null},\"error\":{\"code\":-32001,\"message\":\"no model configured\"}}"
        emit '{"jsonrpc":"2.0","method":"error","params":{"tour_id":"tour-fake","code":-32001,"message":"no model configured"}}'
        continue
      fi
      printf '0 0\n' > "$state_file"
      emit "{\"jsonrpc\":\"2.0\",\"id\":${id:-null},\"result\":{\"tour_id\":\"tour-fake\"}}"
      emit '{"jsonrpc":"2.0","method":"progress","params":{"tour_id":"tour-fake","stage":"plan","message":"Tour planned","pct":100}}'
      emit '{"jsonrpc":"2.0","method":"stop_ready","params":{"tour_id":"tour-fake","stop_id":"stop-1","file":"src/main.rs","start_line":1,"end_line":2,"title":"Main","why_here":"entrypoint"}}'
      emit '{"jsonrpc":"2.0","method":"stop_ready","params":{"tour_id":"tour-fake","stop_id":"stop-2","file":"src/lib.rs","start_line":1,"end_line":1,"title":"Helper","why_here":"utility"}}'
      emit '{"jsonrpc":"2.0","method":"explanation_chunk","params":{"tour_id":"tour-fake","stop_id":"stop-1","delta":"Hello "}}'
      emit '{"jsonrpc":"2.0","method":"explanation_chunk","params":{"tour_id":"tour-fake","stop_id":"stop-1","delta":"world"}}'
      ;;
    resume_tour)
      if [ -f "$state_file" ]; then
        read -r chapter stop < "$state_file"
      else
        chapter=0
        stop=0
      fi
      emit "{\"jsonrpc\":\"2.0\",\"id\":${id:-null},\"result\":{\"tour_id\":\"tour-fake\",\"position\":{\"chapter\":${chapter:-0},\"stop\":${stop:-0}}}}"
      ;;
    set_position)
      chapter=$(printf '%s' "$line" | sed -n 's/.*"chapter"[[:space:]]*:[[:space:]]*\([0-9]*\).*/\1/p')
      stop=$(printf '%s' "$line" | sed -n 's/.*"stop"[[:space:]]*:[[:space:]]*\([0-9]*\).*/\1/p')
      printf '%s %s\n' "${chapter:-0}" "${stop:-0}" > "$state_file"
      emit "{\"jsonrpc\":\"2.0\",\"id\":${id:-null},\"result\":{\"position\":{\"chapter\":${chapter:-0},\"stop\":${stop:-0}}}}"
      ;;
    get_outline)
      if [ -n "$outline_error" ]; then
        emit "{\"jsonrpc\":\"2.0\",\"id\":${id:-null},\"error\":{\"code\":-32002,\"message\":\"tour not found\"}}"
        continue
      fi
      emit "{\"jsonrpc\":\"2.0\",\"id\":${id:-null},\"result\":{\"chapters\":[{\"id\":\"ch-1\",\"title\":\"On-ramp\",\"stops\":[{\"id\":\"stop-1\",\"title\":\"Main\",\"file\":\"src/main.rs\",\"start_line\":1,\"end_line\":2},{\"id\":\"stop-2\",\"title\":\"Helper\",\"file\":\"src/lib.rs\",\"start_line\":1,\"end_line\":1}]}]}}"
      ;;
    explain_stop)
      if [ -n "$explain_error" ]; then
        stop_id=$(printf '%s' "$line" | sed -n 's/.*"stop_id"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')
        emit "{\"jsonrpc\":\"2.0\",\"id\":${id:-null},\"error\":{\"code\":-32005,\"message\":\"LLM failure\"}}"
        emit "{\"jsonrpc\":\"2.0\",\"method\":\"error\",\"params\":{\"tour_id\":\"tour-fake\",\"stop_id\":\"${stop_id:-stop-1}\",\"code\":-32005,\"message\":\"LLM failure\"}}"
        continue
      fi
      emit "{\"jsonrpc\":\"2.0\",\"id\":${id:-null},\"result\":{\"stop_id\":\"stop-1\"}}"
      emit '{"jsonrpc":"2.0","method":"explanation_chunk","params":{"tour_id":"tour-fake","stop_id":"stop-1","delta":" More"}}'
      ;;
    cancel)
      emit "{\"jsonrpc\":\"2.0\",\"id\":${id:-null},\"result\":{\"cancelled\":true}}"
      ;;
    status)
      emit "{\"jsonrpc\":\"2.0\",\"id\":${id:-null},\"result\":{\"engine\":\"ok\",\"tour\":{\"model_present\":true,\"tour_present\":true,\"tour_id\":\"tour-fake\",\"chapters\":1,\"stops\":2,\"explained\":0,\"pending\":2,\"streaming\":0,\"skipped\":0,\"failed\":0,\"stale\":0,\"position\":{\"chapter\":0,\"stop\":0}}}}"
      ;;
    crash)
      exit 1
      ;;
    *)
      emit "{\"jsonrpc\":\"2.0\",\"id\":${id:-null},\"error\":{\"code\":-32601,\"message\":\"method not found: ${method}\"}}"
      ;;
  esac
done
