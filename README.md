# open-webui-spider-rs

An adapter that lets Open WebUI use a local [`spider-rs`](https://github.com/spider-rs/spider) instance as its web page loading engine to fetch and render web pages.

## Installation

The easiest way to deploy is with Docker. Here’s an example `docker-compose.yml`:

```yaml
services:
  spider:
    image: ghcr.io/llukas22/open-webui-spider-rs:latest
    security_opt:
      - seccomp:unconfined
    ports:
      - "8081:8081"
    environment:
      - RUST_LOG=info
      - APP_CHROME_CONNECTION_URL=http://headless-chrome:6000/json/version
      - APP_PORT=8081
      - no_proxy=headless-chrome,localhost,127.0.0.1
      - NO_PROXY=headless-chrome,localhost,127.0.0.1
    depends_on:
      - headless-chrome
    networks:
      - spider-rs-network

  headless-chrome:
    image: ghcr.io/llukas22/headless-browser-playwright:latest
    container_name: headless-chrome
    networks:
      - spider-rs-network
    environment:
      - REMOTE_ADDRESS=headless-chrome
      - HOSTNAME_OVERRIDE=headless-chrome
      - CHROME_ARGS=--remote-allow-origins=*
      - no_proxy=localhost,127.0.0.1
      - NO_PROXY=localhost,127.0.0.1

networks:
  spider-rs-network:
    name: spider-rs-network

```

This starts both `spider-rs` and a headless Chrome instance that `spider-rs` can use to load and render web pages.

## Usage

In Open WebUI, go to **Settings → Web Search** and set **Web-Loader-Engine** to **External**. Then enter the adapter URL, for example:

```text
http://spider:8081
```