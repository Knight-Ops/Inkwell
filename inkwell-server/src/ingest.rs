use futures::StreamExt;
use image::io::Reader as ImageReader;
use img_hash::{HashAlg, HasherConfig};
use reqwest::Client;
use sqlx::{Pool, Sqlite};
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

pub async fn run_ingestion(
    pool: Pool<Sqlite>,
    image_dir: String,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    println!("Starting ingestion job...");

    fs::create_dir_all(&image_dir)?;

    println!("Fetching cards from {}", LORCANA_JSON_URL);
    let client = Client::new();
    let resp = client.get(LORCANA_JSON_URL).send().await?;
    let json_text = resp.text().await?;

    #[derive(serde::Deserialize)]
    struct Wrapper {
        cards: Vec<LorcanaCard>,
    }

    let wrapper: Wrapper = serde_json::from_str(&json_text)?;
    println!("Found {} cards in JSON.", wrapper.cards.len());

    let client = Arc::new(client);

    futures::stream::iter(wrapper.cards)
        .for_each_concurrent(CONCURRENCY_LIMIT, |card_data| {
            let pool = pool.clone();
            let client = client.clone();
            let image_dir = image_dir.clone();
            async move {
                let id = format!("{}-{}", card_data.set_code, card_data.number);
                let local_path = Path::new(&image_dir).join(format!("{}.jpg", id));
                let db_image_url = format!("{}/{}.jpg", IMAGE_DIR, id);

                let process_result = async {
                    // Check if card exists and has complete data
                    let existing_card: Option<sqlx::sqlite::SqliteRow> = sqlx::query(
                        "SELECT id FROM cards WHERE id = ? AND akaze_data IS NOT NULL AND phash IS NOT NULL AND phash != ''"
                    )
                    .bind(&id)
                    .fetch_optional(&pool)
                    .await?;

                    let needs_image_processing = !local_path.exists() || existing_card.is_none();

                    let subtitle = card_data.subtitle.clone().unwrap_or_default();
                    let rarity = card_data.rarity.clone().unwrap_or_else(|| "Unknown".to_string());

                    if needs_image_processing {
                        if !local_path.exists() {
                            println!("Downloading image for {}...", id);
                            let img_bytes = client.get(&card_data.images.full).send().await?.bytes().await?;
                            fs::write(&local_path, img_bytes)?;
                        }

                        let img = ImageReader::open(&local_path)?.decode()?;
                        let processed = inkwell_core::preprocess_image(&img);

                        let phash_str = {
                            let hasher = HasherConfig::new().hash_alg(HashAlg::Gradient).hash_size(12, 12).to_hasher();
                            let hash = hasher.hash_image(&processed);
                            hash.as_bytes()
                                .iter()
                                .map(|b| format!("{:02x}", b))
                                .collect::<String>()
                        };

                        let (_, akaze_bytes) = inkwell_core::compute_akaze_features(&img)?;

                        sqlx::query(
                            r#"
                            INSERT INTO cards (id, name, subtitle, set_code, image_url, phash, meta_json, akaze_data, rarity, card_number)
                            VALUES (?, ?, ?, ?, ?, ?, '{}', ?, ?, ?)
                            ON CONFLICT(id) DO UPDATE SET
                                name = excluded.name,
                                subtitle = excluded.subtitle,
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
                    } else {
                        // Metadata only update
                        sqlx::query(
                            r#"
                            UPDATE cards SET
                                name = ?,
                                subtitle = ?,
                                rarity = ?,
                                set_code = ?,
                                card_number = ?
                            WHERE id = ?
                            "#,
                        )
                        .bind(&card_data.name)
                        .bind(&subtitle)
                        .bind(&rarity)
                        .bind(&card_data.set_code)
                        .bind(card_data.number)
                        .bind(&id)
                        .execute(&pool)
                        .await?;
                    }

                    // Ok to map to Box<dyn Error + Send + Sync> here
                    Result::<(), Box<dyn std::error::Error + Send + Sync>>::Ok(())
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
