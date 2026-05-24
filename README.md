# SecureModelRoute

本地大模型安全路由代理：**LLM API 自动 fallback**、**数据泄露防护（DLP）**、**操作安全（响应侧拦截）**，内置 Web 管理界面。

## 功能

| 模块 | 说明 |
|------|------|
| **大模型路由** | 高/中/低多组 fallback，组内自动切换，OpenAI / Anthropic 协议检测 |
| **DLP（请求侧）** | 内容规则 + 文件路径规则，同类字符随机替换，保持长度不变 |
| **操作安全（响应侧）** | 检查模型返回的 tool_calls，拦截危险操作 |
| **Web GUI** | 访问 `http://127.0.0.1:8080/ui` 管理配置、查看日志 |
| **热加载** | GUI 或 API 保存配置后立即生效 |

## 安装

```bash
# 一键安装到 ~/.local
chmod +x scripts/install.sh
./scripts/install.sh

# 启动（自动打开管理界面）
securemodelroute
```

安装后：
- 二进制：`~/.local/bin/smr`
- 启动器：`~/.local/bin/securemodelroute`
- 配置：`~/.local/etc/securemodelroute/smr.yaml`

## 手动构建

```bash
cargo build --release
./target/release/smr --open
```

首次运行会在 `~/.config/securemodelroute/smr.yaml` 创建默认配置（若未指定 `--config`）。

## 配置 API Key

编辑配置文件，使用环境变量引用：

```bash
export OPENAI_API_KEY=sk-...
export ANTHROPIC_API_KEY=sk-ant-...
```

或在配置中直接填写 `api_key` 字段。

## 客户端接入

将 LLM SDK 的 `base_url` 指向本地代理：

```python
from openai import OpenAI
client = OpenAI(base_url="http://127.0.0.1:8080/v1", api_key="dummy")
```

可选请求头：

| Header | 说明 |
|--------|------|
| `X-SMR-Fallback-Group` | 指定 fallback 组（high/medium/low） |
| `X-SMR-Session-Id` | 会话 ID，用于文件 DLP 触发窗口 |

## 管理界面

启动后访问：**http://127.0.0.1:8080/ui**

- **概览**：代理地址、一键复制、运行状态
- **配置**：JSON 编辑，保存即热加载
- **日志**：DLP 替换、操作拦截、路由事件

## 项目结构

```
SecureModelRoute/
├── crates/
│   ├── smr-protocol/   # 协议检测、消息解析
│   ├── smr-core/       # 路由、DLP、操作安全、GUI、HTTP 服务
│   └── smr-cli/        # 入口 (smr)
├── config/smr.example.yaml
├── scripts/install.sh
└── scripts/package.sh
```

## 开发与测试

```bash
cargo test
cargo clippy -- -D warnings
./scripts/package.sh    # 生成 dist/smr-*.tar.gz
```

## 许可证

MIT
