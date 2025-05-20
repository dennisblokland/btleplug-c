# btleplug-c
[](https://github.com/deviceplug/btleplug/actions/workflows/rust.yml)
A C API wrapper for the [btleplug](https://github.com/deviceplug/btleplug) Rust library that provides platform-agnostic Bluetooth Low Energy (BLE) device communication.
## Overview
This project provides a C-compatible wrapper around the btleplug Rust library, allowing non-Rust applications to utilize Bluetooth Low Energy functionality across multiple platforms.
## Features
- Bluetooth device scanning and discovery
- Connect to BLE peripherals
- Service and characteristic discovery
- Read and write characteristic values
- Handle notifications and indications
- Cross-platform support (Linux, macOS, Windows)

## Supported Platforms
- Linux x64
- macOS x64
- macOS ARM64 (Apple Silicon)
- Windows x64

## Building
This project uses Rust and Cargo for building. Clone the repository and run:
``` bash
cargo build --release
```
For specific platform targets, use:
``` bash
# For Linux
rustup target add x86_64-unknown-linux-gnu
cargo build --release --target x86_64-unknown-linux-gnu

# For macOS (Intel)
rustup target add x86_64-apple-darwin
cargo build --release --target x86_64-apple-darwin

# For macOS (Apple Silicon)
rustup target add aarch64-apple-darwin
cargo build --release --target aarch64-apple-darwin

# For Windows
rustup target add x86_64-pc-windows-msvc
cargo build --release --target x86_64-pc-windows-msvc
```
## Dependencies
- Rust 2021 edition
- btleplug 0.11.5
- tokio 1.36.0 (with rt-multi-thread feature)
- uuid 1.7.0
- futures 0.3.30
- log 0.4.20
- simple-logging 2.0.2

On Linux, you'll also need:
``` bash
sudo apt install libdbus-1-dev pkg-config
```
## Usage
The library exposes a C API for BLE operations. Include the generated library in your C/C++ project to access BLE functionality.
Key functions include:
- Creating and managing BLE modules
- Setting log levels and event callbacks
- Scanning for BLE devices
- Connecting to peripherals
- Working with services and characteristics

## License
See the [LICENSE](LICENSE) file for details.
## Repository
[https://github.com/deviceplug/btleplug](https://github.com/deviceplug/btleplug)
