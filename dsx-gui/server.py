#!/usr/bin/env python3
"""dsx GUI server — serves Vue app static files + proxies WebSocket to app-server on one port."""
import http.server
import mimetypes
import os
import secrets
import socket
import socketserver
import sys
import threading
import urllib.parse

APP_PORT = int(os.environ.get("DSX_GUI_APP_PORT", "9020"))
GUI_PORT = int(os.environ.get("DSX_GUI_PORT", "9021"))
GUI_TOKEN = os.environ.get("DSX_GUI_TOKEN") or secrets.token_urlsafe(32)
DIST_DIR = os.environ.get("DSX_GUI_DIST", os.path.join(os.path.dirname(__file__), "dist"))


def serve_static_file(handler, rel_path):
    """Serve a file from DIST_DIR. Returns True if file was served, False if 404."""
    # Default to index.html for root
    if rel_path in ("", "/", "/index.html"):
        file_path = os.path.join(DIST_DIR, "index.html")
    else:
        # Strip leading slash
        clean = rel_path.lstrip("/")
        file_path = os.path.join(DIST_DIR, clean)

    # Prevent path traversal
    if not os.path.realpath(file_path).startswith(os.path.realpath(DIST_DIR)):
        handler.send_response(403)
        handler.end_headers()
        return True

    if not os.path.isfile(file_path):
        return False

    try:
        with open(file_path, "rb") as f:
            content = f.read()

        content_type, _ = mimetypes.guess_type(file_path)
        if content_type is None:
            content_type = "application/octet-stream"

        handler.send_response(200)
        handler.send_header("Content-Type", content_type)
        handler.send_header("Content-Length", str(len(content)))
        # Cache assets, but never index.html
        if "index.html" in file_path:
            handler.send_header("Cache-Control", "no-cache")
        else:
            handler.send_header("Cache-Control", "public, max-age=3600")
        handler.end_headers()
        handler.wfile.write(content)
    except Exception as e:
        handler.send_response(500)
        handler.end_headers()
        handler.wfile.write(f"Error: {e}".encode())
    return True


class ProxyHTTPRequestHandler(http.server.BaseHTTPRequestHandler):
    """Handles HTTP requests (serve static files) and WebSocket upgrades (proxy to app-server)."""

    def do_GET(self):
        # Check if this is a WebSocket upgrade request
        if self.headers.get("Upgrade", "").lower() == "websocket":
            self._proxy_websocket()
            return

        path = urllib.parse.urlparse(self.path).path

        # Health checks → proxy to app-server
        if path in ("/readyz", "/healthz"):
            self._proxy_http(path)
            return

        # Serve static files from dist/
        if serve_static_file(self, path):
            return

        # SPA fallback: serve index.html for any unknown route
        if not any(path.endswith(ext) for ext in (".js", ".css", ".png", ".svg", ".ico", ".map")):
            if serve_static_file(self, "/index.html"):
                return

        self.send_response(404)
        self.end_headers()

    def do_POST(self):
        self._proxy_http(self.path)

    def do_PUT(self):
        self._proxy_http(self.path)

    def _proxy_http(self, path):
        """Proxy a regular HTTP request to the app-server."""
        try:
            content_len = int(self.headers.get("Content-Length", 0))
            body = self.rfile.read(content_len) if content_len else b""

            req = f"{self.command} {path} HTTP/1.1\r\n"
            req += f"Host: 127.0.0.1:{APP_PORT}\r\n"
            req += "Connection: close\r\n"
            for h in ("Content-Type", "Content-Length", "Accept"):
                v = self.headers.get(h)
                if v:
                    req += f"{h}: {v}\r\n"
            req += "\r\n"

            s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            s.settimeout(5)
            s.connect(("127.0.0.1", APP_PORT))
            s.sendall(req.encode() + body)

            resp = b""
            while True:
                chunk = s.recv(4096)
                if not chunk:
                    break
                resp += chunk
            s.close()

            lines = resp.split(b"\r\n")
            if lines:
                parts = lines[0].split(b" ", 2)
                if len(parts) >= 2:
                    status = int(parts[1])
                else:
                    status = 200
            else:
                status = 200

            body_start = resp.find(b"\r\n\r\n")
            resp_body = resp[body_start + 4:] if body_start >= 0 else b""

            self.send_response(status)
            self.send_header("Content-Type", "text/plain")
            self.send_header("Content-Length", str(len(resp_body)))
            self.end_headers()
            self.wfile.write(resp_body)
        except Exception:
            try:
                self.send_response(502)
                self.end_headers()
            except:
                pass

    def _proxy_websocket(self):
        """Proxy an authenticated same-origin WebSocket connection to app-server."""
        query = urllib.parse.parse_qs(urllib.parse.urlparse(self.path).query)
        origin = self.headers.get("Origin")
        expected_origin = f"http://127.0.0.1:{GUI_PORT}"
        if origin != expected_origin or not secrets.compare_digest(
            query.get("token", [""])[0], GUI_TOKEN
        ):
            self.send_response(403)
            self.end_headers()
            return

        try:
            app_sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            app_sock.settimeout(10)
            app_sock.connect(("127.0.0.1", APP_PORT))

            # The app-server rejects browser Origin headers; authentication is enforced above.
            req = f"GET {self.path} HTTP/1.1\r\n"
            req += f"Host: 127.0.0.1:{APP_PORT}\r\n"
            req += "Upgrade: websocket\r\n"
            req += "Connection: Upgrade\r\n"

            ws_key = self.headers.get("Sec-WebSocket-Key", "")
            ws_version = self.headers.get("Sec-WebSocket-Version", "13")
            ws_protocol = self.headers.get("Sec-WebSocket-Protocol", "")
            ws_extensions = self.headers.get("Sec-WebSocket-Extensions", "")

            if ws_key:
                req += f"Sec-WebSocket-Key: {ws_key}\r\n"
            req += f"Sec-WebSocket-Version: {ws_version}\r\n"
            if ws_protocol:
                req += f"Sec-WebSocket-Protocol: {ws_protocol}\r\n"
            if ws_extensions:
                req += f"Sec-WebSocket-Extensions: {ws_extensions}\r\n"

            req += "\r\n"
            app_sock.sendall(req.encode())

            resp = b""
            while b"\r\n\r\n" not in resp:
                chunk = app_sock.recv(4096)
                if not chunk:
                    app_sock.close()
                    return
                resp += chunk

            self.connection.sendall(resp)

            client_sock = self.connection
            client_sock.setblocking(False)
            app_sock.setblocking(False)

            import select
            while True:
                rlist, _, xlist = select.select([client_sock, app_sock], [], [client_sock, app_sock], 30)
                if xlist:
                    break

                closed = False
                for sock in rlist:
                    try:
                        data = sock.recv(65536)
                        if not data:
                            closed = True
                            break
                        other = app_sock if sock is client_sock else client_sock
                        other.sendall(data)
                    except (BlockingIOError, InterruptedError):
                        continue
                    except (ConnectionResetError, BrokenPipeError, OSError):
                        closed = True
                        break

                if closed:
                    break

            try:
                client_sock.close()
            except:
                pass
            try:
                app_sock.close()
            except:
                pass

        except Exception:
            try:
                self.send_response(502)
                self.end_headers()
            except:
                pass
            try:
                app_sock.close()
            except:
                pass

    def log_message(self, format, *args):
        pass  # silent


class ThreadedTCPServer(socketserver.ThreadingMixIn, socketserver.TCPServer):
    allow_reuse_address = True
    daemon_threads = True


def main():
    port = GUI_PORT
    server = ThreadedTCPServer(("127.0.0.1", port), ProxyHTTPRequestHandler)
    print(f"  dsx GUI: http://127.0.0.1:{port}/?token={GUI_TOKEN}")
    print(f"  (serving {DIST_DIR}, proxies WS → app-server port {APP_PORT})")
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.shutdown()


if __name__ == "__main__":
    main()
