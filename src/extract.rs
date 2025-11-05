use anyhow::{anyhow, Result};
use byteorder::{LittleEndian, ReadBytesExt};
use serde_json::json;
use soulframe_language_downloader::*;
use std::collections::BTreeMap;
use std::ffi::{c_char, c_int, c_void};
use std::fs;
use std::io::Cursor;
use libloading::{Library, Symbol};

/// ZSTD library interface for language decompression
pub struct Zstd {
    #[allow(dead_code)]
    lib: Library,
    create_ddict: Symbol<'static, unsafe extern "C" fn(*const c_char, usize) -> usize>,
    create_dctx: Symbol<'static, unsafe extern "C" fn() -> usize>,
    dctx_set_parameter: Symbol<'static, unsafe extern "C" fn(usize, c_int, c_int) -> usize>,
    decompress_using_ddict: Symbol<'static, unsafe extern "C" fn(usize, *mut c_void, usize, *const c_char, usize, usize) -> usize>,
    free_dctx: Symbol<'static, unsafe extern "C" fn(usize) -> usize>,
    free_ddict: Symbol<'static, unsafe extern "C" fn(usize) -> usize>,
}

impl Zstd {
    pub fn new() -> Result<Self> {
        let lib_path = if cfg!(windows) {
            "./lib/libzstd.dll"
        } else {
            "./lib/libzstd.so"
        };
        
        unsafe {
            let lib = Library::new(lib_path)
                .map_err(|e| anyhow!("Failed to load ZSTD library: {}", e))?;
            
            let create_ddict: Symbol<unsafe extern "C" fn(*const c_char, usize) -> usize> = 
                lib.get(b"ZSTD_createDDict\0")
                    .map_err(|e| anyhow!("Failed to get ZSTD_createDDict: {}", e))?;
            
            let create_dctx: Symbol<unsafe extern "C" fn() -> usize> = 
                lib.get(b"ZSTD_createDCtx\0")
                    .map_err(|e| anyhow!("Failed to get ZSTD_createDCtx: {}", e))?;
            
            let dctx_set_parameter: Symbol<unsafe extern "C" fn(usize, c_int, c_int) -> usize> = 
                lib.get(b"ZSTD_DCtx_setParameter\0")
                    .map_err(|e| anyhow!("Failed to get ZSTD_DCtx_setParameter: {}", e))?;
            
            let decompress_using_ddict: Symbol<unsafe extern "C" fn(usize, *mut c_void, usize, *const c_char, usize, usize) -> usize> = 
                lib.get(b"ZSTD_decompress_usingDDict\0")
                    .map_err(|e| anyhow!("Failed to get ZSTD_decompress_usingDDict: {}", e))?;
            
            let free_dctx: Symbol<unsafe extern "C" fn(usize) -> usize> = 
                lib.get(b"ZSTD_freeDCtx\0")
                    .map_err(|e| anyhow!("Failed to get ZSTD_freeDCtx: {}", e))?;
            
            let free_ddict: Symbol<unsafe extern "C" fn(usize) -> usize> = 
                lib.get(b"ZSTD_freeDDict\0")
                    .map_err(|e| anyhow!("Failed to get ZSTD_freeDDict: {}", e))?;
            
            // Extend lifetimes to 'static - safe because we keep the library alive
            let create_ddict: Symbol<'static, _> = std::mem::transmute(create_ddict);
            let create_dctx: Symbol<'static, _> = std::mem::transmute(create_dctx);
            let dctx_set_parameter: Symbol<'static, _> = std::mem::transmute(dctx_set_parameter);
            let decompress_using_ddict: Symbol<'static, _> = std::mem::transmute(decompress_using_ddict);
            let free_dctx: Symbol<'static, _> = std::mem::transmute(free_dctx);
            let free_ddict: Symbol<'static, _> = std::mem::transmute(free_ddict);
            
            Ok(Self {
                lib,
                create_ddict,
                create_dctx,
                dctx_set_parameter,
                decompress_using_ddict,
                free_dctx,
                free_ddict,
            })
        }
    }
}

pub fn languages_unpack(bin: &[u8], zstd: &Zstd) -> Result<BTreeMap<String, String>> {
    let mut cursor = Cursor::new(bin);
    let mut entries = BTreeMap::new();
    
    // Skip hash (16 bytes)
    cursor.set_position(16);
    
    // Read and verify magic numbers
    let magic1 = cursor.read_u32::<LittleEndian>()?; // 0x14
    let magic2 = cursor.read_u32::<LittleEndian>()?; // 0x2B
    let magic3 = cursor.read_u32::<LittleEndian>()?; // 0x01
    
    if magic1 != 0x14 || magic2 != 0x2B || magic3 != 0x01 {
        return Err(anyhow!("Invalid language file magic numbers"));
    }
    
    // Read number of suffixes
    let num_suffixes = cursor.read_u32::<LittleEndian>()?;
    
    // Skip suffixes
    for _ in 0..num_suffixes {
        let suffix_len = cursor.read_u32::<LittleEndian>()?;
        cursor.set_position(cursor.position() + suffix_len as u64);
    }
    
    // Read dictionary
    let dict_len = cursor.read_u32::<LittleEndian>()?;
    let dict_start = cursor.position() as usize;
    cursor.set_position(cursor.position() + dict_len as u64);
    let dict_bin = &bin[dict_start..dict_start + dict_len as usize];
    
    // Read number of paths
    let num_paths = cursor.read_u32::<LittleEndian>()?;
    
    unsafe {
        // Create ZSTD dictionary and context
        let dict = (zstd.create_ddict)(dict_bin.as_ptr() as *const c_char, dict_bin.len());
        let ctx = (zstd.create_dctx)();
        (zstd.dctx_set_parameter)(ctx, 1000, 1); // ZSTD_d_refMultipleDDicts = 1000
        
        // Process each path
        for _ in 0..num_paths {
            let path_len = cursor.read_u32::<LittleEndian>()?;
            let path_start = cursor.position() as usize;
            cursor.set_position(cursor.position() + path_len as u64);
            let path = String::from_utf8_lossy(&bin[path_start..path_start + path_len as usize]);
            
            let chunk_len = cursor.read_u32::<LittleEndian>()?;
            let chunk_start = cursor.position() as usize;
            cursor.set_position(cursor.position() + chunk_len as u64);
            let chunk = &bin[chunk_start..chunk_start + chunk_len as usize];
            
            let num_labels = cursor.read_u32::<LittleEndian>()?;
            
            for _ in 0..num_labels {
                let name_len = cursor.read_u32::<LittleEndian>()?;
                let name_start = cursor.position() as usize;
                cursor.set_position(cursor.position() + name_len as u64);
                let name = String::from_utf8_lossy(&bin[name_start..name_start + name_len as usize]);
                
                let offset = cursor.read_u32::<LittleEndian>()?;
                let size = cursor.read_u16::<LittleEndian>()?;
                let flags = cursor.read_u16::<LittleEndian>()?;
                
                let mut data = chunk[offset as usize..(offset + size as u32) as usize].to_vec();
                
                // Check if compressed
                if (flags & 0x200) != 0 {
                    let mut data_cursor = Cursor::new(&data);
                    let (decompressed_size, data_offset) = unpack_u32_dyn_le(&data, 0)?;
                    
                    let compressed_data = &data[data_offset..];
                    let mut output = vec![0u8; decompressed_size as usize];
                    
                    let result = (zstd.decompress_using_ddict)(
                        ctx,
                        output.as_mut_ptr() as *mut c_void,
                        decompressed_size as usize,
                        compressed_data.as_ptr() as *const c_char,
                        compressed_data.len(),
                        dict
                    );
                    
                    if result != decompressed_size as usize {
                        return Err(anyhow!("ZSTD decompression failed"));
                    }
                    
                    data = output;
                }
                
                let full_key = format!("{}{}", path, name);
                let text = String::from_utf8_lossy(&data).to_string();
                entries.insert(full_key, text);
            }
        }
        
        // Cleanup ZSTD resources
        (zstd.free_dctx)(ctx);
        (zstd.free_ddict)(dict);
    }
    
    Ok(entries)
}

pub fn extract_languages_for_locale(locale: &str, zstd: &Zstd) -> Result<usize> {
    let h_path_suffix = format!("_{}", locale);
    let h_path = get_download_path("/Languages.bin", Some(&h_path_suffix));
    let h_file_path = format!("{}_H", h_path.to_string_lossy());
    
    let bin = fs::read(&h_file_path)
        .map_err(|_| anyhow!("Languages.bin_H not found for locale {}", locale))?;
    
    let entries = languages_unpack(&bin, zstd)?;
    
    // Create ordered JSON with __order field
    let mut keys: Vec<&String> = entries.keys().collect();
    keys.sort();
    
    let mut ordered = BTreeMap::new();
    ordered.insert("__order".to_string(), json!(keys));
    
    for key in &keys {
        if let Some(value) = entries.get(*key) {
            ordered.insert((*key).clone(), json!(value));
        }
    }
    
    // Write to JSON file
    let output_path = get_extract_path(&format!("/Languages/{}.json", locale), None);
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)?;
    }
    
    let json_content = serde_json::to_string_pretty(&ordered)?;
    fs::write(&output_path, json_content)?;
    
    println!(
        "  ✓ {} strings -> {}",
        keys.len(),
        output_path.to_string_lossy()
    );
    
    Ok(keys.len())
}