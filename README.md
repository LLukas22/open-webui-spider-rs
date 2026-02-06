# open-webui-spider-rs

An adapter that lets Open WebUI use a local [`spider-rs`](https://github.com/spider-rs/spider) instance as its web page loading engine to fetch and render web pages.

## Installation

The easiest way to deploy is with Docker. Here’s an example `docker-compose.yml`:

```yaml
services:
  spider:
    image: ghcr.io/llukas22/open-webui-spider-rs:latest
    container_name: open-webui-spider-rs
    ports:
      - "8080:8080"
    environment:
      - RUST_LOG=info
      - APP_CHROME_CONNECTION_URL=http://headless-chrome:9222/json/version
      - APP_PORT=8080
    depends_on:
      - headless-chrome

  headless-chrome:
    image: ghcr.io/llukas22/headless-browser-playwright:latest
    container_name: headless-chrome
```

This starts both `spider-rs` and a headless Chrome instance that `spider-rs` can use to load and render web pages.

## Usage

In Open WebUI, go to **Settings → Web Search** and set **Web-Loader-Engine** to **External**. Then enter the adapter URL, for example:

```text
http://open-webui-spider-rs:8080
```