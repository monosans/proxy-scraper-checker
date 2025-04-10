# proxy-scraper-checker

[![CI](https://github.com/monosans/proxy-scraper-checker/actions/workflows/ci.yml/badge.svg)](https://github.com/monosans/proxy-scraper-checker/actions/workflows/ci.yml)

![Screenshot](https://github.com/user-attachments/assets/e895154c-b5d9-4efa-948c-289287cbc20a)

HTTP, SOCKS4, SOCKS5 proxies scraper and checker.

- Written in Rust.
- Can determine if the proxy is anonymous.
- Supports determining the geolocation of the proxy exit node.
- Can sort proxies by speed.
- Uses regex to find proxies of format `protocol://username:password@host:port` on a web page or in a local file, allowing proxies to be extracted even from json without code changes.
- Supports proxies with authentication.
- It is possible to specify the URL to which to send a request to check the proxy.
- Supports saving to plain text and json.
- Asynchronous.

You can get proxies obtained using this project in [monosans/proxy-list](https://github.com/monosans/proxy-list).

## Installation and usage

### Binary

> [!NOTE]
> The instructions for Termux are below.

1. Download the archive for your platform from [nightly.link](https://nightly.link/monosans/proxy-scraper-checker/workflows/ci/main?preview). If you can't figure out which archive you need, use [these tables](https://doc.rust-lang.org/beta/rustc/platform-support.html).
1. Unpack the archive into a separate folder.
1. Edit `config.toml` to your preference.
1. Run the executable.

### Docker

> [!WARNING]
> User interface does not work with Docker.

1. [Install `Docker Compose`](https://docs.docker.com/compose/install/).
1. Download [the archive with the program](https://github.com/monosans/proxy-scraper-checker/archive/refs/heads/main.zip).
1. Unpack the archive into a separate folder.
1. Edit `config.toml` to your preference.
1. Run the following commands:

   Windows:

   ```bash
   docker compose build --pull
   docker compose up --no-log-prefix --remove-orphans
   ```

   Linux/macOS:

   ```bash
   docker compose build --pull --build-arg UID=$(id -u) --build-arg GID=$(id -g)
   docker compose up --no-log-prefix --remove-orphans
   ```

### Termux

1. Download Termux from [F-Droid](https://f-droid.org/en/packages/com.termux/). [Don't download it from Google Play](https://github.com/termux/termux-app#google-play-store-experimental-branch).
1. Run the following command. It will automatically download and install `proxy-scraper-checker`.

   ```bash
   bash <(curl -fsSL 'https://raw.githubusercontent.com/monosans/proxy-scraper-checker/main/termux.sh')
   ```

1. Edit `~/proxy-scraper-checker/config.toml` to your preference using a text editor (vim/nano).
1. To run `proxy-scraper-checker` use the following command:
   ```bash
   cd ~/proxy-scraper-checker && ./proxy-scraper-checker
   ```

## License

[MIT](LICENSE)

This product includes GeoLite2 Data created by MaxMind, available from <https://www.maxmind.com>.
