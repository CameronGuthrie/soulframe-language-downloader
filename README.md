# Soulframe Language Downloader

Download and extract Soulframe language files from the official CDN. Built with Rust for performance and cross-platform compatibility.

## Features

- Download language files from Soulframe CDN
- Extract compressed SHCC containers using Oodle decompression
- Decompress language dictionaries using ZSTD
- Convert language data to JSON format
- Support for multiple locales

## Requirements

- Rust 1.70+ (2021 edition)
- Oodle compression library (`oo2core_9.dll` on Windows, `oo2core_9.so` on Linux)
- ZSTD library (`libzstd.dll` on Windows, `libzstd.so` on Linux)

## Setup

### Prerequisites

You must have the following libraries available in the `lib/` directory:
- `oo2core_9.dll` (Windows) or `oo2core_9.so` (Linux) - for SHCC decompression
- `libzstd.dll` (Windows) or `libzstd.so` (Linux) - for language file extraction

### Quick Start

1. **Obtain required DLLs** and place them in the `lib/` directory:
   - `oo2core_9.dll` (Windows) - Oodle compression library
   - `libzstd.dll` (Windows) - ZSTD compression library
   
   ```bash
   mkdir lib
   # Copy your DLLs to ./lib/
   ```

2. **Build the project**:
   ```bash
   cargo build --release
   ```
   
   The build script automatically copies the DLLs from `./lib/` to `./target/release/lib/` so the executables can find them.

3. **Download and extract language files**:
   ```bash
   # Run from any directory - all locales by default
   .\target\release\download.exe
   .\target\release\extract.exe
   
   # Or specify specific locales
   .\target\release\download.exe --locales en,fr,de
   .\target\release\extract.exe --locales en,fr,de
   ```

### Deployment

To deploy the executables to another location, copy both the `.exe` files and the `lib/` folder:

```bash
# Copy executables
copy .\target\release\download.exe <destination>\
copy .\target\release\extract.exe <destination>\

# Copy DLL dependencies
xcopy /E /I .\target\release\lib <destination>\lib
```

The executables will look for DLLs in a `lib/` folder next to themselves.

## Usage

### Download Language Files

Download language files from the Soulframe CDN:

```bash
.\target\release\download.exe
```

Or specify specific locales:

```bash
.\target\release\download.exe --locales "en,fr,de,ja"
```

The downloader will:
- Fetch the primary manifest (`H.Cache.bin`)
- Download localized manifests (`B.Cache.Windows_*.bin`) for each locale
- Download `Languages.bin` files using hashes from the localized manifests
- Verify file integrity using MD5 hashes from the manifest
- Skip re-downloading files that already exist with correct hashes

### Extract Language Files

Extract downloaded language files to JSON:

```bash
.\target\release\extract.exe
```

Or specify specific locales:

```bash
.\target\release\extract.exe --locales "en,fr,de,ja"
```

The extractor will:
- Parse the `Languages.bin_H` file for each locale
- Decompress ZSTD-compressed string data using the embedded dictionary
- Output a JSON file with all localized strings in sorted order
- Generate 62,493+ strings per locale

## Supported Locales

The following locales are supported by default:
- `en` (English)
- `fr` (French) 
- `de` (German)
- `es` (Spanish)
- `it` (Italian)
- `pt` (Portuguese)
- `ru` (Russian)
- `pl` (Polish)
- `tr` (Turkish)
- `ja` (Japanese)
- `ko` (Korean)
- `zh` (Chinese)

## Output Structure

### Downloaded Files
```
downloaded-data/
├── 0/
│   ├── H.Cache.bin_H
│   └── B.Cache.Windows_*.bin_H
└── 0_<locale>/
    └── Languages.bin_H
```

### Extracted Files
```
extracted-data/
└── 0/
    └── Languages/
        ├── en.json
        ├── fr.json
        ├── de.json
        ├── ...
        └── Languages.json (alias to en.json or first available)
```

## Command Line Options

Both `download` and `extract` commands support the following options:

- `--locales, -l <LOCALES>`: Comma-separated list of locales to process
- `--help, -h`: Show help information

## Troubleshooting

### Missing DLL Errors

If you get errors like "Failed to load Oodle library" or "Failed to load Zstd library", ensure:
1. The DLLs are present in `./lib/` directory
2. On Windows, you may need Visual C++ Redistributable installed
3. The DLL versions are compatible with your system architecture (x64)

### Download Failures

If downloads fail with HTTP errors:
1. Check your internet connection
2. Verify the CDN is accessible: `curl https://content.soulframe.com`
3. Try again later - the CDN may be temporarily unavailable

### Different String Counts

If your extracted JSON has a different number of strings than expected (62,493 for most locales), verify:
1. The download completed successfully without errors
2. The source `Languages.bin_H` file is complete and not corrupted
3. You're using compatible versions of the Oodle and ZSTD libraries

## Implementation Notes

This Rust implementation faithfully replicates the original functionality:
- **SHCC unpacking**: Parses chunk headers, supports type 0 (uncompressed) and type 2 (Oodle compressed) chunks
- **Oodle decompression**: FFI bindings to `OodleLZ_Decompress` with proper parameter marshalling
- **Manifest parsing**: Reads path entries, hashes, and validates existing files before re-downloading
- **ZSTD decompression**: Dictionary-based decompression for language strings (flag 0x200)
- **JSON output**: Ordered keys with `__order` array for deterministic output
- **Base64m encoding**: Custom unpadded base64 variant replacing `/` with `-` for URL-safe hashes

### Key Technical Details

- **Base64 encoding**: Uses `BASE64_STANDARD_NO_PAD` - padding (`=`) must be removed for CDN URLs
- **Hash verification**: MD5 hashes from manifest are checked against downloaded file headers
- **URL construction**: Primary format is `https://content.soulframe.com/0[_locale]/path!TYPE_hash`
- **Manifest structure**: Binary format with 4-byte length prefixes, 16-byte MD5 hashes, 4-byte metadata

## Error Handling

The Rust version provides better error handling with detailed error messages using the `anyhow` crate. All functions return `Result` types for proper error propagation.

## Performance

The Rust version offers several performance improvements:
- Memory-safe operations without runtime overhead
- Efficient binary parsing using `byteorder`
- Streaming decompression for large files
- Parallel processing potential (can be extended)

## Cross-Platform Support

This tool supports multiple platforms:
- Windows (x64)
- Linux (x64) 
- macOS (x64, ARM64 with appropriate libraries)

## Building from Source

```bash
# Clone and build
git clone <repository>
cd soulframe-language-downloader/rust
cargo build --release

# Run tests
cargo test

# Install binaries
cargo install --path .
```

## Dependencies

Key dependencies include:
- `reqwest`: HTTP client for CDN downloads
- `clap`: Command-line argument parsing
- `serde_json`: JSON serialization
- `zstd`: ZSTD compression support
- `libloading`: Dynamic library loading
- `anyhow`: Error handling

## License

This project is provided as-is for educational and research purposes.