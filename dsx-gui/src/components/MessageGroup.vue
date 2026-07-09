<script setup>
import { ref, computed } from 'vue'
import { marked } from 'marked'

marked.setOptions({ gfm: true, breaks: true })

const props = defineProps({
  msg: { type: Object, required: true },
  isStreaming: { type: Boolean, default: false },
})

const isUser = computed(() => props.msg.kind === 'userMessage')
const isReasoning = computed(() => props.msg.kind === 'reasoning')
const isSystem = computed(() => props.msg.kind === 'system')
const isError = computed(() => props.msg.isError)

const hasText = computed(() => (props.msg.text || '').trim().length > 0)

// Reasoning: collapsed by default, expandable
const reasoningOpen = ref(false)

// Render markdown to HTML
const html = computed(() => {
  if (isSystem.value) return ''
  const text = props.msg.text || ''
  if (!text) return ''
  try {
    return marked.parse(text)
  } catch (e) {
    return text
  }
})
</script>

<template>
  <!-- ═══ System message (only if has text) ═══ -->
  <div
    v-if="isSystem && hasText"
    class="sys-msg fade-in"
    :class="{ error: isError }"
  >
    {{ msg.text }}
  </div>

  <!-- ═══ Reasoning (collapsible chip) ═══ -->
  <div
    v-else-if="isReasoning && hasText"
    class="reasoning-row fade-in"
  >
    <button
      class="reasoning-toggle"
      @click="reasoningOpen = !reasoningOpen"
    >
      <svg
        class="reasoning-chevron"
        :class="{ open: reasoningOpen }"
        width="10" height="10" viewBox="0 0 10 10" fill="none"
      >
        <path d="M2.5 3.5L5 6L7.5 3.5" stroke="currentColor"
          stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"/>
      </svg>
      <span>思考过程</span>
    </button>

    <div v-show="reasoningOpen" class="reasoning-body md-body" v-html="html"></div>
  </div>

  <!-- ═══ User message (subtle bubble, right-aligned) ═══ -->
  <div
    v-else-if="isUser && hasText"
    class="user-row fade-in"
  >
    <div class="user-bubble">
      <span>{{ msg.text }}</span>
    </div>
  </div>

  <!-- ═══ Assistant message (no bubble, blends with background) ═══ -->
  <div
    v-else-if="!isUser && !isReasoning && !isSystem && hasText"
    class="assistant-row fade-in"
  >
    <div class="assistant-text md-body" v-html="html"></div>
    <span v-if="isStreaming" class="stream-cursor"></span>
  </div>
</template>

<style scoped>
/* ── System message ───────────────────────────────────── */
.sys-msg {
  font-size: 11px;
  color: #6b6b74;
  padding: 4px 0;
  margin-bottom: 12px;
  line-height: 1.5;
}
.sys-msg.error {
  color: #f87171;
}

/* ── Reasoning ────────────────────────────────────────── */
.reasoning-row {
  margin-bottom: 10px;
}
.reasoning-toggle {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  font-size: 11px;
  color: #6b6b74;
  padding: 3px 8px;
  border-radius: 4px;
  background: none;
  border: none;
  cursor: pointer;
  transition: color 0.15s, background 0.15s;
}
.reasoning-toggle:hover {
  color: #9b9ba3;
  background: #151518;
}
.reasoning-chevron {
  transition: transform 0.2s;
  color: #52525a;
}
.reasoning-chevron.open {
  transform: rotate(180deg);
}
.reasoning-body {
  margin-top: 4px;
  padding: 8px 12px;
  font-size: 12px;
  color: #8b8b93;
  font-style: italic;
  line-height: 1.6;
  border-left: 2px solid #26262c;
}

/* ── User message ─────────────────────────────────────── */
.user-row {
  display: flex;
  justify-content: flex-end;
  margin-bottom: 16px;
}
.user-bubble {
  max-width: 85%;
  padding: 9px 14px;
  background: #2a2a30;
  border-radius: 12px;
  border-top-right-radius: 3px;
  font-size: 13.5px;
  line-height: 1.65;
  color: #e8e8ea;
  white-space: pre-wrap;
  word-wrap: break-word;
}

/* ── Assistant message (no bubble) ────────────────────── */
.assistant-row {
  margin-bottom: 18px;
  position: relative;
}
.assistant-text {
  font-size: 13.5px;
  line-height: 1.75;
  color: #d8d8dc;
  word-wrap: break-word;
}

/* ── Streaming cursor ─────────────────────────────────── */
.stream-cursor {
  display: inline-block;
  width: 2px;
  height: 1.1em;
  background: #e8e8ea;
  margin-left: 1px;
  vertical-align: text-bottom;
  animation: blink 0.8s step-end infinite;
}
@keyframes blink {
  50% { opacity: 0; }
}

/* ── Fade in ──────────────────────────────────────────── */
.fade-in {
  animation: fadeIn 0.2s ease;
}
@keyframes fadeIn {
  from { opacity: 0; transform: translateY(3px); }
  to   { opacity: 1; transform: translateY(0); }
}

/* ── Markdown styles (applied via :deep() on .md-body) ── */
.md-body :deep(p) {
  margin: 0.55em 0;
}
.md-body :deep(p:first-child) { margin-top: 0; }
.md-body :deep(p:last-child) { margin-bottom: 0; }

.md-body :deep(strong) {
  font-weight: 600;
  color: #ececef;
}

.md-body :deep(em) {
  color: #c0c0c6;
}

.md-body :deep(a) {
  color: #7ab8ff;
  text-decoration: none;
  border-bottom: 1px solid rgba(122, 184, 255, 0.2);
  transition: border-color 0.15s;
}
.md-body :deep(a:hover) {
  border-bottom-color: #7ab8ff;
}

.md-body :deep(code) {
  font-family: var(--font-mono);
  font-size: 0.86em;
  padding: 1.5px 6px;
  background: #151518;
  border: 1px solid #26262c;
  border-radius: 4px;
  color: #e2c79a;
}

.md-body :deep(pre) {
  margin: 0.8em 0;
  padding: 0;
  background: #0d0d0f;
  border: 1px solid #1e1e23;
  border-radius: 8px;
  overflow: hidden;
}
.md-body :deep(pre code) {
  display: block;
  padding: 14px 18px;
  background: transparent;
  border: none;
  font-size: 12px;
  line-height: 1.65;
  color: #c8c8cc;
  overflow-x: auto;
  white-space: pre;
  word-break: normal;
}

.md-body :deep(h1),
.md-body :deep(h2),
.md-body :deep(h3),
.md-body :deep(h4) {
  font-weight: 600;
  margin: 1.1em 0 0.5em;
  color: #ececef;
  line-height: 1.35;
}
.md-body :deep(h1) { font-size: 1.2em; }
.md-body :deep(h2) { font-size: 1.1em; }
.md-body :deep(h3) { font-size: 1.03em; }
.md-body :deep(h4) { font-size: 1em; }
.md-body :deep(h1:first-child),
.md-body :deep(h2:first-child),
.md-body :deep(h3:first-child) { margin-top: 0; }

.md-body :deep(blockquote) {
  margin: 0.7em 0;
  padding: 2px 14px;
  border-left: 2px solid #3a3a44;
  color: #9b9ba3;
}
.md-body :deep(blockquote p) {
  margin: 0;
}

.md-body :deep(ul),
.md-body :deep(ol) {
  margin: 0.5em 0;
  padding-left: 1.4em;
}
.md-body :deep(li) {
  margin: 0.2em 0;
  line-height: 1.65;
}
.md-body :deep(li::marker) {
  color: #6b6b74;
}

.md-body :deep(hr) {
  border: none;
  border-top: 1px solid #26262c;
  margin: 1em 0;
}

.md-body :deep(table) {
  border-collapse: collapse;
  margin: 0.8em 0;
  width: 100%;
  font-size: 12px;
}
.md-body :deep(th),
.md-body :deep(td) {
  padding: 6px 12px;
  border: 1px solid #26262c;
  text-align: left;
}
.md-body :deep(th) {
  background: #111113;
  font-weight: 600;
  color: #c8c8cc;
}
.md-body :deep(tr:nth-child(even) td) {
  background: #0d0d0f;
}
</style>
