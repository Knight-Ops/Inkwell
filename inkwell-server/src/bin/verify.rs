use image::io::Reader as ImageReader;
use inkwell_core::{akaze_bytes_to_mat, Card, ScanResult};
use opencv::{
    core::{DMatch, Mat, Vector, NORM_HAMMING},
    features2d::BFMatcher,
    prelude::*,
};
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
    // Select akaze_data
    let rows = sqlx::query("SELECT id, name, subtitle, phash, image_url, akaze_data, rarity, set_code, card_number FROM cards")
        .fetch_all(&pool)
        .await?;

    let mut cards = Vec::new();
    for row in rows {
        let akaze_data: Vec<u8> = row.get("akaze_data");
        // Only load if akaze_data exists
        if akaze_data.is_empty() {
            continue;
        }

        cards.push(Card {
            id: row.get("id"),
            name: row.get("name"),
            subtitle: row.get("subtitle"),
            phash: row.get("phash"),
            akaze_data,
            image_url: row.get("image_url"),
            rarity: row.get("rarity"),
            set_code: row.get("set_code"),
            card_number: row.get("card_number"),
        });
    }
    println!("Loaded {} cards with AKAZE data.", cards.len());

    // 3. Hash Input Image
    println!("Computing AKAZE for {}...", image_path);
    let raw_img = ImageReader::open(image_path)?.decode()?;

    let (_kp, query_desc_bytes) = inkwell_core::compute_akaze_features(&raw_img)?;
    if query_desc_bytes.is_empty() {
        println!("No features found in query image.");
        return Ok(());
    }

    let query_mat = akaze_bytes_to_mat(&query_desc_bytes)?;

    // 4. Find Best Match
    let mut matcher = BFMatcher::create(NORM_HAMMING, false)?;

    let mut best_card: Option<Card> = None;
    let mut max_good_matches = 0;
    const MIN_GOOD_MATCHES: usize = 20;
    let ratio_thresh = 0.75;

    for card in cards {
        let train_mat = match akaze_bytes_to_mat(&card.akaze_data) {
            Ok(m) => m,
            Err(_) => continue,
        };

        // Prepare matcher
        DescriptorMatcherTrait::clear(&mut matcher)?;

        let mut train_vec = Vector::<Mat>::new();
        train_vec.push(train_mat);
        matcher.add(&train_vec)?;

        let mut matches = Vector::<Vector<DMatch>>::new();
        matcher.knn_match(&query_mat, &mut matches, 2, &Mat::default(), false)?;

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

    // 5. Report
    if let Some(card) = best_card {
        if max_good_matches >= MIN_GOOD_MATCHES {
            let confidence = (max_good_matches as f64 / 100.0).min(1.0);
            println!("Match Found:");
            println!("  Name: {} ({})", card.name, card.subtitle);
            println!("  ID: {}", card.id);
            println!("  Good Matches: {}", max_good_matches);
            println!("  Confidence: {:.2}", confidence);

            let result = ScanResult {
                card: Some(card),
                confidence,
                global_total_scans: 0,
            };
            println!("JSON: {}", serde_json::to_string(&result)?);
        } else {
            println!(
                "Best match {} had only {} good matches. Below threshold.",
                card.name, max_good_matches
            );
        }
    } else {
        println!("No match found.");
    }

    Ok(())
}
