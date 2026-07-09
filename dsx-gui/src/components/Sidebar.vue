<script setup>
import { inject, ref, computed } from 'vue'
import {
  NButton,
  NInput,
  NList,
  NListItem,
  NScrollbar,
  NSpace,
  NTooltip,
  NPopconfirm,
  NTag,
  useDialog,
  useMessage,
} from 'naive-ui'

const app = inject('app')
const dialog = useDialog()
const message = useMessage()

function formatTime(iso) {
  if (!iso) return ''
  const d = new Date(iso)
  const now = new Date()
  const diff = (now - d) / 1000
  if (diff < 60) return '刚刚'
  if (diff < 3600) return `${Math.floor(diff / 60)} 分钟`
  if (diff < 86400) return `${Math.floor(diff / 3600)} 小时`
  if (diff < 604800) return `${Math.floor(diff / 86400)} 天`
  return d.toLocaleDateString('zh-CN', { month: '2-digit', day: '2-digit' })
}

function threadName(t) {
  return t.name || '新对话'
}

function threadPreview(t) {
  return t.preview || ''
}

function isActive(t) {
  return t.id === app.currentThreadId.value
}

function hasActiveStatus(t) {
  return t.status?.type === 'active'
}

async function handleNew() {
  try {
    await app.newThread()
  } catch (e) {
    message.error('创建对话失败')
  }
}

async function handleSelect(t) {
  if (isActive(t)) return
  await app.switchThread(t.id)
}

function handleDelete(e, t) {
  e.stopPropagation()
  dialog.warning({
    title: '删除对话',
    content: `确定删除「${threadName(t)}」？`,
    positiveText: '删除',
    negativeText: '取消',
    onPositiveClick: async () => {
      await app.deleteThread(t.id)
      message.success('已删除')
    },
  })
}

const listHeight = computed(() => {
  // Approximate: viewport - titlebar(44) - new button area(~60) - search(~50) - padding
  return window.innerHeight - 44 - 60 - 50 - 16
})
</script>

<template>
  <aside class="sidebar">
    <!-- New thread -->
    <div class="sidebar-header">
      <NButton
        block
        type="primary"
        size="medium"
        @click="handleNew"
        :disabled="!app.rpc.connected.value"
      >
        <template #icon>
          <svg width="14" height="14" viewBox="0 0 14 14" fill="none">
            <path d="M7 1.5v11M1.5 7h11" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>
          </svg>
        </template>
        新对话
      </NButton>
    </div>

    <!-- Search -->
    <div class="sidebar-search">
      <NInput
        v-model:value="app.searchTerm.value"
        placeholder="搜索对话…"
        size="small"
        clearable
      >
        <template #prefix>
          <svg width="13" height="13" viewBox="0 0 13 13" fill="none">
            <circle cx="5.5" cy="5.5" r="4" stroke="currentColor" stroke-width="1.3"/>
            <path d="M9 9l3 3" stroke="currentColor" stroke-width="1.3" stroke-linecap="round"/>
          </svg>
        </template>
      </NInput>
    </div>

    <!-- Thread list -->
    <div class="sidebar-list">
      <NScrollbar style="height: 100%">
        <div v-if="!app.filteredThreads.value.length" class="list-empty">
          <span v-if="!app.rpc.connected.value">未连接</span>
          <span v-else-if="app.searchTerm.value">无匹配结果</span>
          <span v-else>暂无对话</span>
        </div>

        <div
          v-for="t in app.filteredThreads.value"
          :key="t.id"
          class="thread-item"
          :class="{ active: isActive(t) }"
          @click="handleSelect(t)"
        >
          <div class="thread-item-content">
            <div class="thread-item-title">
              <span class="title-text">{{ threadName(t) }}</span>
              <NTag
                v-if="hasActiveStatus(t)"
                size="tiny"
                type="info"
                :bordered="false"
                round
              >
                ●
              </NTag>
            </div>
            <div v-if="threadPreview(t)" class="thread-item-preview">
              {{ threadPreview(t) }}
            </div>
            <div class="thread-item-meta">
              <span class="meta-time">{{ formatTime(t.recencyAt || t.updatedAt) }}</span>
              <NTooltip trigger="hover">
                <template #trigger>
                  <button
                    class="delete-btn"
                    @click.stop="handleDelete($event, t)"
                  >
                    <svg width="12" height="12" viewBox="0 0 12 12" fill="none">
                      <path d="M2 3h8M4.5 3V2h3v1M3 3l.5 7h5L9 3"
                        stroke="currentColor" stroke-width="1"
                        stroke-linecap="round" stroke-linejoin="round"/>
                    </svg>
                  </button>
                </template>
                删除对话
              </NTooltip>
            </div>
          </div>
        </div>
      </NScrollbar>
    </div>
  </aside>
</template>

<style scoped>
.sidebar {
  width: 260px;
  min-width: 260px;
  display: flex;
  flex-direction: column;
  background: #0d0d0f;
  border-right: 1px solid #1e1e23;
  height: 100%;
}

.sidebar-header {
  padding: 12px 12px 8px;
}

.sidebar-search {
  padding: 4px 12px 10px;
}

.sidebar-list {
  flex: 1;
  min-height: 0;
  padding: 0 6px 12px;
  overflow: hidden;
}

/* ── Thread item ───────────────────────────────────────── */
.thread-item {
  border-radius: 8px;
  padding: 2px;
  margin-bottom: 2px;
  cursor: pointer;
  transition: background 0.15s;
}
.thread-item:hover {
  background: #18181b;
}
.thread-item.active {
  background: #1f1f23;
}
.thread-item.active .title-text {
  color: #e8e8ea;
  font-weight: 500;
}

.thread-item-content {
  padding: 8px 10px 8px 12px;
}

.thread-item-title {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 6px;
  margin-bottom: 2px;
}

.title-text {
  font-size: 12.5px;
  color: #c8c8cc;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  line-height: 1.4;
  flex: 1;
  min-width: 0;
}

.thread-item-preview {
  font-size: 11px;
  color: #6b6b74;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  line-height: 1.4;
  margin-bottom: 4px;
}

.thread-item-meta {
  display: flex;
  align-items: center;
  justify-content: space-between;
}

.meta-time {
  font-size: 10px;
  color: #52525a;
}

.delete-btn {
  opacity: 0;
  color: #6b6b74;
  padding: 3px;
  border-radius: 4px;
  display: flex;
  align-items: center;
  transition: opacity 0.15s, color 0.15s, background 0.15s;
  background: none;
  border: none;
  cursor: pointer;
}
.thread-item:hover .delete-btn {
  opacity: 1;
}
.delete-btn:hover {
  color: #f87171;
  background: rgba(248, 113, 113, 0.1);
}

.list-empty {
  padding: 32px 16px;
  text-align: center;
  font-size: 12px;
  color: #52525a;
}
</style>
