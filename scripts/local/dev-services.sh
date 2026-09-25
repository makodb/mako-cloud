#!/usr/bin/env bash
# Stand-ins for the object store and mail relay the services expect, for a
# host without Docker (the compose stack in infra/local is the usual way):
#
#   object store  an S3 subset on MAKO_OBJECT_STORE_ENDPOINT's port, checking
#                 each request's signature against the configured keys, with
#                 the two buckets the planes use created up front
#   mail          SMTP on MAKO_DEVELOPER_SMTP_PORT, checking the configured
#                 credentials and keeping every message; an inbox page on 8025
#
# Development only: nothing is replicated, listed, or delivered.
#
#   scripts/local/dev-services.sh start | stop | status
set -euo pipefail
cd "$(dirname "$0")/../.."
[ -f .env ] || { echo ".env is missing; run scripts/local/prepare.sh first" >&2; exit 1; }
set -a; . ./.env; set +a
RUN=.local/run
mkdir -p "$RUN" .local/logs
ref() { local value="${1#file:}"; echo "$PWD/$value"; }
port_of() { local url="${1%/}"; echo "${url##*:}"; }

start() {
  local name=$1; shift
  if [ -f "$RUN/$name.pid" ] && kill -0 "$(cat "$RUN/$name.pid")" 2>/dev/null; then
    echo "$name already running (pid $(cat "$RUN/$name.pid"))"; return
  fi
  nohup python3 "$@" > ".local/logs/$name.log" 2>&1 &
  echo $! > "$RUN/$name.pid"
  echo "$name started (pid $!, log .local/logs/$name.log)"
}

case "${1:-status}" in
  start)
    objects=.local/data/object-store
    mkdir -p "$objects/mako-function-bundles-v1" "$objects/mako-application-objects-v1"
    start object-store scripts/local/object-store-stub.py \
      "$(port_of "${MAKO_OBJECT_STORE_ENDPOINT:-http://127.0.0.1:8333}")" "$objects" \
      "$(ref "$MAKO_OBJECT_STORE_ACCESS_KEY_REF")" "$(ref "$MAKO_OBJECT_STORE_SECRET_KEY_REF")"
    start mail scripts/local/mail-sink.py \
      "${MAKO_DEVELOPER_SMTP_PORT:-1025}" 8025 .local/mail \
      "${MAKO_DEVELOPER_SMTP_USERNAME:-dev}" "$(ref "$MAKO_DEVELOPER_SMTP_PASSWORD_REF")"
    ;;
  stop)
    for name in object-store mail; do
      if [ -f "$RUN/$name.pid" ]; then kill "$(cat "$RUN/$name.pid")" 2>/dev/null || true; rm -f "$RUN/$name.pid"; echo "$name stopped"; fi
    done
    ;;
  status)
    for name in object-store mail; do
      if [ -f "$RUN/$name.pid" ] && kill -0 "$(cat "$RUN/$name.pid")" 2>/dev/null; then echo "$name running (pid $(cat "$RUN/$name.pid"))"; else echo "$name stopped"; fi
    done
    ;;
  *) echo "usage: $0 start | stop | status" >&2; exit 2 ;;
esac
