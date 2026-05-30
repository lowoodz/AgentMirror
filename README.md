# SecureModelRoute

本地大模型安全路由代理：**LLM API 自动 fallback**、**数据泄露防护（DLP）**、**操作安全（响应侧拦截）**，内置 Web 管理界面。

## 功能

| 模块 | 说明 |
|------|------|
| **大模型路由** | 高/中/低多组 fallback，组内自动切换；OpenAI / Anthropic 协议自动检测与跨协议转换 |
| **DLP（请求侧）** | 内容规则 + 文件路径规则（txt/md/docx/pdf/pptx），SessionGuard 触发窗口 |
| **操作安全（响应侧）** | 检查模型返回 tool_calls，observe / enforce 模式 |
| **路径防护** | 按路径配置禁止删除 / 修改 / 访问，复用操作安全拦截 |
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
# Apple Silicon
tar -xzf dist/smr-*-darwin-arm64.tar.gz -C /tmp/smr-arm64 && cd /tmp/smr-arm64 && ./install.sh

# Intel Mac
tar -xzf dist/smr-*-darwin-x86_64.tar.gz -C /tmp/smr-x64 && cd /tmp/smr-x64 && ./install.sh
```

一键打全平台包：

```bash
./scripts/package-all.sh   # darwin-arm64 + darwin-x86_64 + windows-x86_64
```

### 方式三：桌面应用

**macOS：**

```bash
SMR_BUILD_GUI=1 ./scripts/install.sh   # 构建并安装 SecureModelRoute.app 到「应用程序」
# 或解压 dist/smr-*-app.tar.gz 后拖入「应用程序」
```

**Windows：**

```powershell
.\scripts\install.ps1 -Gui           # 构建 Tauri 桌面窗口，安装到「开始菜单」
# 或运行 package.ps1 后安装包内的 SecureModelRoute-*-setup.exe（NSIS 安装器）
```

桌面应用与 Web GUI 相同，内嵌 `http://127.0.0.1:8080/ui`；需先启动 `smr` 服务（`securemodelroute` 或 `-Service`）。

### Windows x86_64

**在 Windows 上构建并安装：**

```powershell
# 需要 Rust (https://rustup.rs) 与 PowerShell 5+
.\scripts\package.ps1
Expand-Archive dist\smr-*-windows-x86_64.zip -DestinationPath $env:TEMP\smr -Force
cd $env:TEMP\smr
.\install.ps1              # 安装到 %USERPROFILE%\.local\bin
.\install.ps1 -Service     # 可选：登录时自动启动（计划任务，失败自动重启 + 日志）
.\install.ps1 -Gui         # 可选：Tauri 桌面应用（开始菜单快捷方式）
securemodelroute           # 启动并打开管理界面
```

**功能对等（macOS ↔ Windows）：**

| 能力 | macOS | Windows |
|------|-------|---------|
| CLI 代理 + DLP + 操作安全 | ✓ | ✓ |
| 内嵌 Web 管理界面 `/ui` | ✓ | ✓ |
| Tauri 桌面窗口（可选） | ✓ `.app` | ✓ `.exe` / NSIS |
| 开机自启 + 崩溃重启 | LaunchAgent | 计划任务 |
| 服务日志 | `~/.local/etc/.../smr.log` | `%USERPROFILE%\.local\etc\...\smr.log` |
| 一键测试 | `./scripts/run_all_tests.sh` | `.\scripts\run_all_tests.ps1` |
| 全量（含 VM） | `./scripts/run_full_tests.sh` | — |

**在 macOS/Linux 上交叉编译 Windows 包：**

```bash
brew install mingw-w64       # macOS（推荐）
./scripts/package-windows.sh # 产出 dist/smr-*-windows-x86_64.zip

# 若无 mingw-w64，脚本会尝试 Zig + cargo-zigbuild：
#   brew install zig && cargo install cargo-zigbuild
```

**在 Windows 虚拟机中远程测试（OpenSSH）：**

```bash
# 确保 VM 可 SSH（~/.ssh/config 中 devserver 等），并已启动
./scripts/windows_vm_test.sh
# 或指定主机：SMR_WINDOWS_HOST=192.168.x.x ./scripts/windows_vm_test.sh
```

**在 macOS 上创建 Windows x86_64 测试 VM（UTM）：** 见 [scripts/vm/WINDOWS_VM.md](scripts/vm/WINDOWS_VM.md)，或运行 `./scripts/vm/setup-windows-vm.sh all`。

**验证（Windows）：**

```powershell
.\scripts\verify.ps1
.\scripts\run_all_tests.ps1   # 需 test_model_api_key.txt
```

## 路径说明

| 路径 | 说明 |
|------|------|
| `~/.local/bin/smr` | 主程序（macOS/Linux） |
| `%USERPROFILE%\.local\bin\smr.exe` | 主程序（Windows） |
| `~/.local/bin/securemodelroute` | 启动器（macOS/Linux，`--open` 打开管理界面） |
| `%USERPROFILE%\.local\bin\securemodelroute.cmd` | 启动器（Windows） |
| `~/.local/etc/securemodelroute/smr.yaml` | 配置文件（install 脚本） |
| `%USERPROFILE%\.local\etc\securemodelroute\smr.yaml` | 配置文件（Windows install 脚本） |
| `~/.config/securemodelroute/smr.yaml` | 配置文件（直接运行 `smr` 时默认） |
| `%APPDATA%\securemodelroute\smr.yaml` | 配置文件（Windows 直接运行 `smr.exe` 时默认） |
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
cargo test                    # 单元 + 集成测试
cargo clippy -- -D warnings
./scripts/verify.sh           # 单元测试 + 冒烟（health/status/ui）

# 需要 test_model_api_key.txt（gitignore，勿提交）
python3 scripts/install_functional_test.py  # 安装级功能冒烟（11 项）
python3 scripts/blackbox_test.py            # 黑盒场景（24 项）
python3 scripts/live_test.py                  # 压测（并发 chat + 流式）
./scripts/run_all_tests.sh                    # macOS/本机全部（verify + 以上）
./scripts/run_full_tests.sh                   # 含 Windows UTM VM（需 zip + UTM 运行中）

# 压测参数（可选）
SMR_STRESS_TOTAL=50 SMR_STRESS_STREAM_TOTAL=20 python3 scripts/live_test.py
SMR_STRESS_SOAK_SEC=60 python3 scripts/live_test.py   # 附加 soak
```

## 许可证

MIT
