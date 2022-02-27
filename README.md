# proxy-scraper-checker

![Screenshot](screenshot.png)

HTTP, SOCKS4, SOCKS5 proxies scraper and checker.

- Asynchronous.
- Uses regex to search for proxies (ip:port format) on a web page, which allows you to pull out proxies even from json without making any changes to the code.
- Supports determining the geolocation of the proxy exit node.
- Can determine if a proxy is anonymous.

For a version that uses Python's built-in `logging` instead of [rich](https://github.com/willmcgugan/rich), see the [simple-output](https://github.com/monosans/proxy-scraper-checker/tree/simple-output) branch.

## Usage

- Make sure `Python` version is 3.7 or higher.
- Install dependencies from `requirements.txt` (`python -m pip install -U -r requirements.txt`).
  - If you want to improve the performance, you can also install extra dependencies. See [aiohttp documentation](https://docs.aiohttp.org/en/stable/index.html#library-installation).
- Edit `config.py` according to your preference.
- Run `main.py`.

## Folders description

When the script finishes running, the following folders will be created (this behavior can be changed in the config):

- `proxies` - proxies with any anonymity level.
- `proxies_anonymous` - anonymous proxies.
- `proxies_geolocation` - same as `proxies`, but including exit-node's geolocation.
- `proxies_geolocation_anonymous` - same as `proxies_anonymous`, but including exit-node's geolocation.

Geolocation format is `ip:port::Country::Region::City`.

## Buy me a coffee

Ask for details in [Telegram](https://t.me/monosans) or [VK](https://vk.com/id607137534).

## License

[MIT](LICENSE)
