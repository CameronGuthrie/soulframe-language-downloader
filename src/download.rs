use anyhow::{anyhow, Result};
use rand::Rng;
use soulframe_language_downloader::*;
use std::collections::HashMap;
use std::fs;

pub struct DownloadClient {
    client: reqwest::blocking::Client,
}

impl DownloadClient {
    pub fn new() -> Self {
        Self {
            client: reqwest::blocking::Client::new(),
        }
    }

    pub fn download_soulframe_file(
        &self,
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
            
            match self.client.get(&url).send() {
                Ok(response) if response.status().is_success() => {
                    println!("Successfully downloaded from {}", url);
                    
                    let mut bin = response.bytes()?.to_vec();
                    let local_path = get_download_path(&normalized_path, Some(suffix));
                    
                    // Create parent directories
                    if let Some(parent) = local_path.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    
                    let shcc_itself_compressed = !bin.starts_with(b"SHCC");
                    
                    if shcc_itself_compressed {
                        let oodle = Oodle::new()?;
                        // Estimate decompressed size (the original uses bin size * 10)
                        bin = oodle.decompress(&bin, bin.len() * 10)?;
                    }
                    
                    let oodle = Oodle::new()?;
                    let data = shcc_unpack(&bin, &oodle)?;
                    
                    // Write H data
                    let h_path = format!("{}_H", local_path.to_string_lossy());
                    fs::write(&h_path, &data.h)?;
                    
                    // Write B data if present
                    if let Some(ref b_data) = data.b {
                        let b_path = format!("{}_B", local_path.to_string_lossy());
                        fs::write(&b_path, b_data)?;
                    }
                    
                    // Verify hash if not default
                    if b64m_hash != "---------------------w" && !shcc_itself_compressed {
                        let computed_hash = shcc_hash(&data);
                        let expected_hash = b64m_decode(b64m_hash)?;
                        if computed_hash != expected_hash {
                            return Err(anyhow!("Hash mismatch for {}", normalized_path));
                        }
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
}

pub struct SoulframeManifest {
    bin: Vec<u8>,
    i: usize,
    entry_i: usize,
    remaining_entries: u32,
    paths: Vec<String>,
    hashes: HashMap<String, Vec<u8>>,
    unks: HashMap<String, Vec<u8>>,
}

impl SoulframeManifest {
    pub fn new(path: &str) -> Result<Self> {
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
            unks: HashMap::new(),
        })
    }
    
    pub fn seek(&mut self, opt_stop_at_path: Option<&str>) -> Option<Vec<u8>> {
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
            
            // Read hash (16 bytes) and unk (4 bytes)
            let hash = self.bin[self.i..self.i + 16].to_vec();
            let unk = self.bin[self.i + 16..self.i + 20].to_vec();
            self.i += 20;
            
            self.paths.push(path.clone());
            self.hashes.insert(path.clone(), hash.clone());
            self.unks.insert(path.clone(), unk);
            
            if let Some(target_path) = opt_stop_at_path {
                if path == target_path {
                    return Some(hash);
                }
            }
        }
        
        None
    }
    
    pub fn get_hash(&mut self, path: &str) -> Option<Vec<u8>> {
        if let Some(hash) = self.hashes.get(path) {
            return Some(hash.clone());
        }
        
        self.seek(Some(path))
    }
    
    pub fn get_paths(&mut self) -> Vec<String> {
        self.seek(None);
        self.paths.clone()
    }
    
    pub fn download_file(&mut self, path: &str, file_type: u8, suffix: Option<&str>, client: &DownloadClient) -> Result<()> {
        let manifest_hash = self.get_hash(path)
            .ok_or_else(|| anyhow!("file not in manifest"))?;
        
        let local_path = get_download_path(path, suffix);
        let h_path = format!("{}_H", local_path.to_string_lossy());
        
        let header_hash = fs::read(&h_path).ok()
            .and_then(|contents| contents.get(0..16).map(|slice| slice.to_vec()));
        
        if Some(&manifest_hash) != header_hash.as_ref() {
            let hash_b64 = b64m_encode(&manifest_hash);
            client.download_soulframe_file(path, file_type, Some(&hash_b64), suffix)?;
        }
        
        Ok(())
    }
}
