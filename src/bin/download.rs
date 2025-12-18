use clap::Parser;
use anyhow::{anyhow, Result};
use rand::Rng;
use soulframe_language_downloader::{find_runtime_lib, TYPE_BIN, TYPE_MANIFEST};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::ffi::{c_char, c_int, c_void};
use libloading::{Library, Symbol};

#[derive(Parser)]
#[command(name = "download")]
#[command(about = "Download Soulframe language files from CDN")]
struct Args {
    /// Locales to download (comma-separated)
    #[arg(short, long, default_value = "en,fr,de,es,it,pt,ru,pl,tr,ja,ko,zh")]
    locales: String,
}

fn get_download_path(path: &str, suffix: Option<&str>) -> PathBuf {
    let suffix = suffix.unwrap_or("");
    let root = std::env::current_dir().unwrap();
    root.join("downloaded-data").join(format!("0{}{}", suffix, path))
}

fn b64m_encode(data: &[u8]) -> String {
    use base64::prelude::*;
    BASE64_STANDARD_NO_PAD.encode(data).replace('/', "-")
}

/// Oodle compression library interface
struct Oodle {
    #[allow(dead_code)]
    lib: Library,
    decompress_fn: Symbol<'static, unsafe extern "C" fn(
        *const c_char, usize, *mut c_void, usize,
        c_int, c_int, c_int, usize, usize, usize, usize, usize, usize, c_int
    ) -> c_int>,
}

impl Oodle {
    fn new() -> Result<Self> {
        let lib_name = if cfg!(windows) {
            "oo2core_9.dll"
        } else {
            "oo2core_9.so"
        };

        let lib_path = find_runtime_lib(lib_name)?;
        
        unsafe {
            let lib = Library::new(&lib_path)
                .map_err(|e| anyhow!("Failed to load Oodle library from {:?}: {}", lib_path, e))?;
            
            let decompress_fn: Symbol<unsafe extern "C" fn(
                *const c_char, usize, *mut c_void, usize,
                c_int, c_int, c_int, usize, usize, usize, usize, usize, usize, c_int
            ) -> c_int> = lib.get(b"OodleLZ_Decompress\0")
                .map_err(|e| anyhow!("Failed to get OodleLZ_Decompress function: {}", e))?;
            
            // Extend the lifetime to 'static - this is safe because we keep the library alive
            let decompress_fn: Symbol<'static, _> = std::mem::transmute(decompress_fn);
            
            Ok(Self { lib, decompress_fn })
        }
    }
    
    fn decompress(&self, compressed: &[u8], decompressed_size: usize) -> Result<Vec<u8>> {
        let mut output = vec![0u8; decompressed_size];
        
        unsafe {
            let result = (self.decompress_fn)(
                compressed.as_ptr() as *const c_char,
                compressed.len(),
                output.as_mut_ptr() as *mut c_void,
                decompressed_size,
                0, 0, 0, 0, 0, 0, 0, 0, 0, 3
            );
            
            if result as usize != decompressed_size {
                return Err(anyhow!("Oodle decompression failed"));
            }
        }
        
        Ok(output)
    }
}

#[derive(Debug, Clone)]
struct ShccData {
    h: Vec<u8>,
    b: Option<Vec<u8>>,
}

fn shcc_decompress_chunk_oodle(bin: &[u8], start: usize, decompressed_size: usize, oodle: &Oodle) -> Result<(Vec<u8>, usize)> {
    let mut decompressed = Vec::new();
    let mut i = start;
    
    while decompressed.len() < decompressed_size {
        if i + 8 > bin.len() {
            return Err(anyhow!("Unexpected end of data in SHCC Oodle chunk"));
        }
        
        let block_info = &bin[i..i + 8];
        i += 8;
        
        if block_info[0] != 0x80 {
            return Err(anyhow!("Invalid block header"));
        }
        
        if (block_info[7] & 0x0F) != 0x01 {
            return Err(anyhow!("Invalid block footer"));
        }
        
        let num1 = ((block_info[0] as u32) << 24) | 
                   ((block_info[1] as u32) << 16) | 
                   ((block_info[2] as u32) << 8) | 
                   (block_info[3] as u32);
        let num2 = ((block_info[4] as u32) << 24) | 
                   ((block_info[5] as u32) << 16) | 
                   ((block_info[6] as u32) << 8) | 
                   (block_info[7] as u32);
        
        let block_compressed_size = ((num1 >> 2) & 0xFFFFFF) as usize;
        let block_decompressed_size = ((num2 >> 5) & 0xFFFFFF) as usize;
        
        if i >= bin.len() || bin[i] != 0x8C {
            return Err(anyhow!("Invalid Oodle block marker"));
        }
        
        if i + block_compressed_size > bin.len() {
            return Err(anyhow!("Block compressed size exceeds available data"));
        }
        
        let block_data = oodle.decompress(&bin[i..i + block_compressed_size], block_decompressed_size)?;
        decompressed.extend_from_slice(&block_data);
        i += block_compressed_size;
    }
    
    Ok((decompressed, i))
}

fn shcc_decompress_chunk(bin: &[u8], start: usize, oodle: &Oodle) -> Result<(Vec<u8>, usize)> {
    if start + 9 > bin.len() {
        return Err(anyhow!("Not enough data for SHCC chunk header"));
    }
    
    let chunk_type = bin[start];
    let decompressed_size = u32::from_le_bytes([
        bin[start + 1], bin[start + 2], bin[start + 3], bin[start + 4]
    ]) as usize;
    let compressed_size = u32::from_le_bytes([
        bin[start + 5], bin[start + 6], bin[start + 7], bin[start + 8]
    ]) as usize;
    
    let mut i = start + 9;
    
    match chunk_type {
        0 => {
            // Uncompressed
            if compressed_size != decompressed_size {
                return Err(anyhow!("Compressed size mismatch for uncompressed chunk"));
            }
            
            if i + compressed_size > bin.len() {
                return Err(anyhow!("Not enough data for uncompressed chunk"));
            }
            
            let data = bin[i..i + compressed_size].to_vec();
            i += decompressed_size;
            Ok((data, i))
        }
        2 => {
            // Oodle compressed
            shcc_decompress_chunk_oodle(bin, i, decompressed_size, oodle)
        }
        _ => Err(anyhow!("Unknown chunk type: {}", chunk_type))
    }
}

fn shcc_unpack(bin: &[u8], oodle: &Oodle) -> Result<ShccData> {
    if bin.len() < 8 {
        return Err(anyhow!("SHCC data too short"));
    }
    
    let mut i = 8; // Skip initial 8 bytes
    
    // Decompress H chunk
    let (h_data, new_i) = shcc_decompress_chunk(bin, i, oodle)?;
    i = new_i;
    
    // Try to decompress B chunk (optional)
    let b_data = if i < bin.len() {
        match shcc_decompress_chunk(bin, i, oodle) {
            Ok((b, _)) => Some(b),
            Err(_) => None, // B chunk is optional
        }
    } else {
        None
    };
    
    Ok(ShccData {
        h: h_data,
        b: b_data,
    })
}

struct SoulframeManifest {
    bin: Vec<u8>,
    i: usize,
    entry_i: usize,
    remaining_entries: u32,
    paths: Vec<String>,
    hashes: HashMap<String, Vec<u8>>,
}

impl SoulframeManifest {
    fn new(path: &str) -> Result<Self> {
        let file_path = get_download_path(path, None);
        let h_path = format!("{}_H", file_path.to_string_lossy());
        
        let bin = fs::read(&h_path)
            .map_err(|_| anyhow!("{} was not found on disk.", path))?;
        
        Ok(Self {
            bin,
            i: 20, // Skip initial 20 bytes
            entry_i: 0,
            remaining_entries: 0,
            paths: Vec::new(),
            hashes: HashMap::new(),
        })
    }
    
    fn seek(&mut self, opt_stop_at_path: Option<&str>) -> Option<Vec<u8>> {
        while self.i < self.bin.len() {
            while self.remaining_entries == 0 {
                if self.i + 4 > self.bin.len() {
                    return None;
                }
                
                self.remaining_entries = u32::from_le_bytes([
                    self.bin[self.i],
                    self.bin[self.i + 1],
                    self.bin[self.i + 2],
                    self.bin[self.i + 3],
                ]);
                self.i += 4;
            }
            
            self.entry_i += 1;
            self.remaining_entries -= 1;
            
            // Read path (4-byte length prefix + string)
            if self.i + 4 > self.bin.len() {
                break;
            }
            
            let path_len = u32::from_le_bytes([
                self.bin[self.i],
                self.bin[self.i + 1],
                self.bin[self.i + 2],
                self.bin[self.i + 3],
            ]) as usize;
            self.i += 4;
            
            if self.i + path_len + 20 > self.bin.len() {
                break;
            }
            
            let path = String::from_utf8_lossy(&self.bin[self.i..self.i + path_len]).to_string();
            self.i += path_len;
            
            // Read hash (16 bytes) and skip unk (4 bytes)
            let hash = self.bin[self.i..self.i + 16].to_vec();
            self.i += 20; // 16 bytes hash + 4 bytes unk
            
            self.paths.push(path.clone());
            self.hashes.insert(path.clone(), hash.clone());
            
            if let Some(target_path) = opt_stop_at_path {
                if path == target_path {
                    return Some(hash);
                }
            }
        }
        
        None
    }
    
    fn get_hash(&mut self, path: &str) -> Option<Vec<u8>> {
        if let Some(hash) = self.hashes.get(path) {
            return Some(hash.clone());
        }
        
        self.seek(Some(path))
    }
    
    fn download_file(&mut self, path: &str, file_type: u8, suffix: Option<&str>, client: &reqwest::blocking::Client) -> Result<bool> {
        let manifest_hash = self.get_hash(path);
        
        if manifest_hash.is_none() {
            return Err(anyhow!("file not in manifest"));
        }
        
        let manifest_hash = manifest_hash.unwrap();
        
        // Check if file already exists with correct hash
        let local_path = get_download_path(path, suffix);
        let h_path = format!("{}_H", local_path.to_string_lossy());
        
        if let Ok(existing_content) = fs::read(&h_path) {
            if existing_content.len() >= 16 {
                let header_hash = &existing_content[0..16];
                if header_hash == manifest_hash {
                    println!("  File {} already exists with correct hash, skipping download", path);
                    return Ok(true);
                }
            }
        }
        
        let hash_b64 = b64m_encode(&manifest_hash);
        download_soulframe_file(client, path, file_type, Some(&hash_b64), suffix)
    }
}

fn download_soulframe_file(
    client: &reqwest::blocking::Client,
    path: &str,
    file_type: u8,
    b64m_hash: Option<&str>,
    suffix: Option<&str>,
) -> Result<bool> {
    let b64m_hash = b64m_hash.unwrap_or("---------------------w");
    let suffix = suffix.unwrap_or("");
    
    let normalized_path = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{}", path)
    };
    
    let req_path = format!("/0{}{}!{:X}_{}", suffix, normalized_path, file_type, b64m_hash);
    
    let mut urls = Vec::new();
    
    // Prefer the CDN, but include origin endpoints and a cache-busting origin URL as fallbacks.
    urls.push(format!("https://content.soulframe.com{}", req_path));
    urls.push(format!("https://origin.soulframe.com{}", req_path));

    let random_id: u32 = rand::thread_rng().gen();
    urls.push(format!("https://origin.soulframe.com/origin/{:08X}{}", random_id, req_path));
    urls.push(format!("https://origin.soulframe.com/origin/0{}", req_path));
    
    for url in urls {
        println!("Attempting download from {}", url);
        
        match client.get(&url).send() {
            Ok(response) if response.status().is_success() => {
                println!("Successfully downloaded from {}", url);
                
                let bin = response.bytes()?.to_vec();
                let local_path = get_download_path(&normalized_path, Some(suffix));
                
                // Create parent directories
                if let Some(parent) = local_path.parent() {
                    fs::create_dir_all(parent)?;
                }
                
                let shcc_itself_compressed = !bin.starts_with(b"SHCC");
                
                let final_bin = if shcc_itself_compressed {
                    let oodle = Oodle::new()?;
                    // Estimate decompressed size (the original uses bin size * 10)
                    oodle.decompress(&bin, bin.len() * 10)?
                } else {
                    bin
                };
                
                let oodle = Oodle::new()?;
                let data = shcc_unpack(&final_bin, &oodle)?;
                
                // Write H data (the decompressed content)
                let h_path = format!("{}_H", local_path.to_string_lossy());
                fs::write(&h_path, &data.h)?;
                
                // Write B data if present
                if let Some(ref b_data) = data.b {
                    let b_path = format!("{}_B", local_path.to_string_lossy());
                    fs::write(&b_path, b_data)?;
                }
                
                return Ok(true);
            }
            Ok(response) => {
                println!(
                    "Download failed from {} (HTTP {})",
                    url,
                    response.status().as_u16()
                );
            }
            Err(e) => {
                println!("Download failed from {}: {}", url, e);
            }
        }
    }
    
    println!("All download attempts failed for {}", normalized_path);
    Ok(false)
}

fn main() -> Result<()> {
    let args = Args::parse();
    
    println!("=== Soulframe Language Downloader ===");
    
    // Parse locales
    let locales: Vec<String> = args.locales
        .split(',')
        .map(|s| s.trim().to_string())
        .collect();
    
    // Create download client - use HTTP/1.1 only and disable automatic decompression
    let client = reqwest::blocking::Client::builder()
        .http1_only()
        .no_gzip()
        .no_brotli()
        .no_deflate()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;
    
    // Ensure base folders exist
    let marker_path = get_download_path("/marker", None);
    if let Some(parent) = marker_path.parent() {
        fs::create_dir_all(parent)?;
    }
    
    // Download primary manifest
    println!("Downloading primary manifest /H.Cache.bin ...");
    if !download_soulframe_file(&client, "/H.Cache.bin", TYPE_MANIFEST, None, None)? {
        println!("x Failed to download /H.Cache.bin");
        return Ok(());
    }
    
    // Load primary manifest
    let mut meta = SoulframeManifest::new("/H.Cache.bin")?;
    
    // Parse all manifest entries
    meta.seek(None);
    println!("Primary manifest loaded with {} files", meta.paths.len());
    
    // Process each locale
    for lang in locales {
        println!("\n--- Locale: {} ---", lang);
        
        // Try to download localized main manifest; fall back to global if missing
        let localized_manifest = format!("/B.Cache.Windows_{}.bin", lang);
        let mut have_localized_manifest = false;
        match meta.download_file(&localized_manifest, TYPE_MANIFEST, None, &client) {
            Ok(true) => {
                println!("  Localized manifest ready for {}", lang);
                have_localized_manifest = true;
            }
            Ok(false) => {
                println!("  x Failed to obtain localized manifest for {}", lang);
            }
            Err(_) => {
                println!("  (no localized manifest entry in primary manifest)");
            }
        }

    // Try to use the localized manifest (either just downloaded or already existing on disk)
    let localized_manifest_h = format!("{}_H", get_download_path(&localized_manifest, None).to_string_lossy());
    match if have_localized_manifest || fs::metadata(&localized_manifest_h).is_ok() { SoulframeManifest::new(&localized_manifest) } else { Err(anyhow!("{} was not found on disk.", &localized_manifest)) } {
            Ok(mut localized_man) => {
                println!("  Using localized manifest for {}", lang);
                let suffix = format!("_{}", lang);
                match localized_man.download_file("/Languages.bin", TYPE_BIN, Some(&suffix), &client) {
                    Ok(true) => {
                        println!("  ✓ Languages.bin downloaded for {}", lang);
                    }
                    Ok(false) => {
                        println!("  x Languages.bin failed for {}", lang);
                    }
                    Err(err) => {
                        println!("  x Languages.bin failed for {}: {}", lang, err);
                    }
                }
            }
            Err(err) => {
                println!("  x Cannot load manifest for {}: {}", lang, err);
            }
        }
    }
    
    println!("\n✓ Download complete! Files saved to ./downloaded-data/");
    println!("Run 'extract' to convert Languages.bin files to JSON.");
    
    Ok(())
}
