<!--
Title: <type>(<scope>): <summary>  (Conventional Commits, English, at most 72 characters)
Write the whole English block first, then the whole Chinese block. Each language has
its own headings; never merge them into one bilingual heading such as "## Summary / 摘要",
and never alternate the two languages inside one list or one table.
Write "None" in an empty English section and "无" in an empty Chinese section; do not
delete the section.
No local paths, user names, machine names or secrets.
标题：<type>(<scope>): <summary>（Conventional Commits，英文，不超过 72 个字符）
先写完整个英文块，再写整个中文块。两种语言各自使用独立标题，
不要合并成 "## Summary / 摘要" 这样的双语标题，也不要在同一张列表或表格里中英交替。
某一节没有内容时，英文写 "None"、中文写 "无"，不要删除该节。
不出现本机路径、用户名、机器名或密钥。
-->

## Summary

<!-- What changed and why, in one to three sentences. -->

## Changes

<!-- One bullet per behavior change. -->

-

## Verification

<!-- Commands actually run and their real results. -->

| Command | Result |
| --- | --- |
| `cargo fmt --check` | |
| `cargo clippy --all-targets -- -D warnings` | |
| `cargo test` | |

**Not verified:**

## Dependencies

<!-- For each dependency added, removed or upgraded: purpose, alternatives considered, reason. -->

None

## Risks and follow-ups

None

## 摘要

<!-- 改了什么、为什么改，一到三句。 -->

## 变更

<!-- 每个行为变化一条。 -->

-

## 验证

<!-- 实际运行的命令及真实结果。 -->

| 命令 | 结果 |
| --- | --- |
| `cargo fmt --check` | |
| `cargo clippy --all-targets -- -D warnings` | |
| `cargo test` | |

**未验证：**

## 依赖

<!-- 新增、移除或升级的每个依赖：用途、考虑过的替代方案、选择原因。 -->

无

## 风险与后续

无
