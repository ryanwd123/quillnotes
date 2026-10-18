fn main() {
    // Embeds the icon, application manifest and version info.
    embed_resource::compile("assets/quillnotes.rc", embed_resource::NONE)
        .manifest_required()
        .unwrap();
    println!("cargo:rerun-if-changed=assets/quillnotes.ico");
    println!("cargo:rerun-if-changed=assets/quillnotes.exe.manifest");
}
