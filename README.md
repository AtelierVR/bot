# Nox Bot

Load testing bot for Nox relay servers, written in Rust.

## Features

- High-performance QUIC-based relay client
- Multiple movement patterns (circular, random teleport, square)
- Configurable bot count and behavior
- Graceful shutdown handling

## Building

```bash
# Development build
cargo build

# Release build (optimized)
cargo build --release
```

## Running

```bash
# Using cargo
cargo run --release --bin nox-test -- --count 10 --movement circular

# Using npm scripts
npm run build
./target/release/nox-test --count 10 --movement circular
```

## Project Structure

- `relay/` - QUIC relay client library
- `test/` - Load testing application
- `api/` - API client (if needed)

## Configuration

Create a config file or use environment variables:

```bash
RELAY_ADDRESS=127.0.0.1:30000
BOT_COUNT=10
MOVEMENT_TYPE=circular
```

## License

MIT
