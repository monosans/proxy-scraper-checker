# proxy-scraper-checker

![TUI Demo](https://github.com/user-attachments/assets/0ac37021-d11c-4f68-b80d-bafdbaeb00bb)

A proxy scraper and checker written in Rust.

It collects HTTP/SOCKS4/SOCKS5 proxies from any number of sources, verifies each one really works, and writes the survivors out with response times, geolocation and network ownership attached.

## Features

- Async Rust, shipped as a single binary for Windows, Linux, macOS and Android. Nothing to install.
- Proxies are pattern-matched out of raw text, HTML or JSON, so any source works untouched. `scheme://user:pass@host:port` and CIDR ranges are understood, from URLs or local files.
- Every proxy has to fetch a real URL _in full_ to survive, so ones that connect and then stall get dropped. Results are deduplicated across sources.
- Response time, exit IP, ASN and city-level geolocation come from offline MaxMind databases, so there are no per-proxy API calls.
- The interactive TUI shows per-protocol progress and logs as the run goes.
- Output is JSON with metadata, plus ready-to-paste plain text lists.

## Related

If you want proxies without running anything, [monosans/proxy-list](https://github.com/monosans/proxy-list) is updated regularly using this tool.

## Safety warning

Checking opens hundreds of simultaneous connections to untrusted hosts. Your ISP may read that as abusive traffic, and cheap routers can drop connectivity when their NAT table fills. Consider a VPN, and lower `max_concurrent_checks` if a run destabilizes your network.

## Quick start

Every option is documented inline in `config.toml`. The binary reads `config.toml` from the current directory; set `PROXY_SCRAPER_CHECKER_CONFIG` to point it somewhere else.

<details>
<summary>Download and run a pre-built binary (easiest)</summary>

> On Android/Termux, see the dedicated section below instead.

1. Download the archive for your platform from [nightly builds](https://nightly.link/monosans/proxy-scraper-checker/workflows/ci/main?preview). Look for artifacts starting with `proxy-scraper-checker-binary-` followed by your platform. If you are not sure which platform you have, check the [platform support table](https://doc.rust-lang.org/beta/rustc/platform-support.html).

2. Extract the archive to a dedicated folder.

3. Edit `config.toml`.

4. Run the executable.

</details>

<details>
<summary>Docker</summary>

> The Docker image logs to stdout instead of showing the interactive TUI.

1. Install [Docker Compose](https://docs.docker.com/compose/install/).

2. Download the archive from [nightly builds](https://nightly.link/monosans/proxy-scraper-checker/workflows/ci/main?preview). Look for artifacts starting with `proxy-scraper-checker-docker-` followed by your CPU architecture.

3. Extract it to a folder and edit `config.toml`.

4. Build and run:

   ```bash
   # Windows
   docker compose build
   docker compose up --no-log-prefix --remove-orphans

   # Linux/macOS
   docker compose build --build-arg UID=$(id -u) --build-arg GID=$(id -g)
   docker compose up --no-log-prefix --remove-orphans
   ```

Results land in `./out` on the host; `output.path` is ignored in Docker.

</details>

<details>
<summary>Android / Termux</summary>

> Install Termux from [F-Droid](https://f-droid.org/en/packages/com.termux/), not Google Play ([why?](https://github.com/termux/termux-app#google-play-store-experimental-branch)).

1. Install with one command:

   ```bash
   bash <(curl -fsSL 'https://raw.githubusercontent.com/monosans/proxy-scraper-checker/main/termux.sh')
   ```

2. Edit the config in a text editor:

   ```bash
   nano ~/proxy-scraper-checker/config.toml
   ```

3. Run it:

   ```bash
   cd ~/proxy-scraper-checker && ./proxy-scraper-checker
   ```

</details>

<details>
<summary>Build from source</summary>

1. Install the Rust toolchain, see <https://rust-lang.org/tools/install/>

2. Clone the repository:

   ```bash
   git clone https://github.com/monosans/proxy-scraper-checker.git
   cd proxy-scraper-checker
   ```

3. Build a release binary with the TUI enabled:

   ```bash
   cargo build --features tui --release --locked
   ```

4. Run with the TUI:

   ```bash
   cargo run --features tui --release --locked
   ```

The binary lands in `target/release/proxy-scraper-checker` on Linux/macOS, or `target\release\proxy-scraper-checker.exe` on Windows.

</details>

## Output

```
out/
├── proxies.json          working proxies with metadata, compact
├── proxies_pretty.json   the same data, indented
└── proxies/
    ├── all.txt           every proxy, with its protocol prefix
    ├── http.txt          one file per enabled protocol, no prefix
    ├── socks4.txt
    └── socks5.txt
```

A text line is `host:port`, or `username:password@host:port` for a proxy that needs credentials, with `protocol://` in front of either in `all.txt`.

```json
{
  "protocol": "socks5",
  "username": null,
  "password": null,
  "host": "1.2.3.4",
  "port": 1080,
  "timeout": 0.42,
  "exit_ip": "5.6.7.8",
  "asn": {
    "autonomous_system_number": 12345,
    "autonomous_system_organization": "Example Networks"
  },
  "geolocation": {
    "country": { "iso_code": "US", "names": { "en": "United States" } },
    "city": { "names": { "en": "Chicago" } },
    "location": {
      "latitude": 41.85,
      "longitude": -87.65,
      "time_zone": "America/Chicago"
    }
  }
}
```

`timeout` is how long the whole request took, in seconds: connection, request and the complete response. That is also what `sort_by_speed` orders by. `exit_ip` is the address the target site actually saw, so comparing it with `host` spots proxies that forward through somewhere else.

`exit_ip`, `asn` and `geolocation` are only filled in when `check_url` returns the exit IP, so pointing it at an ordinary web page leaves all three `null`. `geolocation` mirrors the GeoLite2 City record (abbreviated above); fields the database has no data for are omitted, and names are English only.

Stopping a run early, with `ESC` or `q` in the TUI or `Ctrl-C` anywhere, writes out everything checked so far. If you stop before any proxy has been checked, the previous run's files are left untouched rather than emptied.

## Sponsors

|                                                              |                                                                                                                                                               |
| ------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **[RapidProxy.io](https://www.rapidproxy.io/?ref=monosans)** | <a href="https://www.rapidproxy.io/?ref=monosans"><img width="400" src="https://github.com/user-attachments/assets/143ed7cc-c200-4563-9253-4ccedcd3ecd5"></a> |

Want your name in this section? Support the project and it goes here.

### Support this project

Star the repository so other people can find it. If you are interested in sponsoring, [DM me on Telegram](https://t.me/monosans).

## License

[MIT](LICENSE)

_This product includes GeoLite2 Data created by MaxMind, available from <https://www.maxmind.com>_
