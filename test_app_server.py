#!/usr/bin/env python3
"""Test the codex/dsx app-server WebSocket protocol flow."""

import asyncio
import json
import subprocess
import sys
import time
import signal
import os
from datetime import datetime, timezone

DSX_BINARY = "/Users/bytedance/code/dsx/codex-rs/target/debug/dsx"
WS_URL = "ws://127.0.0.1:9020"
CAPTURE_SECONDS = 12  # a bit more than 10 to account for setup

def now_iso():
    return datetime.now(timezone.utc).strftime("%H:%M:%S.%f")[:-3]

def log(msg):
    print(f"[{now_iso()}] {msg}", flush=True)

async def wait_for_port(host, port, timeout=15):
    """Wait until the WebSocket port is accepting connections."""
    import socket
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            sock.settimeout(1)
            result = sock.connect_ex((host, port))
            sock.close()
            if result == 0:
                return True
        except Exception:
            pass
        await asyncio.sleep(0.3)
    return False

async def send(ws, msg_id, method, params):
    """Send a JSON-RPC message and log it."""
    payload = {
        "jsonrpc": "2.0",
        "id": msg_id,
        "method": method,
        "params": params,
    }
    raw = json.dumps(payload)
    log(f" >>> SEND [{msg_id}] {method}: {json.dumps(params, default=str)}")
    await ws.send(raw)
    return payload

async def receive_all(ws, duration, label):
    """Receive all messages for a given duration."""
    received = []
    deadline = time.time() + duration
    while time.time() < deadline:
        remaining = deadline - time.time()
        if remaining <= 0:
            break
        try:
            msg = await asyncio.wait_for(ws.recv(), timeout=min(remaining, 0.5))
            received.append(msg)
            try:
                parsed = json.loads(msg)
                # Log summary
                msg_id = parsed.get("id", "-")
                method = parsed.get("method", "")
                result = parsed.get("result")
                error = parsed.get("error")

                if method:
                    # Notification from server
                    params_summary = json.dumps(parsed.get("params", {}), default=str)[:300]
                    log(f" <<< RECV [{label}] notification: {method} params={params_summary}")
                elif error:
                    log(f" <<< RECV [{label}] ERROR [{msg_id}]: {json.dumps(error, default=str)}")
                elif result is not None:
                    result_summary = json.dumps(result, default=str)[:300]
                    log(f" <<< RECV [{label}] response [{msg_id}]: {result_summary}")
                else:
                    log(f" <<< RECV [{label}] raw: {msg[:300]}")
            except json.JSONDecodeError:
                log(f" <<< RECV [{label}] non-JSON: {msg[:200]}")
        except asyncio.TimeoutError:
            pass
        except Exception as e:
            log(f" <<< RECV [{label}] exception: {e}")
            break
    return received

async def run_flow(ws, label, thread_params, turn_input_text="hello"):
    """Run the initialize -> thread/start -> turn/start flow."""
    log(f"=== Starting flow [{label}] ===")

    msg_counter = [0]
    def next_id():
        msg_counter[0] += 1
        return f"{label}-{msg_counter[0]}"

    # Step 1: initialize
    init_params = {
        "clientInfo": {"name": "test", "version": "0.1"},
        "capabilities": {"experimentalApi": True},
        "protocolVersion": 1,
    }
    await send(ws, next_id(), "initialize", init_params)
    init_msgs = await receive_all(ws, 3, f"{label}-init")

    # Step 2: thread/start
    await send(ws, next_id(), "thread/start", thread_params)
    thread_msgs = await receive_all(ws, 3, f"{label}-thread")

    # Extract threadId from response (result.thread.id) or notification (params.thread.id)
    thread_id = None
    for m in thread_msgs:
        try:
            parsed = json.loads(m)
            # Response format: result.thread.id
            result = parsed.get("result", {})
            if isinstance(result, dict):
                thread = result.get("thread", {})
                if isinstance(thread, dict):
                    thread_id = thread.get("id") or thread_id
            # Notification format: params.thread.id
            params = parsed.get("params", {})
            if isinstance(params, dict):
                thread = params.get("thread", {})
                if isinstance(thread, dict):
                    thread_id = thread.get("id") or thread_id
        except Exception:
            pass

    # Also check init messages for threadId
    if not thread_id:
        for m in init_msgs:
            try:
                parsed = json.loads(m)
                for container_key in ["result", "params"]:
                    container = parsed.get(container_key, {})
                    if isinstance(container, dict):
                        thread = container.get("thread", {})
                        if isinstance(thread, dict):
                            thread_id = thread.get("id") or thread_id
            except Exception:
                pass

    log(f"[{label}] threadId discovered: {thread_id}")

    # Step 3: turn/start
    turn_params = {
        "threadId": thread_id or "unknown",
        "input": [{"type": "text", "text": turn_input_text, "textElements": []}],
    }
    await send(ws, next_id(), "turn/start", turn_params)

    # Capture for remaining time
    turn_msgs = await receive_all(ws, CAPTURE_SECONDS, f"{label}-turn")

    log(f"=== Flow [{label}] complete. Total msgs: init={len(init_msgs)}, thread={len(thread_msgs)}, turn={len(turn_msgs)} ===")

    return {
        "init": init_msgs,
        "thread": thread_msgs,
        "turn": turn_msgs,
        "threadId": thread_id,
    }

async def main():
    import websockets

    log("Starting dsx app-server...")

    # Start the server process
    proc = subprocess.Popen(
        [DSX_BINARY, "app-server", "--listen", WS_URL],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        preexec_fn=os.setsid,
    )

    try:
        # Wait for port
        log("Waiting for WebSocket port...")
        ready = await wait_for_port("127.0.0.1", 9020, timeout=15)
        if not ready:
            log("ERROR: Server did not become ready in time")
            # Print any stderr
            try:
                proc.wait(timeout=1)
                stderr = proc.stderr.read()
                if stderr:
                    log(f"stderr: {stderr[:2000]}")
            except Exception:
                pass
            return

        log("Server is ready!")

        # Give it a moment to fully initialize
        await asyncio.sleep(0.5)

        # ---- TEST 1: Basic flow (empty thread params) ----
        log("=" * 70)
        log("TEST 1: Basic flow with empty thread/start params")
        log("=" * 70)

        async with websockets.connect(WS_URL, max_size=None) as ws:
            result1 = await run_flow(ws, "T1", {})

        log("Disconnected. Pausing before test 2...")
        await asyncio.sleep(1)

        # ---- TEST 2: Flow with cwd in thread params ----
        log("=" * 70)
        log("TEST 2: Flow with cwd in thread/start params")
        log("=" * 70)

        async with websockets.connect(WS_URL, max_size=None) as ws:
            result2 = await run_flow(ws, "T2", {"cwd": "/Users/bytedance/code/dsx"})

        log("=" * 70)
        log("SUMMARY")
        log("=" * 70)
        log(f"Test 1 (empty params): threadId={result1['threadId']}, messages: init={len(result1['init'])}, thread={len(result1['thread'])}, turn={len(result1['turn'])}")
        log(f"Test 2 (with cwd):   threadId={result2['threadId']}, messages: init={len(result2['init'])}, thread={len(result2['thread'])}, turn={len(result2['turn'])}")

        # Print all raw messages for test 1
        log("")
        log("-" * 70)
        log("ALL RAW MESSAGES - TEST 1 (empty params)")
        log("-" * 70)
        for phase in ["init", "thread", "turn"]:
            log(f"--- {phase} ---")
            for i, msg in enumerate(result1[phase]):
                try:
                    parsed = json.loads(msg)
                    log(f"  [{i}] {json.dumps(parsed, indent=2, default=str)[:1000]}")
                except Exception:
                    log(f"  [{i}] {msg[:500]}")

        # Print all raw messages for test 2
        log("")
        log("-" * 70)
        log("ALL RAW MESSAGES - TEST 2 (with cwd)")
        log("-" * 70)
        for phase in ["init", "thread", "turn"]:
            log(f"--- {phase} ---")
            for i, msg in enumerate(result2[phase]):
                try:
                    parsed = json.loads(msg)
                    log(f"  [{i}] {json.dumps(parsed, indent=2, default=str)[:1000]}")
                except Exception:
                    log(f"  [{i}] {msg[:500]}")

        # Check for errors
        all_msgs = result1["init"] + result1["thread"] + result1["turn"] + result2["init"] + result2["thread"] + result2["turn"]
        errors = []
        for m in all_msgs:
            try:
                parsed = json.loads(m)
                if "error" in parsed:
                    errors.append(parsed)
            except Exception:
                pass

        if errors:
            log("")
            log("=" * 70)
            log(f"ERRORS FOUND: {len(errors)}")
            log("=" * 70)
            for e in errors:
                log(f"  {json.dumps(e, indent=2, default=str)[:500]}")
        else:
            log("")
            log("No errors detected in any messages.")

    except Exception as e:
        log(f"FATAL ERROR: {e}")
        import traceback
        traceback.print_exc()

    finally:
        log("Cleaning up server process...")
        try:
            os.killpg(os.getpgid(proc.pid), signal.SIGTERM)
        except Exception:
            proc.terminate()
        try:
            proc.wait(timeout=5)
        except Exception:
            proc.kill()
            proc.wait()

        # Print server stderr for debugging
        stderr = proc.stderr.read()
        if stderr:
            log(f"Server stderr ({len(stderr)} chars):")
            # Print last 3000 chars
            for line in stderr[-3000:].split("\n"):
                if line.strip():
                    log(f"  [stderr] {line[:200]}")

        stdout = proc.stdout.read()
        if stdout:
            log(f"Server stdout ({len(stdout)} chars):")
            for line in stdout[-2000:].split("\n"):
                if line.strip():
                    log(f"  [stdout] {line[:200]}")

if __name__ == "__main__":
    asyncio.run(main())