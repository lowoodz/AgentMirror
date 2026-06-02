# Windows x86_64 虚拟机（macOS 开发机）

SecureModelRoute 的 Windows 目标是 **x86_64（64 位）**，不是 32 位 i686。在 Apple Silicon Mac 上，用 **UTM + QEMU** 跑完整的 x86_64 Windows（速度较慢，但与 `x86_64-pc-windows-gnu` 产物一致）。

## 快速开始

```bash
cd /path/to/SecureModelRoute
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

1. 打开 **UTM**，启动虚拟机 `SecureModelRoute-Win11-x64`。
2. 按安装向导完成 Windows 11（建议语言 English，版本 Windows 11 Pro 或 Home 均可）。
3. 创建本地 Windows 用户（记下用户名，用于 `config/test.env` 里的 `SMR_WINDOWS_USER`）。
4. 在 VM 内以**管理员**运行：

```powershell
cd C:\Users\Public\smr-test   # 或把仓库拷进 VM 后
Set-ExecutionPolicy Bypass -Scope Process -Force
.\scripts\vm\windows-post-install.ps1
```

5. 在 Mac 上把公钥拷到 VM：

```bash
ssh-copy-id <windows-user>@<VM的局域网IP>
```

6. 复制 `config/test.env.example` → `config/test.env`，设置：

```bash
SMR_WINDOWS_HOST=<VM IP 或 ~/.ssh/config 里的 Host 别名>
SMR_WINDOWS_USER=<Windows 用户名>
```

7. 在 Mac 上交叉编译并远程测试：

```bash
./scripts/package-windows.sh
./scripts/windows_vm_test.sh
```

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

`~/VMs/SecureModelRoute-Win11-x64/isos/Win11_24H2_English_x64.iso`

## 更快替代（可选）

**Windows 11 ARM** 在 Apple Silicon 上原生虚拟化，速度快很多；多数 **x86_64** CLI（如 `smr.exe`）可通过 WOW64 运行。若仅需跑 `windows_vm_test.sh` 安装包测试，可考虑 ARM 镜像；完整平台验证仍推荐 x86_64 来宾系统。

## 相关脚本

| 脚本 | 说明 |
|------|------|
| `scripts/vm/setup-windows-vm.sh` | 主机侧安装 UTM / ISO / 建 VM |
| `scripts/vm/windows-post-install.ps1` | VM 内 OpenSSH + Rust（SSH 测试用） |
| `scripts/package-windows.sh` | macOS 交叉编译 `dist/smr-*-windows-x86_64.zip` |
| `scripts/vm/package-windows-gui.sh` | UTM 来宾内构建 `SecureModelRoute.exe` |
| `scripts/vm/package-windows-setup.sh` | UTM 来宾内 IExpress 产出 `Setup.exe` |
| `scripts/vm/utm-run-test.sh` | 上传 zip，来宾安装 + 功能测试（11 项） |
| `scripts/vm/utm-run-all-tests.sh` | 功能 + 黑盒 + 压测（无需 SSH） |
| `scripts/vm/utm-run-app-blackbox.sh` | 托盘 GUI 安装后黑盒（25 项） |
| `scripts/windows_vm_test.sh` | 从 Mac **SSH** 部署 zip 并 `install.ps1` / `verify.ps1` |

来宾代理测试默认通过 **utmctl file push/exec** 完成，不依赖 SSH；`windows_vm_test.sh` 为可选的 SSH 路径。
