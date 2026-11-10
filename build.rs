fn main() {
    #[cfg(target_os = "windows")]
    {
        let mut res = winres::WindowsResource::new();
        res.set_icon("aegisdisk.ico");
        res.set("ProductName", "AegisDisk");
        res.set("FileDescription", "AegisDisk Storage Diagnostics and Forensics");
        let _ = res.compile();
    }

    // On Windows MSVC, embed UAC manifest to require administrator privileges automatically
    #[cfg(all(target_os = "windows", target_env = "msvc"))]
    {
        println!("cargo:rustc-link-arg=/MANIFESTUAC:level='requireAdministrator' uiAccess='false'");
    }
}
