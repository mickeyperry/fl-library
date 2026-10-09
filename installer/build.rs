fn main() {
    // icon + manifest (asks for admin, DPI aware); payload.zip is produced by tools/build_installer.py
    println!("cargo:rerun-if-changed=app.rc");
    println!("cargo:rerun-if-changed=app.manifest");
    println!("cargo:rerun-if-changed=payload.zip");
    embed_resource::compile("app.rc", embed_resource::NONE).manifest_required().unwrap();
}
