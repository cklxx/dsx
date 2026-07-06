# 工具调用统一聚合方案

## 目标

将相邻的工具调用（shell exec、MCP、web search、patch、hook）统一聚合为一个可折叠的 `GroupedToolCallCell`，在折叠态显示**语义化标题 + 安全信号外展**，展开态显示每个调用的完整输入输出。

## 设计原则

1. **密度优先，安全信号不外漏** — 折叠态必须外展写操作数、失败数、敏感路径触及数
2. **聚合纯展示，不影响安全** — 审批/沙箱/exec_policy 走独立路径，与 grouper 完全解耦
3. **写操作不混入读组** — 类别兼容表严格隔离
4. **单调用不折叠** — 组内只有 1 个调用时正常渲染，不增加视觉开销
5. **Sub-agent 活动放底部状态栏** — 不混入主 agent 的 transcript 聚合

## 架构

```
codex-rs/tui/src/tool_grouper/
  mod.rs        ← ToolCallGrouper (per-agent 作用域的分组控制器)
  category.rs   ← ToolCategory 枚举 + 自动推断 + 兼容表
  safety.rs     ← SafetySignals + 受保护路径检测
  entry.rs      ← ToolCallEntry (统一调用条目)
  cell.rs       ← GroupedToolCallCell (impl HistoryCell)
  render.rs     ← 折叠态/展开态渲染 + 语义标题生成
```

### 数据流

```
事件源                    转换                      分组决策                  渲染
─────────────────────────────────────────────────────────────────────────────
Shell exec start  →  ToolCallEntry  →  ToolCallGrouper  →  GroupedToolCallCell
MCP tool start    →  ToolCallEntry  →  ToolCallGrouper  →  GroupedToolCallCell  
Web search begin  →  ToolCallEntry  →  ToolCallGrouper  →  GroupedToolCallCell
Patch apply       →  ToolCallEntry  →  ToolCallGrouper  →  GroupedToolCallCell
Hook fire         →  ToolCallEntry  →  ToolCallGrouper  →  GroupedToolCallCell

Sub-agent activity → 底部状态栏 (BottomPane)，不入 transcript 聚合
Collab 控制面事件  → 直接入历史 (SpawnAgent/Wait/CloseAgent)，不入 grouper
```

## ToolCategory

```rust
enum ToolCategory {
    FileRead,       // cat, bat, head, tail, read_file
    FileSearch,     // rg, grep, find, fd, search_files
    FileList,       // ls, dir, list_files, tree
    FileWrite,      // apply_patch, edit, write, mv, rm, cp, mkdir
    ShellExec,      // cargo, npm, make, python (非文件探索)
    WebSearch,      // web_search, read_url, curl (含 URL)
    McpTool(String), // MCP server name
    Plan,           // plan tool
    Hook,           // hooks
    Other,
}
```

### 类别兼容表

| 当前组类别 | 可追加 |
|---|---|
| FileRead | FileRead, FileSearch, FileList |
| FileSearch | FileSearch, FileRead, FileList |
| FileList | FileList, FileRead, FileSearch |
| FileWrite | **仅 FileWrite** (不混入读) |
| ShellExec | 仅同命令名 |
| WebSearch | 仅 WebSearch |
| McpTool(a) | 仅 McpTool(a) (同 server) |
| Plan | 仅 Plan |
| Hook | 仅 Hook |
| Other | 仅 Other |

### 推断规则

- Shell 命令：先看 `ParsedCommand` 语义，fallback 到命令名匹配
- 重定向检测：`>` `>>` `| tee` → 写操作
- MCP：从工具名提取 server
- Web：含 `http://` `https://`

## 分组逻辑

```
新调用到达时:
  1. 没有 active group → 新建 group
  2. 有 active group:
     a. 有 text break → flush 旧 group，新建
     b. 组已满 (≥12) → flush 旧 group，新建
     c. 类别兼容 → 追加到当前 group
     d. 类别不兼容 → flush 旧 group，新建
```

**不看时间窗口**，只看类别兼容 + 是否有 text 隔断。

## Per-Agent 作用域

```rust
pub struct ToolCallGrouper {
    active_groups: HashMap<ThreadId, GroupedToolCallCell>,
    text_breaks: HashMap<ThreadId, bool>,
    enabled: bool,
    max_calls_per_group: usize,
}
```

- 主 agent 的工具调用路由到 `MAIN_THREAD_ID`
- Sub-agent 活动**不**入 grouper，放底部状态栏
- `text_break` 也是 per-agent 的

## 安全信号（折叠态外展，不可关闭）

```rust
struct SafetySignals {
    read_count: u32,
    write_count: u32,        // ≥1 标红
    fail_count: u32,         // ≥1 标红
    protected_path_hits: u32, // ≥1 标红 🔒
    network_count: u32,      // 🌐
    shell_exec_count: u32,
}
```

### 受保护路径模式

`.ssh/`, `.env`, `.aws/`, `.kube/`, `/etc/`, `/root/`, `id_rsa`, `id_ed25519`, `.pem`, `.key`, `auth.json`, `credentials`

### 折叠行格式

```
  ╷ 🔍 Explored src/ for "model"  5R 1W 0✗ · 2.4s
  ╷   rg · cat×3 · ls×2
```

- `R` = 读数, `W` = 写数 (红), `✗` = 失败数 (红)
- 🔒 = 触及敏感路径 (红)
- 🌐 = 网络调用

## 语义标题生成

从调用集合提取共同特征，用模板匹配生成一句话：

| 场景 | 模板 | 示例 |
|---|---|---|
| 有共同路径 + 搜索词 | `Explored {path} for "{term}"` | `Explored src/ for "model"` |
| 纯搜索，无共同路径 | `Searched for "{term}"` | `Searched for "TODO"` |
| 有共同路径，读为主 | `Read files in {path}` | `Read files in src/config/` |
| 写操作 | `Modified files in {path}` | `Modified src/config.rs` |
| Web 搜索 | `Researched "{topic}"` | `Researched "async traits"` |
| MCP 同 server | `Used {server}: {tools}` | `Used openmax: list_files` |
| Shell 构建测试 | `Built and tested` | `Built and tested` |
| 兜底 | `{N} tool calls` | `6 tool calls` |

## Sub-Agent 底部状态栏

在 BottomPane 的 footer 区域显示 sub-agent 活动摘要：

```
┌──────────────────────────────────────────────────────────────┐
│ > 输入框...                                                  │
├──────────────────────────────────────────────────────────────┤
│ 🤖 worker-1: 🔧 openmax×2 · worker-2: 🔍 searching...       │  ← 新增
└──────────────────────────────────────────────────────────────┘
```

- 每个活跃 sub-agent 一行摘要：`{nickname}: {当前动作}`
- 空闲的不显示
- 点击/快捷键可跳转到该 agent 的 thread

## 渲染

### 折叠态（默认，≥2 个调用时）

```
  ╷ 🔍📝 Explored src/ for "model resolution"  5R 1W 0✗ · 2.4s
  ╷   rg · cat×3 · ls×2
```

### 展开态

```
  ╷ ▼ 🔍📝 Explored src/ for "model resolution"  5R 1W 0✗ · 2.4s
  ╷ ┌ 1. rg "fn.*model" src/ ─── 0.3s, 15 lines ─────────────┐
  ╷ │ fn resolve_model() at src/config.rs:42                    │
  ╷ │ fn model_for_inference() at src/api.rs:88                 │
  ╷ └──────────────────────────────────────────────────────────┘
  ╷ ┌ 2. cat src/config.rs ─── 0.1s, 45 lines ────────────────┐
  ╷ │ model = "deepseek-v4-pro"                               │
  ╷ │ model_provider = "deepseek"                             │
  ╷ └──────────────────────────────────────────────────────────┘
  ╷   ... (3 more)
```

### 单调用（不折叠）

直接正常渲染，与现在一致。

## 键盘交互

| 按键 | 行为 |
|---|---|
| `Enter`（聚合行上） | 展开/折叠 |
| `Shift+Enter` | 展开所有聚合组 |
| `G` | 临时切换聚合开关（本次会话） |

## 配置

```toml
[tool_display]
group_calls = true              # 总开关
max_calls_per_group = 12        # 单组上限
expand_by_default = false       # 默认折叠
```

## 改动清单

| 文件 | 改动 |
|---|---|
| `tool_grouper/mod.rs` | **新增** ToolCallGrouper (per-agent) |
| `tool_grouper/category.rs` | **新增** ToolCategory + 推断 + 兼容 |
| `tool_grouper/safety.rs` | **新增** SafetySignals + 路径检测 |
| `tool_grouper/entry.rs` | **新增** ToolCallEntry |
| `tool_grouper/cell.rs` | **新增** GroupedToolCallCell |
| `tool_grouper/render.rs` | **新增** 语义标题 + 折叠/展开渲染 |
| `chatwidget.rs` | 加 `tool_grouper` 字段；`flush_active_cell` 感知 grouper |
| `chatwidget/command_lifecycle.rs` | exec start/end 走 grouper |
| `chatwidget/tool_lifecycle.rs` | MCP/web/patch 走 grouper；collab 事件不 flush active cell |
| `history_cell/mod.rs` | 导出 GroupedToolCallCell |
| `bottom_pane/mod.rs` | 加 sub-agent 活动状态栏 |
| `exec_cell/model.rs` | 废弃 is_exploring_call / with_added_call |

## 对抗场景防御

| 场景 | 防御 |
|---|---|
| 10个 `cat /etc/passwd` 藏聚合里 | 折叠行 `🔒 10 calls · 10R`，🔒 红色 |
| `bash -c "rm -rf /"` 伪装 | `detect_protected_paths` 检测到 `/` → 🔒 + W 标红 |
| `cat > secret` 伪装读 | 重定向检测 → FileWrite → 不混入读组 |
| 时间窗口攻击 | 不看时间窗口，看 text 隔断 |
| 50个调用塞一组 | `max_calls_per_group = 12` 自动拆组 |
| MCP 工具名注入 | 显示格式固定：`{server}/{tool}`，特殊字符转义 |
