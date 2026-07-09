<script setup>
import { ref, computed, watch } from 'vue'

const props = defineProps({
  tools: { type: Array, required: true },
})

// Auto-expand if any tool is running or failed
const hasRunning = computed(() => props.tools.some(t => t.status === 'running'))
const hasFailed = computed(() => props.tools.some(t => t.status === 'failed'))
const expanded = ref(hasRunning.value || hasFailed.value)

// Auto-expand when a tool starts running; keep user's manual collapse otherwise
watch(hasRunning, (running) => {
  if (running) expanded.value = true
})

const totalCount = computed(() => props.tools.length)

const typeCounts = computed(() => {
  const counts = {}
  for (const t of props.tools) {
    const label = typeLabel(t.kind)
    counts[label] = (counts[label] || 0) + 1
  }
  return counts
})

const successCount = computed(() =>
  props.tools.filter(t => t.status === 'done').length
)
const failedCount = computed(() =>
  props.tools.filter(t => t.status === 'failed').length
)
const runningCount = computed(() =>
  props.tools.filter(t => t.status === 'running').length
)

const allDone = computed(() =>
  props.tools.every(t => t.status === 'done' || t.status === 'failed')
)

const summaryText = computed(() => {
  const parts = Object.entries(typeCounts.value)
    .map(([k, v]) => `${v} ${k}`)
    .join(' · ')
  return parts || `${totalCount.value} 个调用`
})

const totalDuration = computed(() => {
  let ms = 0
  for (const t of props.tools) {
    if (t.durationMs) ms += t.durationMs
  }
  if (ms > 0) return `${(ms / 1000).toFixed(1)}s`
  return ''
})

function typeLabel(kind) {
  const map = {
    commandExecution: '命令',
    fileChange: '文件',
    webSearch: '搜索',
    mcpToolCall: 'MCP',
    dynamicToolCall: '工具',
    plan: '规划',
    subAgentActivity: '子代理',
  }
  return map[kind] || '工具'
}

function iconFor(kind) {
  const map = {
    commandExecution: '▶',
    fileChange: '✎',
    webSearch: '🔍',
    mcpToolCall: '🔧',
    dynamicToolCall: '🔧',
    plan: '📋',
    subAgentActivity: '🤖',
  }
  return map[kind] || '⚙'
}

function statusColor(t) {
  if (t.status === 'running') return '#60a5fa'
  if (t.status === 'failed') return '#f87171'
  return '#4ade80'
}

function truncate(t, max) {
  if (!t) return ''
  return t.length > max ? t.slice(0, max - 1) + '…' : t
}

function toggle() {
  expanded.value = !expanded.value
}
</script>

<template>
  <div class="tool-group-row fade-in">
    <div class="tg-body">
      <!-- Group header (always visible) -->
      <div
        class="tg-header"
        :class="{ clickable: true, expanded }"
        @click="toggle"
      >
        <div class="tg-header-left">
          <span class="tg-count-badge">
            {{ totalCount }}
          </span>
          <span class="tg-label">工具调用</span>
          <span class="tg-summary">{{ summaryText }}</span>
        </div>

        <div class="tg-header-right">
          <!-- Status badges -->
          <span v-if="runningCount" class="tg-status running">
            <span class="dot-pulse"></span>
            {{ runningCount }} 运行中
          </span>
          <span v-if="failedCount" class="tg-status failed">
            ✗ {{ failedCount }}
          </span>
          <span v-if="successCount && !runningCount" class="tg-status success">
            ✓ {{ successCount }}
          </span>

          <span v-if="totalDuration" class="tg-duration">{{ totalDuration }}</span>

          <svg
            class="tg-chevron"
            :class="{ open: expanded }"
            width="12" height="12" viewBox="0 0 12 12" fill="none"
          >
            <path d="M3 4.5L6 7.5L9 4.5" stroke="currentColor"
              stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"/>
          </svg>
        </div>
      </div>

      <!-- Tool items (collapsible) -->
      <div v-show="expanded" class="tg-items">
        <div
          v-for="tool in tools"
          :key="tool.id"
          class="tg-item"
          :class="{ running: tool.status === 'running', failed: tool.status === 'failed' }"
        >
          <div class="tg-item-left">
            <span class="tg-item-icon">{{ iconFor(tool.kind) }}</span>
            <span class="tg-item-status-dot" :style="{ background: statusColor(tool) }"></span>
          </div>

          <div class="tg-item-body">
            <div class="tg-item-title" :title="tool.title">
              {{ truncate(tool.title, 90) }}
            </div>
            <div v-if="tool.kind === 'commandExecution' && tool.exitCode != null" class="tg-item-sub">
              exit code: {{ tool.exitCode }}
            </div>
            <div v-else-if="tool.kind === 'fileChange' && tool.changes?.length" class="tg-item-sub">
              {{ tool.changes.length }} 个文件变更
            </div>

            <!-- Inline output preview for commands -->
            <div
              v-if="tool.kind === 'commandExecution' && tool.output && tool.output.trim()"
              class="tg-item-output"
            >
              <pre><code>{{ tool.output.trim().slice(-600) }}</code></pre>
            </div>

            <!-- File change list -->
            <div
              v-if="tool.kind === 'fileChange' && tool.changes?.length"
              class="tg-item-files"
            >
              <div
                v-for="(c, i) in tool.changes.slice(0, 10)"
                :key="i"
                class="file-row"
              >
                <span class="file-op" :class="(c.operation || c.type || '').toLowerCase()">
                  {{ (c.operation || c.type || '').charAt(0).toUpperCase() }}
                </span>
                <span class="file-path">{{ c.path || c.filePath || c.file || '?' }}</span>
              </div>
              <div v-if="tool.changes.length > 10" class="file-more">
                +{{ tool.changes.length - 10 }} 更多
              </div>
            </div>
          </div>

          <div class="tg-item-right">
            <span class="tg-item-type-tag">{{ typeLabel(tool.kind) }}</span>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.tool-group-row {
  display: flex;
  margin-bottom: 14px;
}

/* ── Body ─────────────────────────────────────────────── */
.tg-body {
  flex: 1;
  min-width: 0;
}

/* ── Header ───────────────────────────────────────────── */
.tg-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 8px 12px;
  background: #111113;
  border: 1px solid #1e1e23;
  border-radius: 10px;
  cursor: pointer;
  transition: border-color 0.15s, background 0.15s;
  user-select: none;
}
.tg-header:hover {
  background: #151518;
  border-color: #2a2a30;
}
.tg-header.expanded {
  border-bottom-left-radius: 0;
  border-bottom-right-radius: 0;
  border-bottom-color: #1e1e23;
}

.tg-header-left {
  display: flex;
  align-items: center;
  gap: 8px;
  flex: 1;
  min-width: 0;
}

.tg-count-badge {
  width: 20px;
  height: 20px;
  min-width: 20px;
  border-radius: 5px;
  background: #2a2a30;
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 10px;
  font-weight: 600;
  color: #e8e8ea;
  font-family: var(--font-mono);
}

.tg-label {
  font-size: 12px;
  font-weight: 500;
  color: #c8c8cc;
}

.tg-summary {
  font-size: 11px;
  color: #6b6b74;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.tg-header-right {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-shrink: 0;
}

.tg-status {
  font-size: 10px;
  font-weight: 500;
  padding: 2px 6px;
  border-radius: 4px;
  display: flex;
  align-items: center;
  gap: 4px;
}
.tg-status.running {
  color: #60a5fa;
  background: rgba(96, 165, 250, 0.1);
}
.tg-status.failed {
  color: #f87171;
  background: rgba(248, 113, 113, 0.1);
}
.tg-status.success {
  color: #4ade80;
  background: rgba(74, 222, 128, 0.08);
}

.dot-pulse {
  width: 5px;
  height: 5px;
  border-radius: 50%;
  background: #60a5fa;
  animation: pulse-dot 1.2s ease-in-out infinite;
}
@keyframes pulse-dot {
  0%, 100% { opacity: 1; transform: scale(1); }
  50% { opacity: 0.4; transform: scale(0.8); }
}

.tg-duration {
  font-size: 10px;
  color: #52525a;
  font-family: var(--font-mono);
}

.tg-chevron {
  color: #6b6b74;
  transition: transform 0.2s;
  flex-shrink: 0;
}
.tg-chevron.open {
  transform: rotate(180deg);
}

/* ── Items panel ──────────────────────────────────────── */
.tg-items {
  background: #0d0d0f;
  border: 1px solid #1e1e23;
  border-top: none;
  border-radius: 0 0 10px 10px;
  overflow: hidden;
}

.tg-item {
  display: flex;
  align-items: flex-start;
  gap: 8px;
  padding: 9px 12px;
  border-bottom: 1px solid #16161a;
  transition: background 0.1s;
}
.tg-item:last-child {
  border-bottom: none;
}
.tg-item:hover {
  background: #111113;
}
.tg-item.running {
  background: rgba(96, 165, 250, 0.03);
}
.tg-item.failed {
  background: rgba(248, 113, 113, 0.03);
}

.tg-item-left {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 4px;
  flex-shrink: 0;
  padding-top: 1px;
}

.tg-item-icon {
  width: 20px;
  height: 20px;
  border-radius: 4px;
  background: #1a1a20;
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 9px;
  color: #9b9ba3;
  border: 1px solid #26262c;
}

.tg-item-status-dot {
  width: 5px;
  height: 5px;
  border-radius: 50%;
  flex-shrink: 0;
}

.tg-item-body {
  flex: 1;
  min-width: 0;
}

.tg-item-title {
  font-size: 11.5px;
  font-weight: 500;
  color: #c8c8cc;
  font-family: var(--font-mono);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  line-height: 1.4;
}

.tg-item-sub {
  font-size: 10px;
  color: #6b6b74;
  margin-top: 2px;
  font-family: var(--font-mono);
}

.tg-item-output {
  margin-top: 6px;
  border-radius: 5px;
  overflow: hidden;
  border: 1px solid #1a1a1e;
}
.tg-item-output pre {
  margin: 0;
  padding: 7px 10px;
  font-family: var(--font-mono);
  font-size: 10.5px;
  line-height: 1.5;
  color: #8b8b93;
  background: #080809;
  max-height: 120px;
  overflow-y: auto;
  white-space: pre-wrap;
  word-break: break-all;
}
.tg-item-output code {
  font-family: inherit;
  background: none;
  padding: 0;
  color: inherit;
}

.tg-item-files {
  margin-top: 5px;
  display: flex;
  flex-direction: column;
  gap: 2px;
}
.file-row {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: 10.5px;
  font-family: var(--font-mono);
}
.file-op {
  font-size: 9px;
  font-weight: 600;
  width: 16px;
  height: 16px;
  border-radius: 3px;
  display: flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
  text-transform: uppercase;
}
.file-op.create { background: rgba(74, 222, 128, 0.15); color: #4ade80; }
.file-op.delete { background: rgba(248, 113, 113, 0.15); color: #f87171; }
.file-op.modify, .file-op.update { background: rgba(96, 165, 250, 0.15); color: #60a5fa; }
.file-op:not(.create):not(.delete):not(.modify):not(.update) { background: #1a1a20; color: #9b9ba3; }

.file-path {
  color: #8b8b93;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.file-more {
  font-size: 10px;
  color: #52525a;
  padding-left: 22px;
}

.tg-item-right {
  flex-shrink: 0;
  padding-top: 2px;
}
.tg-item-type-tag {
  font-size: 9px;
  color: #52525a;
  padding: 2px 5px;
  background: #151518;
  border-radius: 3px;
  font-family: var(--font-mono);
}

.fade-in {
  animation: fadeIn 0.2s ease;
}
@keyframes fadeIn {
  from { opacity: 0; transform: translateY(3px); }
  to   { opacity: 1; transform: translateY(0); }
}
</style>
