use std::{
    fs::File,
    io::{ErrorKind, Write},
    os::fd::{AsRawFd, BorrowedFd, OwnedFd},
    time::Duration,
};

use calloop::{LoopHandle, PostAction};
use filedescriptor::Pipe;
use strum::IntoEnumIterator;
use wayland_client::{Connection, protocol::wl_data_offer::WlDataOffer};
use wayland_protocols::wp::primary_selection::zv1::client::zwp_primary_selection_offer_v1::ZwpPrimarySelectionOfferV1;

use crate::linux::{
    WaylandClientStatePtr,
    platform::{PIPE_READ_TIMEOUT, read_fd_with_timeout},
};
use gpui::{ClipboardEntry, ClipboardItem, ExternalPaths, Image, ImageFormat, hash};
use smallvec::SmallVec;
use url::Url;

/// Text mime types that we'll offer to other programs.
pub(crate) const TEXT_MIME_TYPES: [&str; 3] =
    ["text/plain;charset=utf-8", "UTF8_STRING", "text/plain"];
pub(crate) const FILE_LIST_MIME_TYPE: &str = "text/uri-list";
/// GNOME's file clipboard format ("copy" line, then one URI per line). Nautilus and
/// other GTK file managers paste files from it.
pub(crate) const GNOME_COPIED_FILES_MIME_TYPE: &str = "x-special/gnome-copied-files";

/// Text mime types that we'll accept from other programs.
/// How long a file paste waits for the owner to send more of its file list.
/// Short, because the read blocks the UI thread; file lists are small.
const FILE_LIST_READ_TIMEOUT: Duration = Duration::from_millis(500);

pub(crate) const ALLOWED_TEXT_MIME_TYPES: [&str; 3] =
    ["text/plain;charset=utf-8", "UTF8_STRING", "text/plain"];

pub(crate) struct Clipboard {
    connection: Connection,
    loop_handle: LoopHandle<'static, WaylandClientStatePtr>,
    self_mime: String,

    // Internal clipboard
    contents: Option<ClipboardItem>,
    primary_contents: Option<ClipboardItem>,
    /// Ownership was requested and the compositor has not announced the new
    /// selection yet: reads answer from `contents`, not from the previous offer.
    ownership_pending: bool,

    // External clipboard
    cached_read: Option<ClipboardItem>,
    current_offer: Option<DataOffer<WlDataOffer>>,
    cached_primary_read: Option<ClipboardItem>,
    current_primary_offer: Option<DataOffer<ZwpPrimarySelectionOfferV1>>,
}

pub(crate) trait ReceiveData {
    fn receive_data(&self, mime_type: String, fd: BorrowedFd<'_>);
}

impl ReceiveData for WlDataOffer {
    fn receive_data(&self, mime_type: String, fd: BorrowedFd<'_>) {
        self.receive(mime_type, fd);
    }
}

impl ReceiveData for ZwpPrimarySelectionOfferV1 {
    fn receive_data(&self, mime_type: String, fd: BorrowedFd<'_>) {
        self.receive(mime_type, fd);
    }
}

#[derive(Clone, Debug)]
/// Wrapper for `WlDataOffer` and `ZwpPrimarySelectionOfferV1`, used to help track mime types.
pub(crate) struct DataOffer<T: ReceiveData> {
    pub inner: T,
    mime_types: Vec<String>,
}

impl<T: ReceiveData> DataOffer<T> {
    pub fn new(offer: T) -> Self {
        Self {
            inner: offer,
            mime_types: Vec::new(),
        }
    }

    pub fn add_mime_type(&mut self, mime_type: String) {
        self.mime_types.push(mime_type)
    }

    fn has_mime_type(&self, mime_type: &str) -> bool {
        self.mime_types.iter().any(|t| t == mime_type)
    }

    fn read_bytes(&self, connection: &Connection, mime_type: &str) -> Option<Vec<u8>> {
        self.read_bytes_within(connection, mime_type, PIPE_READ_TIMEOUT)
    }

    fn read_bytes_within(
        &self,
        connection: &Connection,
        mime_type: &str,
        timeout: Duration,
    ) -> Option<Vec<u8>> {
        let pipe = Pipe::new().unwrap();
        self.inner.receive_data(mime_type.to_string(), unsafe {
            BorrowedFd::borrow_raw(pipe.write.as_raw_fd())
        });
        let fd = pipe.read;
        drop(pipe.write);

        connection.flush().unwrap();

        match read_fd_with_timeout(fd, timeout) {
            Ok(bytes) => Some(bytes),
            Err(err) => {
                log::error!("error reading clipboard pipe: {err:?}");
                None
            }
        }
    }

    fn read_text(&self, connection: &Connection) -> Option<ClipboardItem> {
        let mime_type = self.mime_types.iter().find(|&mime_type| {
            ALLOWED_TEXT_MIME_TYPES
                .iter()
                .any(|&allowed| allowed == mime_type)
        })?;
        let bytes = self.read_bytes(connection, mime_type)?;
        let text_content = match String::from_utf8(bytes) {
            Ok(content) => content,
            Err(e) => {
                log::error!("Failed to convert clipboard content to UTF-8: {}", e);
                return None;
            }
        };

        // Normalize the text to unix line endings, otherwise
        // copying from eg: firefox inserts a lot of blank
        // lines, and that is super annoying.
        let result = text_content.replace("\r\n", "\n").replace('\0', "");
        Some(ClipboardItem::new_string(result))
    }

    /// Files offered as `text/uri-list`, as `[ExternalPaths, String]` like the macOS
    /// pasteboard, so text consumers still get plain text. `Err` when the owner
    /// offered a file list but did not send it within `FILE_LIST_READ_TIMEOUT`.
    fn read_files(&self, connection: &Connection) -> Result<Option<ClipboardItem>, ()> {
        if !self.has_mime_type(FILE_LIST_MIME_TYPE) {
            return Ok(None);
        }
        let bytes = self
            .read_bytes_within(connection, FILE_LIST_MIME_TYPE, FILE_LIST_READ_TIMEOUT)
            .ok_or(())?;
        let paths = parse_uri_list(&String::from_utf8_lossy(&bytes));
        if paths.is_empty() {
            return Ok(None);
        }
        // The text is the paths, one per line, as file managers offer it: a second
        // read from the owner could stall the UI thread for another pipe timeout.
        let text = paths
            .iter()
            .map(|path| path.to_string_lossy())
            .collect::<Vec<_>>()
            .join("\n");
        Ok(Some(ClipboardItem {
            entries: vec![
                ClipboardEntry::ExternalPaths(ExternalPaths(paths)),
                ClipboardEntry::String(gpui::ClipboardString::new(text)),
            ],
        }))
    }

    fn read_image(&self, connection: &Connection) -> Option<ClipboardItem> {
        for format in ImageFormat::iter() {
            let mime_type = format.mime_type();
            if !self.has_mime_type(mime_type) {
                continue;
            }

            if let Some(bytes) = self.read_bytes(connection, mime_type) {
                let id = hash(&bytes);
                return Some(ClipboardItem {
                    entries: vec![ClipboardEntry::Image(Image { format, bytes, id })],
                });
            }
        }
        None
    }
}

impl Clipboard {
    pub fn new(
        connection: Connection,
        loop_handle: LoopHandle<'static, WaylandClientStatePtr>,
    ) -> Self {
        Self {
            connection,
            loop_handle,
            self_mime: format!("pid/{}", std::process::id()),

            contents: None,
            primary_contents: None,
            ownership_pending: false,

            cached_read: None,
            current_offer: None,
            cached_primary_read: None,
            current_primary_offer: None,
        }
    }

    pub fn set(&mut self, item: ClipboardItem) {
        self.contents = Some(item);
    }

    pub fn set_primary(&mut self, item: ClipboardItem) {
        self.primary_contents = Some(item);
    }

    /// Called after `set_selection` for `contents`.
    pub fn ownership_requested(&mut self) {
        self.ownership_pending = true;
    }

    pub fn set_offer(&mut self, data_offer: Option<DataOffer<WlDataOffer>>) {
        self.ownership_pending = false;
        self.cached_read = None;
        self.current_offer = data_offer;
    }

    pub fn set_primary_offer(&mut self, data_offer: Option<DataOffer<ZwpPrimarySelectionOfferV1>>) {
        self.cached_primary_read = None;
        self.current_primary_offer = data_offer;
    }

    pub fn self_mime(&self) -> String {
        self.self_mime.clone()
    }

    /// Mime types to offer for `item`: files add `text/uri-list` and the GNOME format.
    pub fn offered_mime_types(item: &ClipboardItem) -> Vec<&'static str> {
        let mut types = Vec::new();
        if external_paths(item).is_some() {
            types.extend([FILE_LIST_MIME_TYPE, GNOME_COPIED_FILES_MIME_TYPE]);
        }
        types.extend(TEXT_MIME_TYPES);
        types
    }

    pub fn send(&self, mime_type: String, fd: OwnedFd) {
        if let Some(bytes) = self
            .contents
            .as_ref()
            .and_then(|contents| serialize(contents, &mime_type))
        {
            self.send_bytes(fd, bytes);
        }
    }

    pub fn send_primary(&self, _mime_type: String, fd: OwnedFd) {
        if let Some(text) = self
            .primary_contents
            .as_ref()
            .and_then(|contents| contents.text())
        {
            let sanitized = text.replace('\0', "");
            self.send_bytes(fd, sanitized.into_bytes());
        }
    }

    pub fn read(&mut self) -> Option<ClipboardItem> {
        if self.ownership_pending {
            return self.contents.clone();
        }
        let offer = self.current_offer.as_ref()?;
        if let Some(cached) = self.cached_read.clone() {
            return Some(cached);
        }

        if offer.has_mime_type(&self.self_mime) {
            return self.contents.clone();
        }

        let item = match offer.read_files(&self.connection) {
            Ok(Some(files)) => files,
            Ok(None) => offer
                .read_text(&self.connection)
                .or_else(|| offer.read_image(&self.connection))?,
            // An owner that does not send its files will not send text either:
            // do not block the UI thread on it again.
            Err(()) => return None,
        };

        self.cached_read = Some(item.clone());
        Some(item)
    }

    pub fn read_primary(&mut self) -> Option<ClipboardItem> {
        let offer = self.current_primary_offer.as_ref()?;
        if let Some(cached) = self.cached_primary_read.clone() {
            return Some(cached);
        }

        if offer.has_mime_type(&self.self_mime) {
            return self.primary_contents.clone();
        }

        let item = offer
            .read_text(&self.connection)
            .or_else(|| offer.read_image(&self.connection))?;

        self.cached_primary_read = Some(item.clone());
        Some(item)
    }

    pub fn send_bytes(&self, fd: OwnedFd, bytes: Vec<u8>) {
        let mut written = 0;
        self.loop_handle
            .insert_source(
                calloop::generic::Generic::new(
                    File::from(fd),
                    calloop::Interest::WRITE,
                    calloop::Mode::Level,
                ),
                move |_, file, _| {
                    let file = unsafe { file.get_mut() };
                    loop {
                        match file.write(&bytes[written..]) {
                            Ok(n) if written + n == bytes.len() => {
                                written += n;
                                break Ok(PostAction::Remove);
                            }
                            Ok(n) => written += n,
                            Err(err) if err.kind() == ErrorKind::WouldBlock => {
                                break Ok(PostAction::Continue);
                            }
                            Err(_) => break Ok(PostAction::Remove),
                        }
                    }
                },
            )
            .unwrap();
    }
}

fn external_paths(item: &ClipboardItem) -> Option<&ExternalPaths> {
    item.entries().iter().find_map(|entry| match entry {
        ClipboardEntry::ExternalPaths(paths) if !paths.0.is_empty() => Some(paths),
        _ => None,
    })
}

fn file_uris(paths: &ExternalPaths) -> Vec<String> {
    paths
        .0
        .iter()
        .filter_map(|path| Url::from_file_path(path).ok())
        .map(String::from)
        .collect()
}

/// The bytes another program receives for `mime_type`, or `None` when the item has
/// no such representation.
fn serialize(item: &ClipboardItem, mime_type: &str) -> Option<Vec<u8>> {
    let paths = external_paths(item);
    let text = match (mime_type, paths) {
        (FILE_LIST_MIME_TYPE, Some(paths)) => {
            file_uris(paths)
                .iter()
                .fold(String::new(), |mut list, uri| {
                    list.push_str(uri);
                    list.push_str("\r\n");
                    list
                })
        }
        (GNOME_COPIED_FILES_MIME_TYPE, Some(paths)) => std::iter::once("copy".to_string())
            .chain(file_uris(paths))
            .collect::<Vec<_>>()
            .join("\n"),
        (FILE_LIST_MIME_TYPE | GNOME_COPIED_FILES_MIME_TYPE, None) => return None,
        _ => {
            let has_string = item
                .entries()
                .iter()
                .any(|entry| matches!(entry, ClipboardEntry::String(_)));
            match paths {
                // Plain text of a file copy: one path per line, as file managers do.
                Some(paths) if !has_string => paths
                    .0
                    .iter()
                    .map(|path| path.to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("\n"),
                _ => item.text()?,
            }
        }
    };
    Some(text.replace('\0', "").into_bytes())
}

fn parse_uri_list(list: &str) -> SmallVec<[std::path::PathBuf; 2]> {
    list.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| Url::parse(line).ok())
        // `to_file_path` ignores the scheme: Nautilus's Trash offers `trash:///a.txt`.
        .filter(|url| url.scheme() == "file")
        .filter_map(|url| url.to_file_path().ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn files(paths: &[&str]) -> ClipboardItem {
        ClipboardItem {
            entries: vec![ClipboardEntry::ExternalPaths(ExternalPaths(
                paths.iter().map(PathBuf::from).collect(),
            ))],
        }
    }

    fn sent(item: &ClipboardItem, mime_type: &str) -> Option<String> {
        serialize(item, mime_type).map(|bytes| String::from_utf8(bytes).unwrap())
    }

    #[test]
    fn files_are_offered_as_uri_list_gnome_files_and_text() {
        let item = files(&["/home/me/a b.txt", "/tmp/c"]);
        assert_eq!(
            Clipboard::offered_mime_types(&item),
            [
                FILE_LIST_MIME_TYPE,
                GNOME_COPIED_FILES_MIME_TYPE,
                "text/plain;charset=utf-8",
                "UTF8_STRING",
                "text/plain"
            ]
        );
        assert_eq!(
            sent(&item, FILE_LIST_MIME_TYPE).unwrap(),
            "file:///home/me/a%20b.txt\r\nfile:///tmp/c\r\n"
        );
        assert_eq!(
            sent(&item, GNOME_COPIED_FILES_MIME_TYPE).unwrap(),
            "copy\nfile:///home/me/a%20b.txt\nfile:///tmp/c"
        );
        assert_eq!(
            sent(&item, "text/plain").unwrap(),
            "/home/me/a b.txt\n/tmp/c"
        );
    }

    #[test]
    fn a_string_entry_is_the_plain_text_of_a_file_copy() {
        let mut item = files(&["/tmp/c"]);
        item.entries
            .push(ClipboardEntry::String(gpui::ClipboardString::new(
                "custom".into(),
            )));
        assert_eq!(sent(&item, "UTF8_STRING").unwrap(), "custom");
    }

    #[test]
    fn text_is_not_offered_as_files() {
        let item = ClipboardItem::new_string("hello".into());
        assert_eq!(Clipboard::offered_mime_types(&item), TEXT_MIME_TYPES);
        assert_eq!(sent(&item, FILE_LIST_MIME_TYPE), None);
        assert_eq!(sent(&item, "text/plain").unwrap(), "hello");
    }

    #[test]
    fn uri_list_parses_local_files_only() {
        let paths = parse_uri_list(
            "# comment\r\nfile:///home/me/a%20b.txt\r\nhttps://example.com/x\r\ntrash:///d.txt\r\nadmin:///etc/e\r\n\r\nfile:///tmp/c\n",
        );
        assert_eq!(
            paths.as_slice(),
            [PathBuf::from("/home/me/a b.txt"), PathBuf::from("/tmp/c")]
        );
    }
}
