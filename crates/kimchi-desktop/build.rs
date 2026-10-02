//! Embeds the app icon and version into kimchi.exe on Windows.

fn main() {
    println!("cargo:rerun-if-changed=resources/kimchi.ico");
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("resources/kimchi.ico");
        res.set("ProductName", "kimchi");
        res.set("FileDescription", "kimchi — video editor");
        if let Err(e) = res.compile() {
            println!("cargo:warning=couldn't embed the Windows icon: {e}");
        }
    }
}
