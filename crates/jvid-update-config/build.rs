fn main() {
    println!(
        "cargo:rustc-env=JVID_UPDATE_TARGET={}",
        std::env::var("TARGET").unwrap()
    );
}
