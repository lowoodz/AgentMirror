# SecureModelRoute

本地大模型安全路由代理：**LLM API 自动 fallback**、**数据泄露防护（DLP）**、**操作安全（响应侧拦截）**，内置 Web 管理界面。

## 功能

| 模块 | 说明 |
|------|------|
| **大模型路由** | 高/中/低多组 fallback，组内自动切换；OpenAI / Anthropic 协议自动检测与跨协议转换 |
| **DLP（请求侧）** | 内容规则 + 文件路径规则（txt/md/docx/pdf/pptx），SessionGuard 触发窗口 |
| **操作安全（响应侧）** | 检查模型返回 tool_calls，observe / enforce 模式 |
| **Web GUI** | 概览 / 路由 / 配置 / 日志，保存即热加载 |
| **桌面 GUI（可选）** | Tauri 原生窗口，内嵌管理界面 |
| **SSE 流式** | 流式响应中的 tool_calls 安全检查 |
| **grep 核心库** | 文件 DLP 内存级固定字符串匹配（内嵌 `grep-matcher` / `grep-regex`，无需安装 `rg`） |

## 快速安装

### 方式一：一键安装（推荐）

```bash
chmod +x scripts/install.sh
./scripts/install.sh          # 安装 CLI 到 ~/.local
./scripts/install.sh --service # 额外：macOS 开机自启 (LaunchAgent)

securemodelroute              # 启动并打开浏览器管理界面
```

### 方式二：解压发布包

```bash
tar -xzf dist/smr-0.1.0-darwin-*.tar.gz -C /tmp
cd /tmp && ./install.sh
```

### 方式三：桌面应用（macOS）

```bash
SMR_BUILD_GUI=1 ./scripts/install.sh   # 构建并安装 SecureModelRoute.app 到「应用程序」
# 或解压 dist/smr-*-app.tar.gz 后拖入「应用程序」
```

## 路径说明

| 路径 | 说明 |
|------|------|
| `~/.local/bin/smr` | 主程序 |
| `~/.local/bin/securemodelroute` | 启动器（`--open` 打开管理界面） |
| `~/.local/etc/securemodelroute/smr.yaml` | 配置文件（install 脚本） |
| `~/.config/securemodelroute/smr.yaml` | 配置文件（直接运行 `smr` 时默认） |
| http://127.0.0.1:8080/ui | Web 管理界面 |
| http://127.0.0.1:8080/v1 | LLM 代理地址（OpenAI 兼容） |

## 配置 API Key

编辑 `smr.yaml` 中的 endpoint，或使用环境变量：

```bash
export OPENAI_API_KEY=sk-...
export ANTHROPIC_API_KEY=sk-ant-...
```

## 客户端接入

将 LLM 客户端的 `base_url` 指向本地代理：

```python
from openai import OpenAI
client = OpenAI(base_url="http://127.0.0.1:8080/v1", api_key="dummy")
```

可选请求头：

- `X-SMR-Fallback-Group`: `high` / `medium` / `low`
- `X-SMR-Session-Id`: 会话 ID（用于文件路径 DLP 触发窗口）

## 开发与验证

```bash
export CARGO_TARGET_DIR="$PWD/target"
cargo test                    # 15 项测试
cargo clippy -- -D warnings
./scripts/verify.sh           # 端到端健康检查
./scripts/package.sh          # 生成 dist/smr-*.tar.gz
SMR_BUILD_GUI=1 ./scripts/install.sh  # 含桌面应用
```

## 许可证

MIT
