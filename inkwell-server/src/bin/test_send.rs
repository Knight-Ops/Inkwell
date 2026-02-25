use opencv::core::{Mat, Vector};
use std::sync::Arc;

fn main() {
    let vec = Vector::<Mat>::new();
    let arc = Arc::new(vec);
    
    std::thread::spawn(move || {
        let _v = arc;
    }).join().unwrap();
}
