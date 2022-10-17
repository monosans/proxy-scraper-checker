# YYYY-MM-DD

## 2022-10-17

- Treat a proxy as invalid if ip-api returns an invalid response.
- Remove useless source.

## 2022-09-10

- Add more sources.

## 2022-09-08

- Make Proxy hashing faster.
- Remove the ineffective source.

## 2022-08-23

- Add more sources.

## 2022-08-20

- Treat proxy as non-working if status code >= 400.
- Fix a theoretically possible bug when checking for anonymity.

## 2022-08-08

- Improve errors output.

## 2022-08-05

- Add more sources.

## 2022-07-16

- Remove broken sources.
- Improve errors output.

## 2022-07-11

- Many minor improvements.

## 2022-07-07

- Add `User-Agent` header when fetching sources.
- Add more sources.

## 2022-06-16

- Edit uvloop marker.
- Improve sorting code.

## 2022-05-28

- Remove some sources.

## 2022-05-10

- Add `uvloop` for non-Windows platforms to improve performance.

## 2022-04-27

- Add more sources.

## 2022-04-13

- `config.py` -> `config.ini`.

## 2022-04-11

- Add `__slots__` attribute to all objects.
- Improve regex.

## 2022-04-08

- Use `|` as geolocation separator.
- Use `?` instead of `None` if geolocation is unknown.

## 2022-04-07

- Add `multi line` flag to regex.
- Add more sources.

## 2022-04-03

- Add more sources.

## 2022-03-31

- Show a message if there is no proxies in the source.
- Add more sources.
- Set default `TIMEOUT` to 10.

## 2022-02-14

- Delete disabled folders when saving proxies.

## 2022-02-13

- Print absolute path instead of relative.
- Improve regex.

## 2022-02-12

- Replace `maxminddb` with geolocation API.
- Remove `IP_SERVICE` from config.
- Add settings to config:
  - `PROXIES`
  - `PROXIES_ANONYMOUS`
  - `PROXIES_GEOLOCATION`
  - `PROXIES_GEOLOCATION_ANONYMOUS`
- Set default `TIMEOUT` to 5.
- Use regex for finding proxies.
- Add new sources.
- Remove outdated sources.
- Performance improvements.

## 2022-02-09

- GeoLite2 City v2022-02-08

## 2022-02-01

- GeoLite2 City v2022-02-01

## 2022-01-26

- GeoLite2 City v2022-01-25

## 2022-01-19

- GeoLite2 City v2022-01-18

## 2022-01-12

- GeoLite2 City v2022-01-11

## 2022-01-06

- GeoLite2 City v2022-01-04

## 2022-01-04

- Fix sorting

## 2022-01-03

- Add `SAVE_PATH` option
- Fix sorting

## 2022-01-02

- Add `SORT_BY_SPEED` option

## 2022-01-01

- Improve the file saving algorithm

## 2021-12-28

- GeoLite2 City v2021-12-28

## 2021-12-22

- GeoLite2 City v2021-12-21

## 2021-12-16

- GeoLite2 City v2021-12-14
