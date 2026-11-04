# web-app-host

A lightweight HTTP server that serves a static SPA from the `./wwwroot` folder. It listens on `0.0.0.0:8000` over TCP and, on Unix, optionally on a Unix socket.

Features:
- Static files served with ETags;
- A folder request (`/es/`, `/es`) serves that folder's `index.html` — for multi-page static sites;
- SPA fallback to `index.html`;
- `is-alive` endpoint with the application name and version;
- Version placeholders substituted in a configured file at serve time;
- A list of paths excluded from caching;
- `br` and `gzip` copies of the content, compressed at the maximum level at startup — each client gets the best one it accepts;
- Optional desktop/mobile mode — content served from two folders by `User-Agent`;
- Optional password protection via Basic Auth;
- Optional `robots.txt` that forbids crawlers to crawl the site.

## Environment variables

| Variable | Default | Description |
|---|---|---|
| `UNIX_SOCKET` | not set | Unix only. Path to a Unix socket. If set, the server also listens on this socket (TCP `:8000` stays on in any case). |
| `BUILD_NAME` | `web-app-host` (`CARGO_PKG_NAME`) | Application name returned by the `is-alive` endpoint. |
| `BUILD_VERSION` | `CARGO_PKG_VERSION` from `Cargo.toml` | Application version. Returned by `is-alive` and substituted for `${APP_VERSION}` in the injected file. |
| `COMPILE_TIME` | empty string | Build timestamp. Substituted for `${APP_COMPILE_TIME}` in the injected file. |
| `FILE_TO_VERSION_INJECTION` | not set | Path of the file (relative to the site root) in which `${APP_VERSION}` and `${APP_COMPILE_TIME}` are substituted at serve time. If not set, the injection is disabled. |
| `HOME` | set by the system (`/root` in Docker when running as root) | Folder in which the compressed copies of the content are created at startup: `$HOME/wwwroot-br` and `$HOME/wwwroot-gzip`. If not set, no copies are created and files are served uncompressed. See [Compression](#compression). |
| `WITH_MOBILE` | not set | `=1` enables the desktop/mobile mode: content is served from `./wwwroot/desktop` and `./wwwroot/mobile`. Any other value, or no variable, serves `./wwwroot` as usual. |
| `BASIC-AUTH` | not set | Basic Auth password. If set and not empty, a request without the correct password gets `401` with `WWW-Authenticate: Basic`, and the browser shows a login prompt. Any login is accepted — only the password is checked. A name with a dash cannot be set via `export` in a shell, so `BASIC_AUTH` is accepted as well. |
| `NO_ROBOTS` | not set | `=true` (or `=1`) serves `User-agent: *` / `Disallow: /` at `/robots.txt`, which forbids crawlers to crawl (and index) the site. `robots.txt` from `./wwwroot` is ignored in this case. |

## Compression

At startup the service walks the content folder and creates its compressed copies — with the same folder structure:

| Folder | `Content-Encoding` | Compression level |
|---|---|---|
| `$HOME/wwwroot-br` | `br` | Quality 11 (maximum). The window is the smallest one that covers the file, up to 16 MB (the maximum of the standard brotli): it compresses as well as the 16 MB window, but takes less memory — both to compress and to decompress in the browser |
| `$HOME/wwwroot-gzip` | `gzip` | Level 9 (maximum) |

- The HTTP server (including `is-alive`) starts only when all the copies are created. The preparation time is printed to the log: `Content of './wwwroot' is ready in 1.9s: 120 files, 40 br copies, 40 gzip copies`. `br` with quality 11 takes about 1.5 s per megabyte in a single thread. Files are compressed on all available cores (respecting the CPU limit of the container), the biggest ones first. For a big site, take the startup time into account in the liveness probe.
- The copies of the previous run are removed at startup. The content is scanned once: files that appear or change after the startup are not picked up — a restart is required.
- A copy is created only if the file is bigger than 10 KB and the compressed result is at most 80% of the original. Formats that are compressed already — images, video, audio, `woff`/`woff2`, archives — are not compressed at all.
- The variant is chosen by `Accept-Encoding`: `br` first, then `gzip`. An encoding with `q=0` is treated as refused. If there is no suitable copy, the original file is served uncompressed. The response always has `Vary: Accept-Encoding` — with `WITH_MOBILE=1` it is the single header `Vary: Accept-Encoding, User-Agent`.
- Only the index is kept in memory: file paths and ETags. Each variant — the original, `br` and `gzip` — has its own ETag (SHA-256 of its bytes), so `304` is returned only for the ETag of the same variant. The content of the files is read from the disk on each request.
- If `HOME` is not set or a copies folder can not be created, there are no copies and the files are served uncompressed.
- The `FILE_TO_VERSION_INJECTION` file is always served uncompressed: the version is substituted in the original file.
- With `WITH_MOBILE=1` the copies are created separately for `desktop` and `mobile` — see [Desktop / mobile mode](#desktop--mobile-mode-with_mobile1).

Example on real files (release build, single-threaded compression; sizes in bytes, compression time in parentheses):

| File | Original | `br` | `gzip` |
|---|---|---|---|
| `react-dom.development.js` | 1,080,227 | 181,054 (1,451 ms) | 232,853 (53 ms) |
| `bootstrap.min.css` | 232,803 | 22,701 (304 ms) | 30,953 (5 ms) |

## Basic Auth (`BASIC-AUTH`)

- Every request is checked except `is-alive` and `/robots.txt` (with `NO_ROBOTS=true`) — they stay open for health checks and crawlers.
- The application name (`BUILD_NAME`) is used as the `realm`.
- The password is compared by SHA-256 hashes — the comparison time does not depend on how many characters match.

## Desktop / mobile mode (`WITH_MOBILE=1`)

The site root is split into two folders:

| Folder | Description |
|---|---|
| `./wwwroot/desktop/` | Content for desktop. Has its own `index.html` (index + not-found fallback). |
| `./wwwroot/mobile/` | Content for mobile. Has its own `index.html` (index + not-found fallback). |

How it works:

1. The device type is detected by the `User-Agent` header via `rust-common` (`UserAgentString::get_device_type()`). `Mobile` → the `mobile` folder; `Desktop` and `Tablet` → the `desktop` folder. If the header is missing or unreadable — `desktop`.
2. A file is looked up in the folder of the device first, then in the other one. So a mobile client gets the file from `desktop` if `mobile` does not have it, and vice versa. The `index.html` of a requested folder is looked up in the same order: `/es/` serves `mobile/es/index.html`, and if there is none — `desktop/es/index.html`.
3. If the file is nowhere, the `index.html` of the device folder is served (SPA fallback).
4. **`index.html` is never cached** — neither a direct `/index.html` request, nor `/`, nor any SPA route. This is the file that decides which of the two applications the device gets, so a cached copy of it is not acceptable.
5. All responses have `User-Agent` in the `Vary` header — the same URL serves different content, and the caches in between must know it. It is combined with `Accept-Encoding` into a single `Vary: Accept-Encoding, User-Agent` header, since not every cache combines several `Vary` headers.
6. Paths from `./www-system/.disable-cache` are registered for both folders, `FILE_TO_VERSION_INJECTION` is compared with the path of the served file (for a folder request — with the path of its `index.html`), and the file is read from the folder chosen for the device.
7. Each device has its own compressed copies: `$HOME/wwwroot-br/desktop/`, `$HOME/wwwroot-br/mobile/`, and the same for `gzip`. A file taken from the other folder (item 2) is served with the compressed copies of that folder. Files in the root of `./wwwroot` outside `desktop` and `mobile` are neither scanned nor served in this mode.

## Configuration files

| File | Description |
|---|---|
| `./wwwroot/` | Root of the static files. The index file and the not-found fallback is `index.html` (required for SPA routing). With `WITH_MOBILE=1` — `./wwwroot/desktop/` and `./wwwroot/mobile/`. |
| `./www-system/.disable-cache` | Optional list of paths excluded from caching. One path per line, empty lines are ignored. `/` and `/index.html` are always excluded from caching. If the file is missing, a message is printed and an empty list is used. |

## Caching

| What | Headers |
|---|---|
| `/`, `/index.html` and paths from `.disable-cache` | `Cache-Control: no-store, no-cache, must-revalidate, max-age=0`, `Pragma: no-cache`, `Expires: 0`. No ETag is set and `304` is never returned — the content always comes in full. |
| Other files | `ETag` + `Cache-Control: no-cache` — the client caches the file, but revalidates it every time via `If-None-Match` and gets `304` if the file has not changed. The original file and each of its compressed copies have their own ETag. |
| Folder (`/es/`, `/es`) → its `index.html` | Same as the regular file `/es/index.html`: `ETag` + `Cache-Control: no-cache`. `.disable-cache` is checked by both the request path (`/es/`) and the file path (`/es/index.html`). |
| SPA route (no such file) | Without `WITH_MOBILE` — as a regular file (`ETag` + `no-cache`). With `WITH_MOBILE=1` — as `index.html`, i.e. not cached. |

## Default behavior

- TCP: `0.0.0.0:8000` (hardcoded).
- Static files: the `./wwwroot` folder, ETag enabled. At startup the `br` and `gzip` copies are created in `$HOME/wwwroot-br` and `$HOME/wwwroot-gzip`.
- Index / not-found: `index.html`.
- Folder → its `index.html`. If the path ends with `/` or points to a folder, the `index.html` of that folder is served: `/es/` and `/es` → `./wwwroot/es/index.html`, `/es/legal/` → `./wwwroot/es/legal/index.html`. It is then handled as the file `/es/index.html`: `Content-Type` is taken from it (`text/html`), ETag and caching are the same as for a regular file, and `FILE_TO_VERSION_INJECTION` is compared with this path. There is no `/es` → `/es/` redirect, so on a page opened at `/es` the browser resolves relative links from `/`. In a multi-page site use absolute links or links to folders with a trailing slash.
- If the folder has no `index.html` (or there is no such folder at all) — SPA fallback to the root `/index.html`. `/` and `/index.html` are the root index, not cached.
- Paths with `.` or `..` segments or with `\` — including the percent-encoded ones (`%2e`, `%2f`, `%5c`) — are rejected with `403 Forbidden`: a request never leads out of the root folder.
- A request is resolved only to the files scanned at startup — the disk is never read by the path of the request.
- Symlinks in the content are followed only while their real target stays within the content root. A symlink that leads out of it is skipped at startup with the log line `Skipping '…' - it leads out of the content root to '…'`. Files are read by the real path resolved at startup, so a symlink re-pointed later does not lead out of the root either.
- Middlewares (in the order of registration): `IsAliveMiddleware`, `NoRobotsMiddleware` (with `NO_ROBOTS=true`), `BasicAuthMiddleware` (when `BASIC-AUTH` is set), `RequestFlowMiddleware`.
- `RequestFlowMiddleware` splits the handling into two flows: `handle_no_mobile` (default) and `handle_with_mobile` (with `WITH_MOBILE=1`).

## Run example

```bash
env 'BASIC-AUTH=my-password' \
NO_ROBOTS=true \
BUILD_NAME=my-spa \
BUILD_VERSION=1.4.2 \
COMPILE_TIME="2026-05-23T10:00:00Z" \
FILE_TO_VERSION_INJECTION=/index.html \
WITH_MOBILE=1 \
./web-app-host
```
