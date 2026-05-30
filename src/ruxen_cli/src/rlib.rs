//! .rlib artifact format — ar archives containing object code + type metadata.
//!
//! An `.rlib` file is an `ar` archive containing:
//! - `<name>.o` — compiled object code (Cranelift or LLVM output)
//! - `metadata.json` — exported types, function signatures, traits
//! - `hash` — SHA-256 of the source files

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::Path;

/// The compiler version embedded in metadata.
pub const COMPILER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Type metadata stored in .rlib archives.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypeMetadata {
    pub compiler_version: String,
    pub name: String,
    pub version: String,
    pub exports: Exports,
}

/// Exported public API of a piece.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Exports {
    #[serde(default)]
    pub types: Vec<ExportedType>,
    #[serde(default)]
    pub functions: Vec<ExportedFunction>,
    #[serde(default)]
    pub traits: Vec<ExportedTrait>,
    #[serde(default)]
    pub modules: std::collections::BTreeMap<String, Exports>,
}

/// An exported type (class or struct).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportedType {
    pub name: String,
    pub kind: String,
    #[serde(default)]
    pub fields: Vec<ExportedField>,
    #[serde(default)]
    pub methods: Vec<ExportedFunction>,
}

/// An exported field.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportedField {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
    pub visibility: String,
}

/// An exported function or method.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportedFunction {
    pub name: String,
    #[serde(default)]
    pub params: Vec<ExportedParam>,
    pub return_type: String,
    pub visibility: String,
}

/// An exported function parameter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportedParam {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
}

/// An exported trait.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportedTrait {
    pub name: String,
    #[serde(default)]
    pub methods: Vec<ExportedFunction>,
}

/// Create an .rlib archive from object code and type metadata.
pub fn create_rlib(
    output_path: &Path,
    name: &str,
    object_bytes: &[u8],
    metadata: &TypeMetadata,
    source_hash: &str,
) -> Result<(), String> {
    let file =
        std::fs::File::create(output_path).map_err(|e| format!("failed to create .rlib: {}", e))?;

    let mut builder = ar::Builder::new(file);

    // Add object code
    let obj_name = format!("{}.o", name);
    let mut obj_header = ar::Header::new(obj_name.as_bytes().to_vec(), object_bytes.len() as u64);
    obj_header.set_mode(0o644);
    builder
        .append(&obj_header, object_bytes)
        .map_err(|e| format!("failed to write object code to .rlib: {}", e))?;

    // Add metadata.json
    let metadata_json = serde_json::to_string_pretty(metadata)
        .map_err(|e| format!("failed to serialize metadata: {}", e))?;
    let metadata_bytes = metadata_json.as_bytes();
    let mut meta_header = ar::Header::new(b"metadata.json".to_vec(), metadata_bytes.len() as u64);
    meta_header.set_mode(0o644);
    builder
        .append(&meta_header, metadata_bytes)
        .map_err(|e| format!("failed to write metadata to .rlib: {}", e))?;

    // Add hash
    let hash_bytes = source_hash.as_bytes();
    let mut hash_header = ar::Header::new(b"hash".to_vec(), hash_bytes.len() as u64);
    hash_header.set_mode(0o644);
    builder
        .append(&hash_header, hash_bytes)
        .map_err(|e| format!("failed to write hash to .rlib: {}", e))?;

    Ok(())
}

/// Create an .rmeta file (metadata only, no object code) for `ruxen check`.
pub fn create_rmeta(output_path: &Path, metadata: &TypeMetadata) -> Result<(), String> {
    let file = std::fs::File::create(output_path)
        .map_err(|e| format!("failed to create .rmeta: {}", e))?;

    let mut builder = ar::Builder::new(file);

    let metadata_json = serde_json::to_string_pretty(metadata)
        .map_err(|e| format!("failed to serialize metadata: {}", e))?;
    let metadata_bytes = metadata_json.as_bytes();
    let mut meta_header = ar::Header::new(b"metadata.json".to_vec(), metadata_bytes.len() as u64);
    meta_header.set_mode(0o644);
    builder
        .append(&meta_header, metadata_bytes)
        .map_err(|e| format!("failed to write metadata to .rmeta: {}", e))?;

    Ok(())
}

/// Load type metadata from a compiled .rlib file.
pub fn load_rlib_metadata(path: &Path) -> Result<TypeMetadata, String> {
    let file = std::fs::File::open(path)
        .map_err(|e| format!("failed to open .rlib '{}': {}", path.display(), e))?;

    let mut archive = ar::Archive::new(file);
    while let Some(entry) = archive.next_entry() {
        let mut entry = entry.map_err(|e| format!("failed to read .rlib entry: {}", e))?;
        let name = std::str::from_utf8(entry.header().identifier())
            .unwrap_or("")
            .to_string();

        if name == "metadata.json" {
            let mut contents = String::new();
            entry
                .read_to_string(&mut contents)
                .map_err(|e| format!("failed to read metadata from .rlib: {}", e))?;

            let metadata: TypeMetadata = serde_json::from_str(&contents)
                .map_err(|e| format!("failed to parse metadata from .rlib: {}", e))?;

            // Verify compiler version
            if metadata.compiler_version != COMPILER_VERSION {
                return Err(format!(
                    "incompatible .rlib: compiled with ruxenc {} but current compiler is {}",
                    metadata.compiler_version, COMPILER_VERSION
                ));
            }

            return Ok(metadata);
        }
    }

    Err(format!(
        "metadata.json not found in .rlib '{}'",
        path.display()
    ))
}

/// Load type metadata from a .rmeta file (check-only metadata).
pub fn load_rmeta_metadata(path: &Path) -> Result<TypeMetadata, String> {
    // Same format as rlib, just without object code
    load_rlib_metadata(path)
}

/// Extract object code from an .rlib file.
pub fn extract_object_code(path: &Path) -> Result<Vec<u8>, String> {
    let file = std::fs::File::open(path)
        .map_err(|e| format!("failed to open .rlib '{}': {}", path.display(), e))?;

    let mut archive = ar::Archive::new(file);
    while let Some(entry) = archive.next_entry() {
        let mut entry = entry.map_err(|e| format!("failed to read .rlib entry: {}", e))?;
        let name = std::str::from_utf8(entry.header().identifier())
            .unwrap_or("")
            .to_string();

        if name.ends_with(".o") {
            let mut bytes = Vec::new();
            entry
                .read_to_end(&mut bytes)
                .map_err(|e| format!("failed to read object code from .rlib: {}", e))?;
            return Ok(bytes);
        }
    }

    Err(format!(
        "object code not found in .rlib '{}'",
        path.display()
    ))
}

/// Extract the source hash from an .rlib file.
pub fn extract_hash(path: &Path) -> Result<String, String> {
    let file = std::fs::File::open(path)
        .map_err(|e| format!("failed to open .rlib '{}': {}", path.display(), e))?;

    let mut archive = ar::Archive::new(file);
    while let Some(entry) = archive.next_entry() {
        let mut entry = entry.map_err(|e| format!("failed to read .rlib entry: {}", e))?;
        let name = std::str::from_utf8(entry.header().identifier())
            .unwrap_or("")
            .to_string();

        if name == "hash" {
            let mut contents = String::new();
            entry
                .read_to_string(&mut contents)
                .map_err(|e| format!("failed to read hash from .rlib: {}", e))?;
            return Ok(contents);
        }
    }

    Err(format!("hash not found in .rlib '{}'", path.display()))
}

/// Compute SHA-256 hash of a directory's .rx source files.
pub fn hash_sources(dir: &Path) -> Result<String, String> {
    let mut hasher = Sha256::new();
    hash_dir_recursive(dir, &mut hasher)?;
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

fn hash_dir_recursive(dir: &Path, hasher: &mut Sha256) -> Result<(), String> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| format!("failed to read directory: {}", e))?
        .filter_map(|e| e.ok())
        .collect();

    // Sort for deterministic hashing
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            let name = path.file_name().unwrap().to_string_lossy();
            if !name.starts_with('.') && name != "target" {
                hash_dir_recursive(&path, hasher)?;
            }
        } else if path.extension().is_some_and(|ext| ext == "rx") {
            let content = std::fs::read(&path)
                .map_err(|e| format!("failed to read {}: {}", path.display(), e))?;
            // Include relative path in hash so renames are detected
            hasher.update(path.to_string_lossy().as_bytes());
            hasher.update(&content);
        }
    }

    Ok(())
}

/// Compute SHA-256 hash of a single file or byte slice.
pub fn hash_bytes(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("sha256:{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_metadata_serialization() {
        let metadata = TypeMetadata {
            compiler_version: COMPILER_VERSION.to_string(),
            name: "http".to_string(),
            version: "1.0.0".to_string(),
            exports: Exports {
                types: vec![ExportedType {
                    name: "Request".to_string(),
                    kind: "class".to_string(),
                    fields: vec![ExportedField {
                        name: "url".to_string(),
                        ty: "String".to_string(),
                        visibility: "public".to_string(),
                    }],
                    methods: vec![],
                }],
                functions: vec![ExportedFunction {
                    name: "get".to_string(),
                    params: vec![ExportedParam {
                        name: "url".to_string(),
                        ty: "String".to_string(),
                    }],
                    return_type: "Response".to_string(),
                    visibility: "public".to_string(),
                }],
                traits: vec![],
                modules: Default::default(),
            },
        };

        let json = serde_json::to_string_pretty(&metadata).unwrap();
        let reparsed: TypeMetadata = serde_json::from_str(&json).unwrap();
        assert_eq!(reparsed.name, "http");
        assert_eq!(reparsed.exports.types.len(), 1);
        assert_eq!(reparsed.exports.functions.len(), 1);
    }

    #[test]
    fn test_rlib_roundtrip() {
        let tmp = std::env::temp_dir().join(format!("ruxen_rlib_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();

        let rlib_path = tmp.join("test.rlib");
        let object_bytes = b"fake object code";
        let metadata = TypeMetadata {
            compiler_version: COMPILER_VERSION.to_string(),
            name: "test".to_string(),
            version: "0.1.0".to_string(),
            exports: Exports::default(),
        };
        let source_hash = "sha256:abc123";

        create_rlib(&rlib_path, "test", object_bytes, &metadata, source_hash).unwrap();
        assert!(rlib_path.exists());

        // Load metadata back
        let loaded = load_rlib_metadata(&rlib_path).unwrap();
        assert_eq!(loaded.name, "test");
        assert_eq!(loaded.version, "0.1.0");

        // Extract object code
        let obj = extract_object_code(&rlib_path).unwrap();
        assert_eq!(obj, b"fake object code");

        // Extract hash
        let hash = extract_hash(&rlib_path).unwrap();
        assert_eq!(hash, "sha256:abc123");

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_rmeta_creation() {
        let tmp = std::env::temp_dir().join(format!("ruxen_rmeta_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();

        let rmeta_path = tmp.join("test.rmeta");
        let metadata = TypeMetadata {
            compiler_version: COMPILER_VERSION.to_string(),
            name: "test".to_string(),
            version: "0.1.0".to_string(),
            exports: Exports::default(),
        };

        create_rmeta(&rmeta_path, &metadata).unwrap();
        assert!(rmeta_path.exists());

        let loaded = load_rmeta_metadata(&rmeta_path).unwrap();
        assert_eq!(loaded.name, "test");

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_hash_bytes() {
        let h1 = hash_bytes(b"hello");
        let h2 = hash_bytes(b"hello");
        let h3 = hash_bytes(b"world");
        assert_eq!(h1, h2);
        assert_ne!(h1, h3);
        assert!(h1.starts_with("sha256:"));
    }

    #[test]
    fn test_hash_sources() {
        let tmp = std::env::temp_dir().join(format!("ruxen_hash_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("src")).unwrap();

        fs::write(tmp.join("src/main.rx"), "def main\nend\n").unwrap();
        fs::write(tmp.join("src/utils.rx"), "pub def helper\nend\n").unwrap();

        let h1 = hash_sources(&tmp).unwrap();
        assert!(h1.starts_with("sha256:"));

        // Same content should produce same hash
        let h2 = hash_sources(&tmp).unwrap();
        assert_eq!(h1, h2);

        // Changing content should change hash
        fs::write(tmp.join("src/utils.rx"), "pub def other\nend\n").unwrap();
        let h3 = hash_sources(&tmp).unwrap();
        assert_ne!(h1, h3);

        let _ = fs::remove_dir_all(&tmp);
    }
}
