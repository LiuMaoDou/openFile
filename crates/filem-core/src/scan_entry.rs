//! Buffered directory observations. Windows obtains stable file IDs with each
//! directory batch instead of opening every regular file for its identity.
use crate::platform;
use std::{
    fs, io,
    path::{Path, PathBuf},
};

pub(crate) struct Kind {
    directory: bool,
    file: bool,
    link: bool,
}
impl Kind {
    pub fn is_dir(&self) -> bool {
        self.directory
    }
    pub fn is_symlink(&self) -> bool {
        self.link
    }
}
pub(crate) enum Metadata {
    Standard(fs::Metadata),
    #[cfg(windows)]
    Native {
        size: u64,
        modified: i64,
        attributes: u32,
        volume: u32,
        id: u64,
    },
}
impl Metadata {
    pub fn len(&self) -> u64 {
        match self {
            Self::Standard(m) => m.len(),
            #[cfg(windows)]
            Self::Native { size, .. } => *size,
        }
    }
    pub fn modified(&self) -> i64 {
        match self {
            Self::Standard(m) => platform::mtime(m),
            #[cfg(windows)]
            Self::Native { modified, .. } => *modified,
        }
    }
    pub fn attributes(&self) -> u32 {
        match self {
            Self::Standard(m) => platform::attributes(m),
            #[cfg(windows)]
            Self::Native { attributes, .. } => *attributes,
        }
    }
    pub fn kind(&self) -> Kind {
        match self {
            Self::Standard(m) => Kind {
                directory: m.is_dir(),
                file: m.is_file(),
                link: m.file_type().is_symlink(),
            },
            #[cfg(windows)]
            Self::Native { attributes, .. } => Kind {
                directory: attributes & 0x10 != 0,
                file: attributes & 0x10 == 0,
                link: false,
            },
        }
    }
    pub fn is_dir(&self) -> bool {
        self.kind().directory
    }
    pub fn is_file(&self) -> bool {
        self.kind().file
    }
    pub fn identity(&self, path: &Path) -> anyhow::Result<String> {
        match self {
            Self::Standard(m) => platform::identity(path, m),
            #[cfg(windows)]
            Self::Native { volume, id, .. } if *id != 0 => {
                Ok(format!("{}:{}:{}", volume, id >> 32, id & 0xffff_ffff))
            }
            #[cfg(windows)]
            Self::Native { .. } => platform::identity(path, &fs::symlink_metadata(path)?),
        }
    }
}
pub(crate) enum Entry {
    Standard(fs::DirEntry),
    #[cfg(windows)]
    Native {
        path: PathBuf,
        metadata: Metadata,
    },
}
impl Entry {
    pub fn path(&self) -> PathBuf {
        match self {
            Self::Standard(e) => e.path(),
            #[cfg(windows)]
            Self::Native { path, .. } => path.clone(),
        }
    }
    pub fn file_type(&self) -> io::Result<Kind> {
        match self {
            Self::Standard(e) => e.file_type().map(|k| Kind {
                directory: k.is_dir(),
                file: k.is_file(),
                link: k.is_symlink(),
            }),
            #[cfg(windows)]
            Self::Native { metadata, .. } => Ok(metadata.kind()),
        }
    }
    pub fn metadata(self) -> io::Result<Metadata> {
        match self {
            Self::Standard(e) => e.metadata().map(Metadata::Standard),
            #[cfg(windows)]
            Self::Native { path, metadata } => {
                // Keep symlink/cloud-placeholder behavior of the generic backend.
                if metadata.attributes() & 0x400 != 0 {
                    fs::symlink_metadata(path).map(Metadata::Standard)
                } else {
                    Ok(metadata)
                }
            }
        }
    }
}
pub(crate) enum Entries {
    Standard(fs::ReadDir),
    #[cfg(windows)]
    Native(native::Entries),
}
pub(crate) fn read_dir(path: &Path) -> io::Result<Entries> {
    #[cfg(windows)]
    if let Ok(entries) = native::Entries::open(path) {
        return Ok(Entries::Native(entries));
    }
    fs::read_dir(path).map(Entries::Standard)
}
impl Iterator for Entries {
    type Item = io::Result<Entry>;
    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Standard(iter) => iter.next().map(|e| e.map(Entry::Standard)),
            #[cfg(windows)]
            Self::Native(iter) => iter.next(),
        }
    }
}

#[cfg(windows)]
mod native {
    use super::*;
    use std::{
        ffi::OsString,
        os::windows::{
            ffi::{OsStrExt, OsStringExt},
            io::{AsRawHandle, FromRawHandle},
        },
    };
    use windows_sys::Win32::{Foundation::*, Storage::FileSystem::*};
    pub(crate) struct Entries {
        directory: fs::File,
        root: PathBuf,
        volume: u32,
        buffer: Vec<u64>,
        offset: usize,
        ended: bool,
        refill: bool,
    }
    impl Entries {
        pub fn open(path: &Path) -> io::Result<Self> {
            let wide: Vec<_> = path.as_os_str().encode_wide().chain(Some(0)).collect();
            let handle = unsafe {
                CreateFileW(
                    wide.as_ptr(),
                    FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES,
                    FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                    std::ptr::null(),
                    OPEN_EXISTING,
                    FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
                    std::ptr::null_mut(),
                )
            };
            if handle == INVALID_HANDLE_VALUE {
                return Err(io::Error::last_os_error());
            }
            let directory = unsafe { fs::File::from_raw_handle(handle) };
            let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
            if unsafe { GetFileInformationByHandle(handle, &mut info) } == 0 {
                return Err(io::Error::last_os_error());
            }
            if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                return Err(io::Error::other("reparse directory"));
            }
            let mut result = Self {
                directory,
                root: path.into(),
                volume: info.dwVolumeSerialNumber,
                buffer: vec![0; 8192],
                offset: 0,
                ended: false,
                refill: false,
            };
            result.fill()?;
            Ok(result)
        }
        fn fill(&mut self) -> io::Result<()> {
            self.buffer.fill(0);
            let ok = unsafe {
                GetFileInformationByHandleEx(
                    self.directory.as_raw_handle(),
                    FileIdBothDirectoryInfo,
                    self.buffer.as_mut_ptr().cast(),
                    (self.buffer.len() * 8) as u32,
                )
            };
            if ok == 0 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() == Some(ERROR_NO_MORE_FILES as i32) {
                    self.ended = true;
                    return Ok(());
                }
                return Err(error);
            }
            self.offset = 0;
            self.refill = false;
            Ok(())
        }
    }
    impl Iterator for Entries {
        type Item = io::Result<Entry>;
        fn next(&mut self) -> Option<Self::Item> {
            loop {
                if self.ended {
                    return None;
                }
                if self.refill {
                    if let Err(error) = self.fill() {
                        self.ended = true;
                        return Some(Err(error));
                    }
                    if self.ended {
                        return None;
                    }
                }
                let base = std::mem::offset_of!(FILE_ID_BOTH_DIR_INFO, FileName);
                let capacity = self.buffer.len() * 8;
                if self.offset + std::mem::size_of::<FILE_ID_BOTH_DIR_INFO>() > capacity {
                    self.ended = true;
                    return Some(Err(io::Error::other("invalid directory record")));
                }
                let record = unsafe {
                    &*(self
                        .buffer
                        .as_ptr()
                        .cast::<u8>()
                        .add(self.offset)
                        .cast::<FILE_ID_BOTH_DIR_INFO>())
                };
                let bytes = record.FileNameLength as usize;
                let next = record.NextEntryOffset as usize;
                if bytes % 2 != 0
                    || self.offset + base + bytes > capacity
                    || (next != 0
                        && (next % 8 != 0 || next < base + bytes || self.offset + next >= capacity))
                {
                    self.ended = true;
                    return Some(Err(io::Error::other("invalid directory record length")));
                }
                let name =
                    unsafe { std::slice::from_raw_parts(record.FileName.as_ptr(), bytes / 2) };
                let dot = name == [46] || name == [46, 46];
                let path = self.root.join(OsString::from_wide(name));
                let metadata = Metadata::Native {
                    size: record.EndOfFile.max(0) as u64,
                    modified: record
                        .LastWriteTime
                        .saturating_sub(116_444_736_000_000_000)
                        .max(0)
                        / 10_000,
                    attributes: record.FileAttributes,
                    volume: self.volume,
                    id: record.FileId as u64,
                };
                if next == 0 {
                    self.refill = true;
                } else {
                    self.offset += next;
                }
                if !dot {
                    return Some(Ok(Entry::Native { path, metadata }));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(windows)]
    #[test]
    fn native_batches_preserve_ids_across_multiple_buffers() {
        let temp = tempfile::tempdir().unwrap();
        for i in 0..1000 {
            fs::write(
                temp.path().join(format!("中文-{i:04}-long-name.txt")),
                b"sample",
            )
            .unwrap();
        }
        fs::hard_link(
            temp.path().join("中文-0000-long-name.txt"),
            temp.path().join("hard-link.txt"),
        )
        .unwrap();
        let mut names = std::collections::HashSet::new();
        // Exercise the native backend directly; a silent fallback would leave
        // the new multi-buffer implementation untested on Windows CI.
        for entry in native::Entries::open(temp.path()).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let metadata = entry.metadata().unwrap();
            let actual = fs::symlink_metadata(&path).unwrap();
            assert_eq!(
                metadata.identity(&path).unwrap(),
                platform::identity(&path, &actual).unwrap()
            );
            assert_eq!(metadata.len(), actual.len());
            assert_eq!(metadata.modified(), platform::mtime(&actual));
            assert_eq!(metadata.attributes(), platform::attributes(&actual));
            assert!(names.insert(path));
        }
        assert_eq!(names.len(), 1001);
    }
    #[test]
    fn directory_observations_match_file_handles() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("中文 📄.txt"), b"sample").unwrap();
        fs::create_dir(temp.path().join("folder")).unwrap();
        let mut count = 0;
        for entry in read_dir(temp.path()).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let observed = entry.metadata().unwrap();
            let actual = fs::symlink_metadata(&path).unwrap();
            assert_eq!(observed.len(), actual.len());
            assert_eq!(observed.modified(), platform::mtime(&actual));
            assert_eq!(
                observed.identity(&path).unwrap(),
                platform::identity(&path, &actual).unwrap()
            );
            count += 1;
        }
        assert_eq!(count, 2);
    }
}
