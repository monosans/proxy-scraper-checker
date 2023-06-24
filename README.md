# proxy-scraper-checker

[![CI](https://github.com/monosans/proxy-scraper-checker/actions/workflows/ci.yml/badge.svg)](https://github.com/monosans/proxy-scraper-checker/actions/workflows/ci.yml)

![Screenshot](screenshot.png)

HTTP, SOCKS4, SOCKS5 proxies scraper and checker.

- Asynchronous.
- Uses regex to search for proxies (ip:port format) on a web page, allowing proxies to be extracted even from json without making changes to the code.
- It is possible to specify the URL to which to send a request to check the proxy.
- Can sort proxies by speed.
- Supports determining the geolocation of the proxy exit node.
- Can determine if the proxy is anonymous.

You can get proxies obtained using this script in [monosans/proxy-list](https://github.com/victorgeel/proxy-list-update).

## Installation and usage

- Download and unpack [the archive with the program](https://github.com/monosans/proxy-scraper-checker/archive/refs/heads/main.zip).
- Edit `config.ini` according to your preference.
- Install [Python](https://python.org/downloads) (minimum required version is 3.7).
- Install dependencies and run the script. There are 2 ways to do this:

  - Automatic:
    - On Windows run `start.cmd`
    - On Unix-like OS run `start.sh`
  - Manual:
    <details>
      <summary>Windows (click to expand)</summary>

    1. `cd` into the unpacked folder

    1. Install dependencies with the command:

       ```bash
       py -m pip install -U --no-cache-dir --disable-pip-version-check pip setuptools wheel; py -m pip install -U --no-cache-dir --disable-pip-version-check -r requirements.txt
       ```

    1. Run with the command:

       ```bash
       py -m proxy_scraper_checker
       ```

    </details>
    <details>
      <summary>Unix-like OS (click to expand)</summary>

    1. `cd` into the unpacked folder

    1. Install dependencies with the command:

       ```bash
       python3 -m pip install -U --no-cache-dir --disable-pip-version-check pip setuptools wheel && python3 -m pip install -U --no-cache-dir --disable-pip-version-check -r requirements.txt
       ```

    1. Run with the command:

       ```bash
       python3 -m proxy_scraper_checker
       ```

    </details>

## Folders description

When the script finishes running, the following folders will be created (this behavior can be changed in the config):

- `proxies` - proxies with any anonymity level.
- `proxies_anonymous` - anonymous proxies.
- `proxies_geolocation` - same as `proxies`, but includes exit-node's geolocation.
- `proxies_geolocation_anonymous` - same as `proxies_anonymous`, but includes exit-node's geolocation.

Geolocation format is `ip:port|Country|Region|City`.

## Buy me a coffee

Ask for details on [Telegram](https://t.me/monosans) or [VK](https://vk.com/id607137534).

## License

[MIT](LICENSE)
