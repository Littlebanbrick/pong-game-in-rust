# CLAUDE.md

本文件是人与 AI 协作的项目规范。每次会话开工前先通读本文件与 ROADMAP.md，确认当前阶段。

## 项目

Rust + ratatui 的终端 Pong 游戏。Cargo workspace 双 crate，前后端彻底分离，接口见 ARCHITECTURE.md。

## 分工

- **AI**：写全部代码与文档，维护本文件。
- **用户**：审阅每个 commit、试玩验收、做方向决策；可随时要求逐行讲解代码。

## 工作循环

1. 开工先读 CLAUDE.md 与 ROADMAP.md，确认当前阶段与未完成事项。
2. 一次只做一小步（一个可独立描述的改动），改完先向用户展示改动内容，**批准后才 commit**。
3. 阶段性成果（tag / release）必须等用户亲自验收后才发布。
4. 需求不明确时，用 grill-me 方式逐题追问（每次一个问题、给具体选项），达成共识再动手。

## 架构铁律

- `crates/pong-core`（后端）：纯游戏逻辑，禁止任何 I/O 与 UI 依赖。
- `crates/pong-tui`（前端）：渲染与输入，禁止包含任何游戏规则。
- 两 crate 只经消息通道通信，详见 ARCHITECTURE.md。
- 要改边界，先改 ARCHITECTURE.md，再动代码。

## 约定

- 文档用中文；代码、注释、commit message 用英文。
- Conventional Commits（feat / fix / docs / chore / refactor / test）。
- 直推 `main`，不开分支；commit 粒度切小。
- 提交前必须本地通过：
  `cargo fmt --all -- --check`
  `cargo clippy --workspace --all-targets -- -D warnings`
  `cargo test --workspace`
- 推送后确认 GitHub Actions 绿勾。
