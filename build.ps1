# Builds MyTune with the Yapfast engine. Needs LLVM (clang-cl), NASM, Ninja, CMake and Git for Windows (winget).
# Usage: .\build.ps1 [cargo args]   e.g. .\build.ps1 build --release -p mytune-ui
$env:Path += ";C:\Program Files\LLVM\bin;C:\Program Files\NASM;C:\Program Files\Git\usr\bin"
cargo @args
