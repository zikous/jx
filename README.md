# jx

A fast, jq-compatible JSON processor written in Rust.

```sh
cargo build --release
echo '{"users":[{"name":"Ada"},{"name":"Linus"}]}' | ./target/release/jx '.users[].name'
```

## Features

- The jq language: filters, functions, `reduce`/`foreach`, destructuring, assignment, regex, `@formats`, dates
- Familiar jq flags: `-n`, `-r`, `-s`, `-c`, `-S`, `--arg`, `--argjson`, `--slurpfile`, `--stream`, …
- Large inputs are processed in parallel on all cores, with output kept in input order
- Number literals keep their original text (`1.50` stays `1.50`)

## Tests

```sh
cargo test
```
