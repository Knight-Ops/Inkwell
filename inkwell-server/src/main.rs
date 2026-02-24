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
use std::sync::Arc;
use tokio::net::TcpListener;

#[derive(Clone)]
struct AppState {
    #[allow(dead_code)]
    pool: Pool<Sqlite>,
    // Store akaze_data bytes alongside the full card data for speed
    index: Arc<Vec<(Vec<u8>, Card)>>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();
    let database_url =
        std::env::var("DATABASE_URL").unwrap_or_else(|_| "sqlite:inkwell.db".to_string());

    // 1. Setup DB
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect(&database_url)
        .await?;

    // Run migrations
    sqlx::migrate!("../migrations").run(&pool).await?;

    // 2. Load and Index Cards
    println!("Indexing cards for hot-RAM lookup...");
    let rows = sqlx::query("SELECT id, name, subtitle, phash, image_url, akaze_data, rarity, set_code, card_number FROM cards")
        .fetch_all(&pool)
        .await?;

    let mut index = Vec::new();
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

        if !akaze_data.is_empty() {
            index.push((akaze_data, card));
        }
    }
    println!("Indexed {} cards.", index.len());

    let state = AppState {
        pool,
        index: Arc::new(index),
    };

    // 3. Setup Routes
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

    // 4. Start Server
    let addr = "0.0.0.0:4000";
    println!("Listening on http://{}", addr);
    let listener = TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

async fn identify_card(State(state): State<AppState>, body: Bytes) -> Json<ScanResult> {
    println!("Received identification request ({} bytes)", body.len());

    let scan_result = tokio::task::spawn_blocking(move || {
        // 0. Save image for debugging if configured (Synchronous I/O)
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

        // 1. Decode Image
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

        // 2. Compute AKAZE
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

        // 3. Match against index
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

        let mut best_card: Option<Card> = None;
        let mut max_good_matches = 0;

        // Low Match Count threshold (depends on feature count).
        // AKAZE typically extracts 100-1000 features.
        // Let's set a minimum threshold.
        const MIN_GOOD_MATCHES: usize = 50;
        let ratio_thresh = 0.75;

        for (train_bytes, card) in state.index.iter() {
            let train_mat = match akaze_bytes_to_mat(train_bytes) {
                Ok(m) => m,
                Err(_) => continue,
            };

            // Matcher add/clear
            if let Err(e) = DescriptorMatcherTrait::clear(&mut matcher) {
                println!("Matcher clear failed: {}", e);
                continue;
            }
            let mut train_vec = Vector::<Mat>::new();
            train_vec.push(train_mat);
            if matcher.add(&train_vec).is_err() {
                continue;
            }

            let mut matches = Vector::<Vector<DMatch>>::new();
            if matcher
                .knn_match(&query_mat, &mut matches, 2, &Mat::default(), false)
                .is_err()
            {
                continue;
            }

            let mut good_matches = 0;
            for m in matches {
                if m.len() == 2
                    && m.get(0).unwrap().distance < ratio_thresh * m.get(1).unwrap().distance
                {
                    good_matches += 1;
                }
            }

            if good_matches > max_good_matches {
                max_good_matches = good_matches;
                best_card = Some(card.clone());
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

    // 4. Update and fetch global stats if a match was found
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
