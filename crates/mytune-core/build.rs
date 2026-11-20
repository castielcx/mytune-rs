// Compiles taskbar/tap.cpp into the DLL that src/taskbar.rs embeds and loads into explorer.exe.
fn main() {
    println!("cargo:rerun-if-changed=taskbar/tap.cpp");
    if std::env::var_os("CARGO_FEATURE_TASKBAR_LOOK").is_none() {
        return;
    }
    let out = std::env::var("OUT_DIR").unwrap();
    let cl = cc::windows_registry::find_tool("x86_64-pc-windows-msvc", "cl.exe")
        .expect("the taskbar DLL needs MSVC (Visual Studio Build Tools, C++ workload)");
    let run = cl
        .to_command()
        .current_dir(&out)
        .args(["/nologo", "/LD", "/std:c++20", "/EHsc", "/O1", "/MT", "/DUNICODE", "/D_UNICODE"])
        .arg(std::env::current_dir().unwrap().join("taskbar/tap.cpp"))
        .arg("/Fe:mytune-taskbar.dll")
        .args(["/link", "WindowsApp.lib", "ole32.lib", "oleaut32.lib", "user32.lib"])
        .output()
        .expect("cl.exe did not start");
    assert!(run.status.success(), "taskbar DLL did not compile:\n{}", String::from_utf8_lossy(&run.stdout));
}
