# 参与贡献 do-anything

[English](CONTRIBUTING.md)

以下规则适用于每一次改动，不论提交者是人还是编程 Agent。两份文档内容不一致时，以英文版为准。

## 工作流程

1. 从 `main` 拉分支。一个 PR 只做一件事。
2. 提 PR 之前先在本地运行检查：

   ```sh
   cargo fmt --check
   cargo clippy --all-targets -- -D warnings
   cargo test
   ```

3. 按模板提 PR。CI 必须在 Linux、Windows、macOS 上全部通过。
4. PR 使用 squash 合并，PR 标题即提交信息。

## 提交信息与 PR 标题

使用 [Conventional Commits](https://www.conventionalcommits.org/)，英文书写：

```
<type>(<scope>): <summary>
```

- `type`：`feat`、`fix`、`refactor`、`perf`、`docs`、`test`、`build`、`ci`、`chore`。
- `scope` 可选，写模块名，例如 `core`、`http`、`cli`、`bundle`。
- `summary` 用祈使语气，不超过 72 个字符，结尾不加句号。
- 破坏性变更在类型或范围后加 `!`，并在页脚写 `BREAKING CHANGE:`。

示例：`feat(http): add JSON path assertions`

## PR 描述

每个 PR 都使用相同的章节。先写完整个英文块，再写整个中文块。两种语言各自使用独立标题：不要合并成 `## Summary / 摘要` 这样的双语标题，也不要在同一张列表或表格里中英交替。某一节没有内容时，英文写 `None`、中文写 `无`，不要删除该节。

| 英文章节 | 中文章节 | 内容 |
| --- | --- | --- |
| `## Summary` | `## 摘要` | 改了什么、为什么改，一到三句。 |
| `## Changes` | `## 变更` | 行为变化的列表。 |
| `## Verification` | `## 验证` | 实际运行的命令及其真实结果，以及没有验证的部分。 |
| `## Dependencies` | `## 依赖` | 新增、移除或升级的每个依赖，附下文要求的对比。 |
| `## Risks and follow-ups` | `## 风险与后续` | 已知缺口、风险和留待以后的工作。 |

`docs/releases/` 下的发布说明遵循同一规则：先一个使用独立标题的英文块，再一个使用独立标题的中文块。

描述的规则：

- 写真实的命令结果。没有运行过的检查不得写"已通过"。
- 写明没有验证的部分。
- 不出现本机路径、用户名、机器名或密钥。

模板见 [.github/PULL_REQUEST_TEMPLATE.md](.github/PULL_REQUEST_TEMPLATE.md)。

## 项目结构

| 路径 | 用途 |
| --- | --- |
| `src/main.rs` | 只做入口：解析参数、调用库、把结果映射为退出码。 |
| `src/exit.rs` | 定义退出码的唯一位置。 |
| `src/core/` | 领域中立的引擎：用例模型、注册表、事件总线、运行器、失败包、输出。 |
| `src/drivers/` | 每个驱动一个模块（`http`、`cli` 等），各自由一个 cargo feature 控制。 |
| `tests/` | 运行编译后二进制的集成测试。 |
| `tests/self/` | `da` 用来测试自己的用例。 |

`src/core/`、`src/drivers/` 和 `tests/self/` 由第一个用到它们的改动创建。

结构规则：

- `core` 不得引用 `drivers`，其类型中不出现 HTTP、Web 等特定领域的概念。
- 驱动只能通过注册表接入引擎。内置驱动与外部驱动使用同一套注册接口，没有私有通道。
- 每个步骤接收 JSON 参数、返回 JSON 结果，并为参数声明 JSON Schema。
- 退出码只来自 `src/exit.rs`。JSON 输出只在一处产生。

## 代码规范

- 使用 rustfmt 默认格式。Clippy 以 `-D warnings` 运行。
- 不写 `unsafe` 代码；`unsafe_code` lint 设为 `forbid`。
- 使用 `#[allow(...)]` 时必须加注释说明原因。
- 生产环境会执行到的路径上不使用 `unwrap`、`expect` 和 `panic!`，而是返回说明失败原因的错误。
- 每个面向用户的错误对应一个退出码，并告诉用户下一步该做什么。
- 标准输出只承载结果。使用 `--output json` 时，标准输出恰好是一个 JSON 对象。诊断信息写到标准错误。
- 公开项要有文档注释。注释解释原因，不复述代码。
- 不做臆测性的抽象：除注册表扩展点外，不为只有一个实现的东西定义 trait；不为永远不变的值增加配置项。
- 不得把密钥写入日志。输出、报告和失败包中的密钥要脱敏。
- 代码、注释、文档和提交信息使用英文。

跨平台规则：

- 用 `std::path` 拼接路径，不写死分隔符。
- 文本一律按 UTF-8 读写，不依赖控制台代码页。
- 启动进程时不假定特定的 shell。
- 仓库内的文件使用 LF 换行。

## 测试

- 新行为要带测试。修复缺陷时，先写一个在修复前会失败的测试。
- 既测成功路径，也测失败路径。
- 测试不调用外部网络服务或 AI 模型。需要服务时由测试自己启动本地服务。
- 不得为了让检查通过而放宽或删除断言。

## 依赖

先复用，后自研。实现新功能之前，先在 crates.io、lib.rs 和 GitHub 上搜索是否已有库实现。

逐项评估每个候选；存在多个候选时至少对比两个：

| 标准 | 检查内容 |
| --- | --- |
| 维护状态 | 最近约 12 个月内有发布；issue 有人回应。 |
| 采用程度 | 下载量和知名的使用方。 |
| 许可证 | MIT、Apache-2.0、BSD、ISC、Zlib 或 Unicode。不使用 copyleft 许可证。 |
| 体量 | 传递依赖数量、编译时间、对二进制体积的影响。 |
| 安全 | 没有未解决的 RustSec 安全公告。 |
| 平台 | 在 Linux、Windows、macOS 上都能工作。 |
| 质量 | 有测试和文档；性能满足我们的用途。 |

候选库满足以上标准时直接使用。只有出现下列情况之一才自己实现：

- 没有候选满足标准。
- 最好的候选存在影响我们的已确认缺陷或性能问题。需附上 issue 链接或测量数据。
- 需求很小（大约 50 行以内），引入依赖的代价大于收益。

添加依赖时只启用用到的 feature，并在 PR 的 Dependencies 一节记录：库名、用途、考虑过的替代方案、选择它的原因。选择自研时，记录被否决的库及原因。

CI 运行 [cargo-deny](https://github.com/EmbarkStudios/cargo-deny) 检查许可证、安全公告和依赖来源，配置见 `deny.toml`。
