//! Protocol permission ids and channel permission hint bitflags.
//!
//! These ids come from the TeamSpeak permission list. Keeping them in one
//! module avoids duplicated magic numbers in the client implementation.

/// `i_ft_file_upload_power`
pub const FT_FILE_UPLOAD_POWER: u32 = 235;
/// `i_ft_needed_file_upload_power`
pub const FT_NEEDED_FILE_UPLOAD_POWER: u32 = 236;
/// `i_ft_file_download_power`
pub const FT_FILE_DOWNLOAD_POWER: u32 = 237;
/// `i_ft_needed_file_download_power`
pub const FT_NEEDED_FILE_DOWNLOAD_POWER: u32 = 238;
/// `i_ft_file_delete_power`
pub const FT_FILE_DELETE_POWER: u32 = 239;
/// `i_ft_needed_file_delete_power`
pub const FT_NEEDED_FILE_DELETE_POWER: u32 = 240;
/// `i_ft_file_rename_power`
pub const FT_FILE_RENAME_POWER: u32 = 241;
/// `i_ft_needed_file_rename_power`
pub const FT_NEEDED_FILE_RENAME_POWER: u32 = 242;
/// `i_ft_file_browse_power`
pub const FT_FILE_BROWSE_POWER: u32 = 243;
/// `i_ft_needed_file_browse_power`
pub const FT_NEEDED_FILE_BROWSE_POWER: u32 = 244;
/// `i_ft_directory_create_power`
pub const FT_DIRECTORY_CREATE_POWER: u32 = 245;
/// `i_ft_needed_directory_create_power`
pub const FT_NEEDED_DIRECTORY_CREATE_POWER: u32 = 246;

/// ChannelPermissionHint: can join.
pub const HINT_JOIN: u64 = 1;
/// ChannelPermissionHint: can upload files.
pub const HINT_FILE_UPLOAD: u64 = 64;
/// ChannelPermissionHint: can download files.
pub const HINT_FILE_DOWNLOAD: u64 = 128;
/// ChannelPermissionHint: can delete files.
pub const HINT_FILE_DELETE: u64 = 256;
/// ChannelPermissionHint: can rename files.
pub const HINT_FILE_RENAME: u64 = 512;
/// ChannelPermissionHint: can browse files.
pub const HINT_FILE_BROWSE: u64 = 1024;
/// ChannelPermissionHint: can create directories.
pub const HINT_FILE_DIRECTORY_CREATE: u64 = 2048;

// Backwards-compatible aliases for the names used inside the client.
pub const PERM_FT_FILE_UPLOAD_POWER: u32 = FT_FILE_UPLOAD_POWER;
pub const PERM_FT_NEEDED_FILE_UPLOAD_POWER: u32 = FT_NEEDED_FILE_UPLOAD_POWER;
pub const PERM_FT_FILE_DOWNLOAD_POWER: u32 = FT_FILE_DOWNLOAD_POWER;
pub const PERM_FT_NEEDED_FILE_DOWNLOAD_POWER: u32 = FT_NEEDED_FILE_DOWNLOAD_POWER;
pub const PERM_FT_FILE_DELETE_POWER: u32 = FT_FILE_DELETE_POWER;
pub const PERM_FT_NEEDED_FILE_DELETE_POWER: u32 = FT_NEEDED_FILE_DELETE_POWER;
pub const PERM_FT_FILE_RENAME_POWER: u32 = FT_FILE_RENAME_POWER;
pub const PERM_FT_NEEDED_FILE_RENAME_POWER: u32 = FT_NEEDED_FILE_RENAME_POWER;
pub const PERM_FT_FILE_BROWSE_POWER: u32 = FT_FILE_BROWSE_POWER;
pub const PERM_FT_NEEDED_FILE_BROWSE_POWER: u32 = FT_NEEDED_FILE_BROWSE_POWER;
pub const PERM_FT_DIRECTORY_CREATE_POWER: u32 = FT_DIRECTORY_CREATE_POWER;
pub const PERM_FT_NEEDED_DIRECTORY_CREATE_POWER: u32 = FT_NEEDED_DIRECTORY_CREATE_POWER;
