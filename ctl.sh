#!/usr/bin/env bash
# aidash control: start | stop | restart | status | logs
set -u

APP="aidash"
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN="$DIR/target/release/aidash"
SESSION="AI-Dashboard"
LOGFILE="$DIR/aidash.log"

# config (override via env)
: "${AIDASH_INTERVAL:=120}"
: "${AIDASH_LISTEN:=0.0.0.0:8000}"

running() {
  tmux has-session -t "$SESSION" 2>/dev/null
}

cmd_start() {
  if running; then
    echo "$APP already running (tmux session: $SESSION)"
    return 0
  fi
  echo "starting $APP in tmux session '$SESSION'..."
  cd "$DIR"
  tmux new-session -d -s "$SESSION" \
    "AIDASH_INTERVAL=$AIDASH_INTERVAL AIDASH_LISTEN=$AIDASH_LISTEN $BIN 2>&1 | tee $LOGFILE"
  sleep 1
  if running; then
    echo "started (session: $SESSION) on $AIDASH_LISTEN, interval ${AIDASH_INTERVAL}s"
    echo "attach: tmux attach -t $SESSION  (detach: Ctrl+B D)"
  else
    echo "failed to start — check $LOGFILE"
    return 1
  fi
}

cmd_stop() {
  if ! running; then
    echo "$APP not running"
    return 0
  fi
  echo "stopping $APP (killing tmux session '$SESSION')..."
  tmux kill-session -t "$SESSION"
  echo "stopped"
}

cmd_status() {
  if running; then
    echo "$APP running (tmux session: $SESSION) on $AIDASH_LISTEN"
  else
    echo "$APP not running"
    return 1
  fi
}

cmd_attach() {
  if ! running; then
    echo "$APP not running"
    return 1
  fi
  tmux attach -t "$SESSION"
}

cmd_logs() {
  [ -f "$LOGFILE" ] && tail -n 50 "$LOGFILE" || echo "no log file"
}

case "${1:-}" in
  start)   cmd_start ;;
  stop)    cmd_stop ;;
  restart) cmd_stop; cmd_start ;;
  status)  cmd_status ;;
  attach)  cmd_attach ;;
  logs)    cmd_logs ;;
  *)
    echo "usage: $0 {start|stop|restart|status|attach|logs}"
    echo "env: AIDASH_INTERVAL (default 120), AIDASH_LISTEN (default 0.0.0.0:8000)"
    exit 2
    ;;
esac
