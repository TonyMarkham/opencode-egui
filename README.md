# OpenCode EGUI Client

Native Rust desktop client for OpenCode using EGUI.

## Quick Start

### Prerequisites

1. Install cargo-make:

   ```bash
   cargo install cargo-make
   ```

2. Download Whisper model for speech-to-text:

   ```bash
   curl -L https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en.bin \
        -o ~/Downloads/ggml-base.en.bin
   ```

3. **Windows Only**: Set required environment variables:
   ```powershell
   $env:PATH="C:\Program Files\Microsoft Visual Studio\18\Community\Common7\IDE\CommonExtensions\Microsoft\CMake\Ninja;$env:PATH"
   $env:CMAKE_GENERATOR="Ninja"
   $env:LIBCLANG_PATH="C:\Program Files\LLVM\bin"
   ```
   These variables must be set in your current terminal environment before running `cargo make dev`.

### Running

#### Debug
```bash
cargo make dev
```
#### Release
```bash
cargo make release
```

**Note**: On Windows, ensure the environment variables from the prerequisites are set in your current terminal session before running this command.

This will:

- Build the project
- Copy the Whisper model to `target/debug/models/`
- Run the application

### Push-to-Talk

Once running with the model configured:

- **Press and hold `AltRight`** to record
- **Release `AltRight`** to stop and transcribe
- Transcribed text appears in the input field

## Manual Setup

If you don't want to use cargo-make:

1. Build: `cargo build`
2. Place your Whisper model at `target/debug/models/ggml-base.en.bin`
3. Run: `cargo run`

Alternatively, configure a custom model path in Settings > Audio.

## Building the OpenCode Server

The EGUI client requires the OpenCode server to be running. You can either use an existing server or build one from source.

### Prerequisites

- **Bun** 1.3.3 or later (the strict version check has been removed)
- **Node modules**: The server requires the `opencode-openai-codex-auth` plugin to be pre-installed

### Build Instructions

1. **Navigate to the server directory**:

   ```bash
   cd packages/opencode
   ```

2. **Pre-install required plugins**:

   ```bash
   # On Windows
   bun add opencode-openai-codex-auth --cwd C:\Users\<YourUsername>\.cache\opencode

   # On Linux/macOS
   bun add opencode-openai-codex-auth --cwd ~/.cache/opencode
   ```

3. **Build the server**:

   ```bash
   # Default: builds for current platform only
   bun run build

   # With custom version
   bun run build --version=1.0.134.pre.4

   # Build for all platforms (for releases)
   bun run build --all

   # Skip dependency installation (faster rebuilds)
   bun run build --skip-install
   ```

### Build Output

The compiled server binary will be located at:

- **Windows**: `packages/opencode/dist/opencode-windows-x64/bin/opencode.exe`
- **Linux**: `packages/opencode/dist/opencode-linux-x64/bin/opencode`
- **macOS (Apple Silicon)**: `packages/opencode/dist/opencode-darwin-arm64/bin/opencode`
- **macOS (Intel)**: `packages/opencode/dist/opencode-darwin-x64/bin/opencode`

### Running the Server

```bash
# From repository root
./packages/opencode/dist/opencode-windows-x64/bin/opencode.exe serve

# With custom port/hostname
./packages/opencode/dist/opencode-windows-x64/bin/opencode.exe serve --port 3000 --hostname 0.0.0.0
```

The server will start and print:

```
opencode server listening on http://127.0.0.1:<port>
```

### Server Discovery

The EGUI client automatically discovers running OpenCode servers by:

1. Scanning for processes named `opencode`, `bun`, or `node` running the OpenCode server
2. Finding the port the server is listening on
3. Connecting to `http://127.0.0.1:<port>`

If no server is found, the client will attempt to spawn one automatically.

## Features

- Auto server discovery and spawning
- Multi-session tabs
- Real-time streaming with markdown rendering
- Tool call visualization
- Speech-to-text (push-to-talk with AltRight)
- Configurable UI (fonts, chat density)

## Authentication

### Anthropic Pro/Max OAuth

To use your Claude subscription instead of paying for API usage:

1. Run the OAuth flow:
   ```bash
   cargo run -- --oauth
   ```
2. Follow the instructions to authenticate in your browser.
3. Once authenticated, restart the app normally:
   ```bash
   cargo run
   ```
   The client will automatically use your OAuth token and default to the `claude-3-5-sonnet` model.

## Architecture

See [EGUI_PLAN.md](./EGUI_PLAN.md) and [STT_PLAN.md](./STT_PLAN.md) for detailed architecture and implementation plans.
