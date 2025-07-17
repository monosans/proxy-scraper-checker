# proxy-scraper-checker

[![CI](https://github.com/monosans/proxy-scraper-checker/actions/workflows/ci.yml/badge.svg)](https://github.com/monosans/proxy-scraper-checker/actions/workflows/ci.yml)

![Screenshot](https://github.com/user-attachments/assets/0ac37021-d11c-4f68-b80d-bafdbaeb00bb)

HTTP, SOCKS4, SOCKS5 proxies scraper and checker.

- Written in Rust.
- Can determine if the proxy is anonymous.
- Supports determining the geolocation and ASN of the proxy exit node.
- Can sort proxies by speed.
- Uses regex to find proxies of format `protocol://username:password@host:port` on a web page or in a local file, allowing proxies to be extracted even from json without code changes.
- Supports proxies with authentication.
- It is possible to specify the URL to which to send a request to check the proxy.
- Supports saving to plain text and json.
- Asynchronous.

You can get proxies obtained using this project in [monosans/proxy-list](https://github.com/monosans/proxy-list).

## ⚠️ Safety and IP Reputation Considerations

**Important**: This tool makes many network requests and can impact your IP reputation if not used carefully. Please read these guidelines before running the software.

### Potential Risks

- **Proxy sources**: The tool fetches proxy lists from public sources that may include honeypots or monitoring systems
- **Proxy testing**: When checking proxies, your requests pass through potentially malicious or monitored proxy servers
- **Rate limiting**: Making many concurrent requests may trigger rate limiting or detection systems
- **IP reputation**: High-volume automated requests can negatively impact your IP's reputation with various services

### Best Practices for Safe Usage

1. **Use a VPN or proxy when running this tool** to protect your real IP address
2. **Reduce concurrent checks**: Lower `max_concurrent_checks` from the default 4096 to a more conservative value (e.g., 50-100)
3. **Increase timeouts**: Use longer timeout values to avoid appearing overly aggressive
4. **Use trusted proxy sources**: Remove or replace default public sources with your own trusted sources
5. **Run during off-peak hours**: Avoid running during high-traffic periods
6. **Monitor your IP reputation**: Check if your IP gets blacklisted on reputation databases
7. **Use dedicated infrastructure**: Consider running this on a separate server/VPS, not your main development machine
8. **Respect rate limits**: Don't run the tool continuously; space out your scanning sessions

### Recommended Configuration Changes

For safer operation, consider these config.toml modifications:

```toml
[checking]
# Reduce concurrent checks to avoid detection
max_concurrent_checks = 100

# Increase timeout to be less aggressive
timeout = 10.0

[scraping]
# Reduce max proxies per source
max_proxies_per_source = 1000

# Increase scraping timeout
timeout = 10.0
```

A safer configuration template is provided in `config-safe.toml` - copy this file to `config.toml` for more conservative defaults.

### Legal and Ethical Considerations

- Ensure you comply with the terms of service of any proxy sources you use
- Respect robots.txt and rate limiting of target websites
- Use responsibly and avoid overwhelming services with requests
- Consider the privacy implications of routing traffic through unknown proxies

## Installation and usage

### Binary

> [!NOTE]
> There is a separate section for Termux.

1. Download the archive for your platform from [nightly.link](https://nightly.link/monosans/proxy-scraper-checker/workflows/ci/main?preview). If you are not sure which archive you need, use [the table](https://doc.rust-lang.org/beta/rustc/platform-support.html).
1. Unpack the archive into a separate folder.
1. Edit `config.toml` to your preference.
1. Run the executable.

### Docker

> [!NOTE]
> Only a simple user interface in the form of logs is implemented for Docker.

1. [Install `Docker Compose`](https://docs.docker.com/compose/install/).
1. Download the archive for your platform from [nightly.link](https://nightly.link/monosans/proxy-scraper-checker/workflows/ci/main?preview). Look for artifacts named `proxy-scraper-checker-docker`.
1. Unpack the archive into a separate folder.
1. Edit `config.toml` to your preference.
1. Run the following commands:

   Windows:

   ```bash
   docker compose build
   docker compose up --no-log-prefix --remove-orphans
   ```

   Linux/macOS:

   ```bash
   docker compose build --build-arg UID=$(id -u) --build-arg GID=$(id -g)
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
