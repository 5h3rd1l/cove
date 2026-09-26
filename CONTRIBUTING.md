# Contributing to Cove

Thanks for helping! Bug reports, ideas and pull requests are all welcome.

- **Found a bug or have an idea?** Open an issue. Screenshots of the app are great, but please blur or crop any real chat titles first.
- **Want to code?** Look for issues labelled [good first issue](https://github.com/5h3rd1l/cove/labels/good%20first%20issue) or [help wanted](https://github.com/5h3rd1l/cove/labels/help%20wanted).
- **Building:** `cargo build --release` (Rust 1.88+). See [docs/development.md](docs/development.md) for the project layout.
- **Before opening a pull request**, run the same checks CI runs:

```bash
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
```

Cove only ever *reads* other tools' chat history. Please keep it that way: nothing in Cove should modify or delete a coding agent's own files.
