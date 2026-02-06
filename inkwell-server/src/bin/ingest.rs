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

    // Ensure DB file exists for sqlite
    if !database_url.contains("mode=memory") {
        if let Some(path) = database_url.strip_prefix("sqlite:") {
            if !Path::new(path).exists() {
                println!("Database file not found, creating {}", path);
                fs::File::create(path)?;
            }
        }
    }

    let pool = SqlitePoolOptions::new().connect(&database_url).await?;

    sqlx::migrate!("../migrations").run(&pool).await?;

    println!("Database initialized.");

    // 2. Setup Directories
    fs::create_dir_all(IMAGE_DIR)?;

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

    let hasher = Arc::new(
        HasherConfig::new()
            .hash_alg(HashAlg::Gradient)
            .hash_size(12, 12)
            .to_hasher(),
    );

    let client = Arc::new(client);

    futures::stream::iter(wrapper.cards)
        .for_each_concurrent(CONCURRENCY_LIMIT, |card_data| {
            let pool = pool.clone();
            let client = client.clone();
            let hasher = hasher.clone();
            async move {
                // ID construction: set_code-number
                let id = format!("{}-{}", card_data.set_code, card_data.number);
                let image_filename = format!("{}/{}.jpg", IMAGE_DIR, id);
                let path_buf = std::path::PathBuf::from(&image_filename);

                let process_result = async {
                    // 4. Download Image
                    if !path_buf.exists() {
                        println!("Downloading image for {}...", id);
                        let img_bytes = client.get(&card_data.images.full).send().await?.bytes().await?;
                        fs::write(&path_buf, img_bytes)?;
                    }

                    // 5. Compute Hash
                    // Image decoding and hashing is CPU bound.
                    // For massive scale, we'd use spawn_blocking, but for 10 concurrent tasks it's okay.
                    let img = ImageReader::open(&path_buf)?.decode()?;
                    let hash = hasher.hash_image(&img);
                    let phash_str = hash
                        .as_bytes()
                        .iter()
                        .map(|b| format!("{:02x}", b))
                        .collect::<String>();

                    // 6. Insert into DB
                    let subtitle = card_data.subtitle.clone().unwrap_or_default();
                    sqlx::query!(
                        r#"
                        INSERT INTO cards (id, name, subtitle, set_code, image_url, phash, meta_json)
                        VALUES (?, ?, ?, ?, ?, ?, '{}')
                        ON CONFLICT(id) DO UPDATE SET
                            phash = excluded.phash,
                            image_url = excluded.image_url
                        "#,
                        id,
                        card_data.name,
                        subtitle,
                        card_data.set_code,
                        image_filename,
                        phash_str
                    )
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
