//! Windows: embeds the app icon and version info into the .exe.
fn main() {
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../../assets/icon/stylized3dpaint.ico");
        res.set("ProductName", "Stylized3DPaint");
        res.set("FileDescription", "Stylized3DPaint — 3D texture painter");
        if let Err(e) = res.compile() {
            println!("cargo:warning=could not embed the Windows icon: {e}");
        }
    }
    println!("cargo:rerun-if-changed=../../assets/icon/stylized3dpaint.ico");
}
