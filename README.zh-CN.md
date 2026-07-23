# grok-pi — Pi 与 Grok Build 的 Remote TUI 桥接

> 在 Grok Build 原生终端 UI 中运行 Pi Agent Core。

[下载最新版本](https://github.com/Dwsy/grok-pi/releases/latest) · [English](README.md) · [功能矩阵](FEATURE_MATRIX.md) · [架构说明](NATIVE_GROK_TUI_ALIGNMENT.md) · [验证记录](VERIFICATION.md) · [更新日志](CHANGELOG.MD)

> **Remote TUI 桥接。** Pi 的交互式组件通过 Grok Build 原生 Pager 渲染，在保留 Grok 终端体验的同时接入 Pi 的扩展生态。Pi 用户获得 Grok Build 的原生 UI；Grok Build 用户获得 Pi 的模型、工具、会话和扩展能力。

`grok-pi` 将 Pi Agent Runtime 接入 Grok Build 原生 Pager。Pi 负责模型、工具、扩展、会话和 Agent 执行；Grok Pager 负责终端 UI，且是唯一的可见终端界面。

## 安装

### macOS / Linux

```bash
curl -fsSL https://github.com/Dwsy/grok-pi/releases/latest/download/install.sh | sh
```

### Windows

```powershell
irm https://github.com/Dwsy/grok-pi/releases/latest/download/install.ps1 | iex
```

安装脚本会按平台选择 release asset：

| 平台 | Asset |
|---|---|
| macOS Apple Silicon | `grok-pi-macos-aarch64.tar.gz` |
| macOS Intel | `grok-pi-macos-x86_64.tar.gz` |
| Linux x86_64 | `grok-pi-linux-x86_64.tar.gz` |
| Linux ARM64 | `grok-pi-linux-aarch64.tar.gz` |
| Windows x64 | `grok-pi-windows-x86_64.zip` |
| Windows ARM64 | `grok-pi-windows-aarch64.zip` |

默认路径：Unix → `~/.local/bin`；Windows → `%LOCALAPPDATA%\grok-pi\bin`。可用 `GROK_PI_INSTALL_DIR` 覆盖，用 `GROK_PI_VERSION=vX.Y.Z` 钉版本。

Unix 会创建 `pi-grok` 符号链接（Windows 为 `pi-grok.exe` 硬链/副本）：

```bash
grok-pi --help   # 原始名称
pi-grok --help   # 别名
```

`grok-pi` 需要 [Pi](https://pi.dev) **0.80.10 或更高版本**（系统 `pi` / pi.dev 安装器）：

```bash
# 推荐
curl -fsSL https://pi.dev/install.sh | sh
# Windows:
# powershell -c "irm https://pi.dev/install.ps1 | iex"
# 或 npm:
npm install --global @earendil-works/pi-coding-agent
```

Windows 上若旧版 `grok-pi.exe` 找不到裸名 `pi`，可显式指定 shim：

```powershell
$env:PI_BIN = "$env:LOCALAPPDATA\pi-node\current\pi.cmd"
grok-pi --pi-bin $env:PI_BIN
```

## 启动

在任意项目目录下直接运行：

```bash
grok-pi
# 或
pi-grok
```

默认使用 PATH 上的 `pi`，并以当前工作目录作为项目目录。继续上一会话：`grok-pi --continue`。

常用命令：

```bash
grok-pi --help
grok-pi update --check
grok-pi update
```

## 能力概览

| 领域 | 能力 |
|---|---|
| Agent Runtime | Pi 模型、Provider、工具、扩展、skills、会话、重试和压缩 |
| 终端 UI | Grok Pager 输入、斜杠补全、Markdown、工具卡片、diff、对话框和 scrollback |
| **Remote TUI 桥接** | Pi `ctx.ui.custom` 组件通过 Grok Build 原生 Pager 渲染，不创建第二套 TUI |
| Shell 执行 | Bash 集成、后台任务、输出限制、超时和进程树清理 |
| 并行工作 | Pi 子代理，支持前台/后台执行和原生任务视图 |
| Rhai Workflow | 上游 `xai-workflow` 宿主（F2 **Pi workflows**）；`/workflow`、`/workflows`、`/create-workflow`；脚本目录 `~/.grok-pi/workflows` 与 `<repo>/.grok-pi/workflows` |
| 会话流程 | Resume、树导航、标签、回顾、上下文查看和会话选择器 |
| 资源管理 | Pi 扩展、skills、prompt 和主题的原生管理器 |
| 更新 | 基于 GitHub Releases 的更新检查与安装 |

详细行为和有意边界见[功能矩阵（中文）](FEATURE_MATRIX.zh-CN.md) / [English](FEATURE_MATRIX.md)。

## 架构

```mermaid
flowchart LR
    User[终端用户] <--> Pager[Grok Pager\n原生 TUI]
    Pager <--> ACP[ACP]
    ACP <--> Adapter[pi-grok-adapter\nJSONL RPC ↔ ACP]
    Adapter <--> Pi[Pi\nAgent Core]
```

集成包含三个边界：

- **Grok Pager** 负责终端生命周期、输入、渲染、对话框和所有可见 UI。
- **Pi** 负责 Agent loop、模型、Provider、工具、扩展和会话。
- **`pi-grok-adapter`** 是 headless JSONL RPC ↔ ACP 桥接层，不拥有终端，也不渲染第二套 UI。

不修改 Pi 源码。Remote TUI 通过官方扩展 API 接入 Pi RPC 未暴露的能力，并将其投影到原生 Pager 承载面。

## 配置

稳定的内置桥接扩展默认启用；实验性原生命令需要显式开启。

| 变量 | 默认值 | 用途 |
|---|---:|---|
| `PI_GROK_REMOTE_TUI` | `1` | 启用 Pi `ctx.ui.custom` 组件 |
| `PI_GROK_BASH` | `1` | 启用 Grok-owned Bash 集成 |
| `PI_GROK_NATIVE_COMMANDS` | `0` | 启用实验性的 `/pi-*` 命令 |
| `GROK_HOME` | `~/.grok-pi` | 用户状态根目录（与 stock Grok 的 `~/.grok` 隔离） |
| `GROK_PROJECT_DIR` | `.grok-pi` | 仓库内项目配置/workflows/hooks 目录名 |
| `GROK_PI_NO_AUTO_UPDATE` | 未设置 | 禁用后台更新检查 |

Rhai Workflow **默认关闭**（F2 → Agent → **Pi workflows**，改完后需**整进程重启**）。细节见 [功能矩阵](FEATURE_MATRIX.zh-CN.md)、[AGENTS.md 产品态隔离](AGENTS.md#product-state-isolation)。

使用 `--no-extensions` 可关闭所有内置桥接扩展。Pi 启动参数可放在 `--` 之后直接传递：

```bash
grok-pi -- --model openai/gpt-4o
```

## 从源码构建

环境要求：Rust **1.92.0**、Node.js **22.19.0 或更高版本**、npm，以及系统 Pi 安装。

```bash
./build.sh
./target/debug/grok-pi
# 或: PI_BIN=pi ./run-local.sh
```

运行验证：

```bash
./verify.sh
```

静态检查与运行时验收的区别见[验证记录](VERIFICATION.md)。

## 文档

- [功能矩阵](FEATURE_MATRIX.zh-CN.md) —— 支持的行为与有意边界（[English](FEATURE_MATRIX.md)）
- [架构对齐](NATIVE_GROK_TUI_ALIGNMENT.md) —— 组件所有权、协议映射和迁移说明
- [验证记录](VERIFICATION.md) —— 已完成检查与环境阻塞项
- [更新日志](CHANGELOG.MD) —— 版本历史
- [贡献指南](CONTRIBUTING.md) —— 贡献流程

## 许可证

项目及上游声明见 [LICENSE](LICENSE) 和 [THIRD-PARTY-NOTICES](THIRD-PARTY-NOTICES)。
