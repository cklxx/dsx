import { ref, reactive, computed, watch } from "vue";
import { useRpc } from "./useRpc";

/**
 * High-level app state: threads, current thread, messages, turn state.
 * Wires together useRpc notifications into reactive Vue state.
 */
export function useApp() {
  const rpc = useRpc();

  // ── Thread list ──────────────────────────────────────────────
  const threads = ref([]);
  const currentThreadId = ref(null);
  const currentThreadName = ref(null);
  const currentCwd = ref(null);
  const currentModel = ref("");

  // ── Turn state ───────────────────────────────────────────────
  const isStreaming = ref(false);
  const currentTurnId = ref(null);

  // ── Messages (rendered items for the active thread) ──────────
  /** Each entry: { id, kind, role, content, status, ... } */
  const messages = ref([]);
  // Track DOM-relevant element refs by item ID
  const streamingItemId = ref(null);

  // ── UI state ─────────────────────────────────────────────────
  const wsUrl = ref(`ws://${location.hostname}:9021`);
  const searchTerm = ref("");

  // ── Computed ─────────────────────────────────────────────────
  const currentThread = computed(() =>
    threads.value.find((t) => t.id === currentThreadId.value),
  );

  const filteredThreads = computed(() => {
    const q = searchTerm.value.toLowerCase();
    if (!q) return threads.value;
    return threads.value.filter(
      (t) =>
        (t.name || "").toLowerCase().includes(q) ||
        (t.preview || "").toLowerCase().includes(q),
    );
  });

  const canSend = computed(
    () => rpc.connected.value && currentThreadId.value && !isStreaming.value,
  );

  // ── Thread operations ────────────────────────────────────────
  async function loadThreadList() {
    try {
      const r = await rpc.send("thread/list", {
        sortKey: "recency_at",
        sortDirection: "desc",
        limit: 50,
      });
      threads.value = r.data || [];
    } catch (e) {
      console.warn("thread/list failed:", e);
    }
  }

  async function newThread() {
    if (!rpc.connected.value) return;
    try {
      const r = await rpc.send("thread/start", {});
      if (r.thread) {
        currentThreadId.value = r.thread.id;
        currentCwd.value = r.cwd || r.thread.cwd || "";
        currentThreadName.value = r.thread.name || null;
        currentModel.value = r.model || "";
        messages.value = [];
        await loadThreadList();
      }
    } catch (e) {
      console.error("newThread failed:", e);
      throw e;
    }
  }

  async function switchThread(threadId) {
    if (!rpc.connected.value || threadId === currentThreadId.value) return;
    try {
      const r = await rpc.send("thread/resume", { threadId });
      currentThreadId.value = threadId;
      currentCwd.value = r.cwd || r.thread?.cwd || "";
      currentThreadName.value = r.thread?.name || null;
      currentModel.value = r.model || "";

      // Load history
      const read = await rpc.send("thread/read", {
        threadId,
        includeTurns: true,
      });
      messages.value = [];
      if (read.thread?.turns) {
        for (const turn of read.thread.turns) {
          renderHistoryItems(turn.items || []);
        }
      }
      await loadThreadList();
    } catch (e) {
      console.error("switchThread failed:", e);
    }
  }

  async function deleteThread(threadId) {
    try {
      await rpc.send("thread/delete", { threadId });
      if (threadId === currentThreadId.value) {
        currentThreadId.value = null;
        currentThreadName.value = null;
        messages.value = [];
      }
      await loadThreadList();
    } catch (e) {
      console.error("deleteThread failed:", e);
    }
  }

  async function renameThread(threadId, name) {
    try {
      await rpc.send("thread/name/set", { threadId, name });
      currentThreadName.value = name;
      await loadThreadList();
    } catch (e) {
      console.error("renameThread failed:", e);
    }
  }

  // ── Turn operations ──────────────────────────────────────────
  async function sendMessage(text) {
    if (!canSend.value || !text.trim()) return;
    const trimmed = text.trim();

    // Add user message to display immediately
    messages.value.push({
      id: `user-${Date.now()}`,
      kind: "userMessage",
      role: "user",
      text: trimmed,
    });

    try {
      await rpc.send("turn/start", {
        threadId: currentThreadId.value,
        input: [{ type: "text", text: trimmed, text_elements: [] }],
      });
    } catch (e) {
      console.error("turn/start failed:", e);
      throw e;
    }
  }

  async function interrupt() {
    if (!currentThreadId.value) return;
    if (!currentTurnId.value) {
      console.warn("interrupt: no turnId, skipping");
      return;
    }
    try {
      await rpc.send("turn/interrupt", {
        threadId: currentThreadId.value,
        turnId: currentTurnId.value,
      });
    } catch (e) {
      console.error("interrupt failed:", e);
    }
  }

  // ── History rendering (populate messages from thread/read) ───
  function renderHistoryItems(items) {
    for (const item of items) {
      const t = item.type;
      if (t === "userMessage") {
        const text = (item.content || [])
          .filter((c) => c.type === "text")
          .map((c) => c.text || "")
          .join("\n");
        if (text) {
          messages.value.push({
            id: item.id,
            kind: "userMessage",
            role: "user",
            text,
          });
        }
      } else if (t === "agentMessage" && item.text) {
        messages.value.push({
          id: item.id,
          kind: "agentMessage",
          role: "assistant",
          text: item.text,
          status: "done",
        });
      } else if (t === "reasoning") {
        const parts = [];
        if (item.summary?.length) parts.push(...item.summary);
        if (item.content?.length) parts.push(...item.content);
        const text = parts.join("\n\n");
        if (text) {
          messages.value.push({
            id: item.id,
            kind: "reasoning",
            role: "assistant",
            text,
            status: "done",
          });
        }
      } else if (t === "commandExecution") {
        messages.value.push({
          id: item.id,
          kind: "commandExecution",
          role: "assistant",
          title: item.command || "执行命令",
          command: item.command,
          exitCode: item.exitCode,
          output: item.aggregatedOutput || "",
          durationMs: item.durationMs,
          status:
            item.exitCode === 0
              ? "done"
              : item.exitCode != null
                ? "failed"
                : "done",
        });
      } else if (t === "fileChange") {
        const ok =
          !item.status ||
          item.status === "completed" ||
          item.status === "applied";
        messages.value.push({
          id: item.id,
          kind: "fileChange",
          role: "assistant",
          title: `${item.changes?.length || 0} 个文件`,
          changes: item.changes || [],
          status: ok ? "done" : "failed",
        });
      } else if (t === "webSearch") {
        messages.value.push({
          id: item.id,
          kind: "webSearch",
          role: "assistant",
          title: item.query || "搜索",
          query: item.query,
          status: "done",
        });
      } else if (t === "mcpToolCall") {
        messages.value.push({
          id: item.id,
          kind: "mcpToolCall",
          role: "assistant",
          title: `${item.server ? item.server + "/" : ""}${item.tool || ""}`,
          tool: item.tool,
          server: item.server,
          arguments: item.arguments,
          status: item.error ? "failed" : "done",
        });
      } else if (t === "plan") {
        messages.value.push({
          id: item.id,
          kind: "plan",
          role: "assistant",
          title: "规划",
          text: item.text || "",
          status: "done",
        });
      }
    }
  }

  // ── Notification wiring ──────────────────────────────────────
  function setupNotifications() {
    // Thread lifecycle
    rpc.on("thread/started", () => loadThreadList());
    rpc.on("thread/status/changed", (p) => {
      if (p.threadId !== currentThreadId.value) loadThreadList();
      else loadThreadList();
    });
    rpc.on("thread/name/updated", (p) => {
      if (p.threadId === currentThreadId.value && p.threadName) {
        currentThreadName.value = p.threadName;
      }
      loadThreadList();
    });

    // Turn lifecycle
    rpc.on("turn/started", (p) => {
      isStreaming.value = true;
      currentTurnId.value = p.turn?.id || null;
    });

    rpc.on("turn/completed", (p) => {
      isStreaming.value = false;
      currentTurnId.value = null;
      streamingItemId.value = null;
    });

    // Item lifecycle
    rpc.on("item/started", (p) => {
      const item = p.item;
      if (!item) return;
      handleItemStarted(item);
    });

    rpc.on("item/completed", (p) => {
      const item = p.item;
      if (!item) return;
      handleItemCompleted(item);
    });

    // Streaming deltas
    rpc.on("item/agentMessage/delta", (p) => {
      if (p.itemId !== streamingItemId.value) return;
      const msg = messages.value.find((m) => m.id === p.itemId);
      if (msg) {
        msg.text = (msg.text || "") + (p.delta || "");
      }
    });

    rpc.on("item/reasoning/textDelta", (p) => {
      const msg = messages.value.find((m) => m.id === p.itemId);
      if (msg) msg.text = (msg.text || "") + (p.delta || "");
    });

    rpc.on("item/reasoning/summaryTextDelta", (p) => {
      const msg = messages.value.find((m) => m.id === p.itemId);
      if (msg) msg.text = (msg.text || "") + (p.delta || "");
    });

    rpc.on("item/commandExecution/outputDelta", (p) => {
      const msg = messages.value.find((m) => m.id === p.itemId);
      if (msg) {
        let t = p.delta || "";
        try {
          t = atob(t);
        } catch (_) {}
        msg.output = (msg.output || "") + t;
      }
    });

    rpc.on("item/plan/delta", (p) => {
      const msg = messages.value.find((m) => m.id === p.itemId);
      if (msg) msg.text = (msg.text || "") + (p.delta || "");
    });

    rpc.on("item/fileChange/outputDelta", (p) => {
      // Could show patch diffs here; for now, ignore
    });

    // Errors & warnings
    rpc.on("error", (p) => {
      messages.value.push({
        id: `sys-${Date.now()}`,
        kind: "system",
        role: "system",
        text: p.error?.message || p.error || "未知错误",
        isError: true,
      });
      isStreaming.value = false;
    });

    rpc.on("warning", (p) => {
      messages.value.push({
        id: `sys-${Date.now()}`,
        kind: "system",
        role: "system",
        text: "⚠ " + (p.message || ""),
      });
    });
  }

  function handleItemStarted(item) {
    const t = item.type;

    if (t === "agentMessage") {
      const msg = {
        id: item.id,
        kind: "agentMessage",
        role: "assistant",
        text: "",
        status: "streaming",
      };
      messages.value.push(msg);
      streamingItemId.value = item.id;
      return;
    }

    if (t === "reasoning") {
      messages.value.push({
        id: item.id,
        kind: "reasoning",
        role: "assistant",
        text: "",
        status: "streaming",
      });
      return;
    }

    // Tool-like items
    const toolMeta = toolMetaFor(item);
    if (toolMeta) {
      messages.value.push({
        id: item.id,
        kind: t,
        role: "assistant",
        ...toolMeta,
        status: "running",
      });
      return;
    }

    if (t === "contextCompaction") {
      messages.value.push({
        id: item.id || `sys-${Date.now()}`,
        kind: "system",
        role: "system",
        text: "📦 上下文已压缩",
      });
    }
  }

  function handleItemCompleted(item) {
    const t = item.type;
    const msg = messages.value.find((m) => m.id === item.id);
    if (!msg) return;

    msg.status = "done";

    if (t === "agentMessage") {
      if (item.text) msg.text = item.text;
      streamingItemId.value = null;
    } else if (t === "reasoning") {
      const parts = [];
      if (item.summary?.length) parts.push(...item.summary);
      if (item.content?.length) parts.push(...item.content);
      if (parts.length) msg.text = parts.join("\n\n");
    } else if (t === "commandExecution") {
      const ok = item.exitCode === 0;
      msg.status = ok ? "done" : item.exitCode != null ? "failed" : "done";
      msg.exitCode = item.exitCode;
      if (item.command) {
        msg.title =
          item.command +
          (item.durationMs
            ? `  (${(item.durationMs / 1000).toFixed(1)}s)`
            : "");
        msg.command = item.command;
      }
      if (item.aggregatedOutput) msg.output = item.aggregatedOutput;
    } else if (t === "fileChange") {
      const ok =
        !item.status ||
        item.status === "completed" ||
        item.status === "applied";
      msg.status = ok ? "done" : "failed";
      msg.changes = item.changes || [];
      msg.title = `${item.changes?.length || 0} 个文件`;
    } else if (t === "mcpToolCall") {
      msg.status = item.error ? "failed" : "done";
    } else if (t === "webSearch") {
      msg.status = "done";
    } else if (t === "plan") {
      msg.status = "done";
      if (item.text) msg.text = item.text;
    }
  }

  function toolMetaFor(item) {
    const t = item.type;
    if (t === "commandExecution")
      return {
        title: item.command || "执行命令",
        command: item.command,
        output: "",
        exitCode: null,
      };
    if (t === "fileChange")
      return {
        title: `${item.changes?.length || 0} 个文件`,
        changes: item.changes || [],
      };
    if (t === "webSearch")
      return {
        title: item.query || "搜索",
        query: item.query,
      };
    if (t === "mcpToolCall")
      return {
        title: `${item.server ? item.server + "/" : ""}${item.tool || ""}`,
        tool: item.tool,
        server: item.server,
        arguments: item.arguments,
      };
    if (t === "dynamicToolCall")
      return {
        title: `${item.namespace ? item.namespace + "/" : ""}${item.tool || ""}`,
        tool: item.tool,
        namespace: item.namespace,
        arguments: item.arguments,
      };
    if (t === "plan")
      return {
        title: "规划",
        text: item.text || "",
      };
    if (t === "subAgentActivity")
      return {
        title: `${item.kind || ""} → ${item.agentPath || ""}`,
      };
    return null;
  }

  // ── Connection helpers ───────────────────────────────────────
  async function connectAndInit() {
    return new Promise((resolve, reject) => {
      const offConnected = rpc.on("connected", async () => {
        offConnected();
        await loadThreadList();
        // Auto-create a thread if none exist
        if (!currentThreadId.value && threads.value.length === 0) {
          try {
            await newThread();
          } catch (_) {}
        }
        resolve();
      });
      const offError = rpc.on("error", () => {
        offError();
        reject(new Error("连接失败"));
      });
      rpc.connect(wsUrl.value);
      // Safety timeout
      setTimeout(() => {
        if (!rpc.connected.value) reject(new Error("连接超时"));
      }, 10000);
    });
  }

  // Initialize notification listeners immediately
  setupNotifications();

  return {
    // RPC
    rpc,
    // Thread state
    threads,
    currentThreadId,
    currentThreadName,
    currentCwd,
    currentModel,
    currentThread,
    filteredThreads,
    // Turn state
    isStreaming,
    currentTurnId,
    streamingItemId,
    // Messages
    messages,
    // UI state
    wsUrl,
    searchTerm,
    canSend,
    // Actions
    connectAndInit,
    loadThreadList,
    newThread,
    switchThread,
    deleteThread,
    renameThread,
    sendMessage,
    interrupt,
  };
}
