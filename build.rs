fn main() {
    // Icon der exe (Explorer, Taskleiste) per Ressourcendatei einbetten
    embed_resource::compile("app.rc", embed_resource::NONE)
        .manifest_optional()
        .unwrap();
}
