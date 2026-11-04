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
- Optional `robots.txt` that forbids crawlers to crawl the site;
- Hidden files (`.env`, `.git`, …) are never served;
- `Content-Type` by the file extension and protective headers on every file.

## Environment variables

| Variable | Default | Description |
|---|---|---|
| `UNIX_SOCKET` | not set | Unix only. Path to a Unix socket. If set, the server also listens on this socket (TCP `:8000` stays on in any case). In the Docker image the folder of the socket must be writable by the user `web-app-host` (UID `10001`). |
| `WWW_ROOT` | `./wwwroot` | Root folder of the content. A trailing `/` is ignored. With `WITH_MOBILE=1` the `desktop` and `mobile` folders are inside it. |
| `BUILD_NAME` | `web-app-host` (`CARGO_PKG_NAME`) | Application name returned by the `is-alive` endpoint. |
| `BUILD_VERSION` | `CARGO_PKG_VERSION` from `Cargo.toml` | Application version. Returned by `is-alive` and substituted for `${APP_VERSION}` in the injected file. |
| `COMPILE_TIME` | empty string | Build timestamp. Substituted for `${APP_COMPILE_TIME}` in the injected file. |
| `FILE_TO_VERSION_INJECTION` | not set | Path of the file (relative to the site root) in which `${APP_VERSION}` and `${APP_COMPILE_TIME}` are substituted at serve time. If not set, the injection is disabled. |
| `HOME` | set by the system (`/home/web-app-host` in the Docker image) | Folder in which the compressed copies of the content are created at startup: `$HOME/wwwroot-br` and `$HOME/wwwroot-gzip`. If not set, no copies are created and files are served uncompressed. See [Compression](#compression). |
| `ENV_INFO` | not set | Returned by `is-alive` as is. The endpoint stays open even with `BASIC-AUTH`, so do not put anything secret here. |
| `WITH_MOBILE` | not set | `=1` enables the desktop/mobile mode: content is served from `./wwwroot/desktop` and `./wwwroot/mobile`. Any other value, or no variable, serves `./wwwroot` as usual. |
| `BASIC-AUTH` | not set | Basic Auth password. If set and not empty, a request without the correct password gets `401` with `WWW-Authenticate: Basic`, and the browser shows a login prompt. Any login is accepted — only the password is checked. A name with a dash cannot be set via `export` in a shell, so `BASIC_AUTH` is accepted as well. |
| `NO_ROBOTS` | not set | `=true` (or `=1`) serves `User-agent: *` / `Disallow: /` at `/robots.txt`, which forbids crawlers to crawl (and index) the site. `robots.txt` from `./wwwroot` is ignored in this case. |
| `ALLOW_FRAMING` | not set | `=true` (or `=1`) lets other sites show the pages in frames. By default every file is served with `X-Frame-Options: SAMEORIGIN`: only pages of the same site may frame it, so no other site can trick the user into clicking on the page in an invisible frame. Set it if the application is embedded into a page of another origin. |

## Compression

At startup the service walks the content folder, and then creates its compressed copies in the background — with the same folder structure:

| Folder | `Content-Encoding` | Compression level |
|---|---|---|
| `$HOME/wwwroot-br` | `br` | Quality 11 (maximum). The window is the smallest one that covers the file, up to 16 MB (the maximum of the standard brotli): it compresses as well as the 16 MB window, but takes less memory — both to compress and to decompress in the browser |
| `$HOME/wwwroot-gzip` | `gzip` | Level 9 (maximum) |

- The HTTP server starts right after the scan: the scan only finds the files and does not read them. The log shows `Content of './wwwroot' is scanned in 1.2ms: 120 files — they are prepared in the background`.
- The files are prepared in the background by one thread, so the compression takes one core at most. It takes the files one by one, the smallest first: reads the file, calculates the ETag of the original, then creates the `gzip` copy and the `br` one. `br` with quality 11 takes about 1.5 s per megabyte, `gzip` with level 9 is about 25 times faster — so the `gzip` copy goes first, and the client which accepts only `gzip` does not wait for the `br` one. The thread ends when all the files are prepared, logging `Content is prepared in the background in 6.7s: 120 files, 40 br copies, 40 gzip copies`.
- The file is read and its copies are written by the chunks of 64 KB: neither the file nor its copies are kept in memory whole — the ETag and the size of a copy are calculated while it is written. A copy is written to a hidden temp file next to it — `.app.js.temp` for `app.js` — and gets its name when it is complete, so a file with the name of a copy is always the whole copy, even if the service stops while writing it. Being hidden, the temp file can not take the name of the copy of another file. A copy which turns out not to be compressed well is removed.
- The biggest memory need of the service is `br` with quality 11 itself: while it compresses a file it keeps its own working memory of about 20 times the size of the file — around 90 MB for a 3.8 MB script, freed when the file is compressed. The rest of the preparation takes a few megabytes whatever the size of the files is: a 40 MB video gets its ETag with a peak of under 4 MB for the whole service.
- A request always gets the best variant the client accepts — `br` if it accepts `br`, even when the `gzip` copy of the file is ready and the `br` one is not yet. So the client and the caches in the middle keep the very variant, with its ETag, that they get from then on. A client which does not accept `br` does not wait for it. A file which is not compressed (see below) only needs its ETag — it does not wait for the compression.
- If that variant is not ready yet, the request waits: the file is put into the priority queue — the thread takes it right after the file it is preparing — and the request checks every 100 ms whether the file is ready. Requests for the other files are served meanwhile. The `FILE_TO_VERSION_INJECTION` file does not wait: it is always served from the original.
- If the file can not be read, the request gets `500` instead of waiting.
- The copies of the previous run are removed at startup. The content is scanned once: files that appear or change after the startup are not picked up — a restart is required.
- A copy is created only if the file is bigger than 10 KB and the compressed result is at most 80% of the original. Formats that are compressed already — images, video, audio, `woff`/`woff2`, archives — are not compressed at all.
- The variant is chosen by `Accept-Encoding`: `br` first, then `gzip`. An encoding with `q=0` is treated as refused. If there is no suitable copy, the original file is served uncompressed. The response always has `Vary: Accept-Encoding` — with `WITH_MOBILE=1` it is the single header `Vary: Accept-Encoding, User-Agent`.
- Only the index is kept in memory: file paths and ETags. Each variant — the original, `br` and `gzip` — has its own ETag (SHA-256 of its bytes), so `304` is returned only for the ETag of the same variant. The content of the files is read from the disk on each request.
- A file up to 128 KB — the variant which is sent: the original or the copy — is read whole and sent in one piece. A bigger one is read and sent by the chunks of 64 KB, so a request takes about 3 chunks of memory (~192 KB) whatever the size of the file is: one chunk is read while one waits in the queue and one is being sent. 64 KB is the default window of an HTTP/2 stream — a bigger chunk would wait for the client to confirm the window anyway. Such a response has `Content-Length` — the size the file had when the request opened it — and the same headers as the one sent in one piece. If the file can not be opened, the client gets `500`; if it can not be read while it is sent, the response ends before its `Content-Length`, so the client sees it is broken.
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
- A request with a wrong password gets `401` after a 1 s delay, so each guess takes at least a second. Only that request waits: the other requests are served meanwhile. A request without credentials gets `401` at once — this is how the browser asks for the password.
- A wrong password is written to the log as a warning (process `RejectedRequest`) with the method, the path, the `User-Agent`, the socket address and the whole `X-Forwarded-For` chain. Its first address is set by the client and can be forged; the real one is the address added by the proxy. The credentials are never written.
- The service does not limit the attempts per IP — do it in the ingress in front of it. The service has no TLS either: without a TLS-terminating proxy the password goes over the network in clear text.

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

- The ETag is a quoted strong ETag, as RFC 9110 requires: `ETag: "base64-of-sha256"`. It is built and checked by the ETag functions of `my-http-server`, the same as in the other services on the library.
- `If-None-Match` is read as one list, even when it comes as several header lines: `304` is returned if any ETag of the list matches. A weak ETag (`W/"…"`) is compared by its value, and `*` matches any file. A malformed list matches nothing, so the file is sent in full.
- The previous versions of the service sent the same hash without the quotes. Such an ETag still matches, so after an upgrade the clients get `304` instead of downloading the files again.
- `304` carries the same `ETag`, `Cache-Control` and `Vary` as `200` would — the client and the caches in between refresh the stored response with them.

## Response headers

- `Content-Type` is taken by the file extension from the table of `my-http-server` — the same types as the other services on the library: html, css and js with `charset=utf-8`, json, map, webmanifest, xml, txt, csv, yaml, wasm, pdf, images, fonts, video, audio. The service adds the extensions the library does not know: cjs, md, bmp, eot, ogv, m4a, oga, weba, zip, gz. A file with an unknown extension is sent without `Content-Type`.
- Every file is served with `X-Content-Type-Options: nosniff` (the browser does not guess the type by the content), `Referrer-Policy: strict-origin-when-cross-origin` and `X-Frame-Options: SAMEORIGIN` (without it when `ALLOW_FRAMING=true`).
- `Content-Security-Policy` is not set: it depends on the application, so set it in the application or in the ingress.

## Default behavior

- TCP: `0.0.0.0:8000` (hardcoded).
- Static files: the `./wwwroot` folder, ETag enabled. After the start the `br` and `gzip` copies are created in the background in `$HOME/wwwroot-br` and `$HOME/wwwroot-gzip`.
- Index / not-found: `index.html`.
- Folder → its `index.html`. If the path ends with `/` or points to a folder, the `index.html` of that folder is served: `/es/` and `/es` → `./wwwroot/es/index.html`, `/es/legal/` → `./wwwroot/es/legal/index.html`. It is then handled as the file `/es/index.html`: `Content-Type` is taken from it (`text/html; charset=utf-8`), ETag and caching are the same as for a regular file, and `FILE_TO_VERSION_INJECTION` is compared with this path. There is no `/es` → `/es/` redirect, so on a page opened at `/es` the browser resolves relative links from `/`. In a multi-page site use absolute links or links to folders with a trailing slash.
- If the folder has no `index.html` (or there is no such folder at all) — SPA fallback to the root `/index.html`. `/` and `/index.html` are the root index, not cached.
- Paths with `.` or `..` segments or with `\` — including the percent-encoded ones (`%2e`, `%2f`, `%5c`) — are rejected with `403 Forbidden`: a request never leads out of the root folder. Each such request is written to the log as a warning (process `RejectedRequest`) — see [Basic Auth](#basic-auth-basic-auth) for its fields.
- A request is resolved only to the files scanned at startup — the disk is never read by the path of the request.
- Hidden files and folders — the ones whose name starts with `.`, such as `.env`, `.git`, `.DS_Store` — are neither scanned nor served, at any depth. The exception is `.well-known`, used by ACME challenges, `security.txt` and the links of mobile applications; hidden files inside it are skipped as well. Each skipped name is logged at startup: `Skipping '…' - hidden files are not served`. A request for such a file is handled as any unknown path, i.e. by the SPA fallback.
- Symlinks in the content are followed only while their real target stays within the content root. A symlink that leads out of it is skipped at startup with the log line `Skipping '…' - it leads out of the content root to '…'`. Files are read by the real path resolved at startup, so a symlink re-pointed later does not lead out of the root either.
- Middlewares (in the order of registration): `IsAliveMiddleware`, `NoRobotsMiddleware` (with `NO_ROBOTS=true`), `BasicAuthMiddleware` (when `BASIC-AUTH` is set), `RequestFlowMiddleware`.
- `RequestFlowMiddleware` splits the handling into two flows: `handle_no_mobile` (default) and `handle_with_mobile` (with `WITH_MOBILE=1`).

## Docker image

- The service runs as the non-root user `web-app-host` with the fixed UID `10001`, so Kubernetes can check `runAsNonRoot`. Its home folder `/home/web-app-host` keeps the compressed copies of the content.
- The content in `/app/wwwroot` only has to be readable by this user.
- The release workflow builds with the committed `Cargo.lock` and runs `cargo audit` before the build: a dependency with a known vulnerability stops the release.

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
