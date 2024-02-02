# proxy-scraper-checker

[![CI](https://github.com/monosans/proxy-scraper-checker/actions/workflows/ci.yml/badge.svg)](https://github.com/monosans/proxy-scraper-checker/actions/workflows/ci.yml)

![Screenshot](screenshot.png)

HTTP, SOCKS4, SOCKS5 proxies scraper and checker.

- Can determine if the proxy is anonymous.
- Supports determining the geolocation of the proxy exit node.
- Can sort proxies by speed.
- Uses regex to find proxies of format `protocol://username:password@ip:port` on a web page or in a local file, allowing proxies to be extracted even from json without code changes.
- Supports proxies with authentication.
- It is possible to specify the URL to which to send a request to check the proxy.
- Supports saving to plain text and json.
- Asynchronous.

You can get proxies obtained using this project in [monosans/proxy-list](https://github.com/monosans/proxy-list).

## Installation and usage

### Standalone executable

This is the easiest way, but it is only available for x64 Windows, macOS and Linux. Just download the archive for your OS from [nightly.link](https://nightly.link/monosans/proxy-scraper-checker/workflows/ci/main?preview), unzip it, edit `config.toml` and run the executable.

If Windows Defender detects an executable file as a virus, please read [this](https://github.com/Nuitka/Nuitka/issues/2495#issuecomment-1762836583).

### Docker

- [Install `Docker Compose`](https://docs.docker.com/compose/install/).
- Download and unpack [the archive with the program](https://github.com/monosans/proxy-scraper-checker/archive/refs/heads/main.zip).
- Edit `config.toml` to your preference.
- Run the following commands:
  ```bash
  docker compose build --pull
  docker compose up --no-log-prefix
  ```

### Running from source code

#### Desktop

- Install [Python](https://python.org/downloads). The minimum version required is 3.8.
- Download and unpack [the archive with the program](https://github.com/monosans/proxy-scraper-checker/archive/refs/heads/main.zip).
- Edit `config.toml` to your preference.
- Run the script that installs dependencies and starts `proxy-scraper-checker`:
  - On Windows run `start.cmd`
  - On Unix-like operating systems run `start.sh`

#### Termux

To use `proxy-scraper-checker` in Termux, knowledge of the Unix command-line interface is required.

- Download Termux from [F-Droid](https://f-droid.org/en/packages/com.termux/). [Don't download it from Google Play](https://github.com/termux/termux-app#google-play-store-deprecated).
- Run the following command (it will automatically update Termux packages, install Python, and download and install `proxy-scraper-checker`):
  ```bash
  bash <(curl -fsSL 'https://raw.githubusercontent.com/monosans/proxy-scraper-checker/main/install-termux.sh')
  ```
- Edit `~/proxy-scraper-checker/config.toml` to your preference using a text editor (vim/nano).
- To run `proxy-scraper-checker` use the following command:
  ```bash
  cd ~/proxy-scraper-checker && sh start-termux.sh
  ```

## Something else?

All other info is available in `config.toml` file.

## License

[MIT](LICENSE)

This product includes GeoLite2 Data created by MaxMind, available from <https://www.maxmind.com>.
