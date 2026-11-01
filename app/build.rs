// Embeds the app icon into aufmass-app.exe.
fn main() {
    println!("cargo:rerun-if-changed=assets/aufmass.rc");
    println!("cargo:rerun-if-changed=assets/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("assets/aufmass.rc", embed_resource::NONE).manifest_optional().unwrap();
    }
}
