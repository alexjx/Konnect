# Workflow-first MVP 实现 — 2026-08-20

- [x] 审核当前所有未提交改动并运行基线验证
- [x] 提交全部当前改动并创建 annotated baseline tag
- [x] P0：实现 exposure profile、workflow 状态/错误/typed operations
- [x] P1：实现聚合只读 `inspect_design`
- [x] P2：实现单文件 schematic plan/apply/verify MVP
- [x] P3：实现 PCB move/rotate plan/apply/verify MVP
- [x] 更新工具目录、README/DEV、bundled Skills 与配置示例
- [x] 增加 unit、protocol、fixture/mock 与相关回归测试
- [x] 运行完整验证、审查 diff，并提交 MVP

## Review

- 基线提交：`36e2532 feat: extend verified live editing capabilities`
- 基线标签：`pre-workflow-mvp-20260820`
- 暴露模式：legacy 18 个默认工具；expert 增加 7 个流程；workflow 仅 7 个流程 + 2 个观测工具。
- 原理图：typed plan、目标零写入、SHA-256 stale 拒绝、单次原子写、并发幂等、同快照解析/校验。
- PCB：精确文档绑定、live hash、courtyard 投影与 baseline-delta 检查、多 footprint 单 KiCad commit、verify 后才 save。
- 最终验证：`cargo fmt --all -- --check`、`cargo test --workspace`、`cargo clippy --workspace --all-targets`、`git diff --check` 全部退出码 0。
- 真实 KiCad E2E 仍由 weekly/release workflow 执行；本地无真实编辑器会话，因此本轮以 IPC mock 证明事务形状。
