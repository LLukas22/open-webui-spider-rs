extern crate env_logger;

use spider::configuration::ChromeEventTracker;
use spider::features::chrome_common::RequestInterceptConfiguration;
use spider::website::Website;
use spider_transformations::transformation::content;

use env_logger::Env;
use log::info;

use spider::configuration::Fingerprint;
use spider::features::chrome_common::{WaitForDelay, WaitForIdleNetwork, WaitForSelector};
use spider::features::chrome_viewport;
use spider::tokio;
use std::time::Duration;

use axum::{
    Router,
    extract::{Json, State},
    response::IntoResponse,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use utoipa::{OpenApi, ToSchema};
use utoipa_swagger_ui::SwaggerUi;
use config::Config;

#[derive(Clone, Deserialize)]
struct Settings {
    chrome_connection_url: Option<String>,
}

#[derive(Clone)]
struct AppState {
    settings: Settings,
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
        (status = 200, description = "Health check passed", body = String)
    )
)]
async fn health_check() -> impl IntoResponse {
    "OK"
}

async fn scrape_website(website: &Website) -> Vec<spider::page::Page> {
    let mut pages = Vec::new();
    let mut w = website.clone();
    let mut rx2 = w.subscribe(0).expect("receiver enabled");

    tokio::task::spawn(async move {
        w.crawl_smart().await;
        w.unsubscribe();
    });

    while let Ok(page) = rx2.recv().await {
        pages.push(page);
    }

    pages
}

async fn crawl_page(url: &str, chrome_connection_url: &Option<String>) -> anyhow::Result<(String, String)> {
    let conf = content::TransformConfig { return_format: content::ReturnFormat::Markdown, ..Default::default() };


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
        .unwrap();

    info!("Starting scrape...");
    let pages = scrape_website(&website).await;
    info!("Scrape complete!");

    for page in pages.iter() {
        if !page.is_empty() {
            info!("Page URL: {}", page.get_url());
            if url == page.get_url() {
                info!("Matched target URL, processing content...");
                let content = content::transform_content(page, &conf, &None, &None, &None);

                return Ok((url.to_string(), content));
            }
        }
    }

    Ok(("".to_string(), "".to_string()))
}

#[utoipa::path(
    post,
    path = "/",
    request_body = CrawlRequest,
    responses(
        (status = 200, description = "Crawl successful", body = Vec<CrawlResponse>)
    )
)]
async fn crawl_handler(State(state): State<AppState>, Json(payload): Json<CrawlRequest>) -> impl IntoResponse {
    let mut set = tokio::task::JoinSet::new();
    let chrome_connection_url = state.settings.chrome_connection_url.clone();

    for url in payload.urls {
        let chrome_connection_url = chrome_connection_url.clone();
        set.spawn(async move {
            match crawl_page(&url, &chrome_connection_url).await {
                Ok((source, content)) => {
                    if source.is_empty() {
                        None
                    } else {
                        Some(CrawlResponse {
                            page_content: content,
                            metadata: Metadata { source },
                        })
                    }
                }
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
async fn main() -> anyhow::Result<()> {
    let env = Env::default()
        .filter_or("RUST_LOG", "info")
        .write_style_or("RUST_LOG_STYLE", "always");

    env_logger::init_from_env(env);

    let settings = Config::builder()
        .add_source(config::Environment::with_prefix("APP"))
        .set_default("chrome_connection_url", "http://127.0.0.1:9222/json/version")?
        .build()
        .unwrap();

    let settings: Settings = settings.try_deserialize().unwrap();
    let state = AppState { settings };

    let app = Router::new()
        .merge(SwaggerUi::new("/swagger-ui").url("/api-docs/openapi.json", ApiDoc::openapi()))
        .route("/", post(crawl_handler))
        .route("/health", get(health_check))
        .with_state(state);

    let addr = "0.0.0.0:3000";
    info!("Listening on {}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
