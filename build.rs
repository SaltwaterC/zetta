fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let icon = "assets/icons/zetta-terminal-icon.ico";
    let resource = "resources/windows/zetta.rc";

    println!("cargo:rerun-if-changed={icon}");
    println!("cargo:rerun-if-changed={resource}");

    embed_resource::compile(resource, embed_resource::NONE)
        .manifest_required()
        .unwrap();
}
