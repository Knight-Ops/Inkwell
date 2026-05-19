use axum::{
    body::Bytes,
    extract::State,
    routing::{get, post},
    Json, Router,
};
use inkwell_core::{akaze_bytes_to_mat, match_card, Card, GlobalIndex, ScanResult};
use opencv::core::{Mat, Vector};
use sqlx::{sqlite::SqlitePoolOptions, Pool, Row, Sqlite};
use std::str::FromStr;
use std::sync::Arc;
use tokio::net::TcpListener;

mod ingest;

#[derive(Clone)]
struct AppState {
    #[allow(dead_code)]
    pool: Pool<Sqlite>,
    index: Arc<tokio::sync::RwLock<Arc<GlobalIndex>>>,
}

async fn load_index(pool: &Pool<Sqlite>) -> Result<GlobalIndex, sqlx::Error> {
    println!("Indexing cards for hot-RAM lookup...");
    let rows = sqlx::query("SELECT id, name, subtitle, phash, image_url, akaze_data, rarity, promo_grouping, set_code, card_number FROM cards")
        .fetch_all(pool)
        .await?;

    let mut train_vec = Vector::<Mat>::new();
    let mut cards = Vec::new();
    for row in rows {
        let akaze_data: Vec<u8> = row.get("akaze_data");
        let phash_str: String = row.get("phash");

        let card = Card {
            id: row.get("id"),
            name: row.get("name"),
            subtitle: row.get("subtitle"),
            phash: phash_str,
            akaze_data: akaze_data.clone(),
            image_url: row.get("image_url"),
            rarity: row.get("rarity"),
            promo_grouping: row.get("promo_grouping"),
            set_code: row.get("set_code"),
            card_number: row.get("card_number"),
        };

        if let Ok(m) = inkwell_core::akaze_bytes_to_mat(&akaze_data) {
            train_vec.push(m);
            cards.push(card);
        }
    }
    println!("Indexed {} cards.", cards.len());
    Ok(GlobalIndex { train_vec, cards })
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "inkwell_server=info".into()),
        )
        .init();

    let database_url =
        std::env::var("DATABASE_URL").unwrap_or_else(|_| "sqlite:inkwell.db".to_string());

    // Setup DB
    // Ensure parent directories exist for sqlite
    if !database_url.contains("mode=memory") {
        if let Some(path) = database_url.strip_prefix("sqlite:") {
            let path = std::path::Path::new(path);
            if let Some(parent) = path.parent() {
                if !parent.as_os_str().is_empty() {
                    std::fs::create_dir_all(parent)?;
                }
            }
        }
    }

    let connection_options =
        sqlx::sqlite::SqliteConnectOptions::from_str(&database_url)?.create_if_missing(true);

    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(connection_options)
        .await?;

    // Run migrations
    sqlx::migrate!("../migrations").run(&pool).await?;

    // Load and Index Cards
    let index = load_index(&pool).await?;

    let state = AppState {
        pool: pool.clone(),
        index: Arc::new(tokio::sync::RwLock::new(Arc::new(index))),
    };

    // Spawn ingestion background task
    let bg_pool = pool.clone();
    let bg_index = state.index.clone();
    tokio::spawn(async move {
        loop {
            let image_dir =
                std::env::var("CARD_IMAGES_DIR").unwrap_or_else(|_| "card_images".to_string());
            if let Err(e) = ingest::run_ingestion(bg_pool.clone(), image_dir).await {
                eprintln!("Ingestion job failed: {}", e);
            } else {
                match load_index(&bg_pool).await {
                    Ok(new_index) => {
                        let mut wl = bg_index.write().await;
                        *wl = Arc::new(new_index);
                        println!("Reloaded index in background.");
                    }
                    Err(e) => eprintln!("Failed to reload index: {}", e),
                }
            }
            tokio::time::sleep(tokio::time::Duration::from_secs(24 * 60 * 60)).await;
        }
    });

    // Setup Routes
    let app = Router::new()
        .route("/health", get(|| async { "OK" }))
        .route("/api/identify", post(identify_card))
        .route("/api/stats", get(get_stats))
        .nest_service(
            "/card_images",
            tower_http::services::ServeDir::new("card_images"),
        )
        .fallback_service(tower_http::services::ServeDir::new("dist"))
        .with_state(state);

    // Start Server
    let addr = "0.0.0.0:4000";
    println!("Listening on http://{}", addr);
    let listener = TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

async fn identify_card(State(state): State<AppState>, body: Bytes) -> Json<ScanResult> {
    tracing::info!("Received identification request ({} bytes)", body.len());

    let global_index = {
        let rl = state.index.read().await;
        rl.clone()
    };

    let scan_result = tokio::task::spawn_blocking(move || {
        let start_total = std::time::Instant::now();

        // Save image for debugging if configured (Synchronous I/O)
        if let Ok(dir) = std::env::var("CAPTURED_IMAGES_DIR") {
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis();
            let _ = std::fs::create_dir_all(&dir);
            let filename = format!("{}/img_{}.jpg", dir, timestamp);
            let _ = std::fs::write(&filename, &body);
        }

        // Compute AKAZE natively from raw image bytes (bypasses pure-Rust codecs entirely)
        let (_kp, query_desc_bytes) = match inkwell_core::compute_akaze_features_from_bytes(&body) {
            Ok(res) => res,
            Err(e) => {
                tracing::error!("AKAZE computation failed: {}", e);
                return ScanResult {
                    card: None,
                    confidence: 0.0,
                    global_total_scans: 0,
                };
            }
        };

        let akaze_elapsed = start_total.elapsed();

        if query_desc_bytes.is_empty() {
            tracing::warn!("No features found in query image.");
            return ScanResult {
                card: None,
                confidence: 0.0,
                global_total_scans: 0,
            };
        }

        let query_mat = match akaze_bytes_to_mat(&query_desc_bytes) {
            Ok(m) => m,
            Err(e) => {
                tracing::error!("Failed to create query Mat: {}", e);
                return ScanResult {
                    card: None,
                    confidence: 0.0,
                    global_total_scans: 0,
                };
            }
        };

        // Match against index
        const MIN_GOOD_MATCHES: usize = 50;
        let ratio_thresh = 0.75;

        let match_start = std::time::Instant::now();
        let match_res = match_card(&query_mat, &global_index, ratio_thresh, MIN_GOOD_MATCHES);
        let match_elapsed = match_start.elapsed();
        let total_elapsed = start_total.elapsed();

        match match_res {
            Ok(res) => {
                if let Some(ref card) = res.card {
                    tracing::info!(
                        "Match found: {} in {:?}. details: akaze={:?}, match={:?}",
                        card.name,
                        total_elapsed,
                        akaze_elapsed,
                        match_elapsed
                    );
                } else {
                    tracing::info!(
                        "No match found in {:?}. details: akaze={:?}, match={:?}",
                        total_elapsed,
                        akaze_elapsed,
                        match_elapsed
                    );
                }
                res
            }
            Err(e) => {
                tracing::error!("Card matching failed: {}", e);
                ScanResult {
                    card: None,
                    confidence: 0.0,
                    global_total_scans: 0,
                }
            }
        }
    })
    .await
    .unwrap_or_else(|e| {
        eprintln!("Blocking task panicked: {}", e);
        ScanResult {
            card: None,
            confidence: 0.0,
            global_total_scans: 0,
        }
    });

    let mut final_result = scan_result;

    // Update and fetch global stats if a match was found
    if final_result.card.is_some() {
        let _ = sqlx::query(
            "UPDATE system_stats SET value = value + 1 WHERE key = 'total_scanned_cards'",
        )
        .execute(&state.pool)
        .await;
    }

    // Always fetch latest count
    if let Ok(row) = sqlx::query("SELECT value FROM system_stats WHERE key = 'total_scanned_cards'")
        .fetch_one(&state.pool)
        .await
    {
        final_result.global_total_scans = row.get::<i64, _>("value") as u64;
    }

    Json(final_result)
}

async fn get_stats(State(state): State<AppState>) -> Json<serde_json::Value> {
    let mut total = 0;
    if let Ok(row) = sqlx::query("SELECT value FROM system_stats WHERE key = 'total_scanned_cards'")
        .fetch_one(&state.pool)
        .await
    {
        total = row.get::<i64, _>("value") as u64;
    }

    Json(serde_json::json!({
        "total_scanned_cards": total
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    #[tokio::test]
    async fn test_in_memory_db_and_index() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();

        sqlx::migrate!("../migrations").run(&pool).await.unwrap();

        sqlx::query(
            r#"
            INSERT INTO cards (id, name, subtitle, set_code, image_url, phash, rarity, card_number, akaze_data)
            VALUES ('mock_id', 'Mock Name', 'Mock Subtitle', '1', 'url', 'phash', 'Common', 1, ?)
            "#,
        )
        .bind(vec![0u8; 10 * 61])
        .execute(&pool)
        .await
        .unwrap();

        let index = load_index(&pool).await.unwrap();
        assert_eq!(index.cards.len(), 1);
        assert_eq!(index.cards[0].id, "mock_id");
        assert_eq!(index.cards[0].name, "Mock Name");
        assert_eq!(index.cards[0].subtitle, "Mock Subtitle");
        assert_eq!(index.cards[0].rarity, "Common");
        assert_eq!(index.cards[0].card_number, 1);
        assert_eq!(index.train_vec.len(), 1);
    }

    #[tokio::test]
    async fn test_get_stats_handler() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("../migrations").run(&pool).await.unwrap();

        let index = load_index(&pool).await.unwrap();
        let state = AppState {
            pool: pool.clone(),
            index: Arc::new(tokio::sync::RwLock::new(Arc::new(index))),
        };

        let response = get_stats(State(state)).await;
        let json = response.0;
        assert_eq!(json["total_scanned_cards"], 0);
    }

    #[tokio::test]
    async fn test_identify_card_handler_no_match() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("../migrations").run(&pool).await.unwrap();

        let index = load_index(&pool).await.unwrap();
        let state = AppState {
            pool: pool.clone(),
            index: Arc::new(tokio::sync::RwLock::new(Arc::new(index))),
        };

        let mut img_bytes = Vec::new();
        let img = image::DynamicImage::ImageLuma8(image::GrayImage::new(10, 10));
        let mut cursor = std::io::Cursor::new(&mut img_bytes);
        img.write_to(&mut cursor, image::ImageFormat::Png).unwrap();

        let response = identify_card(State(state), axum::body::Bytes::from(img_bytes)).await;
        let result = response.0;
        assert!(result.card.is_none());
        assert_eq!(result.confidence, 0.0);
    }
}
