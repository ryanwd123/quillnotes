fn main() {
    // Embeds the icon, application manifest and version info.
    embed_resource::compile("assets/penmark.rc", embed_resource::NONE)
        .manifest_required()
        .unwrap();
    println!("cargo:rerun-if-changed=assets/penmark.ico");
    println!("cargo:rerun-if-changed=assets/penmark.exe.manifest");
}
