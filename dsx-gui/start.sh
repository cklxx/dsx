#!/bin/bash
# dsx GUI launcher — starts app-server, builds Vue app, serves GUI (with WS proxy), opens browser
set -e

APP_PORT="${DSX_GUI_APP_PORT:-9020}"
GUI_PORT="${DSX_GUI_PORT:-9021}"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

echo "╭─────────────────────────────────────╮"
echo "│  dsx GUI                            │"
echo "│  app-server:  ws://127.0.0.1:$APP_PORT  │"
echo "│  gui:         http://127.0.0.1:$GUI_PORT  │"
echo "╰─────────────────────────────────────╯"
echo ""

# Kill old processes
echo "→ 清理旧进程..."
lsof -ti:"$APP_PORT" 2>/dev/null | xargs kill -9 2>/dev/null || true
lsof -ti:"$GUI_PORT" 2>/dev/null | xargs kill -9 2>/dev/null || true
sleep 0.3

# Find binary
if command -v dsx &>/dev/null; then
  BIN=dsx
elif command -v codex &>/dev/null; then
  BIN=codex
else
  echo "✗ 找不到 codex/dsx 可执行文件"
  exit 1
fi

# Start app-server
echo "→ 启动 app-server ($BIN)..."
$BIN app-server --listen "ws://127.0.0.1:$APP_PORT" &
APP_PID=$!

# Wait for app-server
echo "→ 等待 app-server..."
READY=0
for i in $(seq 1 40); do
  if curl -s -o /dev/null "http://127.0.0.1:$APP_PORT/readyz" 2>/dev/null; then
    READY=1
    break
  fi
  if lsof -ti:"$APP_PORT" &>/dev/null; then
    sleep 0.5
    READY=1
    break
  fi
  sleep 0.3
done

if [ "$READY" -eq 0 ]; then
  echo "✗ app-server 启动失败"
  kill $APP_PID 2>/dev/null || true
  exit 1
fi
echo "✓ app-server 就绪 (pid $APP_PID)"

# Build Vue app
echo "→ 构建前端..."
cd "$SCRIPT_DIR"
if [ ! -d "node_modules" ]; then
  echo "  安装依赖 (npm install)..."
  npm install --silent 2>&1 | tail -3 || true
fi
npm run build 2>&1 | tail -5
echo "✓ 前端构建完成 (dist/)"

# Start the combined HTTP + WebSocket proxy server
echo "→ 启动 GUI + WS 代理服务器..."
DSX_GUI_APP_PORT="$APP_PORT" DSX_GUI_PORT="$GUI_PORT" DSX_GUI_DIST="$SCRIPT_DIR/dist" \
  python3 "$SCRIPT_DIR/server.py" &
GUI_PID=$!
sleep 0.8

# Verify GUI is up
if ! curl -s -o /dev/null "http://127.0.0.1:$GUI_PORT/" 2>/dev/null; then
  echo "✗ GUI 服务器启动失败"
  kill $APP_PID $GUI_PID 2>/dev/null || true
  exit 1
fi

URL="http://127.0.0.1:$GUI_PORT"
echo "✓ 全部就绪: $URL"
echo ""

# Open browser
if command -v open &>/dev/null; then
  open "$URL"
elif command -v xdg-open &>/dev/null; then
  xdg-open "$URL"
elif command -v start &>/dev/null; then
  start "$URL"
fi

echo "按 Ctrl+C 停止所有服务"
echo ""

cleanup() {
  echo ""
  echo "→ 停止服务..."
  kill $APP_PID $GUI_PID 2>/dev/null || true
  exit 0
}
trap cleanup INT TERM
wait
