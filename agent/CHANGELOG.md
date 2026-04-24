# Kernel Flow Agent 开发日志

## 2025-04-06 — 初始版本

### 架构设计

- 确定三层架构：**编排 Agent（Orchestrator）** 派生独立 **子 Agent**（Developer / Reviewer / Tester），通过 **Task 工具** 调度，不依赖 shell 脚本。
- Agent 间通信方式：**共享文件**（manifest / state / review / build-result / test-report），不用消息队列或进程间聊天。
- 管线定义使用 **TOML**（`pipeline.toml`），运行时状态用 **JSON**（`state.json`），讨论冻结输入用 **YAML**（`manifest.yaml`）。

### 创建的文件

| 文件 | 说明 |
|------|------|
| `CLAUDE.md` | 项目根入口，注册 `/kernel:discuss`、`/kernel:init`、`/kernel:run`、`/kernel:status` 命令 |
| `agent/kernel-flow.md` | 编排协议主文档（流程、角色、通信、安全约束） |
| `agent/manifest-example.yaml` | manifest 示例（编译配置、comment 列表、目标机、测试命令） |
| `agent/roles/orchestrator.md` | 主控角色 SOP：读 state → 读 pipeline → 加载 role → 派 subagent → 更新 state |
| `agent/roles/developer.md` | 开发角色 SOP：patch/comment → checkpatch → make → commit → push（review 通过后） |
| `agent/roles/reviewer.md` | 审阅角色 SOP：深度回归分析、分类变更、技术模式、误报消除、结构化输出 |
| `agent/roles/tester.md` | 测试角色 SOP：rsync/ssh 部署 → reboot → uname/dmesg → 测试命令 → 报告 |
| `agent/templates/kernel-flow.toml` | 管线阶段定义：develop → review(gate) → push → test |
| `agent/templates/state-template.json` | state.json 初始模板 |

### 角色体系演变

- 最初设计 5 个角色（Implementer / Builder / Reviewer / Deployer / Tester）。
- 用户决定精简为 **3 个角色**（Developer / Reviewer / Tester），Builder 和 Deployer 分别并入 Developer 和 Tester。
- 后来补充 **Orchestrator** 角色作为第 4 个，负责调度不负责实际工作。

### Reviewer 增强（引入 sashiko 体系）

从 `~/disk/src/sashiko/third_party/prompts/kernel/` 和 `~/disk/src/review-prompts/kernel/` 引入完整的内核 review 参考体系：

- **核心文件**：`review-core.md`（分析协议）、`false-positive-guide.md`（15 种误报模式 + 10 步验证 + 自我辩论）、`callstack.md`（调用链 9 Task 深度分析）、`technical-patterns.md`、`pointer-guards.md`、`severity.md`、`inline-template.md`
- **50+ 子系统 guide**：`subsystems/*.md`（networking、bpf、rcu、locking、mm-*、drm 等），按 diff 触发路径/符号按需加载
- **辅助文件**：`agent/*.md`（debug/fixes/lore 等 17 个）、`slash-commands/*.md`（kreview/kdebug/kverify 等 6 个）、`scripts/`（review_one.sh、claude_xargs.py 等）
- 两个源对比后用 **review-prompts 的更新版** 覆盖了有差异的文件（`false-positive-guide.md` 新增 MAINTAINERS 作者可信规则，`inline-template.md` 措辞更新）
- 所有参考文件放在 `agent/roles/reviewer-refs/` 下，reviewer.md 采用 **渐进加载** 方式引用

### Reviewer 规则调整

1. **Verdict 规则收紧**：任何 finding（包括 Low）都判 REJECT，只有零 finding 才 PASS。
2. **强制建设性修复建议**：每个 finding 必须附带 `Suggested Fix` 列，给出具体修复方法（代码片段/函数名/策略），让 developer 直接可操作。

### 实际测试

1. **简单 patch**（`0001-gpu-nova-core-fix-missing-colon-in-SEC2-boot-debug-m.patch`）：reviewer 子 Agent 正确判定 PASS，验证了格式参数、Fixes 标签、commit message 准确性。
2. **复杂 patch**（`0001-gpu-nova-core-wire-vGPU-mock-bootload-into-probe.patch`，7 文件 149 行）：reviewer 子 Agent 分 8 个 CHANGE 类别做深度分析，找到 3 个 Medium + 5 个 Low，排除 6 个误报，识别 3 个风险。

### 目录结构（当前）

```
orch/
  CLAUDE.md
  agent/
    CHANGELOG.md                         ← 本文件
    kernel-flow.md
    manifest-example.yaml
    roles/
      orchestrator.md
      developer.md
      reviewer.md
      tester.md
      reviewer-refs/
        README.md
        review-core.md
        false-positive-guide.md
        callstack.md
        technical-patterns.md
        pointer-guards.md
        severity.md
        inline-template.md
        missing-fixes-tag.md
        fixes-tag.md
        lore-thread.md
        debugging.md
        debugging-inline.md
        coccinelle.md
        review-stat.md
        sample.txt
        agent/                           # 17 个 agent 子任务文件
        slash-commands/                  # 6 个命令定义
        scripts/                         # 辅助脚本
        docs/                            # github-actions 集成文档
        examples/                        # review-stat 样例
        subsystems/                      # 50+ 子系统 guide
    templates/
      kernel-flow.toml
      state-template.json
    runs/                                # 运行时产物（gitignore）
```

### 待办 / 已知限制

- `runs/` 目录尚未在 `.gitignore` 中注册（需用户在内核树中自行添加）。
- Orchestrator 的「重试上限后人工介入」逻辑未在实际管线中端到端验证。
- mock instance 资源清理（reviewer 在测试中发现的 Medium #1）需要 developer 在后续迭代中处理。
- 子系统 guide 目前是静态拷贝，未与 sashiko 上游建立同步机制。
