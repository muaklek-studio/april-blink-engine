use std::io;

#[derive(Debug, thiserror::Error)]
pub enum AssetError {
    #[error("cannot read `{path}`: {source}")]
    Io {
        path: String,
        #[source]
        source: io::Error,
    },
    #[error("invalid glTF `{path}`: {source}")]
    Gltf {
        path: String,
        #[source]
        source: gltf::Error,
    },
    #[error("cannot decode image `{path}`: {source}")]
    Image {
        path: String,
        #[source]
        source: image::ImageError,
    },
    #[error("invalid asset `{path}`: {reason}")]
    InvalidData { path: String, reason: String },
    #[error("the `{0}` resource is missing from the world")]
    MissingResource(&'static str),
}

impl AssetError {
    pub(crate) fn invalid(path: &str, reason: impl Into<String>) -> Self {
        Self::InvalidData {
            path: path.to_owned(),
            reason: reason.into(),
        }
    }
}
