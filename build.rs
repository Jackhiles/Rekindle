//! Gives the Windows executable Rekindle's icon (shown by Explorer, shortcuts and the taskbar).

fn main() {
    println!("cargo:rerun-if-changed=assets/icon/rekindle.rc");
    println!("cargo:rerun-if-changed=assets/icon/rekindle.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("assets/icon/rekindle.rc", embed_resource::NONE)
            .manifest_optional()
            .unwrap();
    }
}
