// Bettet speichermonitor.ico und das Manifest (highestAvailable) in die Exe ein.
// Tests deshalb mit __COMPAT_LAYER=RunAsInvoker starten, falls Windows sonst Erhöhung verlangt (os error 740).
fn main() {
    embed_resource::compile("app.rc", embed_resource::NONE)
        .manifest_optional()
        .unwrap();
}
