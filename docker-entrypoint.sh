#!/bin/sh
set -eu

CONFIG_DIR="${CONFIG_DIR:-/nox}"
INSTANCE="${INSTANCE:-}"
COUNT="${COUNT:-1}"
MOVEMENT="${MOVEMENT:-none}"
HUMAN="${HUMAN:-1}"
MODE="${MODE:-listen}"
LISTEN_ID="${LISTEN_ID:-0}"
PLAY_FILE="${PLAY_FILE:-}"

if [ -z "$INSTANCE" ]; then
    echo "INSTANCE is required (e.g. INSTANCE=ab195342@hactazia.fr)" >&2
    exit 1
fi

set -- /app/noxbot \
    --config-dir "$CONFIG_DIR" \
    --instance "$INSTANCE" \
    --count "$COUNT" \
    --movement "$MOVEMENT"

if [ "$HUMAN" = "1" ] || [ "$HUMAN" = "true" ]; then
    set -- "$@" --human
fi

case "$MODE" in
    play)
        if [ -z "$PLAY_FILE" ]; then
            echo "PLAY_FILE is required when MODE=play" >&2
            exit 1
        fi
        set -- "$@" --play "$PLAY_FILE"
        ;;
    listen)
        set -- "$@" --listen "$LISTEN_ID"
        ;;
    *)
        echo "MODE must be 'listen' or 'play' (got '$MODE')" >&2
        exit 1
        ;;
esac

exec "$@"
