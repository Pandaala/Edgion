# Pinned Aho-Corasick Differential Oracle

This isolated Cargo project compares normalized ordered block, range, anchored, earliest, streaming, and replacement results from Edgion's native matcher with upstream `aho-corasick` 1.1.5 at commit `5178060ce73d91938f8582d0360e3be031380440`.

It is deliberately outside the Edgion workspace and production dependency graph. Run it explicitly:

```text
cargo run --manifest-path edgion-waf/tests/oracles/aho-corasick-1.1.5/Cargo.toml --target-dir target/waf-oracles --locked
```

Run from the repository root so build artifacts stay under `target/waf-oracles/`.
