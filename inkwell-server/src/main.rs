use axum::{
    body::Bytes,
    extract::State,
    routing::{get, post},
    Json, Router,
};
use hex;
use image::io::Reader as ImageReader;
use img_hash::{HashAlg, HasherConfig};
use inkwell_core::{Card, ScanResult};
use sqlx::{sqlite::SqlitePoolOptions, Pool, Row, Sqlite};
use std::io::Cursor;
use std::sync::Arc;
use tokio::net::TcpListener;

#[derive(Clone)]
struct AppState {
    #[allow(dead_code)]
    pool: Pool<Sqlite>,
    // Store pre-parsed byte hashes alongside the full card data for speed
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

    // 2. Load and Index Cards
    println!("Indexing cards for hot-RAM lookup...");
    let rows = sqlx::query("SELECT id, name, subtitle, phash, image_url FROM cards")
        .fetch_all(&pool)
        .await?;

    let mut index = Vec::new();
    for row in rows {
        let phash_str: String = row.get("phash");
        let hash_bytes = hex::decode(&phash_str).unwrap_or_default();

        let card = Card {
            id: row.get("id"),
            name: row.get("name"),
            subtitle: row.get("subtitle"),
            phash: phash_str,
            image_url: row.get("image_url"),
        };
        index.push((hash_bytes, card));
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

    // 1. Decode Image
    let img_result = ImageReader::new(Cursor::new(&body))
        .with_guessed_format()
        .expect("Format guess failed") // TODO: Handle error better
        .decode();

    let raw_img = match img_result {
        Ok(img) => img,
        Err(e) => {
            println!("Failed to decode image: {}", e);
            return Json(ScanResult {
                card: None,
                confidence: 0.0,
            });
        }
    };

    // 2. Preprocess (Same as verify.rs)
    let preprocess = |img: &image::DynamicImage| -> image::DynamicImage {
        // Resize to a reasonable working size
        let resized = img.resize(500, 500, image::imageops::FilterType::Lanczos3);
        // Convert to grayscale (luma8)
        let mut gray = resized.to_luma8();
        // Contrast stretch
        image::imageops::contrast(&mut gray, 20.0);
        // Blur to reduce noise
        let blurred = image::imageops::blur(&gray, 1.0);
        image::DynamicImage::ImageLuma8(blurred)
    };

    let hasher = HasherConfig::new()
        .hash_alg(HashAlg::Gradient)
        .hash_size(12, 12)
        .to_hasher();

    // 3. Generate rotations
    let mut candidate_hashes = Vec::new();
    let base_processed = preprocess(&raw_img);
    candidate_hashes.push(hasher.hash_image(&base_processed));

    let rot90 = base_processed.rotate90();
    candidate_hashes.push(hasher.hash_image(&rot90));

    let rot180 = base_processed.rotate180();
    candidate_hashes.push(hasher.hash_image(&rot180));

    let rot270 = base_processed.rotate270();
    candidate_hashes.push(hasher.hash_image(&rot270));

    // 4. Find Best Match across all rotations
    let mut best_card: Option<Card> = None;
    let mut min_dist = u32::MAX;

    // Convert candidate hashes strings to bytes
    let candidate_hashes_bytes: Vec<Vec<u8>> = candidate_hashes
        .iter()
        .map(|h| {
            let hex_str = h
                .as_bytes()
                .iter()
                .map(|b| format!("{:02x}", b))
                .collect::<String>();
            hex::decode(&hex_str).unwrap()
        })
        .collect();

    for (card_hash_bytes, card) in state.index.iter() {
        for target_bytes in &candidate_hashes_bytes {
            let dist: u32 = target_bytes
                .iter()
                .zip(card_hash_bytes.iter())
                .map(|(a, b)| (a ^ b).count_ones())
                .sum();

            if dist < min_dist {
                min_dist = dist;
                best_card = Some(card.clone());
            }
        }
    }

    if let Some(card) = best_card {
        // Max distance for 12x12 hash (144 bits) is 144.
        let confidence = 1.0 - (min_dist as f64 / 144.0);

        // Only return a result if confidence is high (>= 85%)
        if confidence >= 0.85 {
            println!("Match found: {} ({:.2})", card.name, confidence);
            Json(ScanResult {
                card: Some(card),
                confidence,
            })
        } else {
            println!(
                "Match checking finished but low confidence ({}). Best was {}",
                confidence, card.name
            );
            Json(ScanResult {
                card: None,
                confidence: 0.0,
            })
        }
    } else {
        println!("No match found.");
        Json(ScanResult {
            card: None,
            confidence: 0.0,
        })
    }
}
