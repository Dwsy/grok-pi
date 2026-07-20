---
id: "2026-07-20-可选-PSM-resume-数据源"
title: "可选 PSM resume 数据源"
status: "in_progress"
created: "2026-07-20"
updated: "2026-07-20"
category: "adapter"
tags: ["adapter", "resume", "psm", "sqlite"]
---

# Issue: 可选 PSM resume 数据源

## Goal

为 `grok-pi /resume` 增加**默认关闭**的 PSM SQLite 加速路径：仅在功能已启用且 PSM 已启动时读取 PSM 的只读 SQLite catalog；任何未运行、缺库、锁定、schema 不兼容或查询失败均回退现有 Pi JSONL 扫描。

## 背景/问题

现有 `publish_session_catalog()` 在 adapter 中阻塞线程扫描 Pi JSONL，数据正确但大目录延迟随文件数增长。外部参考项目 `resume-x` 证明 PSM 已维护 `sessions.db` 与 token/cost 汇总，但它的数据库存在检查不能证明 PSM 正在运行，且其 TUI 实现不能进入本项目。

## 验收标准

- [ ] 默认配置下 `/resume` 不打开 PSM 数据库，行为与当前 JSONL catalog 完全一致。
- [ ] 功能开启且 PSM 运行证明成立时，`current`/`all` catalog 从 PSM SQLite 的 `sessions` 与 `session_details_cache` 查询，保留 Pi 会话路径、命名、模型、token/cost 与最近活动排序。
- [ ] PSM 未启动、数据库不存在/不可读、SQLite busy/损坏、schema 缺字段或查询异常时，不显示错误 UI、不阻塞 resume，回退 JSONL。
- [ ] adapter 保持 headless；不复制 `resume-x` 代码、不修改 Pi JSONL、不启动或控制 PSM 进程。
- [ ] 测试覆盖开关关闭、PSM 可用及全部回退分支。

## 实施阶段

### Phase 1: 规划和准备
- [x] 核对 `resume-x` 的 SQLite 查询字段和只读用途。
- [ ] 核对 PSM 的运行态证明、schema 迁移与 WAL 锁策略。
- [x] 确定默认关闭的 F2/config 开关接缝。

### Phase 2: 执行
- [ ] 在 adapter 中实现只读 PSM catalog provider 与严格 schema 校验。
- [ ] 在 `publish_session_catalog()` 中实现：开关 → PSM 运行态 → SQLite；其余路径 → JSONL。
- [ ] 将启用状态接入 Pager 的可选设置，并保留不可用时的静默回退。

### Phase 3: 验证
- [ ] provider 单元测试（SQLite fixture + 回退）。
- [ ] `cargo test -p pi-grok-adapter`。
- [ ] `cargo check -p xai-grok-pager-bin --bin grok-pi` 与 scope/diff 审查。

## 关键决策

| 决策 | 理由 |
|---|---|
| PSM 只作为 read-through catalog | Pi 仍拥有 session 文件和 `switch_session` 语义；SQLite 仅降低列表读取成本。 |
| fail closed 至 JSONL | PSM 是可选性能层，不能降低 `/resume` 可用性或准确性。 |
| 不复用 `resume-x` 实现 | 用户授权其作为机制参考；本项目须使用 Rust adapter + 原生 Pager。 |
| F2 持久开关，默认 Off | 用户明确选择 F2；功能可发现且跨启动保存，未开启时零 SQLite I/O。 |

## 相关资源

- `~/Dev/AI/pi-session-manager/extensions/resume-x/README.md`
- `~/Dev/AI/pi-session-manager/extensions/resume-x/lib/db.ts`
- `crates/codegen/pi-grok-adapter/src/pi_adapter.rs`
- `crates/codegen/pi-grok-adapter/src/model.rs`

## Notes

`resume-x` 以 `better-sqlite3` 读取 `~/.pi/agent/sessions/sessions.db`：`sessions` 提供 catalog，`session_details_cache` 提供模型与 usage 汇总。它仅判断数据库存在；本需求额外要求 PSM 已启动，必须以 PSM 本身的可验证运行态契约为准。

---

## Status 更新日志

- **[2026-07-20]**: 状态变更 → in_progress，开始核对 PSM 运行态与 SQLite 契约。
