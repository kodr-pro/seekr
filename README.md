# seekr: The Asynchronous Agentic System Operator

<p align="center">
  <img src="docs/logo.png" alt="seekr Logo">
</p>

**seekr** is a high-performance, background-first AI Agent Manager designed to transform your terminal into an autonomous system operator. Originally built as a DeepSeek-native client, seekr has evolved into a full-featured agentic platform that runs persistently in the background, executing long-running tasks while you work elsewhere.

seekr brings "Jarvis-style" persistence to Linux via a client-server architecture, featuring a rugged Terminal UI (TUI) and a headless background daemon (`seekrd`) that communicates over a high-speed SSE (Server-Sent Events) interface.

![License](https://img.shields.io/badge/license-Polyform_Prosperity-blue.svg)
![Rust](https://img.shields.io/badge/rust-2024-orange.svg)
![DeepSeek](https://img.shields.io/badge/AI-DeepSeek-green.svg)
![OpenAI-Compatible](https://img.shields.io/badge/AI-Multi--Model-purple.svg)

---

## 🚀 The Asynchronous Advantage

Traditional AI agents live and die with your terminal session. **seekr** changes the paradigm:

- **Persistent Daemon Architecture:** Launch the Seekr daemon (`seekr daemon`) to maintain a peristent agentic presence. Close the TUI anytime; your tasks continue to execute in the background.
- **Client-Server Flow:** The TUI acts as a lightweight window into the agent's mind. Reconnect to active sessions from any terminal at any time.
- **Automated Lifecycle:** Launch `seekr` normally and the TUI will automatically stand up the background daemon if it isn't already running.

---

## Highlights

- **Unlimited Context Window:** Never run out of memory. Seekr automatically summarizes past conversation segments and injects them into the current context as a "sliding window."
- **Interruptible Agent Loop:** Real-time user steering. Interrupt the agent mid-thought to provide new context or directions.
- **True Multi-Tool Parallelism:** Execute multiple independent tool calls (reading files, searching web, etc.) concurrently for 5-10x performance gains.
- **Premium TUI Experience:** Beautiful, icon-based headers and a custom-built, wrapping-aware scrolling engine for a smooth conversation flow.
- **Dynamic Skills System:** Load and execute custom tools via simple JSON definitions and shell scripts (Python, JS, Bash, etc.).

## Features

- **Terminal UI (TUI):** Built with `ratatui` for a responsive, multi-tabbed interactive experience.
- **Multi-Model & OpenAI API Support:** Full support for configuring multiple LLM providers (OpenAI, DeepSeek, Local, etc.) via the standard OpenAPI format.
- **Asynchronous Tooling:**
  - **Shell:** Execute terminal commands with built-in sandboxing, timeouts, and **background persistence**.
  - **File Edit:** Sophisticated file manipulation using patches and diffs.
  - **Web:** Real-time search and scraping.
  - **Task Management:** Hierarchical goal planning and progress tracking.
- **Real-time SSE Streaming:** Extremely low-latency synchronization between the background daemon and the interactive UI.
- **Seekr Doctor:** Built-in diagnostics command to verify system health and API connectivity.

---

## Getting Started

### Installation

#### 📦 Binary Install (Linux x86_64)

```bash
# Download the binary
curl -L -O https://github.com/kodr-pro/seekr/releases/download/v0.3.0/seekr-v0.3.0-linux-x86_64

# Make it executable and move to path
chmod +x seekr-v0.3.0-linux-x86_64
sudo mv seekr-v0.3.0-linux-x86_64 /usr/local/bin/seekr
```

#### Build from Source

Ensure you have [Rust](https://www.rust-lang.org/tools/install) installed:

```bash
git clone https://github.com/kodr-pro/seekr.git
cd seekr
cargo install --path .
```

---

## CLI Usage

| Command | Description |
| :--- | :--- |
| `seekr` | Launch the main TUI application (Auto-starts daemon). |
| `seekr daemon` | Launch the background daemon independently. |
| `seekr doctor` | Run system diagnostics and health checks. |
| `seekr --resume <session_id>` | Reconnect to an active background session. |

---

## TUI Shortcuts

| `Tab` | Switch focus between Chat and Tasks panel. |
| `Ctrl+G` | Open **Unified Menu** (Sessions, Models, Providers, Settings). |
| `Ctrl+R` | **Clear Chat** history (resets context). |
| `Ctrl+C` | Detach TUI (Tasks continue in background). |

---

## Configuration

**seekr** stores its configuration in `~/.config/seekr/config.toml`.

```toml
[agent]
max_iterations = 25
auto_approve_tools = false
working_directory = "."
context_window_threshold = 40
context_window_keep = 10

[ui]
theme = "dark"
show_reasoning = true
```

---

## License

Distributed under the Polyform Prosperity License 1.0.0. See `LICENSE` for more information regarding personal and commercial use.

---

<p align="center">
  Built with care by <a href="https://kodr.pro">kodr</a>
</p>
