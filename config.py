# -*- coding: utf-8 -*-

# How many seconds to wait for the proxy to make a connection.
# The higher this number, the longer the check will take
# and the more proxies you will receive.
TIMEOUT = 10

# Maximum concurrent connections.
# Don't set higher than 900, please.
MAX_CONNECTIONS = 900

# Set to False to sort proxies alphabetically.
SORT_BY_SPEED = True

# Path to the folder where the proxy folders will be saved.
# Leave the quotes empty to save the proxies to the current directory.
SAVE_PATH = ""

# Enable which proxy folders to create.
# Set to False to disable.

# Proxies with any anonymity level.
PROXIES = True
# Anonymous proxies.
PROXIES_ANONYMOUS = True
# Same as PROXIES, but including exit-node's geolocation.
# Geolocation format is ip:port::Country::Region::City
PROXIES_GEOLOCATION = True
# Same as PROXIES_GEOLOCATION, but including exit-node's geolocation.
PROXIES_GEOLOCATION_ANONYMOUS = True


# PROTOCOL - whether to enable checking certain protocol proxies (True or False).
# PROTOCOL_SOURCES - proxy lists URLs.
HTTP = True
HTTP_SOURCES = (
    "https://api.proxyscrape.com/v2/?request=getproxies&protocol=http",
    "https://openproxy.space/list/http",
    "https://raw.githubusercontent.com/almroot/proxylist/master/list.txt",
    "https://raw.githubusercontent.com/clarketm/proxy-list/master/proxy-list-raw.txt",
    "https://raw.githubusercontent.com/hendrikbgr/Free-Proxy-Repo/master/proxy_list.txt",
    "https://raw.githubusercontent.com/jetkai/proxy-list/main/online-proxies/txt/proxies-http%2Bhttps.txt",
    "https://raw.githubusercontent.com/mmpx12/proxy-list/master/http.txt",
    "https://raw.githubusercontent.com/mmpx12/proxy-list/master/https.txt",
    "https://raw.githubusercontent.com/roosterkid/openproxylist/main/HTTPS_RAW.txt",
    "https://raw.githubusercontent.com/ShiftyTR/Proxy-List/master/http.txt",
    "https://raw.githubusercontent.com/ShiftyTR/Proxy-List/master/https.txt",
    "https://raw.githubusercontent.com/sunny9577/proxy-scraper/master/proxies.txt",
    "https://raw.githubusercontent.com/TheSpeedX/PROXY-List/master/http.txt",
    "https://raw.githubusercontent.com/User-R3X/proxy-list/main/online/http%2Bs.txt",
    "https://raw.githubusercontent.com/Volodichev/proxy-list/main/http.txt",
    "https://www.proxy-list.download/api/v1/get?type=http",
    "https://www.proxy-list.download/api/v1/get?type=https",
)
SOCKS4 = True
SOCKS4_SOURCES = (
    "https://api.proxyscrape.com/v2/?request=getproxies&protocol=socks4",
    "https://openproxy.space/list/socks4",
    "https://raw.githubusercontent.com/jetkai/proxy-list/main/online-proxies/txt/proxies-socks4.txt",
    "https://raw.githubusercontent.com/mmpx12/proxy-list/master/socks4.txt",
    "https://raw.githubusercontent.com/roosterkid/openproxylist/main/SOCKS4_RAW.txt",
    "https://raw.githubusercontent.com/ShiftyTR/Proxy-List/master/socks4.txt",
    "https://raw.githubusercontent.com/TheSpeedX/PROXY-List/master/socks4.txt",
    "https://raw.githubusercontent.com/User-R3X/proxy-list/main/online/socks4.txt",
    "https://www.proxy-list.download/api/v1/get?type=socks4",
)
SOCKS5 = True
SOCKS5_SOURCES = (
    "https://api.proxyscrape.com/v2/?request=getproxies&protocol=socks5",
    "https://openproxy.space/list/socks5",
    "https://raw.githubusercontent.com/hookzof/socks5_list/master/proxy.txt",
    "https://raw.githubusercontent.com/jetkai/proxy-list/main/online-proxies/txt/proxies-socks5.txt",
    "https://raw.githubusercontent.com/manuGMG/proxy-365/main/SOCKS5.txt",
    "https://raw.githubusercontent.com/mmpx12/proxy-list/master/socks5.txt",
    "https://raw.githubusercontent.com/roosterkid/openproxylist/main/SOCKS5_RAW.txt",
    "https://raw.githubusercontent.com/ShiftyTR/Proxy-List/master/socks5.txt",
    "https://raw.githubusercontent.com/TheSpeedX/PROXY-List/master/socks5.txt",
    "https://raw.githubusercontent.com/User-R3X/proxy-list/main/online/socks5.txt",
    "https://www.proxy-list.download/api/v1/get?type=socks5",
)
