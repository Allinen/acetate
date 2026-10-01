//! Windows: the icon in the program itself (Explorer, shortcuts and the taskbar show it;
//! the window's own icon is set when it opens).

fn main() {
    println!("cargo:rerun-if-changed=assets/acetate.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/acetate.ico");
        res.compile().expect("the Windows resources (with a MinGW toolchain: x86_64-w64-mingw32-windres)");
    }
}
