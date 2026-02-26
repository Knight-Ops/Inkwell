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
}
