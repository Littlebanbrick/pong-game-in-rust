# pong-game-in-rust

Rust + ratatui 的终端 Pong 游戏。Cargo workspace 双 crate，前后端彻底分离（见 [ARCHITECTURE.md](ARCHITECTURE.md)）：

- `crates/pong-core`：纯游戏逻辑（模拟、物理、规则、AI），无任何 I/O
- `crates/pong-tui`：渲染与键盘输入，不含游戏规则

## 下载运行

从 [Releases](https://github.com/Littlebanbrick/pong-game-in-rust/releases) 下载对应平台的二进制：

- **Windows 10/11**：`pong-vX.Y.Z-windows-x86_64.exe`，下载后直接双击运行，无任何依赖
- **Linux**：`pong-vX.Y.Z-linux-x86_64`，`chmod +x` 后运行，需要系统安装 ALSA 运行库（Debian/Ubuntu：`sudo apt-get install -y libasound2t64`；Fedora：`sudo dnf install alsa-lib`）

操作：W/S 控制左板，↑/↓ 控制右板（人机模式同为左板）；开局菜单选对手与球速；终局 R 重开、M 回菜单、q 退出。

## 从源码构建

```bash
cargo run --release
```

开发与发布流程见 [ROADMAP.md](ROADMAP.md)；协作规范见 [CLAUDE.md](CLAUDE.md)。
