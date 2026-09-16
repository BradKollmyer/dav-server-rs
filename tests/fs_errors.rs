#![cfg(any(feature = "memfs", feature = "localfs"))]

use dav_server::fs::FsError;
use std::io::{Error, ErrorKind};

#[test]
fn portable_filesystem_errors_keep_their_dav_meaning() {
    for (kind, expected) in [
        (ErrorKind::AlreadyExists, FsError::Exists),
        (ErrorKind::DirectoryNotEmpty, FsError::Exists),
        (ErrorKind::NotFound, FsError::NotFound),
        (ErrorKind::PermissionDenied, FsError::Forbidden),
        (ErrorKind::NotADirectory, FsError::Forbidden),
        (ErrorKind::IsADirectory, FsError::Forbidden),
        (ErrorKind::ReadOnlyFilesystem, FsError::Forbidden),
        (ErrorKind::StorageFull, FsError::InsufficientStorage),
        (ErrorKind::FileTooLarge, FsError::TooLarge),
        (ErrorKind::CrossesDevices, FsError::IsRemote),
        (ErrorKind::Unsupported, FsError::NotImplemented),
        (ErrorKind::Other, FsError::GeneralFailure),
    ] {
        assert_eq!(FsError::from(Error::from(kind)), expected, "{kind:?}");
    }
}

#[cfg(windows)]
#[test]
fn windows_errors_are_not_interpreted_as_posix_errno() {
    for (code, expected) in [
        (5, FsError::Forbidden),             // ERROR_ACCESS_DENIED
        (17, FsError::IsRemote),             // ERROR_NOT_SAME_DEVICE, not POSIX EEXIST
        (80, FsError::Exists),               // ERROR_FILE_EXISTS
        (112, FsError::InsufficientStorage), // ERROR_DISK_FULL
        (183, FsError::Exists),              // ERROR_ALREADY_EXISTS
    ] {
        assert_eq!(FsError::from(Error::from_raw_os_error(code)), expected);
    }
}
