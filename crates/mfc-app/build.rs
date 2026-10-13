//! Su Windows incorpora icona e informazioni di versione nell'eseguibile.

fn main() {
    println!("cargo:rerun-if-changed=../../assets/icon/icon.ico");
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../../assets/icon/icon.ico")
            .set("ProductName", "Mixfonocraft AI Studio")
            .set("FileDescription", "Mixfonocraft AI Studio")
            .set("CompanyName", "Mixfonocraft")
            .set("LegalCopyright", "MIT OR Apache-2.0");
        if let Err(e) = res.compile() {
            println!("cargo:warning=risorse Windows non incorporate: {e}");
        }
    }
}
