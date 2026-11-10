use std::env;
use std::path::Path;
use std::process::Command;

fn main() {
    #[cfg(target_os = "windows")]
    {
        let out_dir = env::var("OUT_DIR").unwrap_or_else(|_| ".".to_string());
        let res_o = Path::new(&out_dir).join("aegisdisk_res.o");

        let windres_candidates = [
            r"C:\Program Files\JetBrains\RustRover 2026.2.5\bin\gdb\win\x64\bin\windres.exe",
            r"C:\Program Files\JetBrains\RustRover 2024.3\bin\gdb\win\x64\bin\windres.exe",
            "windres.exe",
        ];

        let mut compiled = false;
        for exe in windres_candidates {
            let status = Command::new(exe)
                .args(&[
                    "--preprocessor=cmd.exe",
                    "--preprocessor-arg=/c",
                    "--preprocessor-arg=type",
                    "-i",
                    "aegisdisk.rc",
                    "-o",
                    res_o.to_str().unwrap(),
                    "--target=pe-x86-64",
                ])
                .status();

            if let Ok(s) = status {
                if s.success() {
                    println!("cargo:rustc-link-arg={}", res_o.display());
                    compiled = true;
                    break;
                }
            }
        }

        if !compiled {
            let root_res = Path::new("aegisdisk_res.o");
            if root_res.exists() {
                println!("cargo:rustc-link-arg={}", root_res.canonicalize().unwrap().display());
            }
        }
    }

    // On Windows MSVC, embed UAC manifest to require administrator privileges automatically
    #[cfg(all(target_os = "windows", target_env = "msvc"))]
    {
        println!("cargo:rustc-link-arg=/MANIFESTUAC:level='requireAdministrator' uiAccess='false'");
    }
}
