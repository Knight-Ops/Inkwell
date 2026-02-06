use serde::{Deserialize, Serialize};

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

    /// Local path or URL to the reference image
    pub image_url: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScanResult {
    /// The closest matching card, if any found
    pub card: Option<Card>,

    /// 0.0 to 1.0 (Derived from Hamming Distance)
    /// Used by UI to decide whether to show "Success" or "Try Again"
    pub confidence: f64,
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
            image_url: "images/1.jpg".to_string(),
        };
        let serialized = serde_json::to_string(&card).unwrap();
        let deserialized: Card = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized.id, card.id);
    }
}
