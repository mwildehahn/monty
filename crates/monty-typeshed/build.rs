//! Build script to package our vendored typeshed files
//! into a zip archive that can be included in the Ruff binary.
//!
//! This script should be automatically run at build time
//! whenever the script itself changes, or whenever any files
//! in `crates/ty_vendored/vendor/typeshed` change.
#![expect(clippy::unnecessary_debug_formatting)]

use std::{
    collections::BTreeSet,
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
};

use path_slash::PathExt;
use zip::{
    CompressionMethod,
    result::ZipResult,
    write::{FileOptions, ZipWriter},
};

const TYPESHED_SOURCE_DIR: &str = "vendor/typeshed";
const CUSTOM_STUBS_DIR: &str = "custom";
// const TY_EXTENSIONS_STUBS: &str = "ty_extensions/ty_extensions.pyi";
const TYPESHED_ZIP_LOCATION: &str = "/zipped_typeshed.zip";

/// Metadata for a custom stub file that should be included in the packaged
/// stdlib stubs.
///
/// Custom stubs are stored outside `vendor/typeshed` so they can override or
/// extend upstream stubs for modules where Monty intentionally differs.
#[derive(Debug)]
struct CustomStub {
    /// Absolute path to the custom stub file in this crate.
    absolute_path: PathBuf,
    /// Path relative to `CUSTOM_STUBS_DIR` using `/` separators.
    relative_path: String,
    /// Python module name derived from `relative_path`.
    module_name: String,
}

/// Converts a relative stub path into a Python module name.
///
/// Examples:
/// - `datetime.pyi` -> `datetime`
/// - `pathlib/__init__.pyi` -> `pathlib`
/// - `pkg/mod.pyi` -> `pkg.mod`
fn module_name_from_relative_stub_path(relative_path: &str) -> String {
    let module_path = relative_path
        .strip_suffix(".pyi")
        .expect("custom stubs must end with .pyi");
    let module_path = module_path.strip_suffix("/__init__").unwrap_or(module_path);
    module_path.replace('/', ".")
}

/// Discovers custom stub files in `CUSTOM_STUBS_DIR`.
fn collect_custom_stubs() -> Vec<CustomStub> {
    let mut stubs = Vec::new();
    for entry in walkdir::WalkDir::new(CUSTOM_STUBS_DIR) {
        let dir_entry = entry.unwrap();
        let absolute_path = dir_entry.path();
        if !absolute_path.is_file() || absolute_path.extension().is_none_or(|ext| ext != "pyi") {
            continue;
        }
        let relative_path = absolute_path
            .strip_prefix(Path::new(CUSTOM_STUBS_DIR))
            .unwrap()
            .to_slash()
            .expect("Unexpected non-utf8 custom stub path!")
            .into_owned();
        let module_name = module_name_from_relative_stub_path(&relative_path);
        stubs.push(CustomStub {
            absolute_path: absolute_path.to_path_buf(),
            relative_path,
            module_name,
        });
    }
    stubs.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    stubs
}

/// Recursively zip the contents of the entire typeshed directory and patch typeshed
/// on the fly to include the `ty_extensions` module.
///
/// This routine is adapted from a recipe at
/// <https://github.com/zip-rs/zip-old/blob/5d0f198124946b7be4e5969719a7f29f363118cd/examples/write_dir.rs>
fn write_zipped_typeshed_to(writer: File) -> ZipResult<File> {
    let mut zip = ZipWriter::new(writer);
    let custom_stubs = collect_custom_stubs();
    let custom_stub_paths: BTreeSet<&str> = custom_stubs.iter().map(|stub| stub.relative_path.as_str()).collect();
    let custom_stub_modules: BTreeSet<&str> = custom_stubs.iter().map(|stub| stub.module_name.as_str()).collect();

    // Use deflated compression for WASM builds because compiling `zstd-sys` requires clang
    // [source](https://github.com/gyscos/zstd-rs/wiki/Compile-for-WASM) which complicates the build
    // by a lot. Deflated compression is slower but it shouldn't matter much for the WASM use case
    // (WASM itself is already slower than a native build for a specific platform).
    // We can't use `#[cfg(...)]` here because the target-arch in a build script is the
    // architecture of the system running the build script and not the architecture of the build-target.
    // That's why we use the `TARGET` environment variable here.
    let method = if cfg!(feature = "zstd") {
        CompressionMethod::Zstd
    } else if cfg!(feature = "deflate") {
        CompressionMethod::Deflated
    } else {
        CompressionMethod::Stored
    };

    let options = FileOptions::default()
        .compression_method(method)
        .unix_permissions(0o644);

    for entry in walkdir::WalkDir::new(TYPESHED_SOURCE_DIR) {
        let dir_entry = entry.unwrap();
        let absolute_path = dir_entry.path();
        let normalized_relative_path = absolute_path
            .strip_prefix(Path::new(TYPESHED_SOURCE_DIR))
            .unwrap()
            .to_slash()
            .expect("Unexpected non-utf8 typeshed path!");

        // Write file or directory explicitly
        // Some unzip tools unzip files with directory paths correctly, some do not!
        if absolute_path.is_file() {
            if let Some(stdlib_relative_path) = normalized_relative_path.strip_prefix("stdlib/")
                && custom_stub_paths.contains(stdlib_relative_path)
            {
                println!(
                    "skipping vendored file {absolute_path:?} as {normalized_relative_path:?}; overridden by custom stub"
                );
                continue;
            }

            println!("adding file {absolute_path:?} as {normalized_relative_path:?} ...");
            zip.start_file(&*normalized_relative_path, options)?;

            // Patch the VERSIONS file to make `ty_extensions` available
            if normalized_relative_path == "stdlib/VERSIONS" {
                let mut versions = String::new();
                let mut versions_file = File::open(absolute_path)?;
                versions_file.read_to_string(&mut versions).unwrap();
                zip.write_all(versions.as_bytes())?;
                writeln!(&mut zip, "ty_extensions: 3.0-")?;
                let existing_modules: BTreeSet<&str> = versions
                    .lines()
                    .filter_map(|line| line.split_once(':').map(|(module, _)| module.trim()))
                    .collect();
                for module_name in &custom_stub_modules {
                    if !existing_modules.contains(module_name) {
                        writeln!(&mut zip, "{module_name}: 3.0-")?;
                    }
                }
            } else {
                let mut f = File::open(absolute_path)?;
                std::io::copy(&mut f, &mut zip).unwrap();
            }
        } else if !normalized_relative_path.is_empty() {
            // Only if not root! Avoids path spec / warning
            // and mapname conversion failed error on unzip
            println!("adding dir {absolute_path:?} as {normalized_relative_path:?} ...");
            zip.add_directory(normalized_relative_path, options)?;
        }
    }

    for custom_stub in &custom_stubs {
        let zip_path = format!("stdlib/{}", custom_stub.relative_path);
        println!("adding custom stub {:?} as {zip_path:?} ...", custom_stub.absolute_path);
        zip.start_file(zip_path, options)?;
        let mut file = File::open(&custom_stub.absolute_path)?;
        std::io::copy(&mut file, &mut zip).unwrap();
    }

    // // Patch typeshed and add the stubs for the `ty_extensions` module
    // println!("adding file {TY_EXTENSIONS_STUBS} as stdlib/ty_extensions.pyi ...");
    // zip.start_file("stdlib/ty_extensions.pyi", options)?;
    // let mut f = File::open(TY_EXTENSIONS_STUBS)?;
    // std::io::copy(&mut f, &mut zip).unwrap();

    zip.finish()
}

fn main() {
    assert!(Path::new(TYPESHED_SOURCE_DIR).is_dir(), "Where is typeshed?");
    assert!(Path::new(CUSTOM_STUBS_DIR).is_dir(), "Where are custom stubs?");
    let out_dir = std::env::var("OUT_DIR").unwrap();

    // N.B. Deliberately using `format!()` instead of `Path::join()` here,
    // so that we use `/` as a path separator on all platforms.
    // That enables us to load the typeshed zip at compile time in `module.rs`
    // (otherwise we'd have to dynamically determine the exact path to the typeshed zip
    // based on the default path separator for the specific platform we're on,
    // which can't be done at compile time.)
    let zipped_typeshed_location = format!("{out_dir}{TYPESHED_ZIP_LOCATION}");

    let zipped_typeshed_file = File::create(zipped_typeshed_location).unwrap();
    write_zipped_typeshed_to(zipped_typeshed_file).unwrap();
}
