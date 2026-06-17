# Contributing to mshwisper

## Quick Start

```bash
git clone https://github.com/YOUR_USERNAME/mshwisper.git
cd mshwisper
cargo build        # dev build
cargo test         # run tests
cargo clippy       # lints
```

## Project Layout

```
src/
├── main.rs              Entry point, startup banner, nick prompt
├── core/
│   ├── types.rs         Peer, Message, Channel, WirePacket, AppMessage
│   ├── state.rs         AppState — Arc-wrapped shared state
│   └── events.rs        NetEvent / UiCommand channel types
├── crypto/
│   └── mod.rs           LocalKeypair (ECDH), SessionCipher (AES-256-GCM)
├── network/
│   ├── discovery.rs     UDP broadcast peer discovery
│   ├── session.rs       TCP framing helpers
│   ├── transfer.rs      Chunked file transfer
│   └── mesh.rs          Mesh orchestrator — main network task
└── tui/
    ├── app.rs           App state, run loop, input editing helpers
    ├── input.rs         Key event dispatch
    ├── render.rs        3-column layout, all widget rendering
    └── widgets.rs       Message line formatting, nick colors
```

## Guidelines

- Keep the TUI thread free of blocking calls — use `try_recv()` for events
- All secrets must stay in RAM — no `fs::write`, no logging of key material
- New network features go in `network/mesh.rs` or new submodules
- UI features go in `tui/`
- Open an issue before large PRs to discuss the approach

## Pull Requests

1. Fork the repo
2. Create a branch: `git checkout -b feat/my-feature`
3. Make your changes
4. Run `cargo clippy` and fix warnings
5. Push and open a PR against `main`
