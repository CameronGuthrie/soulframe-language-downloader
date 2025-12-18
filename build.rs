// Build script for soulframe-language-downloader
// This helps set up the build environment and provides information about required libraries

use std::env;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=lib/");
    
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let lib_dir = PathBuf::from(&manifest_dir).join("lib");
    let profile = env::var("PROFILE").unwrap();
    let target_dir = PathBuf::from(&manifest_dir)
        .join("target")
        .join(&profile)
        .join("lib");
    
    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    
    // Copy DLLs to target directory so executables can find them
    if cfg!(target_os = "windows") {
        let oodle_src = lib_dir.join("oo2core_9.dll");
        let zstd_src = lib_dir.join("libzstd.dll");
        
        if oodle_src.exists() && zstd_src.exists() {
            // Create target lib directory
            let _ = std::fs::create_dir_all(&target_dir);
            
            // Copy DLLs
            let _ = std::fs::copy(&oodle_src, target_dir.join("oo2core_9.dll"));
            let _ = std::fs::copy(&zstd_src, target_dir.join("libzstd.dll"));
        } else {
            if !oodle_src.exists() {
                println!("cargo:warning=Missing oo2core_9.dll in lib/ directory");
            }
            if !zstd_src.exists() {
                println!("cargo:warning=Missing libzstd.dll in lib/ directory");
            }
        }
    } else {
        let oodle_src = lib_dir.join("oo2core_9.so");
        let zstd_src = lib_dir.join("libzstd.so");
        
        if oodle_src.exists() && zstd_src.exists() {
            let _ = std::fs::create_dir_all(&target_dir);
            let _ = std::fs::copy(&oodle_src, target_dir.join("oo2core_9.so"));
            let _ = std::fs::copy(&zstd_src, target_dir.join("libzstd.so"));
        } else {
            if !oodle_src.exists() {
                println!("cargo:warning=Missing oo2core_9.so in lib/ directory");
            }
            if !zstd_src.exists() {
                println!("cargo:warning=Missing libzstd.so in lib/ directory");
            }
        }
    }
}
