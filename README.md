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

- Download the archive for your OS from [nightly.link](https://nightly.link/monosans/proxy-scraper-checker/workflows/ci/main?preview).
- Unzip the archive into a separate folder.
- Edit `config.toml`.
- Run the executable.

<!-- ### Docker

- [Install `Docker Compose`](https://docs.docker.com/compose/install/).
- Download and unpack [the archive with the program](https://github.com/monosans/proxy-scraper-checker/archive/refs/heads/main.zip).
- Edit `config.toml` to your preference.
- Run the following commands:
  ```bash
  docker compose build --pull
  docker compose up --no-log-prefix --remove-orphans
  ``` -->

## License

[MIT](LICENSE)

This product includes GeoLite2 Data created by MaxMind, available from <https://www.maxmind.com>.
