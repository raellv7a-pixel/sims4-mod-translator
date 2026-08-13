#!/bin/bash
set -e

CARGO="$HOME/.cargo/bin/cargo"
APP_DIR="/home/raell/sims4-mod-translator"

if [ ! -f "$CARGO" ]; then
    echo "Cargo não encontrado em $CARGO"
    exit 1
fi

export QT_QPA_PLATFORM="xcb;wayland"

cd "$APP_DIR"
"$CARGO" run --release
