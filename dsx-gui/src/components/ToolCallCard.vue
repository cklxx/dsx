<script setup>
import { ref, computed } from 'vue'
import { NTag, NTooltip } from 'naive-ui'

const props = defineProps({
  msg: { type: Object, required: true },
})

const expanded = ref(false)

const isRunning = computed(() => props.msg.status === 'running')
const isFailed = computed(() => props.msg.status === 'failed')
const isDone = computed(() => props.msg.status === 'done')

const statusType = computed(() => {
  if (isRunning.value) return 'info'
  if (isFailed.value) return 'error'
  return 'success'
})

const statusText = computed(() => {
  if (isRunning.value) return '运行中'
  if (isFailed.value) return '失败'
  return '完成'
})

const iconChar = computed(() => {
  switch (props.msg.kind) {
    case 'commandExecution': return '▶'
    case 'fileChange': return '✎'
    case 'webSearch': return '🔍'
    case 'mcpToolCall':
    case 'dynamicToolCall': return '🔧'
    case 'plan': return '📋'
    case 'subAgentActivity': return '🤖'
    default: return '⚙'
  }
})

const hasDetail = computed(() => {
  if (props.msg.kind === 'commandExecution') return props.msg.output && props.msg.output.trim()
  if (props.msg.kind === 'fileChange') return props.msg.changes?.length
  if (props.msg.kind === 'mcpToolCall' || props.msg.kind === 'dynamicToolCall') return props.msg.arguments
  return false
})

const detailContent = computed(() => {
  if (props.msg.kind === 'commandExecution') {
    return (props.msg.output || '').trim()
  }
  if (props.msg.kind === 'fileChange') {
    return (props.msg.changes || [])
      .map(c => {
        const path = c.path || c.filePath || c.file || '?'
        const op = c.operation || c.type || ''
        return op ? `${op}  ${path}` : path
      })
      .join('\n')
  }
  if (props.msg.kind === 'mcpToolCall' || props.msg.kind === 'dynamicToolCall') {
    try {
      return JSON.stringify(props.msg.arguments, null, 2)
    } catch {
      return String(props.msg.arguments || '')
    }
  }
  return ''
})

function toggle() {
  if (hasDetail.value) expanded.value = !expanded.value
}

function truncate(t, max) {
  if (!t) return ''
  return t.length > max ? t.slice(0, max - 1) + '…' : t
}
</script>

<template>
  <div class="tool-row fade-in">
    <div class="tool-avatar-col">
      <div class="tool-avatar" :class="{ running: isRunning }">
        {{ iconChar }}
      </div>
    </div>

    <div class="tool-body-col">
      <div
        class="tool-card"
        :class="{ running: isRunning, failed: isFailed, clickable: hasDetail }"
        @click="toggle"
      >
        <div class="tool-card-main">
          <!-- Icon badge -->
          <span class="tool-icon-badge">{{ iconChar }}</span>

          <!-- Title -->
          <span class="tool-title" :title="msg.title">
            {{ truncate(msg.title, 80) }}
          </span>

          <!-- Sub info -->
          <span
            v-if="msg.kind === 'commandExecution' && msg.exitCode != null"
            class="tool-sub"
          >
            exit {{ msg.exitCode }}
          </span>
          <span
            v-else-if="msg.kind === 'fileChange' && msg.changes?.length"
            class="tool-sub"
          >
            {{ msg.changes.length }} 文件
          </span>
        </div>

        <div class="tool-card-right">
          <NTag
            :type="statusType"
            size="tiny"
            round
            :bordered="false"
          >
            {{ statusText }}
          </NTag>

          <svg
            v-if="hasDetail"
            class="expand-icon"
            :class="{ open: expanded }"
            width="12" height="12" viewBox="0 0 12 12" fill="none"
          >
            <path d="M3 4.5L6 7.5L9 4.5" stroke="currentColor"
              stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"/>
          </svg>
        </div>
      </div>

      <!-- Expanded detail panel -->
      <div v-if="expanded && hasDetail" class="tool-detail">
        <pre class="detail-output"><code>{{ detailContent }}</code></pre>
      </div>
    </div>
  </div>
</template>

<style scoped>
.tool-row {
  display: flex;
  gap: 10px;
  margin-bottom: 8px;
  align-items: flex-start;
}

/* Avatar column (spacer to align with message avatar) */
.tool-avatar-col {
  flex-shrink: 0;
  width: 28px;
}
.tool-avatar {
  display: none; /* hidden — icon is inside the card */
}

/* ── Tool card ────────────────────────────────────────── */
.tool-body-col {
  flex: 1;
  min-width: 0;
}

.tool-card {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 8px 12px 8px 10px;
  background: #111113;
  border: 1px solid #1e1e23;
  border-radius: 8px;
  transition: border-color 0.15s, background 0.15s;
  gap: 10px;
}
.tool-card.clickable {
  cursor: pointer;
}
.tool-card.clickable:hover {
  background: #151518;
  border-color: #2a2a30;
}
.tool-card.running {
  border-color: rgba(96, 165, 250, 0.25);
  background: rgba(96, 165, 250, 0.04);
}
.tool-card.failed {
  border-color: rgba(248, 113, 113, 0.25);
  background: rgba(248, 113, 113, 0.04);
}

.tool-card-main {
  display: flex;
  align-items: center;
  gap: 8px;
  flex: 1;
  min-width: 0;
}

.tool-icon-badge {
  width: 22px;
  height: 22px;
  min-width: 22px;
  border-radius: 5px;
  background: #1c1c20;
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 10px;
  color: #9b9ba3;
  border: 1px solid #26262c;
}

.tool-title {
  font-size: 12px;
  font-weight: 500;
  color: #c8c8cc;
  font-family: var(--font-mono);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  flex: 1;
  min-width: 0;
}

.tool-sub {
  font-size: 10px;
  color: #6b6b74;
  flex-shrink: 0;
  font-family: var(--font-mono);
}

.tool-card-right {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-shrink: 0;
}

.expand-icon {
  color: #6b6b74;
  transition: transform 0.2s;
}
.expand-icon.open {
  transform: rotate(180deg);
}

/* ── Detail panel ─────────────────────────────────────── */
.tool-detail {
  margin-top: 2px;
  border-radius: 0 0 8px 8px;
  overflow: hidden;
}

.detail-output {
  margin: 0;
  padding: 12px 16px;
  font-family: var(--font-mono);
  font-size: 11px;
  line-height: 1.55;
  color: #9b9ba3;
  background: #0d0d0f;
  border: 1px solid #1a1a1e;
  border-top: none;
  border-radius: 0 0 8px 8px;
  max-height: 320px;
  overflow-y: auto;
  overflow-x: auto;
  white-space: pre-wrap;
  word-break: break-all;
}

.detail-output code {
  font-family: inherit;
  background: none;
  padding: 0;
  color: inherit;
}

.fade-in {
  animation: fadeIn 0.2s ease;
}
@keyframes fadeIn {
  from { opacity: 0; transform: translateY(3px); }
  to   { opacity: 1; transform: translateY(0); }
}
</style>
