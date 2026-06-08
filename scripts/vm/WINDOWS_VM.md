# Windows x86_64 虚拟机（macOS 开发机）

SafeRoute 的 Windows 目标是 **x86_64（64 位）**，不是 32 位 i686。在 Apple Silicon Mac 上，用 **UTM + QEMU** 跑完整的 x86_64 Windows（速度较慢，但与 `x86_64-pc-windows-gnu` 产物一致）。

## Windows 原生机（推荐）

在 Windows x86_64 上可直接跑完整发布周期（无需 UTM）：

```powershell
Set-ExecutionPolicy Bypass -Scope Process -Force
cd C:\path\to\LLM-SafeRoute
.\scripts\windows\release-cycle.ps1
```

**macOS 原生机**：

```bash
./scripts/release-cycle.sh
```

统一说明：`.cursor/skills/release-cycle/SKILL.md`（macOS + Windows 全流程）。脚本细节见 `scripts/macos/README.md`、`scripts/windows/README.md`。

## 快速开始

```bash
cd /path/to/LLM-SafeRoute
chmod +x scripts/vm/setup-windows-vm.sh scripts/vm/download-win11-iso.py

# 一键（需网络；UTM 可用 App Store / brew / GitHub DMG）
./scripts/vm/setup-windows-vm.sh all
```

分步：

| 步骤 | 命令 |
|------|------|
| 安装 UTM | `./scripts/vm/setup-windows-vm.sh install-utm` |
| 下载 Win11 x64 ISO | `./scripts/vm/setup-windows-vm.sh download-iso` |
| 创建虚拟机 | `./scripts/vm/setup-windows-vm.sh create-vm` |

## 安装 Windows（首次，需图形界面）

1. 打开 **UTM**，启动虚拟机 `SafeRoute-Win11-x64`（已有旧 VM 名可通过 `SMR_VM_NAME` 覆盖）。
2. 按安装向导完成 Windows 11（建议语言 English，版本 Windows 11 Pro 或 Home 均可）。
3. 创建本地 Windows 用户（与 `config/test.env` 中 `SMR_WINDOWS_USER` 一致，建议管理员组）。
4. 在 VM 内以**管理员**运行（仅安装 Rust；**OpenSSH 见下节手动配置**）：

```powershell
cd C:\Users\Public\smr-test   # 或把仓库拷进 VM 后
Set-ExecutionPolicy Bypass -Scope Process -Force
.\scripts\vm\windows-post-install.ps1
```

5. 按 **§ SSH 手动配置** 配置免密 SSH。

6. 复制 `config/test.env.example` → `config/test.env`，设置：

```bash
SMR_WINDOWS_HOST=windows-vm              # ~/.ssh/config 中的 Host 别名
SMR_WINDOWS_USER=your-windows-ssh-user   # VM 内登录账户
SMR_GUEST_STAGING=C:/Users/your-windows-ssh-user/smr-staging
```

7. 在 Mac 上交叉编译并远程测试：

```bash
./scripts/package-windows.sh
./scripts/windows_vm_test.sh
```

## SSH 调试（Mac → UTM 来宾）

在 Mac 的 `~/.ssh/config` 中为 VM 配置 Host 别名（与 `SMR_WINDOWS_HOST` 一致），例如：

```
Host windows-vm
  HostName <VM-LAN-IP>    # Bridged 下常见 192.168.x.x，用 arp -a 查询
  User <SMR_WINDOWS_USER>
  IdentityFile ~/.ssh/id_ed25519
```

```bash
ssh windows-vm
```

API 密钥与 VM 账户信息仅写在本地 **gitignored** 的 `config/test.env`（模板 `config/test.env.example`）；勿提交密码或密钥。

**Administrators 组成员**的公钥在 `C:\ProgramData\ssh\administrators_authorized_keys`。

### SSH 手动配置（新 VM / 重装后）

在 VM 内以 **管理员** PowerShell 手动操作（使用 `SMR_WINDOWS_USER` 对应账户）：

1. 设置 → 应用 → 可选功能 → 添加 **OpenSSH 服务器**（若未安装）
2. `services.msc` → **OpenSSH SSH Server** → 自动 + 启动
3. 将 Mac 公钥内容追加到 `C:\ProgramData\ssh\administrators_authorized_keys`（单行 `ssh-ed25519 AAAA... comment`）
4. 确认 ACL（仅 `Administrators:(F)` 与 `SYSTEM:(F)`，无继承）：
   ```powershell
   icacls C:\ProgramData\ssh\administrators_authorized_keys
   ```
5. **不要**随意修改 `C:\Users\<SMR_WINDOWS_USER>\.ssh\` 或其 `authorized_keys`（除非明确知道后果）
6. Mac 上配置 `~/.ssh/config` Host `windows-vm`，测试：`ssh windows-vm echo ok`

常用调试脚本：

| 脚本 | 说明 |
|------|------|
| `scripts/vm/win-ssh-sdk-probe.ps1` | SafeRoute + OpenAI SDK 黑盒片段 |
| `scripts/vm/windows-app-installed-test.ps1` | 完整安装后 30 项黑盒 |

**安装测试注意**：stage 目录只需 `SafeRoute.exe`；脚本会校验 staged 与 installed 的 exe 大小一致，并核对 GUI 进程路径。

## 资源建议（Apple Silicon + x86_64 模拟）

| 项目 | 建议 |
|------|------|
| 内存 | 8 GB（脚本默认） |
| 磁盘 | 64 GB |
| CPU | 4 核 |
| 网络 | **Bridged**（与 Mac 同网段，便于 SSH） |

## Homebrew 权限

若 `brew install --cask utm` 报 `/usr/local` 不可写，可修复后重试：

```bash
sudo chown -R "$(whoami)" /usr/local/Cellar /usr/local/Homebrew /usr/local/bin /usr/local/lib /usr/local/share /usr/local/var/homebrew
```

或从 [Mac App Store 安装 UTM](https://apps.apple.com/app/utm-virtual-machines/id1538878817)，或手动下载 [UTM.dmg](https://github.com/utmapp/UTM/releases)。

## ISO 手动下载

自动下载失败时，从 [Windows 11 下载页](https://www.microsoft.com/software-download/windows11) 下载 **Windows 11 (multi-edition ISO) x64**，保存为：

`~/VMs/SafeRoute-Win11-x64/isos/Win11_24H2_English_x64.iso`

## 更快替代（可选）

**Windows 11 ARM** 在 Apple Silicon 上原生虚拟化，速度快很多；多数 **x86_64** CLI（如 `smr.exe`）可通过 WOW64 运行。若仅需跑 `windows_vm_test.sh` 安装包测试，可考虑 ARM 镜像；完整平台验证仍推荐 x86_64 来宾系统。

## 相关脚本

| 脚本 | 说明 |
|------|------|
| `scripts/vm/setup-windows-vm.sh` | 主机侧安装 UTM / ISO / 建 VM |
| `scripts/vm/windows-post-install.ps1` | VM 内可选 Rust（**不**配置 OpenSSH） |
| `scripts/package-windows.sh` | macOS 交叉编译 `dist/smr-*-windows-x86_64.zip` + `dist/smr.exe` |
| `scripts/vm/package-windows-gui.sh` | UTM 来宾内构建 `SafeRoute.exe` + **Tauri NSIS** |
| ~~`scripts/vm/package-windows-setup.sh`~~ | **已删除** — 旧 IExpress，请用 NSIS |
| `scripts/vm/utm-run-test.sh` | 上传 zip，来宾安装 + 功能测试（11 项） |
| `scripts/vm/utm-run-all-tests.sh` | 功能 + 黑盒 + 压测（windows-user SSH） |
| `scripts/vm/utm-run-app-blackbox.sh` | 托盘 GUI 安装后黑盒（30 项） |
| `scripts/vm/clean-vm-artifacts.ps1` | VM 内清理 build/test 中间文件 |
| `scripts/vm/win-ssh-sdk-probe.ps1` | SSH 上快速 SDK/安装探测 |
| `scripts/windows_vm_test.sh` | 从 Mac **SSH** 部署 zip 并 `install.ps1` / `verify.ps1` |

来宾测试通过 **windows-user SSH**（`scripts/vm/vm-ssh.sh`）；`windows_vm_test.sh` 为同一路径的 zip 安装测试。
