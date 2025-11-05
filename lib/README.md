# Required Native Libraries

This directory should contain the following libraries for the Soulframe language downloader to work:

## Windows (x64)
- `oo2core_9.dll` - Oodle compression library (for SHCC decompression)
- `libzstd.dll` - ZSTD compression library (for language dictionary decompression)

## Linux (x64)
- `oo2core_9.so` - Oodle compression library
- `libzstd.so` - ZSTD compression library

## macOS (x64/ARM64)
- `oo2core_9.dylib` - Oodle compression library
- `libzstd.dylib` - ZSTD compression library

## Where to Get These Libraries

### oo2core_9
This library is typically found in:
- Existing Warframe/Soulframe game installations
- Warframe cache tools repositories
- Game data extraction tools

### libzstd
This library can be obtained from:
- System package managers (apt, brew, pacman, etc.)
- Official ZSTD releases: https://github.com/facebook/zstd/releases
- Pre-compiled binaries for your platform

## Installation

1. Copy the appropriate libraries for your platform to this directory
2. Make sure the libraries are executable (on Unix systems: `chmod +x *.so` or `chmod +x *.dylib`)
3. Run `cargo build` to verify the libraries are found

## Notes

- The build script will warn you if libraries are missing
- The application will fail at runtime if libraries cannot be loaded
- Make sure to use the correct architecture (x64) versions of the libraries