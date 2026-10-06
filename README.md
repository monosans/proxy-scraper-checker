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

Pre-built archives are produced by CI from the latest commit on `main` and published as the [latest release](https://github.com/monosans/proxy-scraper-checker/releases/latest). Open the section for your system.

<details>
<summary>Windows</summary>

1. Download the archive for your processor:

   | Processor                                | Download                            |
   | ---------------------------------------- | ----------------------------------- |
   | 64-bit Intel or AMD (almost every PC)    | [Download][x86_64-pc-windows-msvc]  |
   | ARM64 (Snapdragon and other ARM laptops) | [Download][aarch64-pc-windows-msvc] |
   | 32-bit                                   | [Download][i686-pc-windows-msvc]    |

   Not sure? Open Settings → System → About and look at "System type": `x64-based processor` means 64-bit Intel or AMD, `ARM-based processor` means ARM64, and `32-bit operating system` needs the 32-bit build whatever the processor.

2. Extract the archive to a dedicated folder.

3. Edit `config.toml`.

4. Run `proxy-scraper-checker.exe`. If SmartScreen shows "Windows protected your PC", click "More info", then "Run anyway".

[aarch64-pc-windows-msvc]: https://github.com/monosans/proxy-scraper-checker/releases/latest/download/proxy-scraper-checker-binary-aarch64-pc-windows-msvc.zip
[i686-pc-windows-msvc]: https://github.com/monosans/proxy-scraper-checker/releases/latest/download/proxy-scraper-checker-binary-i686-pc-windows-msvc.zip
[x86_64-pc-windows-msvc]: https://github.com/monosans/proxy-scraper-checker/releases/latest/download/proxy-scraper-checker-binary-x86_64-pc-windows-msvc.zip

</details>

<details>
<summary>macOS</summary>

1. Download the archive for your processor:

   | Processor                    | Download                         |
   | ---------------------------- | -------------------------------- |
   | Apple Silicon (M1 and newer) | [Download][aarch64-apple-darwin] |
   | Intel                        | [Download][x86_64-apple-darwin]  |

   Not sure? Apple menu → About This Mac shows "Chip: Apple M…" on Apple Silicon and "Processor: … Intel …" on Intel.

2. Extract the archive to a dedicated folder.

3. Edit `config.toml`.

4. Run it from Terminal in that folder:

   ```bash
   ./proxy-scraper-checker
   ```

   If macOS refuses because it cannot verify the developer, clear the download quarantine and run it again:

   ```bash
   xattr -d com.apple.quarantine proxy-scraper-checker
   ```

[aarch64-apple-darwin]: https://github.com/monosans/proxy-scraper-checker/releases/latest/download/proxy-scraper-checker-binary-aarch64-apple-darwin.zip
[x86_64-apple-darwin]: https://github.com/monosans/proxy-scraper-checker/releases/latest/download/proxy-scraper-checker-binary-x86_64-apple-darwin.zip

</details>

<details>
<summary>Linux</summary>

1. Run `uname -m` and download the archive from the matching row. The glibc build suits most distributions (Ubuntu, Debian, Fedora, Arch and so on). Take the musl build on Alpine, or if the glibc build refuses to start.

   | Processor                                                         | `uname -m`    | glibc                                           | musl                                       |
   | ----------------------------------------------------------------- | ------------- | ----------------------------------------------- | ------------------------------------------ |
   | 64-bit Intel or AMD                                               | `x86_64`      | [Download][x86_64-unknown-linux-gnu]            | [Download][x86_64-unknown-linux-musl]      |
   | 64-bit ARM (Raspberry Pi 3 and newer on a 64-bit OS, ARM servers) | `aarch64`     | [Download][aarch64-unknown-linux-gnu]           | [Download][aarch64-unknown-linux-musl]     |
   | 32-bit ARMv7 (Raspberry Pi 2 and newer on a 32-bit OS)            | `armv7l`      | [Download][armv7-unknown-linux-gnueabihf]       | [Download][armv7-unknown-linux-musleabihf] |
   | 32-bit ARMv6 (Raspberry Pi 1, Zero, Zero W)                       | `armv6l`      | [Download][arm-unknown-linux-gnueabihf]         | [Download][arm-unknown-linux-musleabihf]   |
   | 32-bit Intel or AMD                                               | `i686`        | [Download][i686-unknown-linux-gnu]              | [Download][i586-unknown-linux-musl]        |
   | 64-bit RISC-V                                                     | `riscv64`     | [Download][riscv64gc-unknown-linux-gnu]         | [Download][riscv64gc-unknown-linux-musl]   |
   | LoongArch                                                         | `loongarch64` | [Download][loongarch64-unknown-linux-gnu]       | [Download][loongarch64-unknown-linux-musl] |
   | 64-bit PowerPC, little-endian                                     | `ppc64le`     | [Download][powerpc64le-unknown-linux-gnu]       | —                                          |
   | 64-bit PowerPC, big-endian                                        | `ppc64`       | [Download][powerpc64-unknown-linux-gnu]         | —                                          |
   | 32-bit PowerPC                                                    | `ppc`         | [Download][powerpc-unknown-linux-gnu]           | —                                          |
   | IBM Z                                                             | `s390x`       | [Download][s390x-unknown-linux-gnu]             | —                                          |
   | 64-bit SPARC                                                      | `sparc64`     | [Download][sparc64-unknown-linux-gnu]           | —                                          |
   | 32-bit ARMv5                                                      | `armv5tel`    | [Download][armv5te-unknown-linux-gnueabi]       | [Download][armv5te-unknown-linux-musleabi] |
   | 32-bit x86 without SSE2 (Pentium III, Athlon XP and older)        | `i686`        | [Download][i586-unknown-linux-gnu]              | [Download][i586-unknown-linux-musl]        |
   | 32-bit ARMv7 without hardware floating point                      | `armv7l`      | [Download][armv7-unknown-linux-gnueabi]         | [Download][armv7-unknown-linux-musleabi]   |
   | 32-bit ARMv6 without hardware floating point                      | `armv6l`      | [Download][arm-unknown-linux-gnueabi]           | [Download][arm-unknown-linux-musleabi]     |
   | 32-bit ARMv7 with NEON, Thumb-2 build                             | `armv7l`      | [Download][thumbv7neon-unknown-linux-gnueabihf] | —                                          |

2. Extract the archive to a dedicated folder.

3. Edit `config.toml`.

4. Run it from that folder:

   ```bash
   ./proxy-scraper-checker
   ```

[aarch64-unknown-linux-gnu]: https://github.com/monosans/proxy-scraper-checker/releases/latest/download/proxy-scraper-checker-binary-aarch64-unknown-linux-gnu.zip
[aarch64-unknown-linux-musl]: https://github.com/monosans/proxy-scraper-checker/releases/latest/download/proxy-scraper-checker-binary-aarch64-unknown-linux-musl.zip
[arm-unknown-linux-gnueabi]: https://github.com/monosans/proxy-scraper-checker/releases/latest/download/proxy-scraper-checker-binary-arm-unknown-linux-gnueabi.zip
[arm-unknown-linux-gnueabihf]: https://github.com/monosans/proxy-scraper-checker/releases/latest/download/proxy-scraper-checker-binary-arm-unknown-linux-gnueabihf.zip
[arm-unknown-linux-musleabi]: https://github.com/monosans/proxy-scraper-checker/releases/latest/download/proxy-scraper-checker-binary-arm-unknown-linux-musleabi.zip
[arm-unknown-linux-musleabihf]: https://github.com/monosans/proxy-scraper-checker/releases/latest/download/proxy-scraper-checker-binary-arm-unknown-linux-musleabihf.zip
[armv5te-unknown-linux-gnueabi]: https://github.com/monosans/proxy-scraper-checker/releases/latest/download/proxy-scraper-checker-binary-armv5te-unknown-linux-gnueabi.zip
[armv5te-unknown-linux-musleabi]: https://github.com/monosans/proxy-scraper-checker/releases/latest/download/proxy-scraper-checker-binary-armv5te-unknown-linux-musleabi.zip
[armv7-unknown-linux-gnueabi]: https://github.com/monosans/proxy-scraper-checker/releases/latest/download/proxy-scraper-checker-binary-armv7-unknown-linux-gnueabi.zip
[armv7-unknown-linux-gnueabihf]: https://github.com/monosans/proxy-scraper-checker/releases/latest/download/proxy-scraper-checker-binary-armv7-unknown-linux-gnueabihf.zip
[armv7-unknown-linux-musleabi]: https://github.com/monosans/proxy-scraper-checker/releases/latest/download/proxy-scraper-checker-binary-armv7-unknown-linux-musleabi.zip
[armv7-unknown-linux-musleabihf]: https://github.com/monosans/proxy-scraper-checker/releases/latest/download/proxy-scraper-checker-binary-armv7-unknown-linux-musleabihf.zip
[i586-unknown-linux-gnu]: https://github.com/monosans/proxy-scraper-checker/releases/latest/download/proxy-scraper-checker-binary-i586-unknown-linux-gnu.zip
[i586-unknown-linux-musl]: https://github.com/monosans/proxy-scraper-checker/releases/latest/download/proxy-scraper-checker-binary-i586-unknown-linux-musl.zip
[i686-unknown-linux-gnu]: https://github.com/monosans/proxy-scraper-checker/releases/latest/download/proxy-scraper-checker-binary-i686-unknown-linux-gnu.zip
[loongarch64-unknown-linux-gnu]: https://github.com/monosans/proxy-scraper-checker/releases/latest/download/proxy-scraper-checker-binary-loongarch64-unknown-linux-gnu.zip
[loongarch64-unknown-linux-musl]: https://github.com/monosans/proxy-scraper-checker/releases/latest/download/proxy-scraper-checker-binary-loongarch64-unknown-linux-musl.zip
[powerpc-unknown-linux-gnu]: https://github.com/monosans/proxy-scraper-checker/releases/latest/download/proxy-scraper-checker-binary-powerpc-unknown-linux-gnu.zip
[powerpc64-unknown-linux-gnu]: https://github.com/monosans/proxy-scraper-checker/releases/latest/download/proxy-scraper-checker-binary-powerpc64-unknown-linux-gnu.zip
[powerpc64le-unknown-linux-gnu]: https://github.com/monosans/proxy-scraper-checker/releases/latest/download/proxy-scraper-checker-binary-powerpc64le-unknown-linux-gnu.zip
[riscv64gc-unknown-linux-gnu]: https://github.com/monosans/proxy-scraper-checker/releases/latest/download/proxy-scraper-checker-binary-riscv64gc-unknown-linux-gnu.zip
[riscv64gc-unknown-linux-musl]: https://github.com/monosans/proxy-scraper-checker/releases/latest/download/proxy-scraper-checker-binary-riscv64gc-unknown-linux-musl.zip
[s390x-unknown-linux-gnu]: https://github.com/monosans/proxy-scraper-checker/releases/latest/download/proxy-scraper-checker-binary-s390x-unknown-linux-gnu.zip
[sparc64-unknown-linux-gnu]: https://github.com/monosans/proxy-scraper-checker/releases/latest/download/proxy-scraper-checker-binary-sparc64-unknown-linux-gnu.zip
[thumbv7neon-unknown-linux-gnueabihf]: https://github.com/monosans/proxy-scraper-checker/releases/latest/download/proxy-scraper-checker-binary-thumbv7neon-unknown-linux-gnueabihf.zip
[x86_64-unknown-linux-gnu]: https://github.com/monosans/proxy-scraper-checker/releases/latest/download/proxy-scraper-checker-binary-x86_64-unknown-linux-gnu.zip
[x86_64-unknown-linux-musl]: https://github.com/monosans/proxy-scraper-checker/releases/latest/download/proxy-scraper-checker-binary-x86_64-unknown-linux-musl.zip

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

The script picks the right archive by itself. To fetch one by hand, match the output of `getprop ro.product.cpu.abi`:

| `getprop ro.product.cpu.abi` | Download                                                                            |
| ---------------------------- | ----------------------------------------------------------------------------------- |
| `arm64-v8a`                  | [Download][aarch64-linux-android]                                                   |
| `armeabi-v7a`                | [Download][thumbv7neon-linux-androideabi] ([without NEON][armv7-linux-androideabi]) |
| `armeabi`                    | [Download][arm-linux-androideabi]                                                   |
| `x86_64`                     | [Download][x86_64-linux-android]                                                    |
| `x86`                        | [Download][i686-linux-android]                                                      |

[aarch64-linux-android]: https://github.com/monosans/proxy-scraper-checker/releases/latest/download/proxy-scraper-checker-binary-aarch64-linux-android.zip
[arm-linux-androideabi]: https://github.com/monosans/proxy-scraper-checker/releases/latest/download/proxy-scraper-checker-binary-arm-linux-androideabi.zip
[armv7-linux-androideabi]: https://github.com/monosans/proxy-scraper-checker/releases/latest/download/proxy-scraper-checker-binary-armv7-linux-androideabi.zip
[i686-linux-android]: https://github.com/monosans/proxy-scraper-checker/releases/latest/download/proxy-scraper-checker-binary-i686-linux-android.zip
[thumbv7neon-linux-androideabi]: https://github.com/monosans/proxy-scraper-checker/releases/latest/download/proxy-scraper-checker-binary-thumbv7neon-linux-androideabi.zip
[x86_64-linux-android]: https://github.com/monosans/proxy-scraper-checker/releases/latest/download/proxy-scraper-checker-binary-x86_64-linux-android.zip

</details>

<details>
<summary>FreeBSD</summary>

1. Run `uname -m` and download the archive from the matching row:

   | Processor | `uname -m` | Download                           |
   | --------- | ---------- | ---------------------------------- |
   | 64-bit    | `amd64`    | [Download][x86_64-unknown-freebsd] |
   | 32-bit    | `i386`     | [Download][i686-unknown-freebsd]   |

2. Extract the archive to a dedicated folder.

3. Edit `config.toml`.

4. Run it from that folder:

   ```bash
   ./proxy-scraper-checker
   ```

[i686-unknown-freebsd]: https://github.com/monosans/proxy-scraper-checker/releases/latest/download/proxy-scraper-checker-binary-i686-unknown-freebsd.zip
[x86_64-unknown-freebsd]: https://github.com/monosans/proxy-scraper-checker/releases/latest/download/proxy-scraper-checker-binary-x86_64-unknown-freebsd.zip

</details>

<details>
<summary>Docker on Windows</summary>

> The Docker image logs to stdout instead of showing the interactive TUI.

1. Install [Docker Desktop](https://docs.docker.com/desktop/setup/install/windows-install/).

2. Download the archive for your processor:

   | Processor                                | Download                    |
   | ---------------------------------------- | --------------------------- |
   | 64-bit Intel or AMD (almost every PC)    | [Download][docker-amd64]    |
   | ARM64 (Snapdragon and other ARM laptops) | [Download][docker-arm64-v8] |

   Not sure? Open Settings → System → About and look at "System type": `x64-based processor` means 64-bit Intel or AMD, `ARM-based processor` means ARM64.

3. Extract it to a folder and edit `config.toml`.

4. Build and run from that folder:

   ```powershell
   docker compose build
   docker compose up --no-log-prefix --remove-orphans
   ```

Results land in `out` inside that folder; `output.path` is ignored in Docker.

</details>

<details>
<summary>Docker on Linux / macOS</summary>

> The Docker image logs to stdout instead of showing the interactive TUI.

1. Install [Docker Compose](https://docs.docker.com/compose/install/).

2. Run `uname -m` and download the archive from the matching row:

   | Processor                                                    | `uname -m`                  | Download                    |
   | ------------------------------------------------------------ | --------------------------- | --------------------------- |
   | 64-bit Intel or AMD (including Intel Macs)                   | `x86_64`                    | [Download][docker-amd64]    |
   | 64-bit ARM (Apple Silicon Macs, Raspberry Pi on a 64-bit OS) | `aarch64`, `arm64` on macOS | [Download][docker-arm64-v8] |
   | 32-bit ARMv7                                                 | `armv7l`                    | [Download][docker-arm-v7]   |
   | 32-bit Intel or AMD                                          | `i686`                      | [Download][docker-386]      |
   | 64-bit PowerPC, little-endian                                | `ppc64le`                   | [Download][docker-ppc64le]  |
   | 64-bit RISC-V                                                | `riscv64`                   | [Download][docker-riscv64]  |
   | IBM Z                                                        | `s390x`                     | [Download][docker-s390x]    |

   The `ppc64le`, `riscv64` and `s390x` archives compile the program during `docker compose build`, so the first build takes a while.

3. Extract it to a folder and edit `config.toml`.

4. Build and run from that folder. The build arguments make the files in `./out` belong to your user:

   ```bash
   docker compose build --build-arg UID=$(id -u) --build-arg GID=$(id -g)
   docker compose up --no-log-prefix --remove-orphans
   ```

Results land in `./out`; `output.path` is ignored in Docker.

[docker-386]: https://github.com/monosans/proxy-scraper-checker/releases/latest/download/proxy-scraper-checker-docker-386.zip
[docker-amd64]: https://github.com/monosans/proxy-scraper-checker/releases/latest/download/proxy-scraper-checker-docker-amd64.zip
[docker-arm-v7]: https://github.com/monosans/proxy-scraper-checker/releases/latest/download/proxy-scraper-checker-docker-arm-v7.zip
[docker-arm64-v8]: https://github.com/monosans/proxy-scraper-checker/releases/latest/download/proxy-scraper-checker-docker-arm64-v8.zip
[docker-ppc64le]: https://github.com/monosans/proxy-scraper-checker/releases/latest/download/proxy-scraper-checker-docker-ppc64le.zip
[docker-riscv64]: https://github.com/monosans/proxy-scraper-checker/releases/latest/download/proxy-scraper-checker-docker-riscv64.zip
[docker-s390x]: https://github.com/monosans/proxy-scraper-checker/releases/latest/download/proxy-scraper-checker-docker-s390x.zip

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

|                                                                                                                          |                                                                                                                                                                                                                              |
| ------------------------------------------------------------------------------------------------------------------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **[IPcook.com](https://www.ipcook.com/?ref=githubmonosans&utm_source=github&utm_medium=referral&utm_campaign=monosans)** | <a href="https://www.ipcook.com/?ref=githubmonosans&utm_source=github&utm_medium=referral&utm_campaign=monosans"><img width="400" src="https://github.com/user-attachments/assets/a1215c5c-e648-429a-90d4-a4cfb6b0affc"></a> |

Want your name in this section? Support the project and it goes here.

### Support this project

Star the repository so other people can find it. If you are interested in sponsoring, [DM me on Telegram](https://t.me/monosans).

## License

[MIT](LICENSE)

_This product includes GeoLite2 Data created by MaxMind, available from <https://www.maxmind.com>_
