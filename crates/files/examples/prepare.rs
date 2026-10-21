//! Measures how long preparing a file takes (read, encrypt and hash every chunk).
//!
//! `cargo run -p ow-files --example prepare -- <file>`

fn main() {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: prepare <file>");
        return;
    };
    let key = ow_files::FileKey::generate().expect("RNG");
    let device = ow_identity::DeviceKey::generate().expect("RNG");
    let start = std::time::Instant::now();
    match ow_files::prepare(std::path::Path::new(&path), &key, &device, |_| {}) {
        Ok(m) => println!(
            "{} bytes, {} chunks in {:?}",
            m.size(),
            m.count(),
            start.elapsed()
        ),
        Err(e) => eprintln!("error: {e}"),
    }
}
