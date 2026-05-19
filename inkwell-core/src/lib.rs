#[cfg(not(target_arch = "wasm32"))]
use image::DynamicImage;
#[cfg(not(target_arch = "wasm32"))]
use opencv::{
    core::{KeyPoint, Mat, Vector},
    features2d::AKAZE,
    prelude::*,
};
use serde::{Deserialize, Serialize};

/// Computes AKAZE features for an image.
/// Returns a tuple of (KeyPoints, Descriptors serialized as Vec<u8>).
#[cfg(not(target_arch = "wasm32"))]
pub fn compute_akaze_features(
    img: &DynamicImage,
) -> Result<(Vec<KeyPoint>, Vec<u8>), opencv::Error> {
    // Resize to a reasonable working size (optional, but good for performance)
    let resized = img.resize(500, 500, image::imageops::FilterType::Lanczos3);
    let gray = resized.to_luma8();

    // Convert raw pixels to OpenCV Mat
    let (_width, height) = gray.dimensions();

    // Create Mat from slice (copies data)
    let mat_1d = Mat::from_slice(gray.as_raw())?;

    // Reshape to correct dimensions: channels=1, rows=height.
    let mat = mat_1d.reshape(1, height as i32)?;

    // Init AKAZE
    // Use DESCRIPTOR_MLDB for binary descriptors (Hamming distance) and rotation invariance.
    let mut akaze = AKAZE::create_def()?;

    // Detect and Compute
    let mut keypoints = Vector::<KeyPoint>::new();
    let mut descriptors = Mat::default();
    let mask = Mat::default();

    akaze.detect_and_compute(&mat, &mask, &mut keypoints, &mut descriptors, false)?;

    // Convert descriptors Mat to Vec<u8> for storage
    let data_len = descriptors.total() * descriptors.elem_size()?;
    let mut descriptors_bytes = vec![0u8; data_len];
    let data_ptr = descriptors.data_bytes()?;
    descriptors_bytes.copy_from_slice(data_ptr);

    // Convert Vector<KeyPoint> to Vec<KeyPoint>
    let keypoints_vec: Vec<KeyPoint> = keypoints.to_vec();

    Ok((keypoints_vec, descriptors_bytes))
}

/// Computes AKAZE features for raw image bytes (natively decoded and resized).
/// Returns a tuple of (KeyPoints, Descriptors serialized as Vec<u8>).
#[cfg(not(target_arch = "wasm32"))]
pub fn compute_akaze_features_from_bytes(
    img_bytes: &[u8],
) -> Result<(Vec<KeyPoint>, Vec<u8>), opencv::Error> {
    use opencv::{
        core::{Size, Vector},
        imgcodecs, imgproc,
    };

    if img_bytes.is_empty() {
        return Err(opencv::Error::new(
            opencv::core::StsBadArg,
            "Empty image bytes".to_string(),
        ));
    }

    // 1. Construct Vector<u8> from bytes
    let buf = Vector::<u8>::from_slice(img_bytes);

    // 2. Decode bytes natively to grayscale Mat
    let gray_mat = imgcodecs::imdecode(&buf, imgcodecs::IMREAD_GRAYSCALE)?;
    if gray_mat.empty() {
        return Err(opencv::Error::new(
            opencv::core::StsError,
            "Failed to decode image bytes".to_string(),
        ));
    }

    // 3. Resize Mat natively, preserving aspect ratio (to fit inside 500x500 box, matching original image::resize)
    let src_w = gray_mat.cols() as f64;
    let src_h = gray_mat.rows() as f64;
    let scale = (500.0 / src_w).min(500.0 / src_h);
    let target_w = (src_w * scale) as i32;
    let target_h = (src_h * scale) as i32;

    let mut resized_mat = Mat::default();
    imgproc::resize(
        &gray_mat,
        &mut resized_mat,
        Size::new(target_w, target_h),
        0.0,
        0.0,
        imgproc::INTER_LINEAR,
    )?;

    // 4. Init AKAZE
    let mut akaze = AKAZE::create_def()?;

    // 5. Detect and Compute
    let mut keypoints = Vector::<KeyPoint>::new();
    let mut descriptors = Mat::default();
    let mask = Mat::default();

    akaze.detect_and_compute(&resized_mat, &mask, &mut keypoints, &mut descriptors, false)?;

    // 6. Convert descriptors Mat to Vec<u8> for storage
    let data_len = descriptors.total() * descriptors.elem_size()?;
    let mut descriptors_bytes = vec![0u8; data_len];
    let data_ptr = descriptors.data_bytes()?;
    descriptors_bytes.copy_from_slice(data_ptr);

    // 7. Convert Vector<KeyPoint> to Vec<KeyPoint>
    let keypoints_vec: Vec<KeyPoint> = keypoints.to_vec();

    Ok((keypoints_vec, descriptors_bytes))
}

#[cfg(not(target_arch = "wasm32"))]
pub const AKAZE_DESC_SIZE: i32 = 61;

/// Helper to reconstruct Mat from bytes
#[cfg(not(target_arch = "wasm32"))]
pub fn akaze_bytes_to_mat(bytes: &[u8]) -> Result<Mat, opencv::Error> {
    if bytes.is_empty() {
        return Ok(Mat::default());
    }

    // Create Mat from slice (copies data)
    let mat_1d = Mat::from_slice(bytes)?;

    // Reshape. Rows = bytes.len() / 61. Cols = 61.
    let rows = bytes.len() as i32 / AKAZE_DESC_SIZE;

    let mat_view = mat_1d.reshape(1, rows)?;

    // We must ensure we return an owned Mat, not a view/BoxedRef.
    let mut mat_owned = Mat::default();
    mat_view.copy_to(&mut mat_owned)?;

    Ok(mat_owned)
}

/// Preprocesses an image for hashing (Legacy pHash support):
/// - Resize to 500x500 (Lanczos3)
/// - Grayscale
/// - Contrast stretch
/// - Blur
pub fn preprocess_image(img: &image::DynamicImage) -> image::DynamicImage {
    // Resize to a reasonable working size
    let resized = img.resize(500, 500, image::imageops::FilterType::Lanczos3);
    // Convert to grayscale (luma8)
    let gray = resized.to_luma8();
    // Contrast stretch
    image::imageops::contrast(&gray, 20.0);
    // Blur to reduce noise
    let blurred = image::imageops::blur(&gray, 1.0);
    image::DynamicImage::ImageLuma8(blurred)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Card {
    /// Unique ID (e.g., "set1-001-en")
    pub id: String,

    /// Display Name (e.g., "Mickey Mouse")
    pub name: String,

    /// Subtitle (e.g., "Brave Little Tailor")
    pub subtitle: String,

    /// The 64-bit Perceptual Hash stored as a Hex String
    /// Example: "8f03c2998f03c299"
    pub phash: String,

    /// AKAZE binary descriptors serialized as bytes
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub akaze_data: Vec<u8>,

    /// Local path or URL to the reference image
    pub image_url: String,

    /// Rarity of the card (e.g., "Common", "Rare")
    pub rarity: String,

    /// Promo grouping string, such as "P3".
    pub promo_grouping: Option<String>,

    /// Set the card belongs to (e.g., "1")
    pub set_code: String,

    /// Number of the card within the set
    pub card_number: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScanResult {
    /// The closest matching card, if any found
    pub card: Option<Card>,

    /// Used by UI to decide whether to show "Success" or "Try Again"
    pub confidence: f64,

    /// Total number of cards successfully scanned globally (persistent)
    #[serde(default)]
    pub global_total_scans: u64,
}

/// Global Index structure for hot-RAM lookup of cards
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone)]
pub struct GlobalIndex {
    pub train_vec: Vector<Mat>,
    pub cards: Vec<Card>,
}

/// Matches a query image's descriptors Mat against a GlobalIndex
#[cfg(not(target_arch = "wasm32"))]
pub fn match_card(
    query_mat: &Mat,
    global_index: &GlobalIndex,
    ratio_thresh: f64,
    min_good_matches: usize,
) -> Result<ScanResult, opencv::Error> {
    use opencv::{
        core::{DMatch, NORM_HAMMING},
        features2d::BFMatcher,
    };

    if query_mat.empty() {
        return Ok(ScanResult {
            card: None,
            confidence: 0.0,
            global_total_scans: 0,
        });
    }

    if global_index.train_vec.is_empty() {
        return Ok(ScanResult {
            card: None,
            confidence: 0.0,
            global_total_scans: 0,
        });
    }

    // Use BFMatcher with NORM_HAMMING
    let mut matcher = BFMatcher::create(NORM_HAMMING, false)?;
    matcher.add(&global_index.train_vec)?;
    matcher.train()?;

    let mut matches = Vector::<Vector<DMatch>>::new();
    matcher.knn_match(query_mat, &mut matches, 2, &Mat::default(), false)?;

    let mut best_card: Option<Card> = None;
    let mut max_good_matches = 0;
    let mut votes = std::collections::HashMap::new();

    for m in matches {
        let m = m.to_vec();
        if let [m0, m1, ..] = m.as_slice() {
            if m0.distance < (ratio_thresh as f32) * m1.distance {
                let img_idx = m0.img_idx as usize;
                *votes.entry(img_idx).or_insert(0) += 1;
            }
        }
    }

    for (card_idx, vote_count) in votes {
        if vote_count > max_good_matches {
            max_good_matches = vote_count;
            best_card = global_index.cards.get(card_idx).cloned();
        }
    }

    if let Some(card) = best_card {
        if max_good_matches >= min_good_matches {
            let confidence = (max_good_matches as f64 / 100.0).min(1.0);
            Ok(ScanResult {
                card: Some(card),
                confidence,
                global_total_scans: 0,
            })
        } else {
            Ok(ScanResult {
                card: None,
                confidence: 0.0,
                global_total_scans: 0,
            })
        }
    } else {
        Ok(ScanResult {
            card: None,
            confidence: 0.0,
            global_total_scans: 0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_card_serialization() {
        let card = Card {
            id: "set1-001".to_string(),
            name: "Mickey Mouse".to_string(),
            subtitle: "Brave Little Tailor".to_string(),
            phash: "8f03c2998f03c299".to_string(),
            akaze_data: vec![],
            image_url: "images/1.jpg".to_string(),
            rarity: "Legendary".to_string(),
            promo_grouping: None,
            set_code: "1".to_string(),
            card_number: 1,
        };
        let serialized = serde_json::to_string(&card).unwrap();
        let deserialized: Card = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized.id, card.id);
        assert_eq!(deserialized.rarity, "Legendary");
        assert_eq!(deserialized.set_code, "1");
        assert_eq!(deserialized.card_number, 1);
    }

    #[test]
    fn test_preprocess_image() {
        use image::{DynamicImage, GenericImageView, GrayImage};
        let img = DynamicImage::ImageLuma8(GrayImage::new(100, 100));
        let processed = preprocess_image(&img);
        assert_eq!(processed.width(), 500);
        assert_eq!(processed.height(), 500);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn test_akaze_features_and_reconstruction() {
        use image::{DynamicImage, GrayImage};
        let mut img_raw = GrayImage::new(500, 500);
        for x in 0..500 {
            for y in 0..500 {
                if ((x / 50) + (y / 50)) % 2 == 0 {
                    img_raw.put_pixel(x, y, image::Luma([255]));
                } else {
                    img_raw.put_pixel(x, y, image::Luma([0]));
                }
            }
        }
        let img = DynamicImage::ImageLuma8(img_raw);
        let (kp, desc_bytes) = compute_akaze_features(&img).unwrap();
        assert!(!kp.is_empty(), "Keypoints should be detected");
        assert!(
            !desc_bytes.is_empty(),
            "Descriptor bytes should not be empty"
        );

        let reconstructed = akaze_bytes_to_mat(&desc_bytes).unwrap();
        assert_eq!(reconstructed.rows(), kp.len() as i32);
        assert_eq!(reconstructed.cols(), AKAZE_DESC_SIZE);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn test_compute_akaze_features_from_bytes() {
        use image::{DynamicImage, GrayImage};
        let mut img_raw = GrayImage::new(500, 500);
        for x in 0..500 {
            for y in 0..500 {
                if ((x / 50) + (y / 50)) % 2 == 0 {
                    img_raw.put_pixel(x, y, image::Luma([255]));
                } else {
                    img_raw.put_pixel(x, y, image::Luma([0]));
                }
            }
        }
        let img = DynamicImage::ImageLuma8(img_raw);
        let mut jpeg_bytes = Vec::new();
        img.write_to(
            &mut std::io::Cursor::new(&mut jpeg_bytes),
            image::ImageOutputFormat::Jpeg(85),
        )
        .unwrap();

        let (kp, desc_bytes) = compute_akaze_features_from_bytes(&jpeg_bytes).unwrap();
        assert!(
            !kp.is_empty(),
            "Keypoints should be detected from JPEG bytes"
        );
        assert!(
            !desc_bytes.is_empty(),
            "Descriptor bytes should not be empty"
        );

        let reconstructed = akaze_bytes_to_mat(&desc_bytes).unwrap();
        assert_eq!(reconstructed.rows(), kp.len() as i32);
        assert_eq!(reconstructed.cols(), AKAZE_DESC_SIZE);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn test_match_card() {
        let mut train_vec = Vector::<Mat>::new();

        let mut d1_bytes = vec![0u8; 2 * 61];
        d1_bytes[61] = 1; // second descriptor has first byte = 1
        let d1_temp = Mat::from_slice(&d1_bytes).unwrap();
        let d1 = d1_temp.reshape(1, 2).unwrap();
        let mut d1_owned = Mat::default();
        d1.copy_to(&mut d1_owned).unwrap();
        train_vec.push(d1_owned);

        let mut d2_bytes = vec![255u8; 2 * 61];
        d2_bytes[61] = 254; // second descriptor has first byte = 254
        let d2_temp = Mat::from_slice(&d2_bytes).unwrap();
        let d2 = d2_temp.reshape(1, 2).unwrap();
        let mut d2_owned = Mat::default();
        d2.copy_to(&mut d2_owned).unwrap();
        train_vec.push(d2_owned);

        let cards = vec![
            Card {
                id: "c1".to_string(),
                name: "Card 1".to_string(),
                subtitle: "".to_string(),
                phash: "".to_string(),
                akaze_data: vec![],
                image_url: "".to_string(),
                rarity: "".to_string(),
                promo_grouping: None,
                set_code: "1".to_string(),
                card_number: 1,
            },
            Card {
                id: "c2".to_string(),
                name: "Card 2".to_string(),
                subtitle: "".to_string(),
                phash: "".to_string(),
                akaze_data: vec![],
                image_url: "".to_string(),
                rarity: "".to_string(),
                promo_grouping: None,
                set_code: "1".to_string(),
                card_number: 2,
            },
        ];

        let index = GlobalIndex { train_vec, cards };

        let mut query = Mat::default();
        // Query has 1 descriptor of all 0s
        let q_bytes = vec![0u8; 1 * 61];
        let q_temp = Mat::from_slice(&q_bytes).unwrap();
        let q = q_temp.reshape(1, 1).unwrap();
        q.copy_to(&mut query).unwrap();

        let res = match_card(&query, &index, 0.75, 1).unwrap();
        assert!(res.card.is_some());
        assert_eq!(res.card.unwrap().id, "c1");
    }
}
