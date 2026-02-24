use futures::StreamExt;
use image::io::Reader as ImageReader;
use img_hash::{HashAlg, HasherConfig};
use reqwest::Client;
use sqlx::sqlite::SqlitePoolOptions;
use std::{fs, path::Path, sync::Arc};

const LORCANA_JSON_URL: &str = "https://lorcanajson.org/files/current/en/allCards.json";
const IMAGE_DIR: &str = "card_images";
const CONCURRENCY_LIMIT: usize = 10;

#[derive(serde::Deserialize, Debug)]
struct LorcanaCard {
    name: String,
    #[serde(alias = "version")]
    subtitle: Option<String>,
    #[serde(alias = "setCode")]
    set_code: String,
    number: u32,
    rarity: Option<String>,
    images: LorcanaImages,
}

#[derive(serde::Deserialize, Debug)]
struct LorcanaImages {
    #[serde(alias = "full")]
    full: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();

    // 1. Setup DB
    let database_url =
        std::env::var("DATABASE_URL").unwrap_or_else(|_| "sqlite:inkwell.db".to_string());

    // Ensure parent directories exist for sqlite
    if !database_url.contains("mode=memory") {
        if let Some(path) = database_url.strip_prefix("sqlite:") {
            let path = Path::new(path);
            if let Some(parent) = path.parent() {
                if !parent.as_os_str().is_empty() {
                    fs::create_dir_all(parent)?;
                }
            }
        }
    }

    use sqlx::sqlite::SqliteConnectOptions;
    use std::str::FromStr;

    let connection_options = SqliteConnectOptions::from_str(&database_url)?.create_if_missing(true);

    let pool = SqlitePoolOptions::new()
        .connect_with(connection_options)
        .await?;

    sqlx::migrate!("../migrations").run(&pool).await?;

    println!("Database initialized.");

    // 2. Setup Directories
    let image_dir = std::env::var("CARD_IMAGES_DIR").unwrap_or_else(|_| IMAGE_DIR.to_string());
    fs::create_dir_all(&image_dir)?;

    // 3. Fetch JSON
    println!("Fetching cards from {}", LORCANA_JSON_URL);
    let client = Client::new();
    let resp = client.get(LORCANA_JSON_URL).send().await?;
    let json_text = resp.text().await?;

    // The JSON is likely an object with "cards" array or just an array.
    // LorcanaJSON allCards.json is structured as an object with keys "cards" which is an array.
    #[derive(serde::Deserialize)]
    struct Wrapper {
        cards: Vec<LorcanaCard>,
    }

    let wrapper: Wrapper = serde_json::from_str(&json_text)?;
    println!("Found {} cards.", wrapper.cards.len());

    let hasher_config = Arc::new(
        HasherConfig::new()
            .hash_alg(HashAlg::Gradient)
            .hash_size(12, 12),
    );

    let client = Arc::new(client);

    futures::stream::iter(wrapper.cards)
        .for_each_concurrent(CONCURRENCY_LIMIT, |card_data| {
            let pool = pool.clone();
            let client = client.clone();
            let hasher_config = hasher_config.clone();
            let image_dir = image_dir.clone();
            async move {
                // ID construction: set_code-number
                let id = format!("{}-{}", card_data.set_code, card_data.number);
                let local_path = Path::new(&image_dir).join(format!("{}.jpg", id));
                let db_image_url = format!("{}/{}.jpg", IMAGE_DIR, id);

                let process_result = async {
                    // 4. Download Image
                        if !local_path.exists() {
                            println!("Downloading image for {}...", id);
                            let img_bytes = client.get(&card_data.images.full).send().await?.bytes().await?;
                            fs::write(&local_path, img_bytes)?;
                        }

                        // 5. Compute Hash & Features
                        // Image decoding and hashing is CPU bound.
                        let img = ImageReader::open(&local_path)?.decode()?;
                    // Legacy pHash
                    let processed = inkwell_core::preprocess_image(&img);
                    let hasher = hasher_config.to_hasher();
                    let hash = hasher.hash_image(&processed);
                    let phash_str = hash
                        .as_bytes()
                        .iter()
                        .map(|b| format!("{:02x}", b))
                        .collect::<String>();

                    // AKAZE Featues
                    // We discard keypoints for storage, we only need descriptors for matching
                    let (_, akaze_bytes) = inkwell_core::compute_akaze_features(&img)?;

                    // 6. Insert into DB
                    let subtitle = card_data.subtitle.clone().unwrap_or_default();
                    let rarity = card_data.rarity.clone().unwrap_or_else(|| "Unknown".to_string());
                    sqlx::query(
                        r#"
                        INSERT INTO cards (id, name, subtitle, set_code, image_url, phash, meta_json, akaze_data, rarity, card_number)
                        VALUES (?, ?, ?, ?, ?, ?, '{}', ?, ?, ?)
                        ON CONFLICT(id) DO UPDATE SET
                            phash = excluded.phash,
                            image_url = excluded.image_url,
                            akaze_data = excluded.akaze_data,
                            rarity = excluded.rarity,
                            set_code = excluded.set_code,
                            card_number = excluded.card_number
                        "#,
                    )
                    .bind(&id)
                    .bind(&card_data.name)
                    .bind(&subtitle)
                    .bind(&card_data.set_code)
                    .bind(&db_image_url)
                    .bind(&phash_str)
                    .bind(&akaze_bytes)
                    .bind(&rarity)
                    .bind(card_data.number)
                    .execute(&pool)
                    .await?;

                    println!("Processed {}: {} [{}]", id, card_data.name, phash_str);
                    Ok::<(), Box<dyn std::error::Error>>(())
                }
                .await;

                if let Err(e) = process_result {
                    eprintln!("Error processing card {}-{}: {}", card_data.set_code, card_data.number, e);
                }
            }
        })
        .await;

    println!("Ingestion complete.");
    Ok(())
}
