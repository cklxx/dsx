<script setup>
import { onMounted, provide, ref, computed, h } from 'vue'
import {
  NConfigProvider,
  NMessageProvider,
  NDialogProvider,
  NButton,
  NSpace,
  NTag,
  NTooltip,
  NSpin,
  NResult,
  darkTheme,
} from 'naive-ui'
import { useApp } from './composables/useApp'
import Sidebar from './components/Sidebar.vue'
import ChatView from './components/ChatView.vue'

const app = useApp()
provide('app', app)

const connecting = ref(true)
const connectError = ref(null)

const themeOverrides = {
  common: {
    primaryColor: '#e8e8ea',
    primaryColorHover: '#ffffff',
    primaryColorPressed: '#d0d0d4',
    primaryColorSuppl: '#e8e8ea',
    infoColor: '#60a5fa',
    successColor: '#4ade80',
    warningColor: '#fbbf24',
    errorColor: '#f87171',
    textColorBase: '#e8e8ea',
    textColor1: '#e8e8ea',
    textColor2: '#9b9ba3',
    textColor3: '#6b6b74',
    borderColor: '#26262c',
    dividerColor: '#1e1e23',
    bodyColor: '#0a0a0b',
    cardColor: '#111113',
    modalColor: '#151518',
    popoverColor: '#151518',
    borderRadius: '8px',
    borderRadiusSmall: '6px',
    fontSize: '13px',
    fontSizeSmall: '12px',
    fontSizeTiny: '11px',
  },
  Button: {
    colorPrimary: '#2a2a30',
    colorHoverPrimary: '#35353c',
    colorPressedPrimary: '#3f3f48',
    borderPrimary: '1px solid #2a2a30',
    textColorPrimary: '#e8e8ea',
    textColorHoverPrimary: '#ffffff',
    textColorPressedPrimary: '#ffffff',
  },
  Input: {
    color: '#151518',
    colorFocus: '#151518',
    border: '1px solid #26262c',
    borderHover: '1px solid #35353c',
    borderFocus: '1px solid #4a4a54',
    boxShadowFocus: 'none',
    borderRadius: '8px',
    fontSizeMedium: '13px',
  },
  Tag: {
    borderRadius: '6px',
  },
  List: {
    color: 'transparent',
    colorHover: '#18181b',
    borderColor: 'transparent',
  },
  Scrollbar: {
    color: '#26262c',
    colorHover: '#3a3a44',
  },
}

onMounted(async () => {
  try {
    await app.connectAndInit()
  } catch (e) {
    connectError.value = e.message
  } finally {
    connecting.value = false
  }
})

async function reconnect() {
  connecting.value = true
  connectError.value = null
  try {
    await app.connectAndInit()
  } catch (e) {
    connectError.value = e.message
  } finally {
    connecting.value = false
  }
}

const displayName = computed(() => {
  if (app.currentThreadName.value) return app.currentThreadName.value
  if (app.currentCwd.value) return app.currentCwd.value
  return 'dsx'
})
</script>

<template>
  <NConfigProvider :theme="darkTheme" :theme-overrides="themeOverrides">
    <NMessageProvider>
      <NDialogProvider>
        <div class="app-shell">
          <!-- ═══ Title bar ════════════════════════════════════ -->
          <header class="titlebar">
            <div class="titlebar-left">
              <span class="logo-mark">⚡</span>
              <span class="logo-text">dsx</span>
            </div>

            <div class="titlebar-center">
              <span class="title-text">{{ displayName }}</span>
            </div>

            <div class="titlebar-right">
              <NSpace align="center" :size="10">
                <NTag
                  v-if="app.currentModel.value"
                  size="small"
                  :bordered="false"
                  round
                  type="default"
                >
                  {{ app.currentModel.value }}
                </NTag>
                <NTooltip trigger="hover">
                  <template #trigger>
                    <span
                      class="conn-dot"
                      :class="{ on: app.rpc.connected.value }"
                    ></span>
                  </template>
                  {{ app.rpc.connected.value ? '已连接' : '未连接' }}
                </NTooltip>
              </NSpace>
            </div>
          </header>

          <!-- ═══ Main content ═══════════════════════════════ -->
          <div class="main-area">
            <Sidebar />
            <ChatView />
          </div>

          <!-- ═══ Connection overlay ══════════════════════════ -->
          <div v-if="connecting || connectError" class="conn-overlay">
            <div v-if="connecting" class="conn-loading">
              <NSpin size="large" />
              <span class="conn-loading-text">连接中…</span>
            </div>
            <div v-else-if="connectError" class="conn-error">
              <NResult
                status="error"
                title="无法连接"
                :description="connectError"
                size="small"
              >
                <template #footer>
                  <NButton type="primary" @click="reconnect">重试</NButton>
                </template>
              </NResult>
            </div>
          </div>
        </div>
      </NDialogProvider>
    </NMessageProvider>
  </NConfigProvider>
</template>

<style scoped>
.app-shell {
  display: flex;
  flex-direction: column;
  height: 100vh;
  background: #0a0a0b;
}

/* ── Title bar ─────────────────────────────────────────── */
.titlebar {
  height: 44px;
  min-height: 44px;
  display: flex;
  align-items: center;
  padding: 0 16px;
  border-bottom: 1px solid #1e1e23;
  background: #111113;
  -webkit-app-region: drag;
  user-select: none;
  z-index: 10;
}
.titlebar-left {
  display: flex;
  align-items: center;
  gap: 7px;
  width: 240px;
  flex-shrink: 0;
}
.logo-mark {
  font-size: 15px;
  line-height: 1;
}
.logo-text {
  font-weight: 600;
  font-size: 13px;
  letter-spacing: 0.03em;
  color: #e8e8ea;
}
.titlebar-center {
  flex: 1;
  display: flex;
  justify-content: center;
  min-width: 0;
}
.title-text {
  font-size: 12px;
  font-weight: 500;
  color: #9b9ba3;
  max-width: 420px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-family: var(--font-mono);
}
.titlebar-right {
  width: 240px;
  display: flex;
  justify-content: flex-end;
  flex-shrink: 0;
}
.conn-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: #6b6b74;
  display: inline-block;
  transition: background 0.2s;
  cursor: pointer;
}
.conn-dot.on {
  background: #4ade80;
  box-shadow: 0 0 6px rgba(74, 222, 128, 0.4);
}

/* ── Main area ─────────────────────────────────────────── */
.main-area {
  flex: 1;
  display: flex;
  min-height: 0;
}

/* ── Overlay ───────────────────────────────────────────── */
.conn-overlay {
  position: fixed;
  inset: 44px 0 0 0;
  background: rgba(10, 10, 11, 0.9);
  backdrop-filter: blur(12px);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 100;
}
.conn-loading {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 16px;
}
.conn-loading-text {
  font-size: 13px;
  color: #9b9ba3;
}
.conn-error {
  width: 360px;
}
</style>
