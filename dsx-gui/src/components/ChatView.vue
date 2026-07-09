<script setup>
import { inject, ref, watch, nextTick, computed } from 'vue'
import { NScrollbar } from 'naive-ui'
import MessageGroup from './MessageGroup.vue'
import ToolGroup from './ToolGroup.vue'
import Composer from './Composer.vue'

const app = inject('app')
const scrollRef = ref(null)

function scrollToBottom(smooth = true) {
  nextTick(() => {
    if (scrollRef.value) {
      scrollRef.value.scrollTo({ top: 999999, behavior: smooth ? 'smooth' : 'auto' })
    }
  })
}

watch(() => app.messages.value.length, () => scrollToBottom(true))
watch(
  () => {
    const last = app.messages.value[app.messages.value.length - 1]
    return last?.text?.length || 0
  },
  () => scrollToBottom(false)
)
watch(() => app.isStreaming.value, (val) => { if (val) scrollToBottom(true) })

const TOOL_KINDS = ['commandExecution', 'fileChange', 'webSearch', 'mcpToolCall', 'dynamicToolCall', 'plan', 'subAgentActivity']

function isTool(msg) {
  return TOOL_KINDS.includes(msg.kind)
}

/**
 * Group messages into render blocks:
 *   - Single non-tool message → { type: 'message', msg }
 *   - Consecutive tool messages → { type: 'toolGroup', tools: [...] }
 */
const renderBlocks = computed(() => {
  const blocks = []
  let i = 0
  const msgs = app.messages.value

  while (i < msgs.length) {
    if (isTool(msgs[i])) {
      // Collect consecutive tool items
      const group = []
      while (i < msgs.length && isTool(msgs[i])) {
        group.push(msgs[i])
        i++
      }
      blocks.push({ type: 'toolGroup', id: `tg-${group[0].id}`, tools: group })
    } else {
      blocks.push({ type: 'message', id: msgs[i].id, msg: msgs[i] })
      i++
    }
  }
  return blocks
})

const showWelcome = computed(() =>
  !app.messages.value.length && !app.isStreaming.value && app.rpc.connected.value
)
</script>

<template>
  <section class="chat-view">
    <div class="messages-container">
      <NScrollbar ref="scrollRef" class="messages-scrollbar">
        <div class="messages-inner">
          <!-- Welcome -->
          <div v-if="showWelcome" class="welcome-area">
            <div class="welcome-icon">⚡</div>
            <div class="welcome-title">dsx</div>
            <div class="welcome-desc">DeepSeek V4 agent</div>
            <div class="welcome-hints">
              <div class="hint-item"><span class="hint-key">Enter</span><span class="hint-label">发送</span></div>
              <div class="hint-item"><span class="hint-key">Shift+Enter</span><span class="hint-label">换行</span></div>
              <div class="hint-item"><span class="hint-key">Esc</span><span class="hint-label">中断</span></div>
            </div>
          </div>

          <!-- Render blocks -->
          <template v-for="block in renderBlocks" :key="block.id">
            <MessageGroup
              v-if="block.type === 'message'"
              :msg="block.msg"
              :is-streaming="app.streamingItemId.value === block.msg.id"
            />
            <ToolGroup
              v-else
              :tools="block.tools"
            />
          </template>

          <!-- Typing indicator -->
          <div v-if="app.isStreaming.value && !app.streamingItemId.value" class="typing-row">
            <div class="typing-dots"><span></span><span></span><span></span></div>
          </div>
        </div>
      </NScrollbar>
    </div>

    <Composer />
  </section>
</template>

<style scoped>
.chat-view {
  flex: 1;
  display: flex;
  flex-direction: column;
  min-width: 0;
  height: 100%;
  background: #0a0a0b;
}
.messages-container {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
}
.messages-scrollbar {
  flex: 1;
  min-height: 0;
}
.messages-inner {
  max-width: 780px;
  margin: 0 auto;
  padding: 28px 36px 20px;
}

/* Welcome */
.welcome-area {
  display: flex;
  flex-direction: column;
  align-items: center;
  padding: 72px 0 56px;
}
.welcome-icon { font-size: 40px; margin-bottom: 14px; opacity: 0.85; line-height: 1; }
.welcome-title { font-size: 22px; font-weight: 600; color: #e8e8ea; margin-bottom: 4px; }
.welcome-desc { font-size: 12px; color: #6b6b74; margin-bottom: 28px; }
.welcome-hints { display: flex; gap: 20px; }
.hint-item { display: flex; align-items: center; gap: 6px; }
.hint-key {
  font-family: var(--font-mono); font-size: 10px; padding: 2px 7px;
  background: #151518; border: 1px solid #26262c; border-radius: 4px; color: #9b9ba3;
}
.hint-label { font-size: 11px; color: #6b6b74; }

/* Typing */
.typing-row {
  display: flex;
  padding: 4px 0 12px;
}
.typing-dots {
  display: inline-flex;
  gap: 5px;
  padding: 10px 0;
}
.typing-dots span {
  width: 5px;
  height: 5px;
  border-radius: 50%;
  background: #52525a;
  animation: typing-bounce 1.3s infinite ease-in-out;
}
.typing-dots span:nth-child(2) { animation-delay: 0.15s; }
.typing-dots span:nth-child(3) { animation-delay: 0.3s; }
@keyframes typing-bounce {
  0%, 60%, 100% { transform: translateY(0); opacity: 0.4; }
  30% { transform: translateY(-4px); opacity: 1; }
}
</style>
