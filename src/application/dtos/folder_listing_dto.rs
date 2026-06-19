use serde::Serialize;
use utoipa::ToSchema;

use super::file_dto::FileDto;
use super::folder_dto::FolderDto;

/// Combined DTO that returns both sub-folders and files for a given folder
/// in a single response, eliminating the double-fetch on every navigation.
#[derive(Debug, Serialize, ToSchema)]
pub struct FolderListingDto {
    /// Sub-folders inside the requested folder
    pub folders: Vec<FolderDto>,
    /// Files inside the requested folder
    pub files: Vec<FileDto>,
    /// Ids (folders + files in this listing) the caller has favorited. Lets the
    /// client render star badges without a separate per-navigation favorites
    /// fetch. Sorted for a stable response / ETag.
    pub favorite_ids: Vec<String>,
    /// Ids in this listing the caller has an outgoing share/grant on (incl.
    /// public links). Sorted for a stable response / ETag.
    pub shared_ids: Vec<String>,
}
