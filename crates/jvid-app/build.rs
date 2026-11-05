fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=../../assets/jvid.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        winresource::WindowsResource::new()
            .set_icon("../../assets/jvid.ico")
            .set("ProductName", "jvid")
            .set("FileDescription", "jvid video editor")
            .set("OriginalFilename", "jvid-app.exe")
            .compile()?;
    }
    Ok(())
}
