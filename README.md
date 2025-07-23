# 🚀 proxy-scraper-checker

[![CI](https://github.com/monosans/proxy-scraper-checker/actions/workflows/ci.yml/badge.svg)](https://github.com/monosans/proxy-scraper-checker/actions/workflows/ci.yml)

![TUI Demo](https://github.com/user-attachments/assets/0ac37021-d11c-4f68-b80d-bafdbaeb00bb)

**A lightning-fast, feature-rich proxy scraper and checker built in Rust.**

Collect, test, and organize HTTP/SOCKS4/SOCKS5 proxies from multiple sources with detailed metadata and intelligent filtering.

## ✨ Key Features

- **🔥 Blazing Performance** - Rust-powered async engine with configurable concurrency
- **🌍 Rich Metadata** - ASN, geolocation, and response time data via offline MaxMind databases
- **🎯 Smart Parsing** - Advanced regex engine extracts proxies from any format (`protocol://user:pass@host:port`)
- **🔐 Auth Support** - Handles username/password authentication seamlessly
- **📊 Interactive TUI** - Real-time progress monitoring with beautiful terminal interface
- **⚡ Flexible Output** - JSON (with metadata) and plain text formats
- **🎛️ Configurable** - Extensive options for sources, timeouts, and checking
- **📁 Local & Remote** - Supports both web URLs and local files as proxy sources
- **🐳 Docker Ready** - Containerized deployment with volume mounting

## 🔗 Related

Get pre-checked proxies from [monosans/proxy-list](https://github.com/monosans/proxy-list) - updated regularly using this tool.

## ⚠️ SAFETY WARNING ⚠️

This tool makes many network requests and can impact your IP-address reputation. Consider using a VPN for safer operation.

## 🚀 Quick Start

> All configuration options are documented in `config.toml` - edit it to customize sources, timeouts, and output preferences.

<details>
<summary>💻 Binary Installation</summary>

> **Note:** For Termux users, see the dedicated section below.

1. **Download** the appropriate binary from [nightly builds](https://nightly.link/monosans/proxy-scraper-checker/workflows/ci/main?preview)
   - Not sure which one? Check the [platform support table](https://doc.rust-lang.org/beta/rustc/platform-support.html)

2. **Extract** the archive to a dedicated folder

3. **Configure** by editing `config.toml` to your needs

4. **Run** the executable

</details>

<details>
<summary>🐳 Docker Installation</summary>

> **Note:** Docker version uses a simplified log-based interface (no TUI).

1. **Install** [Docker Compose](https://docs.docker.com/compose/install/)

2. **Download** the docker archive from [nightly builds](https://nightly.link/monosans/proxy-scraper-checker/workflows/ci/main?preview)
   - Look for artifacts named `proxy-scraper-checker-docker`

3. **Extract** to a folder and configure `config.toml`

4. **Build and run:**

   ```bash
   # Windows
   docker compose build
   docker compose up --no-log-prefix --remove-orphans

   # Linux/macOS
   docker compose build --build-arg UID=$(id -u) --build-arg GID=$(id -g)
   docker compose up --no-log-prefix --remove-orphans
   ```

</details>

<details>
<summary>📱 Termux Installation</summary>

> **Important:** Download Termux from [F-Droid](https://f-droid.org/en/packages/com.termux/), not Google Play ([why?](https://github.com/termux/termux-app#google-play-store-experimental-branch)).

1. **Auto-install** with one command:

   ```bash
   bash <(curl -fsSL 'https://raw.githubusercontent.com/monosans/proxy-scraper-checker/main/termux.sh')
   ```

2. **Configure** using a text editor:

   ```bash
   nano ~/proxy-scraper-checker/config.toml
   ```

3. **Run the tool:**
   ```bash
   cd ~/proxy-scraper-checker && ./proxy-scraper-checker
   ```

</details>

## 📄 License

[MIT](LICENSE)

_This product includes GeoLite2 Data created by MaxMind, available from https://www.maxmind.com_
