fn main() {
    println!("cargo:rerun-if-changed=assets/maklyxa.rc");
    println!("cargo:rerun-if-changed=assets/maklyxa.ico");

    if cfg!(windows) && std::env::var_os("CARGO_CFG_TARGET_OS").is_some_and(|os| os == "windows") {
        embed_resource::compile("assets/maklyxa.rc", embed_resource::NONE)
            .manifest_required()
            .expect("compile MaklyxaTube's Windows icon");
    }
}
