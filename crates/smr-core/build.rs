use std::fs::File;
use std::io::Read;
use std::path::Path;

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for b in bytes {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn main() {
    let path = Path::new("assets/index.html");
    println!("cargo:rerun-if-changed={}", path.display());

    let mut file = File::open(path).expect("read assets/index.html");
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).expect("read assets/index.html");

    let digest = format!("{:016x}", fnv1a64(&bytes));
    println!("cargo:rustc-env=SMR_UI_DIGEST={digest}");
}
