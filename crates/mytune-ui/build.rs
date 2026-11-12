// Puts assets/MyTune.ico (the C# app's icon) into the exe, so Explorer, the taskbar and Task Manager show it.
fn main() {
    println!("cargo:rerun-if-changed=assets/MyTune.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/MyTune.ico");
        res.set("ProductName", "MyTune");
        res.set("FileDescription", "MyTune");
        if let Err(e) = res.compile() {
            println!("cargo:warning=could not embed the exe icon: {e}");
        }
        // Bin only: tests and examples must run without elevation. (MSVC linker.)
        println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg-bins=/MANIFESTUAC:level='requireAdministrator' uiAccess='false'");
    }
}
