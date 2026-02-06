use hex;
use image::io::Reader as ImageReader;
use img_hash::{HashAlg, HasherConfig};
use inkwell_core::{Card, ScanResult};
use sqlx::{sqlite::SqlitePoolOptions, Row};
use std::env;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();

    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: verify <image_path>");
        return Ok(());
    }
    let image_path = &args[1];

    // 1. Connect to DB
    let database_url =
        std::env::var("DATABASE_URL").unwrap_or_else(|_| "sqlite:inkwell.db".to_string());
    let pool = SqlitePoolOptions::new().connect(&database_url).await?;

    // 2. Load Cards
    println!("Loading cards from DB...");
    let rows = sqlx::query("SELECT id, name, subtitle, phash, image_url FROM cards")
        .fetch_all(&pool)
        .await?;

    let mut cards = Vec::new();
    for row in rows {
        cards.push(Card {
            id: row.get("id"),
            name: row.get("name"),
            subtitle: row.get("subtitle"),
            phash: row.get("phash"),
            image_url: row.get("image_url"),
        });
    }
    println!("Loaded {} cards.", cards.len());

    // 3. Hash Input Image with Preprocessing and Rotation
    println!("Hashing {}...", image_path);
    let raw_img = ImageReader::open(image_path)?.decode()?;

    let hasher = HasherConfig::new()
        .hash_alg(HashAlg::Gradient)
        .hash_size(12, 12)
        .to_hasher();

    // Generate hashes for 0, 90, 180, 270 degrees
    let mut candidate_hashes = Vec::new();
    let base_processed = inkwell_core::preprocess_image(&raw_img);
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
    let mut best_rotation_index = 0;

    // Convert candidate hashes strings to bytes once
    let candidate_hashes_types: Vec<(String, Vec<u8>)> = candidate_hashes
        .iter()
        .map(|h| {
            let hex_str = h
                .as_bytes()
                .iter()
                .map(|b| format!("{:02x}", b))
                .collect::<String>();
            // We can just use the internal bytes directly from the hasher output if we want, but let's match existing logic
            // existing logic: hex string -> hex decode -> count ones
            let bytes = hex::decode(&hex_str).unwrap();
            (hex_str, bytes)
        })
        .collect();

    // Print the primary hash (0 deg) for debug
    println!("Target Hash (0 deg): {}", candidate_hashes_types[0].0);

    for card in cards {
        let card_hash_bytes = match hex::decode(&card.phash) {
            Ok(b) => b,
            Err(_) => continue, // skip invalid db hashes
        };

        for (rot_idx, (_, target_bytes)) in candidate_hashes_types.iter().enumerate() {
            let dist: u32 = target_bytes
                .iter()
                .zip(card_hash_bytes.iter())
                .map(|(a, b)| (a ^ b).count_ones())
                .sum();

            if dist < min_dist {
                min_dist = dist;
                best_card = Some(card.clone());
                best_rotation_index = rot_idx;
            }
        }
    }

    // 5. Report
    if let Some(card) = best_card {
        // Max distance for 12x12 hash (144 bits) is 144.
        // Existing logic used 64.0 which implies 8x8 hash.
        // We are using 12x12 hash, so max dist is 144.
        let confidence = 1.0 - (min_dist as f64 / 144.0);

        println!("Match Found:");
        println!("  Name: {} ({})", card.name, card.subtitle);
        println!("  ID: {}", card.id);
        println!("  Distance: {}", min_dist);
        println!("  Rotation: {} deg", best_rotation_index * 90);
        println!("  Confidence: {:.2}", confidence);

        // Output for automated tools to parse if needed
        let result = ScanResult {
            card: Some(card),
            confidence,
        };
        println!("JSON: {}", serde_json::to_string(&result)?);
    } else {
        println!("No match found.");
    }

    Ok(())
}
