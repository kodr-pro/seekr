# Autonomous Tools

The power of **seekr** lies in its toolset. These tools allow the AI agent to interact with your system and the web to fulfill your requests.

## Included Tools

### 🐚 Shell Tool
The shell tool allows the agent to execute commands on your system.
- **Capabilities:** Run scripts, install packages, check system logs, etc.
- **Security:** By default, all commands require user approval unless `auto_approve_tools` is set to `true`.

### 📂 File Edit Tool
A specialized tool for managing files in your repository.
- **Capabilities:** Read file contents, create new files, apply patches to existing files.
- **Intelligence:** The agent understands file structures and can perform precise edits without overwriting unrelated data.

### 🌐 Web Tool
Connects the agent to the internet.
- **Capabilities:** Search for information using Google-like queries, fetch and parse webpage content into markdown.
- **Use Case:** Researching documentation, finding solutions to bugs, or staying updated with latest library versions.

### 📋 Task Tool
Used by the agent to manage its own internal state and goals.
- **Capabilities:** Break down complex objectives into smaller, manageable tasks.
- **Tracking:** Users can see the current active task and the history of completed tasks in the **Tasks** tab.

## Tool Safety

**seekr** is designed with safety in mind. Every tool invocation that could modify your system (like Shell and File Edit) is presented to you for confirmation. You can see:
1. The exact command or edit being proposed.
2. The agent's reasoning behind the action.
3. The option to **Approve**, **Reject**, or **Edit** the command.

---

[Next: Terminal UI](/ui)
