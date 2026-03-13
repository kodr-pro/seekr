# seekr

<p align="center">
  <img src="docs/seekr-logo.png" alt="seekr Logo">
</p>

**seekr** is a high-performance AI Agent Manager featuring a sleek Terminal UI, powered by the DeepSeek native API. It brings the power of autonomous agents directly to your terminal with a robust toolset for shell execution, file management, and web exploration.

![License](https://img.shields.io/badge/license-Polyform_Prosperity-blue.svg)
![Rust](https://img.shields.io/badge/rust-2021-orange.svg)
![DeepSeek](https://img.shields.io/badge/AI-DeepSeek-green.svg)

---

## Features

- **Terminal UI (TUI):** Built with `ratatui` for a responsive, multi-tabbed interactive experience.
- **Native DeepSeek Integration:** Low-latency access to DeepSeek's powerful reasoning and chat models.
- **Extensible Skills System:** Refactored tool architecture allowing for global and repository-specific custom skills via simple JSON and scripts.
- **Autonomous Tools:**
  - **Shell:** Execute terminal commands with built-in sandboxing and timeouts.
  - **File Edit:** Sophisticated file manipulation using patches and diffs.
  - **Web:** Real-time search and scraping.
  - **Task Management:** Goal planning and progress tracking.
- **Rich Activity Stream:** Real-time visibility into agent thoughts and tool executions with diff previews.
- **Session Persistence:** Automatic saving and loading of chat history and agent state.
- **Seekr Doctor:** Built-in diagnostics command to verify system health and API connectivity.

---

## Getting Started

### Prerequisites

- [Rust](https://www.rust-lang.org/tools/install) (latest stable version)
- A [DeepSeek API Key](https://platform.deepseek.com/)

### Installation

Clone the repository and build the project:

```bash
git clone https://github.com/kodr-pro/seekr.git
cd seekr
cargo build --release
```

Run the installation:

```bash
cargo install --path .
```

Alternatively, you can build and run it manually:

```bash
cargo run --release
```

On your first run, **seekr** will guide you through a setup wizard to configure your DeepSeek API key and preferences.

---

## CLI Commands

| Command | Description |
| :--- | :--- |
| `seekr` | Launch the main TUI application. |
| `seekr doctor` | Run system diagnostics and health checks. |
| `seekr --resume <session_id>` | Resume a previous session by its ID. |

---

## Skills & Extensibility

**seekr** features a dynamic skills system. It loads tools from:
1. **Bundled Core Skills:** Essential file, shell, and task tools.
2. **Global Skills:** Located in `~/.config/seekr/skills/`.
3. **Local Skills:** Located in `./.seekr/skills/` within your current working directory.

Each skill is a directory containing a `skill.json` definition and any necessary scripts (Python, Shell, etc.).

---

## Configuration

**seekr** stores its configuration in `~/.config/seekr/config.toml`. You can manually edit this file or use the built-in setup wizard.

```toml
[api]
key = "your-api-key-here"
model = "deepseek-chat"
base_url = "https://api.deepseek.com"

[agent]
max_iterations = 25
auto_approve_tools = false
working_directory = "."

[ui]
theme = "dark"
show_reasoning = true
```

---

## Documentation

For more detailed guides and API references, check out our [Documentation](https://docs.page/kodr-pro/seekr).

---

## Contributing

Contributions are welcome! Please feel free to submit a Pull Request.

1. Fork the Project
2. Create your Feature Branch (`git checkout -b feature/AmazingFeature`)
3. Commit your Changes (`git commit -m 'Add some AmazingFeature'`)
4. Push to the Branch (`git push origin feature/AmazingFeature`)
5. Open a Pull Request

---

## License

Distributed under the Polyform Prosperity License 1.0.0. See `LICENSE` for more information regarding personal and commercial use.

---

<p align="center">
  Built with care by the kodr team
</p>
