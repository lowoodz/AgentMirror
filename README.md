# SecureModelRoute

本地大模型安全路由代理：**LLM API 自动 fallback**、**数据泄露防护（DLP）**、**操作安全（请求/响应双侧）**、**重要路径防护**，内置 Web 管理界面与可选系统托盘桌面应用。

## 功能

| 模块 | 说明 |
|------|------|
| **大模型路由** | 多组 fallback（如 high/medium/low），组内按顺序自动切换；上游错误、malformed JSON、流式未出首 token 时触发 fallback；OpenAI ↔ Anthropic 请求/响应双向协议转换 |
| **DLP（请求侧）** | **内容保护区**：全文/片段匹配（`min_fragment_len` + `min_fragment_ratio`），Secret 类随机替换并尽量保持英文大小写；**文件保护区**：磁盘索引（SQLite 签名 + Bloom 预过滤 + mmap 校验），流式分块建索引，SessionGuard 仅保存规则元数据；`notify` 监听变更并重建 |
| **SessionGuard** | 仅在 **tool_call / tool_result** 文本中检测受保护路径；响应侧 tool_call 也可触发；触发后按 `trigger_window` 对后续 N 次请求持续脱敏 |
| **操作安全** | 请求侧与响应侧 **tool 相关字段** 均检查；`observe`（仅记录）/ `enforce`（拦截）；规则按 command_exec / api_call / network_access + 关键字 |
| **路径防护** | 独立规则：`deny_delete` / `deny_modify` / `deny_access`；目录自动覆盖子路径；与操作安全共用拦截逻辑 |
| **内置凭证预设** | `pipeline.builtin_credential_presets: true` 时匹配 `sk-`、`AKIA`、`ghp_` 等前缀模板（全文匹配，不杀片段） |
| **Web 管理界面** | 见下节；保存即写 YAML 并热加载，SessionGuard 状态保留 |
| **桌面应用（可选）** | Tauri 托盘应用：**内嵌** `smr` 服务；关主窗口隐藏到托盘/菜单栏，进程与服务不退出 |
| **审计与持久化** | 结构化请求审计写入 SQLite；内存事件日志可通过 API 查询 |
| **SSE 流式** | 流式响应中增量解析 tool_calls 并做操作安全/DLP；首个 content token 发出后锁定当前 endpoint，不再 fallback |

## 安全流水线（实现概要）

**请求进入代理：**

1. 解析 JSON，检测客户端协议（OpenAI / Anthropic）。
2. 若开启操作安全：仅对 tool 相关字段做危险行为匹配（含路径防护）。
3. 若开启 DLP：注册路径触发器；对会话内已激活的 SessionGuard 做内容/文件片段脱敏。
4. 按 fallback 组转发；必要时经 UnifiedRequest 做跨协议请求转换。

**响应返回客户端：**

1. 非流式：检查 tool_calls（操作安全 + 响应侧 SessionGuard 触发）；按客户端协议做响应格式转换。
2. 流式（SSE）：边转发边扫描；操作安全/DLP 在 chunk 管道中处理；首 token 前允许组内 fallback。

全局总开关：`pipeline.security_enabled`（Web 界面右上角「安全防护总开关」）。关闭后 DLP 与操作安全均不生效。

## 大语料文件 DLP（磁盘索引）

面向 **16GB 内存 / 8 核笔记本、约 10GB 文本语料** 的可扩展方案（替代原「全量读入 RAM + Aho-Corasick」）：

| 阶段 | 内容 | 状态 |
|------|------|------|
| **P0** | 磁盘索引：流式分块 → xxHash 签名 → SQLite；Bloom 预过滤；命中后 mmap/读文件校验；SessionGuard 仅存 `FileRule` | **已实现** |
| **P1** | 增量索引：按文件 mtime/size 跳过未变文件；`gen/{generation}/` 世代目录 + `current.json` 原子切换 | **已实现** |
| **P2** | 扫描优化：haystack 分块 + 多线程 Bloom；ripgrep 字面量预过滤（≥8KB haystack） | **已实现** |
| **P3** | 超大 haystack（>2MB 单字段）流式扫描与配额 | 规划中 |

**索引目录：** `~/.config/securemodelroute/file-index/{rule_id}/`

| 文件 | 说明 |
|------|------|
| `current.json` | 当前生效世代指针 |
| `gen/{generation}/index.db` | 签名表 `(sig_hash, path, byte_offset, byte_len)` |
| `gen/{generation}/bloom.bin` | 签名 Bloom 过滤器（建索引后加载到内存） |
| `gen/{generation}/files.json` | 各文件 mtime/size 指纹（增量比对用） |
| `gen/{generation}/manifest.json` | 世代、签名数、文件数、skipped/reindexed 统计 |
| `gen/{generation}/literals.json` | rg 预过滤用字面量样本（建索引时从签名抽取） |

**运行时流程：**

1. 后台线程为每条启用的 `file_rules` 建索引；`/api/status` 的 `file_index_ready` 为 true 后生效。
2. tool_call / tool_result 文本命中受保护路径 **且提到具体文件** → SessionGuard 激活（**仅扫描 tool 中点名的文件**，不扫同目录其它文件；多条规则时仍取最具体路径规则）。
3. 后续 N 次请求（`trigger_window`）对 JSON 提取字段做 haystack 扫描：Bloom → SQLite 查候选 → 读源文件字节校验 → 脱敏。

**YAML 可调参数（`file_rules[].index`）：**

```yaml
file_rules:
  - id: corp-docs
    path: /data/docs
    enabled: true
    recursive: true
    trigger_window: 5
    match_mode: fragment
    min_fragment_len: 32
    formats: [txt, md, json, yaml, rs, py]
    index:
      chunk_size: 8192          # 建索引分块大小
      chunk_overlap: 64
      signature_stride: 128     # 块内采样步长
      signatures_per_chunk: 16
      max_full_file_bytes: 524288   # 小于此且 full 模式则整文件索引
      max_haystack_bytes: 2097152   # 单字段最大扫描长度
      bloom_megabytes: 64
      build_workers: 8
      scan_workers: 4              # haystack 分块并行 Bloom 扫描线程数
      scan_rg_prefilter: true      # ≥8KB haystack 启用 ripgrep 字面量预过滤
      scan_rg_literals_max: 2048   # 预过滤字面量样本上限
```

## Web 管理界面

地址：`http://127.0.0.1:8080/ui`（端口以 `server.listen` 为准）

| 页面 | 功能 |
|------|------|
| **概览** | 代理 URL、默认组、DLP/操作安全状态、文件索引是否就绪 |
| **模型路由** | 三组 fallback 卡片，拖拽排序；🟢 OpenAI / 🔵 Anthropic 协议标识 |
| **数据防泄露** | 文件保护区（拖入路径）+ 内容保护区（标签输入）；底部 **保存 DLP** 同时保存两类规则 |
| **重要路径防护** | 路径 + 防护级别表格；独立 **保存路径防护** |
| **操作拦截** | 行为类型 + 关键字规则表格 |
| **日志** | 请求审计（SQLite）+ 实时事件 |
| **高级 YAML** | 完整配置编辑、保存并应用、从磁盘重载 |

## 快速安装

### 方式一：源码一键安装（开发/本机）

```bash
chmod +x scripts/install.sh
./scripts/install.sh           # CLI → ~/.local/bin
./scripts/install.sh --service # 额外：无 GUI 时 macOS LaunchAgent 后台服务
./scripts/install.sh --gui     # CLI + 托盘桌面应用
./scripts/install.sh --all     # CLI + 托盘 GUI + 登录自启（--background，仅托盘）

securemodelroute               # 启动 CLI 并打开浏览器
```

### 方式二：解压发布包

```bash
# Apple Silicon
tar -xzf dist/smr-*-darwin-arm64.tar.gz -C /tmp/smr-arm64 && cd /tmp/smr-arm64 && ./install.sh

# Intel Mac
tar -xzf dist/smr-*-darwin-x86_64.tar.gz -C /tmp/smr-x64 && cd /tmp/smr-x64 && ./install.sh
```

### 方式三：桌面应用

**macOS：**

```bash
./scripts/install.sh --all
# 或 SMR_BUILD_GUI=1 ./scripts/install.sh
# 或解压 dist/smr-*-darwin-*-app.tar.gz → 拖入「应用程序」
```

**Windows 一键安装（推荐）：**

```powershell
# dist/SecureModelRoute-*-x64-Setup.exe（IExpress，内含 CLI + 托盘 GUI）
.\SecureModelRoute-0.1.0-x64-Setup.exe
```

构建：`./scripts/package-windows-setup.sh`（需先 `package-windows.sh` + `package-windows-desktop.sh`）

**Windows 分步安装：**

```powershell
.\install.ps1 -All      # CLI + 托盘 GUI + 登录启动项（--background）；有 GUI 时不装计划任务
.\install.ps1 -Service  # 仅无 GUI 时的登录计划任务 + 崩溃重启
.\install.ps1 -Gui        # 仅托盘 GUI
.\install.ps1             # 仅 CLI
```

**托盘行为（macOS / Windows 相同）：**

- 桌面应用启动时 **内嵌** HTTP 服务，无需单独再跑 `smr`。
- 关闭主窗口 → 隐藏到菜单栏/系统托盘，**服务保持运行**。
- `--background` / `--tray-only`：启动后不显示主窗口（用于登录自启）。
- 环境变量 `SMR_CONFIG` 可指定配置文件路径（GUI 与 CLI 均支持）。

在 macOS 上交叉编译的 CLI zip 使用 **GNU**（`x86_64-pc-windows-gnu`）；Windows 本机/UTM 内 GUI 使用 **MSVC**。两者均为 x86_64，可并存。

### 打包

```bash
./scripts/package-all.sh              # macOS CLI×2 + app + Windows CLI + 桌面 + Setup（UTM 运行时）
./scripts/package-macos.sh            # 仅 macOS
./scripts/package-windows.sh          # Windows CLI zip（GNU 交叉编译）
./scripts/package-windows-desktop.sh  # Windows 便携 exe（UTM 内 MSVC 构建）
./scripts/package-windows-setup.sh    # Windows Setup.exe（UTM + IExpress）
```

**`dist/` 主要产物：**

| 文件 | 说明 |
|------|------|
| `smr-*-darwin-arm64.tar.gz` / `smr-*-darwin-x86_64.tar.gz` | macOS CLI 发布包 |
| `smr-*-darwin-*-app.tar.gz` | macOS `SecureModelRoute.app` |
| `target/release/bundle/dmg/*.dmg` | macOS 安装镜像（package-macos 附带产出） |
| `smr-*-windows-x86_64.zip` | Windows CLI（GNU） |
| `smr-*-windows-x86_64-app.zip` | Windows 便携 `SecureModelRoute.exe` |
| `SecureModelRoute-*-x64-Setup.exe` | Windows 一键安装 |
| `smr-*-windows-x86_64-full.zip` | 仅含 Setup.exe 的完整包 |

x86_64 Windows 桌面在 macOS 上构建：`./scripts/package-windows-desktop.sh`（默认 ARM UTM 来宾交叉编译 x86_64）；Windows 本机：`.\scripts\package.ps1`。

### Windows x86_64（本机构建）

```powershell
.\scripts\package.ps1
Expand-Archive dist\smr-*-windows-x86_64.zip -DestinationPath $env:TEMP\smr -Force
cd $env:TEMP\smr
.\install.ps1 -All
securemodelroute
```

**功能对等（macOS ↔ Windows）：**

| 能力 | macOS | Windows |
|------|-------|---------|
| CLI 代理 + DLP + 操作安全 + 路径防护 | ✓ | ✓ |
| 内嵌 Web 管理界面 `/ui` | ✓ | ✓ |
| 托盘桌面应用（内嵌服务） | ✓ `.app` | ✓ `SecureModelRoute.exe` |
| 关窗隐藏到托盘 | ✓ | ✓ |
| 登录自启 | LaunchAgent `--background` | 启动文件夹快捷方式 `--background` |
| 无 GUI 后台服务 | LaunchAgent | 计划任务（`-Service` 且无 `-Gui`） |
| 服务日志 | `~/.local/etc/.../smr.log` | `%USERPROFILE%\.local\etc\...\smr.log` |
| 一键测试 | `./scripts/run_all_tests.sh` | `.\scripts\run_all_tests.ps1` |
| UTM 全量测试 | `./scripts/run_full_tests.sh` | `./scripts/vm/utm-run-all-tests.sh` |
| 安装后黑盒测试 | `./scripts/run_installed_app_tests.sh` | 同上（含 Windows UTM 阶段） |

**macOS 交叉编译 Windows CLI：**

```bash
brew install mingw-w64
./scripts/package-windows.sh
# 或无 mingw：brew install zig && cargo install cargo-zigbuild
```

**UTM 来宾测试（无需 SSH）：**

```bash
./scripts/vm/utm-run-all-tests.sh       # zip 安装 + 功能 + 黑盒 + 压测
./scripts/vm/utm-run-app-blackbox.sh    # 托盘 GUI 安装后 25 项黑盒
```

**SSH 远程测试（可选）：** `./scripts/windows_vm_test.sh` — 见 [scripts/vm/WINDOWS_VM.md](scripts/vm/WINDOWS_VM.md)

## 路径说明

| 路径 | 说明 |
|------|------|
| `~/.local/bin/smr` | 主程序（install 脚本，macOS/Linux） |
| `%USERPROFILE%\.local\bin\smr.exe` | 主程序（Windows install） |
| `~/.local/bin/securemodelroute` | CLI 启动器（`--open` 打开管理界面） |
| `%USERPROFILE%\.local\bin\securemodelroute.cmd` | Windows CLI 启动器 |
| `~/.local/etc/securemodelroute/smr.yaml` | 配置（install 脚本写入） |
| `~/.config/securemodelroute/smr.yaml` | 配置（直接运行 `smr` / GUI 默认） |
| `%APPDATA%\securemodelroute/smr.yaml` | Windows 直接运行时的默认配置 |
| `~/.config/securemodelroute/data/smr.db` | 请求审计 SQLite |
| `~/.config/securemodelroute/file-index/` | 文件 DLP 磁盘索引（按 rule_id 子目录） |
| `http://127.0.0.1:8080/health` | 健康检查 |
| `http://127.0.0.1:8080/ui` | Web 管理界面 |
| `http://127.0.0.1:8080/v1` | OpenAI 兼容代理 |
| `http://127.0.0.1:8080/v1/messages` | Anthropic Messages API |

## 配置

示例见 `config/smr.example.yaml`。endpoint 可用 `api_key` 或 `api_key_env`：

```yaml
pipeline:
  security_enabled: true
  dlp_enabled: true
  operation_security_mode: enforce   # observe | enforce
  builtin_credential_presets: true

fallback_groups:
  high:
    - id: my-model
      base_url: "https://api.example.com/v1"
      model: "gpt-4o-mini"
      api_key_env: OPENAI_API_KEY
      protocol: openai               # openai | anthropic，可省略（按 URL 推断）
      timeout_secs: 120
```

环境变量示例：

```bash
export OPENAI_API_KEY=sk-...
export ANTHROPIC_API_KEY=sk-ant-...
export SMR_CONFIG=/path/to/smr.yaml   # 覆盖默认配置路径
```

## 管理 API

| 方法 | 路径 | 说明 |
|------|------|------|
| GET | `/api/status` | 监听地址、默认组、安全开关、文件索引状态、代理 URL |
| GET/PUT | `/api/config` | 读写完整 YAML 配置（PUT 保存并热加载） |
| GET | `/api/events?limit=50` | 内存事件（DLP/拦截/错误等） |
| GET | `/api/audits?limit=50` | SQLite 请求审计 |
| PUT | `/api/reload` | 从磁盘重载配置（保留 SessionGuard） |

## 客户端接入

将 LLM 客户端的 `base_url` 指向本地代理：

```python
from openai import OpenAI
client = OpenAI(base_url="http://127.0.0.1:8080/v1", api_key="dummy")
```

Anthropic SDK 示例：`base_url="http://127.0.0.1:8080"`，路径 `/v1/messages`。

可选请求头：

- `X-SMR-Fallback-Group`：fallback 组名（如 `high`）
- `X-SMR-Session-Id`：会话 ID（SessionGuard 与审计关联；未传则自动生成）

## 开发与验证

```bash
export CARGO_TARGET_DIR="$PWD/target"
cargo test
cargo clippy -- -D warnings
./scripts/verify.sh                         # 单元测试 + health/status/ui 冒烟

# 需要 test_model_api_key.txt（gitignore，勿提交）
python3 scripts/install_functional_test.py    # 安装级功能（11 项）
python3 scripts/blackbox_test.py              # 黑盒（24 项；SMR_ATTACH=1 可附着已运行实例）
python3 scripts/live_test.py                  # 压测
./scripts/run_all_tests.sh                    # 本机全套
./scripts/run_full_tests.sh                   # 本机 + Windows UTM（zip + 来宾运行中）
./scripts/run_installed_app_tests.sh          # 从 dist 安装托盘应用后黑盒（macOS + 可选 Windows UTM）

# 压测参数（可选）
SMR_STRESS_TOTAL=50 SMR_STRESS_STREAM_TOTAL=20 python3 scripts/live_test.py
SMR_STRESS_SOAK_SEC=60 python3 scripts/live_test.py
SMR_SKIP_VM_TESTS=1 ./scripts/run_full_tests.sh   # 跳过 UTM 阶段
```

实现进度与方案对齐见 [TODO.md](TODO.md)。

## 许可证

MIT
