use std::path::{Path, PathBuf};

use derive_getters::Getters;
use ignore::{
    Walk, WalkBuilder,
    types::{Types, TypesBuilder},
};
use miette::{IntoDiagnostic as _, Result, miette};

use tracing::debug;

use crate::fs::manifest::manifest_filenames;

#[derive(Getters, Clone, Debug)]
pub(crate) struct CloudflareFileManifest {
    files: Vec<PathBuf>,
}

// TODO: Load in the `excludes` section of the Wranger.toml file and respect those.
// TODO: Determine if we should upload everything in `node_modules` or include
//       that as part of the build step.
impl CloudflareFileManifest {
    /// Build a new Manifest using the given root directory.
    pub async fn new<P: AsRef<Path>>(root: P) -> Result<Self> {
        debug!("Building Cloudflare manifest");
        let directory = root.as_ref().to_path_buf();

        // We must provide a valid directory.
        if !directory.metadata().is_ok_and(|meta| meta.is_dir()) {
            return Err(miette!(format!(
                "The provided path `{}` is not a valid directory",
                directory.display()
            )));
        }

        let mut files = Vec::new();

        let manifest_filenames = manifest_filenames();

        // Build the file tree walker.
        let walker = walk_builder(directory.clone());

        for entry in walker {
            debug!("Processing entry: {:?}", entry.clone().unwrap().path());
            let file_entry = entry.into_diagnostic()?;

            // Ignore directories, we only want files.
            if file_entry.file_type().map_or(false, |ft| ft.is_dir()) {
                continue;
            }

            let file_path = file_entry.path().to_path_buf();

            // Skip files with names that match manifest filenames
            if let Some(filename) = file_path.file_name().and_then(|n| n.to_str()) {
                if manifest_filenames.contains(&filename.to_string()) {
                    continue;
                }
            }

            files.push(file_path);
        }

        debug!(
            "Finished building Cloudflare manifest with {} files",
            files.len()
        );

        // A manifest with no files should be an error
        if files.is_empty() {
            return Err(miette!(format!(
                "No files found in directory `{}` to upload",
                directory.display()
            )));
        }

        Ok(Self { files })
    }
}

/// Build a file loader that loads web development files.
fn types_matches() -> Types {
    let mut builder = TypesBuilder::new();
    builder.add_defaults();
    builder
        .select("ts")
        .select("js")
        .select("css")
        .select("html");
    builder.build().unwrap()
}

/// Builder the Walker that walks the file tree looking for files.
/// It obeys the type filters we created.
fn walk_builder(dir: PathBuf) -> Walk {
    let types = types_matches();
    WalkBuilder::new(dir)
        .standard_filters(true)
        .parents(false)
        .types(types)
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use std::fs;
    use tempfile::TempDir;

    /// Create a temporary directory with the given structure
    fn create_test_directory(files: &[(&str, &str)]) -> TempDir {
        let temp_dir = TempDir::new().expect("Failed to create temp directory");

        for (path, content) in files {
            let file_path = temp_dir.path().join(path);

            // Create parent directories if they don't exist
            if let Some(parent) = file_path.parent() {
                fs::create_dir_all(parent).expect("Failed to create parent directories");
            }

            fs::write(&file_path, content).expect("Failed to write test file");
        }

        temp_dir
    }

    #[tokio::test]
    async fn test_new_with_valid_js_and_ts_files() {
        let temp_dir = create_test_directory(&[
            ("index.js", "console.log('Hello World');"),
            ("main.ts", "console.log('TypeScript Hello');"),
            ("utils.js", "export function helper() {}"),
        ]);

        let manifest = CloudflareFileManifest::new(temp_dir.path())
            .await
            .expect("Failed to create manifest");

        let files = manifest.files();
        assert_eq!(files.len(), 3);

        // Check that all files are present
        let file_names: Vec<String> = files
            .iter()
            .filter_map(|p| p.file_name().and_then(|n| n.to_str()))
            .map(String::from)
            .collect();

        assert!(file_names.contains(&"index.js".to_string()));
        assert!(file_names.contains(&"main.ts".to_string()));
        assert!(file_names.contains(&"utils.js".to_string()));
    }

    #[tokio::test]
    async fn test_new_excludes_manifest_files() {
        let temp_dir = create_test_directory(&[
            ("index.js", "console.log('Hello World');"),
            ("MultiTool.toml", "name = 'test'"),
            ("MultiTool.json", r#"{"name": "test"}"#),
            ("main.ts", "console.log('TypeScript Hello');"),
        ]);

        let manifest = CloudflareFileManifest::new(temp_dir.path())
            .await
            .expect("Failed to create manifest");

        let files = manifest.files();
        assert_eq!(files.len(), 2);

        let file_names: Vec<String> = files
            .iter()
            .filter_map(|p| p.file_name().and_then(|n| n.to_str()))
            .map(String::from)
            .collect();

        // Should include JS/TS files
        assert!(file_names.contains(&"index.js".to_string()));
        assert!(file_names.contains(&"main.ts".to_string()));

        // Should NOT include manifest files
        assert!(!file_names.contains(&"MultiTool.toml".to_string()));
        assert!(!file_names.contains(&"MultiTool.json".to_string()));
    }

    #[tokio::test]
    async fn test_new_includes_web_files() {
        let temp_dir = create_test_directory(&[
            ("index.js", "console.log('Hello World');"),
            ("style.css", "body { margin: 0; }"),
            ("README.md", "# Test Project"),
            ("config.yaml", "version: 1"),
            ("data.json", r#"{"test": true}"#),
            ("script.ts", "interface Test {}"),
            ("page.html", "<html><body>Hello</body></html>"),
            ("settings.toml", "[section]"),
            ("data.xml", "<root><item>test</item></root>"),
            ("notes.txt", "Some notes here"),
        ]);

        let manifest = CloudflareFileManifest::new(temp_dir.path())
            .await
            .expect("Failed to create manifest");

        let files = manifest.files();
        assert_eq!(files.len(), 4);

        let file_names: Vec<String> = files
            .iter()
            .filter_map(|p| p.file_name().and_then(|n| n.to_str()))
            .map(String::from)
            .collect();

        // Should include web development file types
        assert!(file_names.contains(&"index.js".to_string()));
        assert!(file_names.contains(&"script.ts".to_string()));
        assert!(file_names.contains(&"style.css".to_string()));
        assert!(file_names.contains(&"page.html".to_string()));

        // Should NOT include configuration, text, and JSON files
        assert!(!file_names.contains(&"data.json".to_string()));
        assert!(!file_names.contains(&"README.md".to_string()));
        assert!(!file_names.contains(&"config.yaml".to_string()));
        assert!(!file_names.contains(&"settings.toml".to_string()));
        assert!(!file_names.contains(&"data.xml".to_string()));
        assert!(!file_names.contains(&"notes.txt".to_string()));
    }

    #[tokio::test]
    async fn test_new_with_nested_directories() {
        let temp_dir = create_test_directory(&[
            ("src/index.js", "console.log('Main');"),
            ("src/components/Button.ts", "export class Button {}"),
            ("lib/utils.js", "export const helper = () => {};"),
            ("tests/test.ts", "describe('test', () => {});"),
            ("deep/nested/path/module.js", "export default {};"),
        ]);

        let manifest = CloudflareFileManifest::new(temp_dir.path())
            .await
            .expect("Failed to create manifest");

        let files = manifest.files();
        assert_eq!(files.len(), 5);

        // Check that nested files are included with correct paths
        let paths: Vec<String> = files
            .iter()
            .map(|p| {
                p.strip_prefix(temp_dir.path())
                    .unwrap()
                    .to_string_lossy()
                    .to_string()
            })
            .collect();

        assert!(paths.iter().any(|p| p.ends_with("src/index.js")));
        assert!(
            paths
                .iter()
                .any(|p| p.ends_with("src/components/Button.ts"))
        );
        assert!(paths.iter().any(|p| p.ends_with("lib/utils.js")));
        assert!(paths.iter().any(|p| p.ends_with("tests/test.ts")));
        assert!(
            paths
                .iter()
                .any(|p| p.ends_with("deep/nested/path/module.js"))
        );
    }

    #[tokio::test]
    async fn test_new_ignores_directories() {
        let temp_dir = create_test_directory(&[("index.js", "console.log('Hello World');")]);

        // Create some empty directories
        fs::create_dir_all(temp_dir.path().join("empty_dir")).unwrap();
        fs::create_dir_all(temp_dir.path().join("src/components")).unwrap();

        let manifest = CloudflareFileManifest::new(temp_dir.path())
            .await
            .expect("Failed to create manifest");

        let files = manifest.files();
        assert_eq!(files.len(), 1);

        let file_names: Vec<String> = files
            .iter()
            .filter_map(|p| p.file_name().and_then(|n| n.to_str()))
            .map(String::from)
            .collect();

        assert_eq!(file_names, vec!["index.js"]);
    }

    #[tokio::test]
    async fn test_new_with_empty_directory_returns_error() {
        let temp_dir = TempDir::new().expect("Failed to create temp directory");

        let result = CloudflareFileManifest::new(temp_dir.path()).await;

        assert!(result.is_err());
        let error = result.unwrap_err();
        let error_msg = error.to_string();
        assert!(error_msg.contains("No files found in directory"));
    }

    #[tokio::test]
    async fn test_new_with_only_unsupported_files_returns_error() {
        let temp_dir = create_test_directory(&[
            ("image.png", "binary image data"),
            ("video.mp4", "binary video data"),
            ("archive.zip", "binary archive data"),
            ("README.md", "# Documentation"),
            ("config.yaml", "version: 1"),
            ("settings.toml", "[section]"),
            ("notes.txt", "Some notes"),
        ]);

        let result = CloudflareFileManifest::new(temp_dir.path()).await;

        assert!(result.is_err());
        let error = result.unwrap_err();
        let error_msg = error.to_string();
        assert!(error_msg.contains("No files found in directory"));
        assert!(error_msg.contains("to upload"));
    }

    #[tokio::test]
    async fn test_new_with_only_manifest_files_returns_error() {
        let temp_dir = create_test_directory(&[
            ("MultiTool.toml", "[package]\nname = 'test'"),
            ("MultiTool.json", r#"{"name": "test"}"#),
        ]);

        let result = CloudflareFileManifest::new(temp_dir.path()).await;

        assert!(result.is_err());
        let error = result.unwrap_err();
        let error_msg = error.to_string();
        assert!(error_msg.contains("No files found in directory"));
        assert!(error_msg.contains("to upload"));
    }

    #[tokio::test]
    async fn test_new_with_invalid_path_returns_error() {
        let invalid_path = "/this/path/does/not/exist";

        let result = CloudflareFileManifest::new(invalid_path).await;

        assert!(result.is_err());
        let error = result.unwrap_err();
        let error_msg = error.to_string();
        assert!(error_msg.contains("is not a valid directory"));
    }

    #[tokio::test]
    async fn test_new_with_file_instead_of_directory_returns_error() {
        let temp_dir = TempDir::new().expect("Failed to create temp directory");
        let file_path = temp_dir.path().join("not_a_directory.txt");
        fs::write(&file_path, "some content").expect("Failed to write test file");

        let result = CloudflareFileManifest::new(&file_path).await;

        assert!(result.is_err());
        let error = result.unwrap_err();
        let error_msg = error.to_string();
        assert!(error_msg.contains("is not a valid directory"));
    }
}
