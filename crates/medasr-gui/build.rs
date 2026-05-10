// Embeds the Windows app icon into the .exe via a resource script.
// On non-Windows targets this is a no-op.

fn main() {
    #[cfg(windows)]
    {
        // Tell cargo to rebuild if the icon or the .rc change.
        println!("cargo:rerun-if-changed=app.rc");
        println!("cargo:rerun-if-changed=../../assets/icon/icon.ico");
        let _ = embed_resource::compile("app.rc", embed_resource::NONE);
    }
}
