use opencv::core::{Mat, Vector};
use std::sync::Arc;

fn main() {
    let vec = Vector::<Mat>::new();
    let arc = Arc::new(vec);
    
    let a1 = arc.clone();
    let a2 = arc.clone();
    
    std::thread::spawn(move || {
        let _ = &*a1;
    });
    std::thread::spawn(move || {
        let _ = &*a2;
    });
}
