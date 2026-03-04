# Tauri + React + Typescript

This template should help get you started developing with Tauri, React and Typescript in Vite.

## Recommended IDE Setup

- [VS Code](https://code.visualstudio.com/) + [Tauri](https://marketplace.visualstudio.com/items?itemName=tauri-apps.tauri-vscode) + [rust-analyzer](https://marketplace.visualstudio.com/items?itemName=rust-lang.rust-analyzer)

## Feed Server

Set `VITE_MESSAGE_BOARD_BASE_URL` to point to the message-board service.

Example:

```bash
VITE_MESSAGE_BOARD_BASE_URL=http://127.0.0.1:3100 pnpm tauri dev --release
```
