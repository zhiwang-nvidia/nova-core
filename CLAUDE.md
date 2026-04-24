# Kernel Development Agent

本项目集成了 `agent/` 目录下的 AI Agent 内核开发管线。

## 命令

| 命令 | 用途 |
|------|------|
| `/kernel:discuss` | 与用户讨论需求（patch 内容、maintainer comment、目标机等），讨论结束后冻结为 manifest |
| `/kernel:init <id> [pipeline]` | 初始化一次运行：创建 `agent/runs/<id>/` 目录，复制指定 pipeline 模板和 state.json，提示填写 manifest |
| `/kernel:run <id>` | 按 orchestrator SOP 自动执行管线，派生子 Agent 完成各阶段 |
| `/kernel:status <id>` | 查看当前运行状态（stage、round、已完成阶段） |
| `/kernel:dev <任务描述>` | 直接以 developer 角色执行任务，不走完整管线 |
| `/kernel:verify` | 直接以 verifier 角色验证构建质量（LLVM、Clippy、警告、配置等） |

## 执行 `/kernel:discuss` 时

1. 与用户对话，澄清以下信息：
   - 要修什么（patch 文件 / maintainer comment 原文）
   - 基于哪个分支 / commit
   - 编译配置（ARCH、CROSS_COMPILE、defconfig）
   - 目标机（SSH 别名、部署方式、是否允许 reboot）
   - 测试命令与成功判据
2. 讨论结束后，生成 `agent/runs/<id>/manifest.yaml`（参考 `agent/manifest-example.yaml`）。
3. 让用户确认 manifest 内容无误后，视为冻结。

## 执行 `/kernel:init <id>` 时

1. 创建 `agent/runs/<id>/` 目录及 `agent/runs/<id>/reviews/` 子目录。
2. 根据 `[pipeline]` 参数选择模板，复制到 `agent/runs/<id>/pipeline.toml`：
   - 无参数或 `dvr`：`agent/templates/dev-verify-review.toml`（精简管线：develop → [verify + review]）
   - `full`：`agent/templates/kernel-flow.toml`（完整管线：develop → [verify + review] → push → test）
3. 复制 `agent/templates/state-template.json` → `agent/runs/<id>/state.json`，填入 `run_id`。
4. 若 `agent/runs/<id>/manifest.yaml` 不存在，提示用户先执行 `/kernel:discuss`。

## 执行 `/kernel:run <id>` 时

1. 读取 `agent/roles/orchestrator.md`，严格按其 SOP 执行。
2. 主控 Agent 不写内核代码、不做审阅判断、不 SSH 目标机。
3. 每个阶段通过 Task 工具派生独立子 Agent，子 Agent 的 prompt 包含：
   - 对应角色 SOP 全文（从 `agent/roles/<role>.md` 读取）
   - 该阶段所有 inputs 的文件内容
   - 内核树的工作目录路径
4. Gate 阶段解析 Verdict → PASS 推进 / REJECT 回退。
5. 每步更新 `agent/runs/<id>/state.json`。

## 执行 `/kernel:status <id>` 时

读取 `agent/runs/<id>/state.json`，输出当前阶段、轮次、已完成列表、最后错误。

## 执行 `/kernel:verify` 时

1. 读取 `agent/roles/verifier.md`，通过 Task 工具派生独立子 Agent 执行。
2. 子 Agent 的 prompt 包含 `verifier.md` SOP 全文和内核树路径。
3. 子 Agent 在 tmux session `kernel-verify` 中逐项运行检查。
4. 子 Agent 完成后返回 `verify-result.json` 内容，主控将结果反馈给用户。

## 执行 `/kernel:dev <任务描述>` 时

1. 读取 `agent/roles/developer.md`，按其 SOP 直接执行（不派生子 Agent）。
2. 无需 manifest、orchestrator、state.json，适用于快速开发任务（修 bug、改代码、解决编译错误等）。
3. 执行流程：
   - 理解用户给出的任务描述
   - 定位相关代码并修改
   - 按 developer SOP 执行 style check、工具链验证、编译验证
   - 将结果直接反馈给用户

## 框架文件

详见 `agent/kernel-flow.md`。

| 路径 | 说明 |
|------|------|
| `agent/kernel-flow.md` | 编排协议详细文档 |
| `agent/roles/orchestrator.md` | 主控角色 SOP |
| `agent/roles/developer.md` | 开发角色 SOP |
| `agent/roles/reviewer.md` | 审阅角色 SOP |
| `agent/roles/debugger.md` | 远端部署/测试/调试角色 SOP |
| `agent/roles/verifier.md` | 构建质量验证角色 SOP |
| `agent/roles/build-config.md` | 共享构建配置（developer 和 verifier 共用） |
| `agent/templates/kernel-flow.toml` | 完整管线模板（develop → [verify + review] → push → test） |
| `agent/templates/dev-verify-review.toml` | 精简管线模板（develop → [verify + review]） |
| `agent/templates/state-template.json` | state.json 初始模板 |
| `agent/manifest-example.yaml` | manifest 字段示例 |
| `agent/scripts/verify.sh` | 全量构建质量验证脚本（verifier 用） |
| `agent/scripts/quick-build.sh` | 快速编译+Clippy+警告收集脚本（developer 用） |
| `agent/scripts/check-commit-msgs.sh` | Commit message 规范检查脚本（developer 用） |
| `agent/scripts/match-subsystems.sh` | 子系统 guide 自动匹配脚本（reviewer 用） |
