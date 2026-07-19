import { ref, reactive } from "vue";

/**
 * WebSocket JSON-RPC client for the dsx app-server protocol (v2).
 *
 * Protocol conventions:
 *   - All structs use camelCase serialization (threadId, itemId, etc.)
 *   - ThreadItem enum variants use camelCase type tags (agentMessage, commandExecution, ...)
 *   - Notifications: { method, params }
 *   - Responses:       { id, result }  or  { id, error }
 *   - Server requests: { id, method, params }  → client responds with { id, result }
 */
export function useRpc() {
  const ws = ref(null);
  const connected = ref(false);
  const connecting = ref(false);
  const reqId = ref(0);
  const pending = new Map(); // id → { resolve, reject, timer }

  // Notification subscribers — keyed by method name, value is Set of callbacks
  const subscribers = new Map();

  function on(method, cb) {
    if (!subscribers.has(method)) subscribers.set(method, new Set());
    subscribers.get(method).add(cb);
    return () => subscribers.get(method)?.delete(cb);
  }

  function emit(method, params) {
    subscribers.get(method)?.forEach((cb) => cb(params));
    // Also emit a wildcard for global listeners
    subscribers.get("*")?.forEach((cb) => cb(method, params));
  }

  function connect(url) {
    if (ws.value) ws.value.close();
    connecting.value = true;

    const socket = new WebSocket(url);
    ws.value = socket;

    socket.onopen = () => {
      // Initialize the session
      send("initialize", {
        clientInfo: { name: "dsx-gui", version: "0.3.0" },
        capabilities: { experimentalApi: true, optOutNotificationMethods: [] },
      })
        .then(() => {
          connected.value = true;
          connecting.value = false;
          emit("connected");
        })
        .catch((err) => {
          console.error("initialize failed:", err);
          connecting.value = false;
          socket.close();
        });
    };

    socket.onclose = () => {
      connected.value = false;
      connecting.value = false;
      ws.value = null;
      // Reject all pending requests
      pending.forEach(({ reject }) => reject(new Error("连接已关闭")));
      pending.clear();
      emit("disconnected");
    };

    socket.onerror = () => {
      connecting.value = false;
      emit("error");
    };

    socket.onmessage = (e) => {
      let msg;
      try {
        msg = JSON.parse(e.data);
      } catch {
        return;
      }

      // Response to our request
      if (msg.id !== undefined && !msg.method) {
        const p = pending.get(msg.id);
        if (p) {
          clearTimeout(p.timer);
          pending.delete(msg.id);
          if (msg.error) p.reject(msg.error);
          else p.resolve(msg.result);
        }
        return;
      }

      // Server-to-client request (approval prompts, etc.)
      if (msg.method && msg.id !== undefined) {
        handleServerRequest(socket, msg);
        return;
      }

      // Notification
      if (msg.method) {
        emit(msg.method, msg.params || {});
      }
    };
  }

  function disconnect() {
    ws.value?.close();
  }

  function send(method, params, timeoutMs = 30000) {
    if (!ws.value || ws.value.readyState !== WebSocket.OPEN) {
      return Promise.reject(new Error("未连接"));
    }
    const id = ++reqId.value;
    ws.value.send(JSON.stringify({ id, method, params }));

    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        if (pending.has(id)) {
          pending.delete(id);
          reject(new Error(`超时: ${method}`));
        }
      }, timeoutMs);
      pending.set(id, { resolve, reject, timer });
    });
  }

  /** Decline privileged requests until the GUI has an explicit approval prompt. */
  function handleServerRequest(socket, msg) {
    const m = msg.method;
    if (
      m === "item/commandExecution/requestApproval" ||
      m === "item/fileChange/requestApproval"
    ) {
      socket.send(
        JSON.stringify({ id: msg.id, result: { decision: "decline" } }),
      );
    } else if (m === "item/permissions/requestApproval") {
      socket.send(JSON.stringify({ id: msg.id, result: { accept: false } }));
    } else if (m === "currentTime/read") {
      socket.send(
        JSON.stringify({
          id: msg.id,
          result: { isoTime: new Date().toISOString() },
        }),
      );
    } else {
      socket.send(JSON.stringify({ id: msg.id, result: {} }));
    }
    emit("server-request", msg);
  }

  return {
    // State
    ws,
    connected,
    connecting,
    // Methods
    connect,
    disconnect,
    send,
    on,
  };
}
