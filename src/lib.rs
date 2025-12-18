use anyhow::{anyhow, Result};
use base64::prelude::*;
use libloading::{Library, Symbol};
use std::ffi::{c_char, c_int, c_void};
use std::path::PathBuf;
use std::{collections::HashSet, env};

// This library provides core functionality that can be used by the binaries
// For now, we'll keep it minimal to avoid import issues

// Manifest type IDs changed with Soulframe 40.0.0 (Pluto tool uses 0xE for 40+).
pub const TYPE_MANIFEST: u8 = 0xE;
pub const TYPE_BIN: u8 = 0x2C;

pub fn find_runtime_lib(lib_filename: &str) -> Result<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();

    if let Ok(dir) = env::var("SOULFRAME_LIB_DIR") {
        let base = PathBuf::from(dir);
        candidates.push(base.join(lib_filename));
    }

    if let Ok(exe) = env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            candidates.push(exe_dir.join("lib").join(lib_filename));
            candidates.push(exe_dir.join(lib_filename));

            for ancestor in exe_dir.ancestors().take(8) {
                candidates.push(ancestor.join("lib").join(lib_filename));
            }
        }
    }

    if let Ok(cwd) = env::current_dir() {
        candidates.push(cwd.join("lib").join(lib_filename));
        candidates.push(cwd.join(lib_filename));

        for ancestor in cwd.ancestors().take(8) {
            candidates.push(ancestor.join("lib").join(lib_filename));
        }
    }

    let mut seen = HashSet::new();
    candidates.retain(|p| seen.insert(p.to_path_buf()));

    for candidate in &candidates {
        if candidate.exists() {
            return Ok(candidate.to_path_buf());
        }
    }

    let attempted = candidates
        .into_iter()
        .map(|p| format!("  - {}", p.display()))
        .collect::<Vec<_>>()
        .join("\n");

    Err(anyhow!(
        "Missing required runtime library {lib_filename}. Tried:\n{attempted}\n\
Set SOULFRAME_LIB_DIR to a folder containing the DLL/SO, or place it in ./lib/ next to the executable."
    ))
}

pub fn get_download_path(path: &str, suffix: Option<&str>) -> PathBuf {
    let suffix = suffix.unwrap_or("");
    let root = std::env::current_dir().unwrap();
    root.join("downloaded-data").join(format!("0{}{}", suffix, path))
}

pub fn get_extract_path(path: &str, suffix: Option<&str>) -> PathBuf {
    let suffix = suffix.unwrap_or("");
    let root = std::env::current_dir().unwrap();
    root.join("extracted-data").join(format!("0{}{}", suffix, path))
}

pub fn b64m_encode(data: &[u8]) -> String {
    BASE64_STANDARD_NO_PAD.encode(data).replace('/', "-")
}

pub fn b64m_decode(data: &str) -> Result<Vec<u8>> {
    let normalized = data.replace('-', "/");
    BASE64_STANDARD_NO_PAD.decode(normalized).map_err(|e| anyhow!("Base64 decode error: {}", e))
}

/// Oodle compression library interface
pub struct Oodle {
    #[allow(dead_code)]
    lib: Library,
    decompress_fn: Symbol<'static, unsafe extern "C" fn(
        *const c_char, usize, *mut c_void, usize,
        c_int, c_int, c_int, usize, usize, usize, usize, usize, usize, c_int
    ) -> c_int>,
}

impl Oodle {
    pub fn new() -> Result<Self> {
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
    
    pub fn decompress(&self, compressed: &[u8], decompressed_size: usize) -> Result<Vec<u8>> {
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
pub struct ShccData {
    pub h: Vec<u8>,
    pub b: Option<Vec<u8>>,
    pub b_raw: Option<Vec<u8>>,
}

pub fn shcc_decompress_chunk_oodle(bin: &[u8], start: usize, decompressed_size: usize, oodle: &Oodle) -> Result<(Vec<u8>, usize)> {
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

pub fn shcc_decompress_chunk(bin: &[u8], start: usize, oodle: &Oodle) -> Result<(Vec<u8>, usize)> {
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

pub fn shcc_unpack(bin: &[u8], oodle: &Oodle) -> Result<ShccData> {
    if bin.len() < 8 {
        return Err(anyhow!("SHCC data too short"));
    }
    
    let mut i = 8; // Skip initial 8 bytes
    
    // Decompress H chunk
    let (h_data, new_i) = shcc_decompress_chunk(bin, i, oodle)?;
    i = new_i;
    
    // Try to decompress B chunk
    let mut b_data = None;
    let mut b_raw = None;
    
    if i < bin.len() {
        let b_start = i;
        match shcc_decompress_chunk(bin, i, oodle) {
            Ok((b, _)) => {
                b_data = Some(b);
                // B_raw is the compressed data without the 9-byte header and 15-byte footer
                if b_start + 9 < bin.len() && bin.len() >= 15 {
                    b_raw = Some(bin[b_start + 9..bin.len() - 15].to_vec());
                }
            }
            Err(_) => {
                // B chunk is optional
            }
        }
    }
    
    Ok(ShccData {
        h: h_data,
        b: b_data,
        b_raw,
    })
}

pub fn shcc_hash(data: &ShccData) -> Vec<u8> {
    let mut hasher = md5::Context::new();
    hasher.consume(b"SHCC\x1F\x00\x00\x00");
    
    if data.h.len() >= 17 {
        hasher.consume(&data.h[16..]);
    }
    
    if let Some(ref b_raw) = data.b_raw {
        hasher.consume(b_raw);
    }
    
    hasher.compute().0.to_vec()
}

pub fn unpack_u32_dyn_le(bin: &[u8], start: usize) -> Result<(u32, usize)> {
    let mut value = 0u32;
    let mut i = start;
    let mut shift = 0u32;
    
    while shift < 28 {
        if i >= bin.len() {
            return Err(anyhow!("Unexpected end of data in dynamic u32"));
        }
        
        let byte = bin[i];
        i += 1;
        
        value |= ((byte & 0x7f) as u32) << shift;
        
        if (byte & 0x80) == 0 {
            return Ok((value, i));
        }
        
        shift += 7;
    }
    
    // Handle the final byte
    if i >= bin.len() {
        return Err(anyhow!("Unexpected end of data in dynamic u32 final byte"));
    }
    
    let byte = bin[i];
    i += 1;
    
    if byte > 0xF {
        return Err(anyhow!("Invalid final byte in dynamic u32: {}", byte));
    }
    
    value |= (byte as u32) << shift;
    
    Ok((value, i))
}
