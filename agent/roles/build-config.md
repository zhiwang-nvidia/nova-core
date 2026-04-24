# 共享构建配置

本文件定义了 developer 和 verifier 共用的构建标准。所有角色在执行构建相关操作时，必须遵守本文件的规则。

## 编译器

- 所有 `make` 命令必须带 `LLVM=1`，确保 C 和 Rust 使用同一 LLVM 后端。
- 使用 `LLVM=1` 时无需 `CROSS_COMPILE`（LLVM 通过 `ARCH` 推断目标架构）。

## 必需 .config 配置项

| 配置项 | 期望值 | 说明 |
|--------|--------|------|
| `CONFIG_CC_IS_CLANG` | `y` | 由 `LLVM=1` 自动设置 |
| `CONFIG_RUST` | `y` | Rust 支持 |
| `CONFIG_KUNIT` | `y` | KUnit 测试框架 |
| `CONFIG_RUST_KERNEL_DOCTESTS` | `y` | Rust doctest 支持 |
| `CONFIG_DRM` | `y` | DRM 子系统 |
| `CONFIG_FWCTL` | `y` | 固件控制框架 |
| `CONFIG_NOVA_CORE` | `m` | Nova 核心驱动（编译为模块） |
| `CONFIG_DRM_NOVA` | 不启用 | 暂不编译 DRM Nova 前端 |

## .config 生成流程

首次编译或配置变更时，按以下顺序一次性完成精简 + 必需项设置：

```bash
# 1. 用 streamline_config.pl 精简，只保留当前系统实际使用的模块
cp .config .config.full
lsmod | perl scripts/kconfig/streamline_config.pl > .config.stripped
mv .config.stripped .config

# 2. 叠加必需配置项（streamline 可能会丢掉这些）
./scripts/config --enable RUST
./scripts/config --enable KUNIT
./scripts/config --enable RUST_KERNEL_DOCTESTS
./scripts/config --enable DRM
./scripts/config --enable FWCTL
./scripts/config --module NOVA_CORE
./scripts/config --disable DRM_NOVA

# 3. 解决依赖
make LLVM=1 olddefconfig
```

验证命令：

```
grep -E '^CONFIG_RUST=|^CONFIG_KUNIT=|^CONFIG_RUST_KERNEL_DOCTESTS=|^CONFIG_DRM=|^CONFIG_NOVA_CORE=|^CONFIG_CC_IS_CLANG=|^CONFIG_FWCTL=' .config
```

**注意**: streamline + 必需项 + olddefconfig 必须作为一个整体执行，不要单独跑 streamline 后就开始编译。

## 编译命令

```
make LLVM=1 -j$(nproc)
```

## 质量检查

### Rust 工具链

```
make LLVM=1 rustavailable
```

- 通过标准: 输出 `Rust is available!`
- 必须在编译前执行。若失败，用 `rustup` 切换到内核要求的 rustc 版本。

### Clippy

```
make LLVM=1 CLIPPY=1 -j$(nproc) 2>&1 | tee /tmp/clippy-output.txt
```

- 通过标准: 零 clippy 警告。
- 扫描输出中的 `warning:` 行（排除 `warning: ... generated N warnings` 汇总行）。
- 有警告则必须修复后重新检查。

### rustfmt（内核 Rust 代码）

```
make LLVM=1 rustfmt
git diff --name-only -- '*.rs'
```

- 作用范围: 内核树中的 Rust 源码（`rust/`、`drivers/gpu/drm/nova/` 等），由内核构建系统的 `rustfmt` target 控制。
- 通过标准: `git diff --name-only -- '*.rs'` 无输出（所有 `.rs` 文件已格式化）。
- developer 应在提交前运行并修复格式问题。
- verifier 检查后用 `git checkout -- '*.rs'` 还原（不修改代码）。

### 编译零警告

```
make LLVM=1 -j$(nproc) 2>&1 | tee /tmp/build-output.txt
```

- 通过标准: 变更文件（`drivers/gpu/drm/nova/`、`rust/`）零编译警告。
- 内核树中其他子系统的已有警告归类为 **KNOWN**，不算失败，但必须在报告中列出具体内容（文件、行号、警告信息），以便人工确认。

### checkpatch

对变更的 patch 运行 checkpatch，与具体语言无关：

```
git diff HEAD~1 | scripts/checkpatch.pl --no-tree -
```

或对单个文件：

```
scripts/checkpatch.pl --no-tree -f <变更文件>
```

- 通过标准: 零 ERROR、零 WARNING。
- checkpatch 检查 patch 格式、编码风格、commit message 等，适用于所有文件类型（C、Rust 等）。
- 无变更时可跳过。

### Rust 文档（可选）

```
make LLVM=1 rustdoc
```

- 检查文档注释是否正确渲染。

### Rust Doctest

- 依赖 `CONFIG_KUNIT=y` 和 `CONFIG_RUST_KERNEL_DOCTESTS=y`（见上方必需配置项）。
- 编译后内核启动时，doctest 作为 KUnit 测试自动运行，`dmesg | grep kunit` 查看结果。
- 若需配置: `make LLVM=1 menuconfig` → *Kernel hacking → Rust hacking → Doctests for the `kernel` crate*。

## 项目结构

| 路径 | 说明 |
|------|------|
| `rust/` | Rust 内核抽象层（bindings、helpers、kernel crate） |
| `drivers/gpu/drm/nova/` | Nova GPU 驱动（Rust） |
