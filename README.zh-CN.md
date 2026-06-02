# SecureModelRoute

**SecureModelRoute** 是面向 LLM 客户端的**本地安全代理**。将 IDE 或 Agent 的 API 地址指向 `http://127.0.0.1:8080/v1`，即可在访问云端模型前获得 **自动 fallback 路由**、**数据防泄露（DLP）**、**操作安全**与**重要路径防护**，并附带 Web 管理界面与可选系统托盘桌面应用。

**English:** [README.md](README.md)

## 功能概览

| 模块 | 说明 |
|------|------|
| **大模型路由** | 多组 fallback（`high` / `medium` / `low`），组内按序尝试；上游错误、畸形 JSON、流式未出首 token 时切换；支持 OpenAI ↔ Anthropic 请求/响应转换 |
| **DLP — 内容** | 全文/片段规则（`min_fragment_len`、`min_fragment_ratio`）；Secret 类随机替换并尽量保持大小写；可选内置凭证前缀模板（`sk-`、`AKIA`、`ghp_` 等） |
| **DLP — 文件** | 磁盘索引（SQLite 签名 + Bloom 预过滤 + 字节校验），适合大语料；流式分块建索引；`notify` 监听变更并重建 |
| **SessionGuard** | 在 **tool_call** / **tool_result** 中检测受保护路径/文件后，对后续 **N** 次请求（`trigger_window`）持续脱敏 |
| **操作安全** | 请求与响应侧 **tool 相关字段** 检查；`observe`（仅记录）/ `enforce`（拦截）；按 command_exec / api_call / network_access + 关键字 |
| **路径防护** | `deny_delete` / `deny_modify` / `deny_access`；目录自动覆盖子路径 |
| **Web 管理界面** | `/ui` 配置路由、DLP、路径与操作规则；保存即写 YAML 并热加载，保留 SessionGuard 状态 |
| **桌面应用（可选）** | Tauri 托盘应用内嵌代理；关主窗口后隐藏到托盘/菜单栏，服务不退出 |
| **审计** | 结构化请求写入 SQLite；事件可通过 API 查询 |

全局总开关：`pipeline.security_enabled`（Web 界面右上角「安全防护总开关」）。关闭后 DLP 与操作安全均不生效。

## 快速开始

### 源码构建安装

```bash
chmod +x scripts/install.sh
./scripts/install.sh           # CLI → ~/.local/bin
./scripts/install.sh --gui     # CLI + 托盘桌面应用
./scripts/install.sh --all     # CLI + GUI + 登录自启（仅托盘）

securemodelroute               # 启动代理并打开管理界面
```

**Windows（PowerShell）：**

```powershell
.\install.ps1 -All             # CLI + 托盘 GUI + 登录快捷方式
securemodelroute
```

### 发布包

从 `dist/` 解压对应平台包，运行 `./install.sh`（macOS/Linux）或 `.\install.ps1`（Windows）。自行打包可参考 `scripts/` 下脚本。

### 客户端接入

默认监听：`127.0.0.1:8080`（见配置 `server.listen`）。

| 地址 | 用途 |
|------|------|
| `http://127.0.0.1:8080/v1` | OpenAI 兼容 API |
| `http://127.0.0.1:8080/v1/messages` | Anthropic Messages API |
| `http://127.0.0.1:8080/ui` | Web 管理界面 |
| `http://127.0.0.1:8080/health` | 健康检查 |

```python
from openai import OpenAI
client = OpenAI(base_url="http://127.0.0.1:8080/v1", api_key="dummy")
```

可选请求头：

- `X-SMR-Fallback-Group` — 如 `high`、`medium`、`low`
- `X-SMR-Session-Id` — 关联 SessionGuard 与审计（未传则自动生成）

Anthropic SDK：`base_url="http://127.0.0.1:8080"`，路径 `/v1/messages`。

### 配置文件位置

| 平台 | 常见路径 |
|------|----------|
| macOS / Linux（install 脚本） | `~/.local/etc/securemodelroute/smr.yaml` |
| macOS / Linux（直接运行 `smr`） | `~/.config/securemodelroute/smr.yaml` |
| Windows | `%APPDATA%\securemodelroute\smr.yaml` |

环境变量 `SMR_CONFIG` 可覆盖路径。上游 API Key 请用 `api_key_env` 引用环境变量，勿写入版本库。

## 配置说明

完整示例：[`config/smr.example.yaml`](config/smr.example.yaml)。

```yaml
server:
  listen: "127.0.0.1:8080"
  default_fallback_group: high

pipeline:
  security_enabled: true
  dlp_enabled: true
  operation_security_mode: enforce   # observe | enforce
  builtin_credential_presets: true

fallback_groups:
  high:
    - id: openai-primary
      base_url: "https://api.openai.com/v1"
      model: "gpt-4o-mini"
      api_key_env: OPENAI_API_KEY
      protocol: openai              # openai | anthropic，可省略（按 URL 推断）
      timeout_secs: 120

content_rules:
  - id: example-secret
    enabled: true
    match_mode: full
    category: secret
    value: "请替换为占位符"

file_rules:
  - id: corp-docs
    path: /data/docs
    enabled: true
    recursive: true
    trigger_window: 5
    match_mode: fragment
    min_fragment_len: 32
    formats: [txt, md, json, yaml, rs, py]   # 可按需扩展
```

**`fallback_groups`** — 命名端点列表；默认组为 `server.default_fallback_group`。流式响应在发出首个 content token 后锁定当前端点，不再 fallback。

**`content_rules`** — 内存中的全文/片段规则，作用于请求/响应 JSON 字段。用于密钥、短语，以及**无扩展名**、无法靠后缀索引的敏感内容。

**`file_rules`** — 需索引与保护的目录或文件。注意：

- **`formats`** — 扩展名**不带**前导点（如 `txt`、`md`）。可按语料增加任意后缀；仅列出的后缀参与索引。
- **无后缀文件** — 不会被 `formats` 选中，请用 **`content_rules`** 单独防护。
- **`trigger_window`** — tool 提及受保护文件触发 SessionGuard 后，对后续多少次请求继续脱敏。
- **`index`** — 分块大小、Bloom、并发、haystack 上限、ripgrep 预过滤等（见 `config/smr.example.yaml` 或旧版文档中的 `file_rules[].index` 示例）。

**`operation_rules`** / **`path_protection_rules`** — 操作关键字与路径级拦截级别。

```bash
export OPENAI_API_KEY=sk-...
export ANTHROPIC_API_KEY=sk-ant-...
export SMR_CONFIG=/path/to/smr.yaml
```

## 文件 DLP 索引（概要）

大文本语料通过磁盘索引扩展，避免整库载入内存。

**索引根目录：** `~/.config/securemodelroute/file-index/{rule_id}/`（随平台配置目录变化）。

| 文件 | 作用 |
|------|------|
| `current.json` | 当前生效世代 |
| `gen/{n}/index.db` | 签名 → 文件偏移与长度 |
| `gen/{n}/bloom.bin` | 内存 Bloom 预过滤 |
| `gen/{n}/files.json` | 文件 mtime/size，用于增量重建 |
| `gen/{n}/manifest.json` | 构建统计 |
| `gen/{n}/literals.json` | ripgrep 预过滤样本（可选） |

**运行流程：**

1. 每条启用的 `file_rules` 在后台建索引；`/api/status` 中 `file_index_ready` 为 true 后参与 DLP。
2. **tool_call** / **tool_result** 命中受保护**具体文件**时激活 SessionGuard。
3. 随后 `trigger_window` 次请求内，对 JSON 字段做 haystack 扫描：Bloom → SQLite 候选 → 读源文件校验 → 脱敏。

SessionGuard 仅保存规则元数据，不在内存中持有整库明文。

## Web 管理界面

地址：**`http://127.0.0.1:8080/ui`**（端口以 `server.listen` 为准）。

| 页面 | 功能 |
|------|------|
| 概览 | 代理 URL、默认组、DLP/操作安全状态、文件索引是否就绪 |
| 模型路由 | 三组 fallback，拖拽排序；保存后热更新 |
| 数据防泄露 | 文件保护区 + 内容保护区（标签） |
| 重要路径防护 | 路径与防护级别 |
| 操作拦截 | 行为类型 + 关键字 |
| 日志 | SQLite 请求审计 + 实时事件 |
| 高级 YAML | 完整配置编辑、保存、从磁盘重载 |

**国际化：** 界面支持 **English** 与 **中文**，可在页头切换语言。

**管理 API（节选）：**

| 方法 | 路径 | 说明 |
|------|------|------|
| GET | `/api/status` | 监听地址、安全开关、索引状态、代理 URL |
| GET/PUT | `/api/config` | 读写 YAML；PUT 保存并热加载 |
| GET | `/api/events?limit=50` | 近期 DLP/拦截等事件 |
| GET | `/api/audits?limit=50` | 请求审计 |
| PUT | `/api/reload` | 从磁盘重载（保留 SessionGuard） |

## 流量正文快照（调试）

用于核对脱敏与路由效果，可将代理前后的 JSON 正文落盘：

```yaml
logging:
  level: info
  redact_content: true
  save_traffic_bodies: true      # 默认 false
  traffic_max_body_bytes: 32768
```

开启 `save_traffic_bodies` 后，在大小限制内保存经代理的 JSON 正文，便于排查；仅在可信环境使用，生产环境请关闭。可在高级 YAML 或配置中的 `logging` 段修改。

## 开发与测试

```bash
cargo test
cargo clippy -- -D warnings
./scripts/verify.sh
```

对接真实上游进行功能/黑盒测试前，复制示例密钥文件（已 gitignore）：

```bash
cp test_model_api_key.example.txt test_model_api_key.txt
# 编辑 test_model_api_key.txt 填入密钥，切勿提交
```

```bash
python3 scripts/install_functional_test.py
python3 scripts/blackbox_test.py
./scripts/run_all_tests.sh          # macOS/Linux
.\scripts\run_all_tests.ps1         # Windows
```

实现进度见 [TODO.md](TODO.md)。旧版中文 README 备份：[docs/README.legacy.zh-CN.md](docs/README.legacy.zh-CN.md)。

## 许可证

MIT
