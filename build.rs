fn main() {
    // On Windows MSVC, embed UAC manifest to require administrator privileges automatically
    #[cfg(all(target_os = "windows", target_env = "msvc"))]
    {
        println!("cargo:rustc-link-arg=/MANIFESTUAC:level='requireAdministrator' uiAccess='false'");
    }
}
