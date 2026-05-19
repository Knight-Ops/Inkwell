use criterion::{Criterion, criterion_group, criterion_main};
use inkwell_core::{Card, GlobalIndex, match_card};
use opencv::{
    core::{Mat, Vector},
    prelude::*,
};
use std::hint::black_box;

fn create_mock_index(num_cards: usize) -> GlobalIndex {
    let mut train_vec = Vector::<Mat>::new();
    let mut cards = Vec::new();

    for i in 0..num_cards {
        let bytes = vec![i as u8; 20 * 61];
        let mat_temp = Mat::from_slice(&bytes).unwrap();
        let mat = mat_temp.reshape(1, 20).unwrap();
        let mut mat_owned = Mat::default();
        mat.copy_to(&mut mat_owned).unwrap();
        train_vec.push(mat_owned);

        cards.push(Card {
            id: format!("card-{}", i),
            name: format!("Mock Card {}", i),
            subtitle: "".to_string(),
            phash: "".to_string(),
            akaze_data: vec![],
            image_url: "".to_string(),
            rarity: "Common".to_string(),
            promo_grouping: None,
            set_code: "1".to_string(),
            card_number: i as u32,
        });
    }

    GlobalIndex { train_vec, cards }
}

fn bench_card_matching(c: &mut Criterion) {
    let index_10 = create_mock_index(10);
    let index_100 = create_mock_index(100);
    let index_1000 = create_mock_index(1000);

    let query_bytes = vec![5u8; 20 * 61];
    let query_temp = Mat::from_slice(&query_bytes).unwrap();
    let query_mat_ref = query_temp.reshape(1, 20).unwrap();
    let mut query_mat = Mat::default();
    query_mat_ref.copy_to(&mut query_mat).unwrap();

    let mut group = c.benchmark_group("card_matching");

    group.bench_function("index_10", |b| {
        b.iter(|| {
            let res = match_card(
                black_box(&query_mat),
                black_box(&index_10),
                black_box(0.75),
                black_box(5),
            );
            assert!(res.is_ok());
        })
    });

    group.bench_function("index_100", |b| {
        b.iter(|| {
            let res = match_card(
                black_box(&query_mat),
                black_box(&index_100),
                black_box(0.75),
                black_box(5),
            );
            assert!(res.is_ok());
        })
    });

    group.bench_function("index_1000", |b| {
        b.iter(|| {
            let res = match_card(
                black_box(&query_mat),
                black_box(&index_1000),
                black_box(0.75),
                black_box(5),
            );
            assert!(res.is_ok());
        })
    });

    group.finish();
}

criterion_group!(benches, bench_card_matching);
criterion_main!(benches);
