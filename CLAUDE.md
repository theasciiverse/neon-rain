# Neon Rain

Our own demo for the Asciiverse catalog (kind `art`, id `neon-rain`): a
rainy neon street at night seen from straight above. Rust + crossterm, one
file (`src/main.rs`), MIT. Started 2026-10-20 after the GIF of the
unreleased Terminal Rain roguelike (TIGSource devlog 2015 to 2018); none of
its art or code is used.

- Remote: `origin` is github.com/theasciiverse/neon-rain (private), branch
  `main`.
- Release: tag `v<version>-av.<n>` (first `v1.0.0-av.1`); the workflow
  builds static musl Linux, macOS and Windows on `*-av.*` tags.
- av starts it as `neon-rain`; any key, Ctrl+C, SIGTERM and SIGHUP quit;
  resize rebuilds the street; `-s SEED` fixes the layout.
- Static build, in a copy of the tree: `docker run --rm --cpus 4 --memory
  8g -v "$PWD":/src -w /src rust:1-alpine sh -c 'apk add musl-dev && cargo
  build --release --locked -j2'`.
- Preview: `asciiverse/record-preview.sh target/release/neon-rain
  asciiverse/neon-rain.cast` (seed 4, 25 s, capped at 20 fps).
- Session log: `RUST_LOG=debug neon-rain 2>neon-rain.log`.
