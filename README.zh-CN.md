<div align="center">

# test-anything

**一个二进制，测试一切——为编程 Agent 和与它们协作的人而做。**

用 YAML 写测试，用 `ta` 运行，得到明确的结论、精确的退出码和机器可读的证据。

[![CI](https://github.com/LnYo-Cly/test-anything/actions/workflows/ci.yml/badge.svg)](https://github.com/LnYo-Cly/test-anything/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/LnYo-Cly/test-anything?sort=semver)](https://github.com/LnYo-Cly/test-anything/releases/latest)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Rust 1.89+](https://img.shields.io/badge/rust-1.89%2B-orange.svg)](https://www.rust-lang.org)
![Platforms](https://img.shields.io/badge/platforms-Linux%20%7C%20macOS%20%7C%20Windows-lightgrey.svg)

[English](README.md) · **简体中文**

[安装](#安装) · [快速开始](#快速开始) · [使用指南](docs/guide.md) · [参与贡献](CONTRIBUTING.zh-CN.md)

</div>

---

## 为什么用 `ta`？

- **单个小巧的静态二进制。** 不需要运行时、包管理器，也不用下载浏览器。支持 Linux、macOS、Windows 的 x86_64 与 Arm64。
- **为 Agent 设计。** 每次运行都可以输出为一个 JSON 对象，每种结果都有独立的退出码，每次失败都会在磁盘上留下失败包，终端输出消失后 Agent 仍能读取。
- **声明式用例。** 步骤、断言、变量都写在普通 YAML 里，人能审阅，Agent 能编写。无效用例在执行前就被拒绝，并给出行号和列号。
- **天生跨层。** 动作从用户或客户端发起的地方开始（接口、命令，以后还有界面）；任何步骤的输出都可以用 JSONPath 断言。
- **插件化。** 内置步骤通过注册表接入，以后的外部驱动也走同一个注册表。核心不知道 HTTP 或进程的存在。
- **自己测试自己。** 项目自身的自测套件用 `ta` 编写，在 CI 的每个平台上运行。

## 安装

**Linux 和 macOS**

```sh
curl -fsSL https://raw.githubusercontent.com/LnYo-Cly/test-anything/main/install.sh | sh
```

**Windows（PowerShell）**

```powershell
irm https://raw.githubusercontent.com/LnYo-Cly/test-anything/main/install.ps1 | iex
```

脚本会按你的系统和 CPU 选择对应构建、按 `SHA256SUMS` 校验，且不需要管理员权限。

如果访问 `raw.githubusercontent.com` 较慢或无法访问，可改用 jsDelivr 镜像：

```sh
curl -fsSL https://cdn.jsdelivr.net/gh/LnYo-Cly/test-anything@main/install.sh | sh
```

```powershell
irm https://cdn.jsdelivr.net/gh/LnYo-Cly/test-anything@main/install.ps1 | iex
```

指定版本、下载镜像等选项见[安装说明](docs/guide.md#installation)；也可以在 [Releases](https://github.com/LnYo-Cly/test-anything/releases) 页面直接下载压缩包。

## 快速开始

创建 `cases/smoke.yaml`：

```yaml
name: GitHub API returns the repository
steps:
  - http.request:
      url: https://api.github.com/repos/LnYo-Cly/test-anything
      headers: { User-Agent: ta-demo }
    expect:
      - path: $.status
        equals: 200
      - path: $.body.license.spdx_id
        equals: MIT
```

再创建 `cases/git.yaml`，测试一个命令行程序：

```yaml
name: git is installed
steps:
  - cli.run:
      command: git
      args: ["--version"]
    expect:
      - path: $.exit_code
        equals: 0
      - path: $.stdout
        contains: "git version 3"
```

运行：

```console
$ ta run cases/
FAIL cases/git.yaml [git is installed] 70 ms
  cli.run: 1 of 2 assertions failed
    $.stdout contains "git version 3" (actual "git version 2.45.1\n")
  bundle: .ta/failures/1794581562319-43704/0-git-is-installed
PASS cases/smoke.yaml [GitHub API returns the repository] 1857 ms
Summary: 2 cases (1 passed, 1 failed, 0 errors), 2 steps (1 passed, 1 failed, 0 errors, 0 skipped), 1936 ms
$ echo $?
1
```

失败的用例指出了哪条断言失败、实际值是什么，证据保存在 `.ta/failures/…` 中。加上 `--output json` 可得到单个 JSON 对象而不是文本。

用例只对已经运行的程序或服务发起调用：`ta` 不负责启动、重启或停止它们。[被测服务](docs/guide.md#the-service-under-test)一节给出“先启动、再运行、最后停止”的模式，[在 Windows 上保存 JSON](docs/guide.md#capturing-json-on-windows)一节说明如何避免被 shell 重新编码。

## 现在能测什么

| 步骤 | 测试对象 | 可断言的结果 |
| --- | --- | --- |
| `http.request` | HTTP 与 HTTPS 接口 | `status`、`headers`、`body`（JSON 会被解析） |
| `cli.run` | 命令行程序（不经 shell 启动） | `exit_code`、`stdout`、`stderr`、`json` |
| `db.query` | 用 SQLite 或 PostgreSQL 读回前一步写入的数据 | `columns`、`rows`、`row_count`、`truncated` |

断言：在任意 [JSONPath](https://www.rfc-editor.org/rfc/rfc9535.html) 上使用 `equals`、`not_equals`、`contains`、`exists`。变量：`vars`、`save`、`--var name=value` 和 `--vars-file`，以 `${name}` 引用；`${env:NAME}` 读取环境变量，其值在输出与失败包中替换为 `***`。

用 `project`、`suite`、`layer` 和 `tags` 给用例分类，再用 `--project`、`--suite`、`--layer`、`--tag`、`--skip-tag` 选择要运行的部分。

用例默认逐个执行；`--jobs N` 同时执行最多 N 个用例，输出顺序与退出码不变。

用 `observe:` 声明观察者：每个步骤前后只读采集其他层的证据（内置 `log.file` 报告步骤期间日志文件新增的行），结果并入步骤结果的 `observed.<名字>`，可用现有 `expect` 断言并写入失败包。详见[观察者](docs/guide.md#observers)。

`db.query` 用一条只读语句把数据读回来：接口说成功时，用例还可以直接确认库里到底写了什么。SQLite 与 PostgreSQL 都支持，`INSERT`、`UPDATE`、`DELETE`、DDL 和一次多条语句一律被拒绝。详见[数据库查询](docs/guide.md#dbquery)。

## 给编程 Agent 用

`ta` 的设计目标是由 Agent 驱动，不需要人去看终端：

| 需要 | `ta` 提供 |
| --- | --- |
| 结论 | 退出码：`0` 通过、`1` 失败、`2` 用法错误、`3` 用例无效、`4` 环境错误、`5` 超时 |
| 细节 | `ta --output json run …`：一个 JSON 对象，包含每个步骤的结果和每条断言 |
| 运行后的证据 | 每个失败用例一个失败包：`case.yaml`、`report.json`、`meta.json`（最后写入） |
| 外部输入 | 用 `--var name=value` 传入地址、令牌和路径 |
| 不执行就检查用例 | `ta lint cases/`：与 `ta run` 相同的校验，一次报出所有文件的所有问题，不执行任何步骤 |
| 机器可读的用例格式 | `ta schema`：包含所有已注册步骤的 JSON Schema，编辑器也能直接使用 |

完整说明见[使用指南](docs/guide.md)：[用例格式](docs/guide.md#case-files)、[输出](docs/guide.md#output)、[失败包](docs/guide.md#failure-bundles)、[用例检查](docs/guide.md#linting)、[Schema](docs/guide.md#schema)、[退出码](docs/guide.md#exit-codes)。使用指南目前只有英文版。

## 文档

- [使用指南](docs/guide.md)——安装选项、用例格式、步骤、失败包、输出、退出码（英文）
- [发布说明](https://github.com/LnYo-Cly/test-anything/releases)
- [贡献指南](CONTRIBUTING.zh-CN.md)——工作流程、代码规范、依赖政策

## 参与贡献

欢迎提交 Issue 和 Pull Request。请先阅读 [CONTRIBUTING.zh-CN.md](CONTRIBUTING.zh-CN.md)（[English](CONTRIBUTING.md)），其中说明了提交与 PR 格式、项目结构以及如何选择新依赖。本地构建与测试：

```sh
cargo build
cargo test
```

## 许可证

[MIT](LICENSE)
