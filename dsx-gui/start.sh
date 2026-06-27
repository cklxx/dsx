#!/bin/bash
# dsx GUI launcher — starts app-server, serves GUI, opens browser
set -e

APP_PORT="${DSX_GUI_APP_PORT:-9020}"
GUI_PORT="${DSX_GUI_PORT:-9021}"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

echo "=== dsx GUI ==="
echo "  app-server:  ws://127.0.0.1:$APP_PORT"
echo "  gui:         http://127.0.0.1:$GUI_PORT"
echo ""

# Kill old processes
lsof -ti:"$APP_PORT" 2>/dev/null | xargs kill -9 2>/dev/null || true
lsof -ti:"$GUI_PORT" 2>/dev/null | xargs kill -9 2>/dev/null || true

# Start app-server
echo "Starting app-server..."
codex app-server --listen "ws://127.0.0.1:$APP_PORT" &
APP_PID=$!

# Wait for app-server
for i in $(seq 1 30); do
  if curl -s -o /dev/null "http://127.0.0.1:$APP_PORT/readyz" 2>/dev/null; then
    break
  fi
  sleep 0.3
done
echo "App-server ready (pid $APP_PID)"

# Update gui.html with the right port
sed "s|ws://localhost:9020|ws://localhost:$APP_PORT|g" "$SCRIPT_DIR/gui.html" > /tmp/dsx-gui.html

# Serve the HTML (tiny Python HTTP server)
echo "Serving GUI..."
python3 -c "
import http.server, os, urllib.parse, signal, sys

class Handler(http.server.SimpleHTTPRequestHandler):
    def do_GET(self):
        path = urllib.parse.urlparse(self.path).path
        if path == '/' or path == '/index.html':
            self.send_response(200)
            self.send_header('Content-Type','text/html; charset=utf-8')
            self.end_headers()
            with open('/tmp/dsx-gui.html','rb') as f:
                self.wfile.write(f.read())
        else:
            super().do_GET()

    def log_message(self, fmt, *args):
        pass  # silent

signal.signal(signal.SIGINT, lambda *_: sys.exit(0))
http.server.HTTPServer(('127.0.0.1',$GUI_PORT), Handler).serve_forever()
" &
GUI_PID=$!
sleep 0.5

# Open browser
URL="http://127.0.0.1:$GUI_PORT"
if command -v open &>/dev/null; then open "$URL"
elif command -v xdg-open &>/dev/null; then xdg-open "$URL"
elif command -v start &>/dev/null; then start "$URL"
fi

echo "Press Ctrl+C to stop"
cleanup() { kill $APP_PID $GUI_PID 2>/dev/null; rm -f /tmp/dsx-gui.html; exit 0; }
trap cleanup INT TERM
wait
