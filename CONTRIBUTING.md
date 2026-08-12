# Contributing to BifrostDNS

Thanks for your interest in contributing! This is a small project, so the process is lightweight.

## Getting Started

```bash
git clone https://github.com/minutemailco/bifrost-dns.git
cd bifrost-dns
cargo build
```

## Development Workflow

1. Fork the repo and create a branch for your change.
2. Make your changes. Keep diffs minimal and focused.
3. Ensure all checks pass:
   ```bash
   cargo fmt --check
   cargo clippy -- -D warnings
   cargo test
   ```
4. Open a pull request with a clear description of what and why.

## Code Style

- Follow `rustfmt` defaults — run `cargo fmt` before committing.
- No clippy warnings (`cargo clippy -- -D warnings`).
- Match the existing code style and patterns.
- Keep dependencies minimal — this project values a small footprint.

## Commit Messages

Use clear, conventional commit messages:

```
feat: add support for PTR records
fix: handle IPv6 fallback server parsing
docs: add Docker Compose example
```

## Reporting Issues

Open a GitHub issue with:
- What you expected
- What actually happened
- Steps to reproduce (commands, config)
- Logs if relevant (`journalctl -u bifrost-dns` or `docker logs`)

## License

By contributing, you agree that your contributions are licensed under the MIT License.
