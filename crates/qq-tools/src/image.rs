//! Image reading tool for LLM agents.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;

use qq_core::{
    Error, ImageData, PropertySchema, Tool, ToolDefinition, ToolOutput, ToolParameters,
    TypedContent,
};

const MAX_IMAGE_SIZE: u64 = 20 * 1024 * 1024; // 20 MB

/// Tool that reads an image file and returns its visual content to the LLM.
pub struct ReadImageTool {
    project_root: PathBuf,
}

#[derive(Deserialize)]
struct ReadImageArgs {
    path: String,
}

impl ReadImageTool {
    pub fn new(project_root: PathBuf) -> Self {
        Self { project_root }
    }

    fn resolve_path(&self, path: &str) -> PathBuf {
        let p = PathBuf::from(path);
        if p.is_absolute() {
            p
        } else {
            self.project_root.join(p)
        }
    }
}

#[async_trait]
impl Tool for ReadImageTool {
    fn name(&self) -> &str {
        "read_image"
    }

    fn description(&self) -> &str {
        "Read an image file and return its visual content"
    }

    fn tool_description(&self) -> &str {
        "Read an image file from disk and return its visual content for analysis. \
         Supports PNG, JPEG, GIF, and WebP formats. Maximum file size is 20MB. \
         Paths can be absolute or relative to the project root."
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new("read_image", self.tool_description()).with_parameters(
            ToolParameters::new().add_property(
                "path",
                PropertySchema::string(
                    "Path to the image file (absolute or relative to project root)",
                ),
                true,
            ),
        )
    }

    fn is_blocking(&self) -> bool {
        true
    }

    async fn execute(&self, arguments: Value) -> Result<ToolOutput, Error> {
        let args: ReadImageArgs = serde_json::from_value(arguments)
            .map_err(|e| Error::Unknown(format!("Invalid arguments: {}", e)))?;

        let resolved = self.resolve_path(&args.path);

        if !resolved.exists() {
            return Ok(ToolOutput::error(format!(
                "File not found: {}",
                resolved.display()
            )));
        }

        if !resolved.is_file() {
            return Ok(ToolOutput::error(format!(
                "Not a file: {}",
                resolved.display()
            )));
        }

        let metadata = std::fs::metadata(&resolved)
            .map_err(|e| Error::Unknown(format!("Cannot read metadata: {}", e)))?;

        if metadata.len() > MAX_IMAGE_SIZE {
            return Ok(ToolOutput::error(format!(
                "File too large: {} bytes (max {})",
                metadata.len(),
                MAX_IMAGE_SIZE
            )));
        }

        let bytes = std::fs::read(&resolved)
            .map_err(|e| Error::Unknown(format!("Failed to read file: {}", e)))?;

        let image = match ImageData::from_bytes(&bytes) {
            Ok(img) => img,
            Err(e) => return Ok(ToolOutput::error(format!("Failed to decode image: {}", e))),
        };

        let info = format!(
            "{} ({}x{}, {} bytes)",
            image.media_type,
            image.width,
            image.height,
            bytes.len()
        );

        Ok(ToolOutput::with_content(
            vec![TypedContent::text(info), TypedContent::image(image)],
            false,
        ))
    }
}

/// Create image tools for the registry.
pub fn create_image_tools(project_root: PathBuf) -> Vec<Arc<dyn Tool>> {
    vec![Arc::new(ReadImageTool::new(project_root))]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_definition() {
        let tool = ReadImageTool::new(PathBuf::from("/tmp"));
        let def = tool.definition();
        assert_eq!(def.name, "read_image");
        assert!(def.parameters.required().contains(&"path".to_string()));
        assert!(def.parameters.properties().unwrap().contains_key("path"));
    }

    #[test]
    fn test_resolve_path_absolute() {
        let tool = ReadImageTool::new(PathBuf::from("/project"));
        assert_eq!(
            tool.resolve_path("/abs/path.png"),
            PathBuf::from("/abs/path.png")
        );
    }

    #[test]
    fn test_resolve_path_relative() {
        let tool = ReadImageTool::new(PathBuf::from("/project"));
        assert_eq!(
            tool.resolve_path("img/photo.png"),
            PathBuf::from("/project/img/photo.png")
        );
    }

    #[tokio::test]
    async fn test_not_found() {
        let tool = ReadImageTool::new(PathBuf::from("/tmp"));
        let result = tool
            .execute(serde_json::json!({"path": "/nonexistent/file.png"}))
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.text_content().contains("not found"));
    }

    #[tokio::test]
    async fn test_not_an_image() {
        // Create a temp text file
        let dir = std::env::temp_dir().join("qq_test_image");
        std::fs::create_dir_all(&dir).ok();
        let file = dir.join("not_image.txt");
        std::fs::write(&file, "hello world").unwrap();

        let tool = ReadImageTool::new(dir.clone());
        let result = tool
            .execute(serde_json::json!({"path": file.to_str().unwrap()}))
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(
            result.text_content().contains("decode") || result.text_content().contains("image")
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn test_success_with_png() {
        // Minimal valid 1x1 red PNG
        let png_bytes: Vec<u8> = vec![
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, // PNG signature
            0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, // IHDR chunk
            0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, // 1x1
            0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53, // 8-bit RGB
            0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, // IDAT chunk
            0x54, 0x08, 0xD7, 0x63, 0xF8, 0xCF, 0xC0, 0x00, // compressed data
            0x00, 0x00, 0x02, 0x00, 0x01, 0xE2, 0x21, 0xBC, // ...
            0x33, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, // IEND chunk
            0x44, 0xAE, 0x42, 0x60, 0x82,
        ];

        let dir = std::env::temp_dir().join("qq_test_image_png");
        std::fs::create_dir_all(&dir).ok();
        let file = dir.join("test.png");
        std::fs::write(&file, &png_bytes).unwrap();

        let tool = ReadImageTool::new(dir.clone());
        let result = tool
            .execute(serde_json::json!({"path": "test.png"}))
            .await
            .unwrap();
        assert!(!result.is_error);
        assert_eq!(result.content.len(), 2); // text info + image
        assert!(result.text_content().contains("image/png"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_create_image_tools() {
        let tools = create_image_tools(PathBuf::from("/tmp"));
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name(), "read_image");
    }
}
