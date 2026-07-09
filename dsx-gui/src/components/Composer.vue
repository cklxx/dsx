<script setup>
import { inject, ref, watch, nextTick, onMounted, onUnmounted } from 'vue'
import { NButton, NTooltip } from 'naive-ui'

const app = inject('app')
const textarea = ref(null)
const inputText = ref('')
const sending = ref(false)

function autoResize() {
  const el = textarea.value
  if (!el) return
  el.style.height = 'auto'
  el.style.height = Math.min(el.scrollHeight, 220) + 'px'
}

watch(inputText, () => autoResize())

async function handleSend() {
  const text = inputText.value.trim()
  if (!text || sending.value) return

  if (app.isStreaming.value) {
    await app.interrupt()
    return
  }

  if (!app.canSend.value) {
    if (!app.currentThreadId.value && app.rpc.connected.value) {
      try { await app.newThread() } catch { return }
    } else {
      return
    }
  }

  sending.value = true
  try {
    await app.sendMessage(text)
    inputText.value = ''
    nextTick(() => autoResize())
  } finally {
    sending.value = false
  }
}

function handleKeydown(e) {
  if (e.key === 'Escape' && app.isStreaming.value) {
    e.preventDefault()
    app.interrupt()
    return
  }
  if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) {
    e.preventDefault()
    handleSend()
  }
}

function handleInterrupt() {
  app.interrupt()
}

// Global Esc handler
function onKey(e) {
  if (e.key === 'Escape' && app.isStreaming.value) {
    app.interrupt()
  }
}

onMounted(() => {
  window.addEventListener('keydown', onKey)
  nextTick(() => autoResize())
})
onUnmounted(() => {
  window.removeEventListener('keydown', onKey)
})
</script>

<template>
  <div class="composer-area">
    <div class="composer-inner">
      <div
        class="composer-box"
        :class="{ focused: false, streaming: app.isStreaming.value }"
      >
        <textarea
          ref="textarea"
          v-model="inputText"
          class="composer-textarea"
          :placeholder="app.rpc.connected.value
            ? '输入消息…  (Enter 发送, Shift+Enter 换行)'
            : '未连接到 app-server'"
          :disabled="!app.rpc.connected.value"
          rows="1"
          @keydown="handleKeydown"
        ></textarea>

        <div class="composer-bottom">
          <!-- CWD tag -->
          <div class="composer-info">
            <span v-if="app.currentCwd.value" class="cwd-text">
              📁 {{ app.currentCwd.value }}
            </span>
          </div>

          <!-- Action buttons -->
          <div class="composer-actions">
            <!-- Interrupt button -->
            <NTooltip v-if="app.isStreaming.value" trigger="hover">
              <template #trigger>
                <button
                  class="action-btn interrupt-btn"
                  title="中断 (Esc)"
                  @click="handleInterrupt"
                >
                  <svg width="14" height="14" viewBox="0 0 14 14" fill="none">
                    <rect x="3" y="3" width="8" height="8" rx="2" fill="currentColor"/>
                  </svg>
                </button>
              </template>
              中断生成 (Esc)
            </NTooltip>

            <!-- Send button -->
            <NTooltip v-else trigger="hover">
              <template #trigger>
                <button
                  class="action-btn send-btn"
                  :class="{ active: inputText.trim() }"
                  :disabled="!inputText.trim() || !app.rpc.connected.value || sending"
                  @click="handleSend"
                >
                  <svg width="15" height="15" viewBox="0 0 15 15" fill="none">
                    <path d="M1.5 7.5h11M8 3l4.5 4.5L8 12"
                      stroke="currentColor" stroke-width="1.5"
                      stroke-linecap="round" stroke-linejoin="round"/>
                  </svg>
                </button>
              </template>
              发送 (Enter)
            </NTooltip>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.composer-area {
  padding: 12px 36px 18px;
  background: #0a0a0b;
  border-top: 1px solid #1a1a1e;
}

.composer-inner {
  max-width: 780px;
  margin: 0 auto;
}

.composer-box {
  background: #151518;
  border: 1px solid #26262c;
  border-radius: 12px;
  padding: 12px 14px 10px;
  transition: border-color 0.15s, box-shadow 0.15s;
}
.composer-box:focus-within {
  border-color: #3a3a44;
  box-shadow: 0 0 0 3px rgba(232, 232, 234, 0.04);
}

.composer-textarea {
  width: 100%;
  resize: none;
  border: none;
  outline: none;
  background: transparent;
  color: #e8e8ea;
  font-size: 13.5px;
  line-height: 1.65;
  font-family: var(--font);
  min-height: 24px;
  max-height: 220px;
  padding: 0;
  margin-bottom: 6px;
}
.composer-textarea::placeholder {
  color: #52525a;
}
.composer-textarea:disabled {
  opacity: 0.4;
  cursor: not-allowed;
}

/* ── Bottom bar ───────────────────────────────────────── */
.composer-bottom {
  display: flex;
  align-items: center;
  justify-content: space-between;
  min-height: 28px;
}

.composer-info {
  flex: 1;
  min-width: 0;
  overflow: hidden;
}

.cwd-text {
  font-size: 10px;
  color: #52525a;
  font-family: var(--font-mono);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  display: block;
}

/* ── Action buttons ───────────────────────────────────── */
.composer-actions {
  display: flex;
  align-items: center;
  gap: 6px;
  flex-shrink: 0;
}

.action-btn {
  width: 30px;
  height: 30px;
  border-radius: 7px;
  display: flex;
  align-items: center;
  justify-content: center;
  border: none;
  cursor: pointer;
  transition: background 0.15s, color 0.15s, opacity 0.15s;
}

.send-btn {
  background: #2a2a30;
  color: #9b9ba3;
}
.send-btn.active {
  background: #e8e8ea;
  color: #0a0a0b;
}
.send-btn.active:hover {
  background: #ffffff;
}
.send-btn:disabled {
  opacity: 0.35;
  cursor: not-allowed;
}

.interrupt-btn {
  background: rgba(248, 113, 113, 0.15);
  color: #f87171;
}
.interrupt-btn:hover {
  background: rgba(248, 113, 113, 0.25);
}
</style>
