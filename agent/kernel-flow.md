# Kernel Flow: AI Agent 内核开发管线

本框架将内核补丁开发流程编排为一条自动化管线。用户完成需求讨论后，管线按阶段自动执行，由编排 Agent（主控）派生独立子 Agent 完成每个阶段。

## 流程概览

```
discuss → freeze manifest
    ↓
[自动化] develop (patch + build)
    → ┬─ verify (build quality gate) ─┐
      └─ review  (code logic gate)   ─┘  ← 并行执行
        → ANY REJECT → develop (循环)
        → ALL PASS   → push → test (deploy + verify)
```

## 角色体系

| 角色 | 职责 | SOP 文件 |
|------|------|----------|
| Orchestrator | 读 state/pipeline，派生子 Agent，推进状态机 | `agent/roles/orchestrator.md` |
| Developer | 应用 patch、修 comment、编译内核、review 通过后 push | `agent/roles/developer.md` |
| Reviewer | 对照 comment 审阅 diff，产出 PASS/REJECT 裁决 | `agent/roles/reviewer.md` |
| Debugger | 部署到目标机、跑测试、远端调试 | `agent/roles/debugger.md` |

执行每个阶段前，主控必须读取对应角色的 SOP 并将其作为子 Agent 的 prompt 核心。

## 编排协议

### 1. 冻结输入

讨论结束后，产出 `agent/runs/<id>/manifest.yaml`（见 `agent/manifest-example.yaml`）。后续所有阶段只认这份文件，不再 reinterpret 聊天。

### 2. 状态管理

运行时状态保存在 `agent/runs/<id>/state.json`：

```json
{
  "run_id": "20250406-1",
  "current_stage": "develop",
  "round": 1,
  "stages_completed": [],
  "history": [],
  "last_error": null
}
```

每次阶段转换后，主控向 `history` 追加记录（阶段名、轮次、verdict、时间戳），用于 `/kernel:status` 展示运行轨迹和诊断反复出现的问题模式。

重试上限由 `pipeline.toml` 中每个 gate 的 `max_iterations` 控制（如 verify: 3, review: 5），不再使用全局 `max_rounds`。

### 3. 主控 Agent 执行循环

每一轮：

1. **读 `state.json`** → 获取 `current_stage`。
2. **读 `pipeline.toml`** → 获取该阶段的 `role`、`inputs`、`outputs`、`type`。
3. **检查 `parallel_group`** → 若当前阶段有此字段，收集同组所有阶段。
4. **对每个阶段**（单个或并行组内所有成员）：
   - 验证 inputs → 所有必选输入文件必须存在，缺失则停止。
   - 读 `agent/roles/<role>.md` → 获取角色 SOP 全文。
   - 组装子 Agent 的 prompt = SOP 全文 + inputs 内容。
5. **派生子 Agent**（Task 工具）：
   - **单个阶段** → 一次 Task 调用。
   - **并行组** → 在**同一条消息**中发出多个 Task 调用（确保真正并行）。
   - **Developer**: `readonly: false`，可改代码、跑终端。
   - **Verifier**: `readonly: true`，可在 tmux 中跑构建检查命令。
   - **Reviewer**: `readonly: true`，只读，禁止改代码。
   - **Debugger**: `readonly: false`，需要 SSH 到目标机。
6. **收取子 Agent 返回** → 检查 outputs 是否产出。
7. **若 gate 类型**：
   - 读 outputs 文件，找到 `## Verdict` 标题，提取其后第一个 `PASS` 或 `REJECT` 单词（忽略空行、代码围栏、空白）。
   - **单个 gate**: `PASS` → 推进，`REJECT` → 回退到 `on_reject`，`round++`。
   - **并行 gate 组**: ALL PASS → 推进到组后首个阶段；ANY REJECT → 回退，`round++`。多个 REJECT 时 developer 同时收到所有反馈，一轮修完。
   - `round` 超过 rejecting gate 的 `max_iterations` → 管线停止，报告失败，等待人工介入。
8. **更新 `state.json`**。

### 4. Agent 间通信

通信方式：**共享文件（inputs/outputs）**。

```
agent/runs/<id>/
  manifest.yaml              # 冻结的输入（讨论产出）
  state.json                 # 运行时状态（主控读写）
  pipeline.toml              # 管线配置（从模板复制）
  build-result.json          # developer 产出 → verifier/debugger 消费
  verify-result.json         # verifier 产出 → developer 消费（REJECT 时）
  push-result.json           # developer 产出 → debugger 消费
  reviews/
    maintainer-review.md     # reviewer 产出 → developer 消费（REJECT 时）
  test-report.md             # debugger 产出（最终报告）
```

每个 Agent **只写自己阶段的 outputs**，**只读 pipeline 声明的 inputs**。主控通过文件存在性和内容判定阶段完成。

并行组（如 `quality-gate`）中的 Agent 互不读取对方的 outputs——verifier 不读 review，reviewer 不读 verify-result。两者独立执行，由主控合并判定。

### 5. 安全约束

- Reviewer 以 `readonly: true` 派生，**不可修改任何文件**。
- Push 阶段只允许 manifest 中声明的 remote 和分支前缀。
- 目标机 reboot 仅在 `manifest.yaml` 中 `allow_reboot: true` 时允许。
- 每个 gate 的 `max_iterations` 防止无限循环。

## 关键文件

| 文件 | 用途 |
|------|------|
| `agent/kernel-flow.md` | 本文档：编排协议 |
| `agent/templates/kernel-flow.toml` | 管线模板（阶段定义） |
| `agent/templates/state-template.json` | state.json 初始模板 |
| `agent/roles/orchestrator.md` | 主控角色 SOP |
| `agent/roles/developer.md` | 开发角色 SOP |
| `agent/roles/reviewer.md` | 审阅角色 SOP |
| `agent/roles/reviewer-refs/` | 审阅参考文件（技术模式、误报指南、调用链分析等） |
| `agent/roles/reviewer-refs/subsystems/` | 子系统专用 guide（50+，按触发路径加载） |
| `agent/roles/debugger.md` | 远端部署/测试/调试角色 SOP |
| `agent/manifest-example.yaml` | manifest 示例 |
| `agent/runs/<id>/manifest.yaml` | 单次运行的冻结输入 |
| `agent/runs/<id>/state.json` | 单次运行的状态 |
| `agent/runs/<id>/pipeline.toml` | 单次运行的管线配置（从模板复制） |
