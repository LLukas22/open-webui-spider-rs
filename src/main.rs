use anyhow::{Context, Result};
use axum::{
    Router,
    extract::{Json, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use config::Config;
use env_logger::Env;
use log::{info, warn};
use moka::future::Cache;
use serde::{Deserialize, Serialize};
use spider::configuration::{ChromeEventTracker, Fingerprint};
use spider::features::chrome_common::{
    RequestInterceptConfiguration, WaitForDelay, WaitForIdleNetwork, WaitForSelector,
};
use spider::features::chrome_viewport;
use spider::tokio;
use spider::website::Website;
use spider_transformations::transformation::content;
use std::time::{Duration, Instant};
use tokio::signal;
use utoipa::{OpenApi, ToSchema};
use utoipa_swagger_ui::SwaggerUi;

#[derive(Clone, Deserialize)]
struct Settings {
    chrome_connection_url: Option<String>,
    cache_ttl_seconds: u64,
    cache_max_entries: u64,
    server_port: u16,
}

#[derive(Clone)]
struct AppState {
    settings: Settings,
    http_client: reqwest::Client,
    cache: Cache<String, CachedPage>,
}

#[derive(Deserialize, ToSchema)]
struct CrawlRequest {
    #[schema(example = json!(["https://www.google.com"]))]
    urls: Vec<String>,
}

#[derive(Serialize, ToSchema)]
struct CrawlResponse {
    page_content: String,
    metadata: Metadata,
}

#[derive(Serialize, ToSchema)]
struct Metadata {
    source: String,
}

#[derive(Clone)]
struct CachedPage {
    source: String,
    content: String,
}

#[derive(OpenApi)]
#[openapi(
    paths(
        crawl_handler,
        health_check
    ),
    components(
        schemas(CrawlRequest, CrawlResponse, Metadata)
    ),
    tags(
        (name = "spider", description = "Spider API")
    )
)]
struct ApiDoc;

#[utoipa::path(
    get,
    path = "/health",
    responses(
        (status = 200, description = "Health check passed", body = String),
        (status = 503, description = "Chromium unreachable", body = String)
    )
)]
async fn health_check(State(state): State<AppState>) -> impl IntoResponse {
    let chrome_connection_url = match &state.settings.chrome_connection_url {
        Some(url) => url.as_str(),
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "Chromium connection URL not configured",
            );
        }
    };

    match state.http_client.get(chrome_connection_url).send().await {
        Ok(resp) if resp.status().is_success() => (StatusCode::OK, "OK"),
        _ => (
            StatusCode::SERVICE_UNAVAILABLE,
            "Chromium instance unreachable",
        ),
    }
}

async fn crawl_single_page(website: &Website, target_url: &str) -> Option<spider::page::Page> {
    let mut w = website.clone();
    let mut rx = w.subscribe(0).expect("receiver enabled");

    tokio::task::spawn(async move {
        w.crawl_smart().await;
        w.unsubscribe();
    });

    while let Ok(page) = rx.recv().await {
        if page.is_empty() {
            continue;
        }
        if page.get_url() == target_url {
            return Some(page);
        }
    }

    None
}

async fn crawl_page_uncached(
    url: &str,
    chrome_connection_url: &Option<String>,
) -> Result<Option<CachedPage>> {
    let started_at = Instant::now();
    let conf = content::TransformConfig {
        return_format: content::ReturnFormat::Markdown,
        ..Default::default()
    };

    let mut interception = RequestInterceptConfiguration::new(true);
    let mut tracker = ChromeEventTracker::default();

    interception.block_javascript = false;
    interception.block_stylesheets = false;
    interception.block_visuals = false;
    interception.block_ads = false;
    interception.block_analytics = true;

    tracker.responses = true;
    tracker.requests = true;

    let viewport = chrome_viewport::randomize_viewport(&chrome_viewport::DeviceType::Desktop);

    let website = Website::new(url)
        .with_limit(1)
        .with_chrome_intercept(interception)
        .with_wait_for_delay(Some(WaitForDelay::new(Some(Duration::from_millis(200)))))
        .with_wait_for_idle_network(Some(WaitForIdleNetwork::new(Some(Duration::from_millis(2000)))))
        .with_wait_for_idle_dom(Some(WaitForSelector::new(
            Some(Duration::from_millis(5000)),
            "body".into(),
        )))
        .with_block_assets(true)
        .with_viewport(Some(viewport))
        .with_user_agent(Some("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/136.0.0.0 Safari/537.36"))
        .with_stealth(true)
        .with_return_page_links(true)
        .with_event_tracker(Some(tracker))
        .with_fingerprint_advanced(Fingerprint::None)
        .with_chrome_connection(chrome_connection_url.clone())
        .build()
        .context("Failed to build website crawler")?;

    let page = crawl_single_page(&website, url).await;

    match page {
        Some(page) => {
            let content = content::transform_content(&page, &conf, &None, &None, &None);
            info!("Crawled {} in {}ms", url, started_at.elapsed().as_millis());
            Ok(Some(CachedPage {
                source: url.to_string(),
                content,
            }))
        }
        None => {
            warn!(
                "No matching page for {} after {}ms",
                url,
                started_at.elapsed().as_millis()
            );
            Ok(None)
        }
    }
}

#[utoipa::path(
    post,
    path = "/",
    request_body = CrawlRequest,
    responses(
        (status = 200, description = "Crawl successful", body = Vec<CrawlResponse>)
    )
)]
async fn crawl_handler(
    State(state): State<AppState>,
    Json(payload): Json<CrawlRequest>,
) -> impl IntoResponse {
    let mut set = tokio::task::JoinSet::new();
    let chrome_connection_url = state.settings.chrome_connection_url.clone();
    let cache = state.cache.clone();

    for url in payload.urls {
        let chrome_connection_url = chrome_connection_url.clone();
        let cache = cache.clone();
        set.spawn(async move {
            if let Some(cached) = cache.get(&url).await {
                return Some(CrawlResponse {
                    page_content: cached.content,
                    metadata: Metadata {
                        source: cached.source,
                    },
                });
            }

            match crawl_page_uncached(&url, &chrome_connection_url).await {
                Ok(Some(cached)) => {
                    cache.insert(url.to_string(), cached.clone()).await;
                    Some(CrawlResponse {
                        page_content: cached.content,
                        metadata: Metadata {
                            source: cached.source,
                        },
                    })
                }
                Ok(None) => None,
                Err(e) => {
                    log::error!("Error crawling {}: {}", url, e);
                    None
                }
            }
        });
    }

    let mut results = Vec::new();
    while let Some(res) = set.join_next().await {
        if let Ok(Some(crawled)) = res {
            results.push(crawled);
        }
    }

    Json(results).into_response()
}

#[tokio::main]
async fn main() -> Result<()> {
    let env = Env::default()
        .filter_or("RUST_LOG", "info")
        .write_style_or("RUST_LOG_STYLE", "always");

    env_logger::init_from_env(env);

    let settings = Config::builder()
        .add_source(config::Environment::with_prefix("APP"))
        .set_default(
            "chrome_connection_url",
            "http://127.0.0.1:9222/json/version",
        )?
        .set_default("cache_ttl_seconds", 600_u64)?
        .set_default("cache_max_entries", 1000_u64)?
        .set_default("server_port", 8080_u16)?
        .build()
        .context("Failed to build configuration")?;

    let settings: Settings = settings
        .try_deserialize()
        .context("Failed to deserialize settings")?;

    if settings.cache_ttl_seconds == 0 {
        warn!("Cache TTL is set to 0; caching is effectively disabled.");
    }
    if settings.cache_max_entries == 0 {
        warn!("Cache max entries is set to 0; caching is effectively disabled.");
    }

    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .context("Failed to initialize HTTP client")?;

    let cache = Cache::builder()
        .time_to_live(Duration::from_secs(settings.cache_ttl_seconds))
        .max_capacity(settings.cache_max_entries)
        .build();

    let port = settings.server_port;

    let state = AppState {
        settings,
        http_client,
        cache,
    };

    let app = Router::new()
        .merge(SwaggerUi::new("/swagger-ui").url("/api-docs/openapi.json", ApiDoc::openapi()))
        .route("/", post(crawl_handler))
        .route("/health", get(health_check))
        .with_state(state);

    let addr = format!("0.0.0.0:{}", port);
    info!("Listening on http://{}", addr);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(err) = signal::ctrl_c().await {
            warn!("Failed to install Ctrl+C handler: {}", err);
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match signal::unix::signal(signal::unix::SignalKind::terminate()) {
            Ok(mut stream) => {
                stream.recv().await;
            }
            Err(err) => {
                warn!("Failed to install SIGTERM handler: {}", err);
            }
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    info!("Shutdown signal received, stopping server.");
}
