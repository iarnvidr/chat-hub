# Contributing

Contributions are welcome via focused pull requests. Do not submit real
provider conversations, credentials, tokens, or personal data; fixtures must
be synthetic. Keep changes within the stated MVP scope and document any
behavior that remains unverified.

Before opening a pull request, run:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets
```

CI also builds on macOS. Changes to capture, import, persistence, or API
behavior should include synthetic tests where practical. Never claim live
provider compatibility without testing it and documenting the scope.
