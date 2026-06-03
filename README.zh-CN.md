# SafeRoute

**通往大模型智能的安全之路——更快、更稳、更放心。**

- SafeRoute 是一款轻量本地模型代理/路由，兼容 OpenAI / Anthropic 客户端协议，
- 将 IDE、Agent 或 SDK 的 `base_url` 指向 `http://127.0.0.1:8080/v1`，即可安全、可靠地调用/访问多个大模型，例如 GPT、Claude Opus、Gemini、DeepSeek、GLM、Kimi等，
- 无需手动切换，API 调用失败、Token额度不足、频率限制时，自动Fallback/回退，全程无中断。
- 同时提供数据防泄漏、数据脱敏、操作拦截、文件路径防护等安全保障，
- 满足个人用户使用LLM和Agent时，对安全、可靠的基本需求。

**English:** [README.md](README.md)

SafeRoute 管理界面 — 模型路由

---

## 产品定位


| 维度            | 说明                                                                                              |
| ------------- | ----------------------------------------------------------------------------------------------- |
| **Route（路由）** | `high` / `medium` / `low` 三组有序 fallback；上游失败、畸形 JSON、流式未出首 token 时自动切换；内置 OpenAI ↔ Anthropic 转换 |
| **Fast（性能）**  | Rust 实现、本地转发、原生流式；单文件配置、热加载；可选托盘应用，常驻不占桌面                                                       |
| **Safe（安全）**  | 内容/文件 DLP、tool 操作拦截、重要路径防护；总开关一键启停                                                              |


> 客户端改一行地址，本地起一个进程，走更稳、更快的模型通路。

---

## 快速开始

```bash
chmod +x scripts/install.sh
./scripts/install.sh --all     # CLI + 托盘 + 登录自启

securemodelroute               # 启动并打开管理界面
```

**Windows：** `.\install.ps1 -All`，然后 `securemodelroute`

**客户端：**

```python
from openai import OpenAI
client = OpenAI(base_url="http://127.0.0.1:8080/v1", api_key="dummy")
```


| 地址                                  | 用途                     |
| ----------------------------------- | ---------------------- |
| `http://127.0.0.1:8080/v1`          | OpenAI 兼容 API          |
| `http://127.0.0.1:8080/v1/messages` | Anthropic Messages API |
| `http://127.0.0.1:8080/ui`          | Web 管理界面               |
| `http://127.0.0.1:8080/health`      | 健康检查                   |


可选请求头：`X-SMR-Fallback-Group`（`high` / `medium` / `low`）、`X-SMR-Session-Id`（SessionGuard 与审计）。

---

## 功能

### 模型路由

- 三组 fallback，管理界面拖拽排序
- 请求头指定路由组
- 流式响应在首个 content token 前可 fallback
- 自动识别协议并做跨厂商映射

### 数据安全（DLP）

在模型访问/获取敏感数据前，自动脱敏，防止数据经模型泄露出去；

- **内容规则** — 全文/片段匹配密钥、短语及无后缀敏感串
- **文件规则** — 大语料磁盘索引（Bloom + SQLite + 字节校验），变更增量重建
- **SessionGuard** — tool 提及受保护文件后，后续 *N* 次请求持续脱敏（`trigger_window`）
- 可选内置凭证前缀模板（`sk-`、`AKIA`、`ghp_` 等）

### 操作安全

- 请求/响应侧 **tool 相关字段** 检查
- `observe` 仅记录 / `enforce` 拦截
- 按 command_exec、api_call、network_access + 关键字配置

### 路径防护

- `deny_delete` / `deny_modify` / `deny_access`；目录覆盖子路径

### 运维

- Web 管理 `/ui`（中/英）
- 可选 Tauri 托盘（macOS / Windows）
- SQLite 审计与实时事件
- 可选流量快照（调试，单文件最大 20 MiB）

总开关：`pipeline.security_enabled`（界面右上角）。

---

## 配置

示例：`[config/smr.example.yaml](config/smr.example.yaml)`

```yaml
server:
  listen: "127.0.0.1:8080"
  default_fallback_group: high

pipeline:
  security_enabled: true
  dlp_enabled: true
  operation_security_mode: enforce

fallback_groups:
  high:
    - id: primary
      base_url: "https://api.openai.com/v1"
      model: "gpt-4o-mini"
      api_key_env: OPENAI_API_KEY
    - id: fallback
      base_url: "https://api.anthropic.com/v1"
      model: "claude-sonnet-4-20250514"
      protocol: anthropic
      api_key_env: ANTHROPIC_API_KEY
```

**配置路径**


| 平台                        | 常见位置                                     |
| ------------------------- | ---------------------------------------- |
| macOS / Linux（install 脚本） | `~/.local/etc/securemodelroute/smr.yaml` |
| macOS / Linux（直接 `smr`）   | `~/.config/securemodelroute/smr.yaml`    |
| Windows                   | `%APPDATA%\securemodelroute\smr.yaml`    |


`SMR_CONFIG` 可覆盖路径。API Key 请用 `api_key_env`，勿提交明文。

**文件索引目录：** `{config_dir}/file-index/{rule_id}/`

**流量快照（仅调试）：**

```yaml
logging:
  save_traffic_bodies: true
  traffic_max_body_bytes: 20971520   # 20 MiB cap
```

文件位置：`{config_dir}/traffic/*.body`

---

## 管理界面

`http://127.0.0.1:8080/ui` — 概览、路由、DLP、路径、操作规则、日志、YAML 编辑。


| API                             | 说明             |
| ------------------------------- | -------------- |
| `GET /api/status`               | 监听地址、安全开关、索引状态 |
| `GET/PUT /api/config`           | 读写配置；PUT 热加载   |
| `GET /api/traffic`              | 快照列表           |
| `GET /api/traffic/{id}`         | 完整快照内容         |
| `GET /api/events`、`/api/audits` | 事件与审计          |


---

## 开发与测试

```bash
cargo test && ./scripts/verify.sh
cp config/test.env.example config/test.env   # gitignored；填写 SMR_GLM_API_KEY / SMR_DEEPSEEK_API_KEY
./scripts/run_all_tests.sh
```

旧版 README 备份：[docs/](docs/)。

---

## 许可证

MIT