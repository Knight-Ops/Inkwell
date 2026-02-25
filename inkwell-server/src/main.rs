use axum::{
    body::Bytes,
    extract::State,
    routing::{get, post},
    Json, Router,
};
use image::io::Reader as ImageReader;
use inkwell_core::{akaze_bytes_to_mat, Card, ScanResult};
use opencv::{
    core::{DMatch, Mat, Vector, NORM_HAMMING},
    features2d::BFMatcher,
    prelude::*,
};
use sqlx::{sqlite::SqlitePoolOptions, Pool, Row, Sqlite};
use std::io::Cursor;
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

struct GlobalIndex {
    train_vec: Vector<Mat>,
    cards: Vec<Card>,
}

async fn load_index(pool: &Pool<Sqlite>) -> Result<GlobalIndex, sqlx::Error> {
    println!("Indexing cards for hot-RAM lookup...");
    let rows = sqlx::query("SELECT id, name, subtitle, phash, image_url, akaze_data, rarity, set_code, card_number FROM cards")
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
    println!("Received identification request ({} bytes)", body.len());

    let global_index = {
        let rl = state.index.read().await;
        rl.clone()
    };

    let scan_result = tokio::task::spawn_blocking(move || {
        // Save image for debugging if configured (Synchronous I/O)
        if let Ok(dir) = std::env::var("CAPTURED_IMAGES_DIR") {
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis();
            let _ = std::fs::create_dir_all(&dir);
            let filename = format!("{}/img_{}.jpg", dir, timestamp);
            if let Err(e) = std::fs::write(&filename, &body) {
                println!("Failed to save image: {}", e);
            } else {
                println!("Saved image to {}", filename);
            }
        }

        // Decode Image
        let img_result = ImageReader::new(Cursor::new(&body))
            .with_guessed_format()
            .expect("Format guess failed") // TODO: Handle error better
            .decode();

        let raw_img = match img_result {
            Ok(img) => img,
            Err(e) => {
                println!("Failed to decode image: {}", e);
                return ScanResult {
                    card: None,
                    confidence: 0.0,
                    global_total_scans: 0,
                };
            }
        };

        // Compute AKAZE
        let (_kp, query_desc_bytes) = match inkwell_core::compute_akaze_features(&raw_img) {
            Ok(res) => res,
            Err(e) => {
                println!("AKAZE computation failed: {}", e);
                return ScanResult {
                    card: None,
                    confidence: 0.0,
                    global_total_scans: 0,
                };
            }
        };

        if query_desc_bytes.is_empty() {
            println!("No features found in query image.");
            return ScanResult {
                card: None,
                confidence: 0.0,
                global_total_scans: 0,
            };
        }

        let query_mat = match akaze_bytes_to_mat(&query_desc_bytes) {
            Ok(m) => m,
            Err(e) => {
                println!("Failed to create query Mat: {}", e);
                return ScanResult {
                    card: None,
                    confidence: 0.0,
                    global_total_scans: 0,
                };
            }
        };

        // Match against index
        // Use BFMatcher with NORM_HAMMING
        let mut matcher = match BFMatcher::create(NORM_HAMMING, false) {
            Ok(m) => m,
            Err(e) => {
                println!("Failed to create BFMatcher: {}", e);
                return ScanResult {
                    card: None,
                    confidence: 0.0,
                    global_total_scans: 0,
                };
            }
        };

        if let Err(e) = matcher.add(&global_index.train_vec) {
            println!("Matcher add failed: {}", e);
            return ScanResult {
                card: None,
                confidence: 0.0,
                global_total_scans: 0,
            };
        }

        if let Err(e) = matcher.train() {
            println!("Matcher train failed: {}", e);
            return ScanResult {
                card: None,
                confidence: 0.0,
                global_total_scans: 0,
            };
        }

        let mut best_card: Option<Card> = None;
        let mut max_good_matches = 0;

        // Low Match Count threshold (depends on feature count).
        // AKAZE typically extracts 100-1000 features.
        // Let's set a minimum threshold.
        const MIN_GOOD_MATCHES: usize = 50;
        let ratio_thresh = 0.75;

        let mut matches = Vector::<Vector<DMatch>>::new();
        if matcher
            .knn_match(&query_mat, &mut matches, 2, &Mat::default(), false)
            .is_err()
        {
            println!("knn_match failed");
            return ScanResult {
                card: None,
                confidence: 0.0,
                global_total_scans: 0,
            };
        }

        let mut votes = std::collections::HashMap::new();

        for m in matches {
            if m.len() == 2
                && m.get(0).unwrap().distance < ratio_thresh * m.get(1).unwrap().distance
            {
                let best_match = m.get(0).unwrap();
                let img_idx = best_match.img_idx as usize;

                *votes.entry(img_idx).or_insert(0) += 1;
            }
        }

        for (card_idx, vote_count) in votes {
            if vote_count > max_good_matches {
                max_good_matches = vote_count;
                best_card = Some(global_index.cards[card_idx].clone());
            }
        }

        if let Some(card) = best_card {
            if max_good_matches >= MIN_GOOD_MATCHES {
                // Primitive confidence: cap at 100 matches?
                let confidence = (max_good_matches as f64 / 100.0).min(1.0);
                println!(
                    "Match found: {} ({} good matches)",
                    card.name, max_good_matches
                );
                ScanResult {
                    card: Some(card),
                    confidence,
                    global_total_scans: 0,
                }
            } else {
                println!(
                    "Best match {} had only {} good matches. Below threshold.",
                    card.name, max_good_matches
                );
                ScanResult {
                    card: None,
                    confidence: 0.0,
                    global_total_scans: 0,
                }
            }
        } else {
            println!("No match found.");
            ScanResult {
                card: None,
                confidence: 0.0,
                global_total_scans: 0,
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
