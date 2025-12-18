use clap::Parser;
use anyhow::{anyhow, Result};
use libloading::{Library, Symbol};
use soulframe_language_downloader::find_runtime_lib;
use std::collections::BTreeMap;
use std::ffi::c_void;
use std::fs;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "extract")]
#[command(about = "Extract downloaded Languages.bin files to JSON per locale")]
struct Args {
    /// Locales to extract (comma-separated)
    #[arg(short, long, default_value = "en,fr,de,es,it,pt,ru,pl,tr,ja,ko,zh")]
    locales: String,
}

fn get_download_path(path: &str, suffix: Option<&str>) -> PathBuf {
    let suffix = suffix.unwrap_or("");
    let root = std::env::current_dir().unwrap();
    root.join("downloaded-data").join(format!("0{}{}", suffix, path))
}

fn get_extract_path(path: &str, suffix: Option<&str>) -> PathBuf {
    let suffix = suffix.unwrap_or("");
    let root = std::env::current_dir().unwrap();
    root.join("extracted-data").join(format!("0{}{}", suffix, path))
}

fn read_u32_le(bin: &[u8], i: &mut usize) -> Result<u32> {
    if *i + 4 > bin.len() { return Err(anyhow!("Unexpected EOF reading u32")); }
    let v = u32::from_le_bytes([bin[*i], bin[*i + 1], bin[*i + 2], bin[*i + 3]]);
    *i += 4;
    Ok(v)
}

fn read_u16_le(bin: &[u8], i: &mut usize) -> Result<u16> {
    if *i + 2 > bin.len() { return Err(anyhow!("Unexpected EOF reading u16")); }
    let v = u16::from_le_bytes([bin[*i], bin[*i + 1]]);
    *i += 2;
    Ok(v)
}

fn read_s4(bin: &[u8], i: &mut usize) -> Result<Vec<u8>> {
    let len = read_u32_le(bin, i)? as usize;
    if *i + len > bin.len() { return Err(anyhow!("Unexpected EOF reading s4")); }
    let v = bin[*i..*i + len].to_vec();
    *i += len;
    Ok(v)
}

fn unpack_u32_dyn_le(bin: &[u8], i: &mut usize) -> Result<u32> {
    let mut value: u32 = 0;
    let mut shift: u32 = 0;
    while shift < 28 {
        if *i >= bin.len() { return Err(anyhow!("Unexpected EOF in dyn u32")); }
        let byte = bin[*i];
        *i += 1;
        value |= ((byte & 0x7f) as u32) << shift;
        if (byte & 0x80) == 0 { return Ok(value); }
        shift += 7;
    }
    if *i >= bin.len() { return Err(anyhow!("Unexpected EOF in dyn u32 final")); }
    let byte = bin[*i];
    *i += 1;
    if byte > 0x0F { return Err(anyhow!("Invalid final dyn u32 byte: {}", byte)); }
    value |= (byte as u32) << shift;
    Ok(value)
}

// Minimal Zstd FFI wrapper to match Pluto behavior
struct Zstd {
    lib: Library,
    create_ddict: Symbol<'static, unsafe extern "C" fn(*const u8, usize) -> usize>,
    create_dctx: Symbol<'static, unsafe extern "C" fn() -> usize>,
    dctx_set_param: Symbol<'static, unsafe extern "C" fn(usize, i32, i32) -> usize>,
    decompress_using_ddict: Symbol<'static, unsafe extern "C" fn(usize, *mut c_void, usize, *const u8, usize, usize) -> usize>,
    free_dctx: Symbol<'static, unsafe extern "C" fn(usize) -> usize>,
    free_ddict: Symbol<'static, unsafe extern "C" fn(usize) -> usize>,
}

impl Zstd {
    fn new() -> Result<Self> {
        let lib_name = if cfg!(windows) { "libzstd.dll" } else { "libzstd.so" };
        let lib_path = find_runtime_lib(lib_name)?;
        
        unsafe {
            let lib = Library::new(&lib_path)
                .map_err(|e| anyhow!("Failed to load Zstd library from {:?}: {}", lib_path, e))?;
            let create_ddict: Symbol<unsafe extern "C" fn(*const u8, usize) -> usize> = lib.get(b"ZSTD_createDDict\0")?;
            let create_dctx: Symbol<unsafe extern "C" fn() -> usize> = lib.get(b"ZSTD_createDCtx\0")?;
            let dctx_set_param: Symbol<unsafe extern "C" fn(usize, i32, i32) -> usize> = lib.get(b"ZSTD_DCtx_setParameter\0")?;
            let decompress_using_ddict: Symbol<unsafe extern "C" fn(usize, *mut c_void, usize, *const u8, usize, usize) -> usize> = lib.get(b"ZSTD_decompress_usingDDict\0")?;
            let free_dctx: Symbol<unsafe extern "C" fn(usize) -> usize> = lib.get(b"ZSTD_freeDCtx\0")?;
            let free_ddict: Symbol<unsafe extern "C" fn(usize) -> usize> = lib.get(b"ZSTD_freeDDict\0")?;
            // Extend lifetimes
            let create_ddict = std::mem::transmute(create_ddict);
            let create_dctx = std::mem::transmute(create_dctx);
            let dctx_set_param = std::mem::transmute(dctx_set_param);
            let decompress_using_ddict = std::mem::transmute(decompress_using_ddict);
            let free_dctx = std::mem::transmute(free_dctx);
            let free_ddict = std::mem::transmute(free_ddict);
            Ok(Self { lib, create_ddict, create_dctx, dctx_set_param, decompress_using_ddict, free_dctx, free_ddict })
        }
    }
}

fn languages_unpack(bin: &[u8]) -> Result<(BTreeMap<String, String>, Vec<u8>)> {
    let mut i = 0usize;
    if bin.len() < 16 + 12 { return Err(anyhow!("Languages.bin too short")); }
    // skip 16-byte hash and 3 u32 constants
    i += 16; // hash
    i += 4; // 0x14
    i += 4; // 0x2B
    i += 4; // 0x01

    let num_suffixes = read_u32_le(bin, &mut i)? as usize;
    for _ in 0..num_suffixes { let _ = read_s4(bin, &mut i)?; }

    let dict_bin = read_s4(bin, &mut i)?;
    let num_paths = read_u32_le(bin, &mut i)? as usize;

    let zstd = Zstd::new()?;
    let dict_handle;
    let dctx_handle;
    unsafe {
        dict_handle = (zstd.create_ddict)(dict_bin.as_ptr(), dict_bin.len());
        dctx_handle = (zstd.create_dctx)();
        // Mirrors Pluto: set parameter 1000 to 1
        let _ = (zstd.dctx_set_param)(dctx_handle, 1000, 1);
    }

    let mut entries: BTreeMap<String, String> = BTreeMap::new();

    for _ in 0..num_paths {
        let path_bytes = read_s4(bin, &mut i)?;
        let path = String::from_utf8_lossy(&path_bytes).to_string();
        let chunk = read_s4(bin, &mut i)?;
        let num_labels = read_u32_le(bin, &mut i)? as usize;

        for _ in 0..num_labels {
            let name_bytes = read_s4(bin, &mut i)?;
            let name = String::from_utf8_lossy(&name_bytes).to_string();
            let offset = read_u32_le(bin, &mut i)? as usize;
            let size = read_u16_le(bin, &mut i)? as usize;
            let flags = read_u16_le(bin, &mut i)? as u32;

            if offset + size > chunk.len() { return Err(anyhow!("Label slice out of bounds")); }
            let mut data = &chunk[offset..offset + size];

            let value_bytes: Vec<u8> = if (flags & 0x200) != 0 { // compressed with zstd + dict
                let mut di = 0usize;
                let decompressed_size = unpack_u32_dyn_le(data, &mut di)? as usize;
                if di > data.len() { return Err(anyhow!("Invalid dyn len offset")); }
                let src = &data[di..];
                let mut out = vec![0u8; decompressed_size];
                let wrote;
                unsafe {
                    wrote = (zstd.decompress_using_ddict)(
                        dctx_handle,
                        out.as_mut_ptr() as *mut c_void,
                        decompressed_size,
                        src.as_ptr(),
                        src.len(),
                        dict_handle,
                    );
                }
                if wrote != decompressed_size { return Err(anyhow!("ZSTD decompression size mismatch: {} != {}", wrote, decompressed_size)); }
                out
            } else {
                data.to_vec()
            };

            let key = format!("{}{}", path, name);
            let value = String::from_utf8_lossy(&value_bytes).to_string();
            entries.insert(key, value);
        }
    }

    unsafe {
        let _ = (zstd.free_dctx)(dctx_handle);
        let _ = (zstd.free_ddict)(dict_handle);
    }

    Ok((entries, dict_bin))
}

fn main() -> Result<()> {
    let args = Args::parse();
    
    println!("=== Extract downloaded Languages.bin -> JSON ===");
    
    // Parse locales
    let locales: Vec<String> = args.locales
        .split(',')
        .map(|s| s.trim().to_string())
        .collect();
    
    // Ensure extract base folder exists
    let marker_path = get_extract_path("/marker", None);
    if let Some(parent) = marker_path.parent() {
        fs::create_dir_all(parent)?;
    }
    
    // Check which locales are present
    let mut present = Vec::new();
    for locale in &locales {
        let suffix = format!("_{}", locale);
        let h_path = get_download_path("/Languages.bin", Some(&suffix));
        let h_file_path = format!("{}_H", h_path.to_string_lossy());
        
        if fs::metadata(&h_file_path).is_ok() {
            present.push(locale.clone());
        }
    }
    
    if present.is_empty() {
        println!("No downloaded Languages.bin found. Run download command first.");
        return Ok(());
    }
    
    println!("Found {} locales to extract: {}", present.len(), present.join(", "));

    // Perform real extraction
    for locale in &present {
        let suffix = format!("_{}", locale);
        let h_path = get_download_path("/Languages.bin", Some(&suffix));
        let h_file_path = format!("{}_H", h_path.to_string_lossy());

        println!("[{}] Reading {}", locale, h_file_path);
        let bin = fs::read(&h_file_path)?;
        let (entries, _dict) = languages_unpack(&bin)?;

        // Order keys for deterministic output
        let mut keys: Vec<String> = entries.keys().cloned().collect();
        keys.sort();

        // Build JSON object with __order and all keys
        let mut ordered: BTreeMap<String, serde_json::Value> = BTreeMap::new();
        ordered.insert("__order".to_string(), serde_json::Value::Array(keys.iter().map(|k| serde_json::Value::String(k.clone())).collect()));
        for k in &keys {
            if let Some(v) = entries.get(k) {
                ordered.insert(k.clone(), serde_json::Value::String(v.clone()));
            }
        }

        let output_path = get_extract_path(&format!("/Languages/{}.json", locale), None);
        if let Some(parent) = output_path.parent() { fs::create_dir_all(parent)?; }
        let json = serde_json::to_string_pretty(&ordered)?;
        fs::write(&output_path, json)?;
        println!("  ✓ {} strings -> {}", keys.len(), output_path.to_string_lossy());
    }
    
    // Create alias Languages.json to en if present, else first present
    let alias_path = get_extract_path("/Languages/Languages.json", None);
    
    if present.contains(&"en".to_string()) {
        let en_path = get_extract_path("/Languages/en.json", None);
        if let Ok(content) = fs::read_to_string(&en_path) {
            fs::write(&alias_path, content)?;
            println!("Alias written: Languages.json -> en.json");
        }
    } else if !present.is_empty() {
        let first = &present[0];
        let first_path = get_extract_path(&format!("/Languages/{}.json", first), None);
        if let Ok(content) = fs::read_to_string(&first_path) {
            fs::write(&alias_path, content)?;
            println!("Alias written: Languages.json -> {}.json", first);
        }
    }
    
    println!("\nDone. Output under ./extracted-data/0/Languages/");
    
    Ok(())
}
