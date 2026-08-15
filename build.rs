fn main() {
    slint_build::compile("ui/appwindow.slint").expect("Slint build failed");

    #[cfg(target_os = "windows")]
    {
        let mut res = winres::WindowsResource::new();
        res.set_icon("assets/app_icon.ico");
        res.set_language(0x0409); // English (US) / Neutral
        res.set("ProductName", "Litecord");
        res.set("FileDescription", "Litecord - Ultra-Lightweight Discord Client");
        res.set("LegalCopyright", "Litecord Project");
        if let Err(e) = res.compile() {
            println!("cargo:warning=winres icon compilation failed: {:?}", e);
        }
    }
}
