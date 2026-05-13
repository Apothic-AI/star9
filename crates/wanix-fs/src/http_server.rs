use std::collections::BTreeMap;

use http::Method;

use crate::{
    apply_sync_patch, clean_path, lstat, read_dir, read_file, write_file, Error, ErrorKind,
    FileMode, FsRef, HttpRequest, HttpResponse, Result,
};

const ALLOW_METHODS: &str = "GET, HEAD, PUT, DELETE, PATCH, MOVE";
const MULTIPART_BOUNDARY: &str = "wanix-httpfs-boundary";

#[derive(Clone)]
pub struct HttpFsHandler {
    fsys: FsRef,
}

impl HttpFsHandler {
    pub fn new(fsys: FsRef) -> Self {
        Self { fsys }
    }

    pub fn handle(&self, request: HttpRequest) -> Result<HttpResponse> {
        let target = parse_request_target(&request.url);
        let response = match request.method {
            Method::GET => self.handle_get(&target.path, &request.headers),
            Method::HEAD => self.handle_head(&target.path),
            Method::PUT => self.handle_put(&target.path, target.had_trailing_slash, &request),
            Method::DELETE => self.handle_delete(&target.path),
            Method::PATCH => self.handle_patch(&target.path, &request),
            _ if request.method.as_str() == "MOVE" => self.handle_move(&target.path, &request),
            _ => Ok(method_not_allowed()),
        };
        match response {
            Ok(response) => Ok(response),
            Err(err) => Ok(response_from_error(err)),
        }
    }

    fn handle_get(
        &self,
        name: &str,
        headers: &BTreeMap<String, String>,
    ) -> std::result::Result<HttpResponse, Error> {
        let metadata = lstat(self.fsys.as_ref(), name)?;
        if metadata.is_dir() && accepts_multipart(headers) {
            return self.multipart_directory_response(name);
        }

        if metadata.mode.is_symlink() {
            let body = self.fsys.readlink(name)?.into_bytes();
            return Ok(HttpResponse {
                status: 200,
                headers: metadata_headers(&metadata, body.len() as u64),
                body,
            });
        }

        if metadata.is_dir() {
            let body = directory_listing(self.fsys.as_ref(), name)?;
            return Ok(HttpResponse {
                status: 200,
                headers: metadata_headers(&metadata, body.len() as u64),
                body,
            });
        }

        let body = read_file(self.fsys.as_ref(), name)?;
        Ok(HttpResponse {
            status: 200,
            headers: metadata_headers(&metadata, body.len() as u64),
            body,
        })
    }

    fn handle_head(&self, name: &str) -> std::result::Result<HttpResponse, Error> {
        let metadata = lstat(self.fsys.as_ref(), name)?;
        Ok(HttpResponse {
            status: 200,
            headers: metadata_headers(&metadata, metadata.size),
            body: Vec::new(),
        })
    }

    fn handle_put(
        &self,
        name: &str,
        had_trailing_slash: bool,
        request: &HttpRequest,
    ) -> std::result::Result<HttpResponse, Error> {
        let content_type = header_value(&request.headers, "Content-Type").unwrap_or_default();
        let is_directory =
            content_type.eq_ignore_ascii_case("application/x-directory") || had_trailing_slash;
        let is_symlink = content_type.eq_ignore_ascii_case("application/x-symlink");

        if is_symlink {
            let target = String::from_utf8_lossy(&request.body).into_owned();
            self.fsys.symlink(&target, name)?;
            self.apply_metadata_headers(name, &request.headers)?;
            return Ok(ok_response());
        }

        if is_directory {
            let mode = mode_header_or_default(
                header_value(&request.headers, "Content-Mode").as_deref(),
                FileMode::DIR | FileMode::from_perm(0o755),
            );
            match self.fsys.mkdir(name, FileMode::from_perm(mode.perm())) {
                Ok(()) => {}
                Err(err)
                    if err.kind() == ErrorKind::AlreadyExists
                        && lstat(self.fsys.as_ref(), name)
                            .map(|metadata| metadata.is_dir())
                            .unwrap_or(false) => {}
                Err(err) => return Err(err),
            }
            self.apply_metadata_headers(name, &request.headers)?;
            return Ok(ok_response());
        }

        let mode = mode_header_or_default(
            header_value(&request.headers, "Content-Mode").as_deref(),
            FileMode::from_perm(0o644),
        );
        write_file(
            self.fsys.as_ref(),
            name,
            &request.body,
            FileMode::from_perm(mode.perm()),
        )?;
        self.apply_metadata_headers(name, &request.headers)?;
        Ok(ok_response())
    }

    fn handle_delete(&self, name: &str) -> std::result::Result<HttpResponse, Error> {
        self.fsys.remove(name)?;
        Ok(ok_response())
    }

    fn handle_move(
        &self,
        name: &str,
        request: &HttpRequest,
    ) -> std::result::Result<HttpResponse, Error> {
        let Some(destination) = header_value(&request.headers, "Destination") else {
            return Ok(bad_request("Destination header required"));
        };
        let destination = parse_request_target(&destination).path;
        self.fsys.rename(name, &destination)?;
        Ok(ok_response())
    }

    fn handle_patch(
        &self,
        name: &str,
        request: &HttpRequest,
    ) -> std::result::Result<HttpResponse, Error> {
        if header_value(&request.headers, "Content-Type")
            .is_some_and(|value| value.eq_ignore_ascii_case("application/x-tar"))
        {
            apply_sync_patch(self.fsys.as_ref(), &request.body)?;
            return Ok(ok_response());
        }
        self.apply_metadata_headers(name, &request.headers)?;
        Ok(ok_response())
    }

    fn multipart_directory_response(&self, name: &str) -> std::result::Result<HttpResponse, Error> {
        let metadata = lstat(self.fsys.as_ref(), name)?;
        let entries = read_dir(self.fsys.as_ref(), name)?;
        let root_body = directory_listing(self.fsys.as_ref(), name)?;
        let mut body = Vec::new();

        let mut root_headers = metadata_headers(&metadata, metadata.size);
        root_headers.insert("Content-Location".to_string(), http_path(name));
        append_multipart_part(&mut body, MULTIPART_BOUNDARY, &root_headers, &root_body);

        for entry in entries {
            let child_name = child_path(name, &entry.name);
            let child_metadata = lstat(self.fsys.as_ref(), &child_name)?;
            let mut headers = metadata_headers(&child_metadata, child_metadata.size);
            headers.insert("Content-Location".to_string(), http_path(&child_name));
            let part_body = if child_metadata.is_dir() {
                directory_listing(self.fsys.as_ref(), &child_name)?
            } else {
                headers.remove("Content-Length");
                headers.insert(
                    "Content-Range".to_string(),
                    format!("bytes 0-0/{}", child_metadata.size),
                );
                Vec::new()
            };
            append_multipart_part(&mut body, MULTIPART_BOUNDARY, &headers, &part_body);
        }

        body.extend_from_slice(format!("--{MULTIPART_BOUNDARY}--\r\n").as_bytes());

        Ok(HttpResponse::new(200)
            .with_header(
                "Content-Type",
                format!("multipart/mixed; boundary={MULTIPART_BOUNDARY}"),
            )
            .with_header("Content-Length", body.len().to_string())
            .with_body(body))
    }

    fn apply_metadata_headers(
        &self,
        name: &str,
        headers: &BTreeMap<String, String>,
    ) -> std::result::Result<(), Error> {
        let _ = lstat(self.fsys.as_ref(), name)?;

        if let Some(mode) = header_value(headers, "Content-Mode") {
            ignore_unsupported(self.fsys.chmod(
                name,
                mode_header_or_default(Some(mode.as_str()), FileMode::from_perm(0o644)),
            ))?;
        }

        if let Some(ownership) = header_value(headers, "Content-Ownership") {
            let (uid, gid) = parse_ownership(&ownership);
            ignore_unsupported(self.fsys.chown(name, uid, gid))?;
        }

        if let Some(modified) =
            header_value(headers, "Content-Modified").and_then(|value| value.parse::<u64>().ok())
        {
            ignore_unsupported(self.fsys.chtimes(
                name,
                std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(modified),
            ))?;
        }

        Ok(())
    }
}

fn ignore_unsupported(result: std::result::Result<(), Error>) -> std::result::Result<(), Error> {
    match result {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == ErrorKind::NotSupported => Ok(()),
        Err(err) => Err(err),
    }
}

fn parse_request_target(url: &str) -> RequestTarget {
    let trimmed = url.trim();
    let without_fragment = trimmed.split('#').next().unwrap_or(trimmed);
    let path = if let Some((_, remainder)) = without_fragment.split_once("://") {
        remainder
            .find('/')
            .map(|index| &remainder[index..])
            .unwrap_or("/")
    } else if let Some(remainder) = without_fragment.strip_prefix("//") {
        remainder
            .find('/')
            .map(|index| &remainder[index..])
            .unwrap_or("/")
    } else {
        without_fragment
    };
    let path = path.split('?').next().unwrap_or(path);
    let had_trailing_slash = path.ends_with('/');
    let path = path.trim_start_matches('/').trim();
    let path = if path.is_empty() {
        ".".to_string()
    } else {
        clean_path(path)
    };
    RequestTarget {
        path,
        had_trailing_slash,
    }
}

fn accepts_multipart(headers: &BTreeMap<String, String>) -> bool {
    header_value(headers, "Accept").is_some_and(|accept| {
        accept
            .split(',')
            .any(|value| value.trim().eq_ignore_ascii_case("multipart/mixed"))
    })
}

fn directory_listing(
    fsys: &dyn crate::FileSystem,
    name: &str,
) -> std::result::Result<Vec<u8>, Error> {
    let mut body = String::new();
    for entry in read_dir(fsys, name)? {
        body.push_str(&entry.name);
        body.push(' ');
        body.push_str(&http_mode(entry.metadata.mode));
        body.push('\n');
    }
    Ok(body.into_bytes())
}

fn metadata_headers(metadata: &crate::Metadata, content_length: u64) -> BTreeMap<String, String> {
    BTreeMap::from([
        (
            "Content-Type".to_string(),
            content_type(metadata.mode).to_string(),
        ),
        ("Content-Length".to_string(), content_length.to_string()),
        ("Content-Mode".to_string(), http_mode(metadata.mode)),
        (
            "Content-Modified".to_string(),
            unix_secs(metadata.modified).to_string(),
        ),
        (
            "Content-Ownership".to_string(),
            format!("{}:{}", metadata.uid, metadata.gid),
        ),
    ])
}

fn content_type(mode: FileMode) -> &'static str {
    if mode.is_dir() {
        "application/x-directory"
    } else if mode.is_symlink() {
        "application/x-symlink"
    } else {
        "application/octet-stream"
    }
}

fn http_mode(mode: FileMode) -> String {
    mode.unix_type_and_perm().to_string()
}

fn mode_header_or_default(value: Option<&str>, default: FileMode) -> FileMode {
    let Some(value) = value else {
        return default;
    };
    let Ok(bits) = value.parse::<u32>() else {
        return default;
    };
    let perm = bits & 0o777;
    match bits & 0o170000 {
        0o040000 => FileMode::DIR | FileMode::from_perm(perm),
        0o120000 => FileMode::SYMLINK | FileMode::from_perm(perm),
        _ => FileMode::from_perm(perm),
    }
}

fn parse_ownership(value: &str) -> (u32, u32) {
    value
        .split_once(':')
        .map(|(uid, gid)| {
            (
                uid.parse::<u32>().unwrap_or(0),
                gid.parse::<u32>().unwrap_or(0),
            )
        })
        .unwrap_or((0, 0))
}

fn header_value(headers: &BTreeMap<String, String>, name: &str) -> Option<String> {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.clone())
}

fn http_path(name: &str) -> String {
    if name == "." {
        "/".to_string()
    } else {
        format!("/{}", clean_path(name))
    }
}

fn child_path(parent: &str, name: &str) -> String {
    if parent == "." {
        clean_path(name)
    } else {
        clean_path(&format!("{parent}/{name}"))
    }
}

fn append_multipart_part(
    body: &mut Vec<u8>,
    boundary: &str,
    headers: &BTreeMap<String, String>,
    part_body: &[u8],
) {
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    for (key, value) in headers {
        body.extend_from_slice(format!("{key}: {value}\r\n").as_bytes());
    }
    body.extend_from_slice(b"\r\n");
    body.extend_from_slice(part_body);
    body.extend_from_slice(b"\r\n");
}

fn unix_secs(time: std::time::SystemTime) -> u64 {
    time.duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn ok_response() -> HttpResponse {
    HttpResponse::new(200).with_body("OK")
}

fn method_not_allowed() -> HttpResponse {
    HttpResponse::new(405)
        .with_header("Allow", ALLOW_METHODS)
        .with_body("Method Not Allowed")
}

fn bad_request(message: &str) -> HttpResponse {
    HttpResponse::new(400).with_body(message)
}

fn response_from_error(err: Error) -> HttpResponse {
    let (status, body) = match err.kind() {
        ErrorKind::NotFound => (404, "Not Found".to_string()),
        ErrorKind::PermissionDenied => (403, "Permission Denied".to_string()),
        ErrorKind::AlreadyExists => (409, err.to_string()),
        ErrorKind::Invalid => (400, err.to_string()),
        ErrorKind::NotEmpty => (409, err.to_string()),
        ErrorKind::NotSupported => (501, err.to_string()),
        _ => (500, err.to_string()),
    };
    HttpResponse::new(status).with_body(body)
}

struct RequestTarget {
    path: String,
    had_trailing_slash: bool,
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::io::Cursor;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, SystemTime};

    use http::Method;

    use super::{http_mode, HttpFsHandler};
    use crate::{
        exists, fs_ref, lstat, read_dir, read_file, write_file, FileMode, FileSystem, FsRef,
        HttpFs, HttpRequest, HttpResponse, HttpTransport, MemFs, Result,
    };

    #[derive(Clone)]
    struct HandlerTransport {
        handler: HttpFsHandler,
        requests: Arc<Mutex<Vec<HttpRequest>>>,
    }

    impl HandlerTransport {
        fn new(fsys: FsRef) -> Self {
            Self {
                handler: HttpFsHandler::new(fsys),
                requests: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn requests(&self) -> Vec<HttpRequest> {
            self.requests.lock().unwrap().clone()
        }
    }

    impl HttpTransport for HandlerTransport {
        fn request(&self, request: HttpRequest) -> Result<HttpResponse> {
            self.requests.lock().unwrap().push(request.clone());
            self.handler.handle(request)
        }
    }

    fn request(method: Method, url: &str) -> HttpRequest {
        HttpRequest {
            method,
            url: url.to_string(),
            headers: BTreeMap::new(),
            body: Vec::new(),
        }
    }

    fn tar_patch_bytes(
        entries: impl IntoIterator<Item = (impl Into<String>, impl Into<Vec<u8>>)>,
    ) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for (path, body) in entries {
            let path = path.into();
            let body = body.into();
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Regular);
            header.set_mode(0o644);
            header.set_size(body.len() as u64);
            header.set_cksum();
            builder
                .append_data(&mut header, path, Cursor::new(body))
                .unwrap();
        }
        builder.finish().unwrap();
        builder.into_inner().unwrap()
    }

    fn tar_delete_patch(entries: impl IntoIterator<Item = (impl Into<String>, bool)>) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for (path, recursive) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Regular);
            header.set_mode(0);
            header.set_size(0);
            let mut pax = vec![("delete", b"".as_slice())];
            if recursive {
                pax.push(("recursive", b"1".as_slice()));
            }
            builder.append_pax_extensions(pax).unwrap();
            header.set_cksum();
            builder
                .append_data(&mut header, path.into(), Cursor::new(Vec::new()))
                .unwrap();
        }
        builder.finish().unwrap();
        builder.into_inner().unwrap()
    }

    #[test]
    fn httpfs_handler_get_file_returns_metadata_and_body() {
        let fs = MemFs::from_entries([("hello.txt", b"hello".to_vec())]);
        let handler = HttpFsHandler::new(fs_ref(fs.clone()));

        let response = handler
            .handle(request(
                Method::GET,
                "https://example.invalid/hello.txt?download=1",
            ))
            .unwrap();

        let metadata = lstat(&fs, "hello.txt").unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(response.body, b"hello");
        assert_eq!(response.headers["Content-Type"], "application/octet-stream");
        assert_eq!(response.headers["Content-Length"], "5");
        assert_eq!(response.headers["Content-Mode"], http_mode(metadata.mode));
        assert_eq!(
            response.headers["Content-Modified"],
            metadata
                .modified
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_secs()
                .to_string()
        );
        assert_eq!(response.headers["Content-Ownership"], "0:0");
    }

    #[test]
    fn httpfs_handler_head_returns_metadata_without_body() {
        let fs = MemFs::from_entries([("hello.txt", b"hello".to_vec())]);
        let handler = HttpFsHandler::new(fs_ref(fs.clone()));

        let response = handler.handle(request(Method::HEAD, "hello.txt")).unwrap();

        let metadata = lstat(&fs, "hello.txt").unwrap();
        assert_eq!(response.status, 200);
        assert!(response.body.is_empty());
        assert_eq!(
            response.headers["Content-Length"],
            metadata.size.to_string()
        );
        assert_eq!(response.headers["Content-Mode"], http_mode(metadata.mode));
    }

    #[test]
    fn httpfs_handler_get_directory_returns_plain_listing() {
        let fs = MemFs::new();
        fs.mkdir("sub", FileMode::from_perm(0o755)).unwrap();
        write_file(&fs, "file.txt", b"hello", FileMode::from_perm(0o644)).unwrap();
        let handler = HttpFsHandler::new(fs_ref(fs));

        let response = handler.handle(request(Method::GET, "/")).unwrap();

        assert_eq!(response.status, 200);
        assert_eq!(
            String::from_utf8(response.body).unwrap(),
            "file.txt 33188\nsub 16877\n"
        );
        assert_eq!(response.headers["Content-Type"], "application/x-directory");
        assert_eq!(response.headers["Content-Length"], "25");
    }

    #[test]
    fn httpfs_handler_multipart_directory_listing_is_parsed_by_httpfs() {
        let fs = MemFs::new();
        write_file(&fs, "alpha.txt", b"alpha", FileMode::from_perm(0o644)).unwrap();
        fs.mkdir("beta", FileMode::from_perm(0o755)).unwrap();
        fs.symlink("alpha.txt", "link").unwrap();
        let transport = Arc::new(HandlerTransport::new(fs_ref(fs)));
        let httpfs = HttpFs::new("https://example.invalid", transport.clone());

        let entries: Vec<_> = read_dir(&httpfs, ".")
            .unwrap()
            .into_iter()
            .map(|entry| (entry.name, entry.metadata.mode, entry.metadata.size))
            .collect();

        assert_eq!(
            entries,
            vec![
                ("alpha.txt".to_string(), FileMode::from_perm(0o644), 5),
                (
                    "beta".to_string(),
                    FileMode::DIR | FileMode::from_perm(0o755),
                    2
                ),
                (
                    "link".to_string(),
                    FileMode::SYMLINK | FileMode::from_perm(0o777),
                    9
                ),
            ]
        );

        let requests = transport.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, Method::GET);
        assert_eq!(requests[0].headers["Accept"], "multipart/mixed");
    }

    #[test]
    fn httpfs_handler_put_file_creates_contents_and_metadata() {
        let fs = MemFs::new();
        let handler = HttpFsHandler::new(fs_ref(fs.clone()));
        let modified = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let mut request = request(Method::PUT, "/note.txt?rev=2");
        request.headers = BTreeMap::from([
            (
                "Content-Type".to_string(),
                "application/octet-stream".to_string(),
            ),
            ("Content-Mode".to_string(), "33152".to_string()),
            ("Content-Ownership".to_string(), "7:8".to_string()),
            ("Content-Modified".to_string(), "1700000000".to_string()),
        ]);
        request.body = b"hello".to_vec();

        let response = handler.handle(request).unwrap();

        let metadata = lstat(&fs, "note.txt").unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(read_file(&fs, "note.txt").unwrap(), b"hello");
        assert_eq!(metadata.mode, FileMode::from_perm(0o600));
        assert_eq!(metadata.uid, 7);
        assert_eq!(metadata.gid, 8);
        assert_eq!(metadata.modified, modified);
    }

    #[test]
    fn httpfs_handler_put_directory_creates_directory() {
        let fs = MemFs::new();
        let handler = HttpFsHandler::new(fs_ref(fs.clone()));
        let mut request = request(Method::PUT, "folder/");
        request.headers = BTreeMap::from([
            (
                "Content-Type".to_string(),
                "application/x-directory".to_string(),
            ),
            ("Content-Mode".to_string(), "16832".to_string()),
        ]);

        let response = handler.handle(request).unwrap();

        let metadata = lstat(&fs, "folder").unwrap();
        assert_eq!(response.status, 200);
        assert!(metadata.is_dir());
        assert_eq!(metadata.mode, FileMode::DIR | FileMode::from_perm(0o700));
    }

    #[test]
    fn httpfs_handler_put_symlink_creates_link_target() {
        let fs = MemFs::new();
        let handler = HttpFsHandler::new(fs_ref(fs.clone()));
        let mut request = request(Method::PUT, "/link");
        request.headers = BTreeMap::from([(
            "Content-Type".to_string(),
            "application/x-symlink".to_string(),
        )]);
        request.body = b"target.txt".to_vec();

        let response = handler.handle(request).unwrap();

        assert_eq!(response.status, 200);
        assert_eq!(fs.readlink("link").unwrap(), "target.txt");
        assert!(lstat(&fs, "link").unwrap().mode.is_symlink());
    }

    #[test]
    fn httpfs_handler_delete_removes_existing_path() {
        let fs = MemFs::from_entries([("gone.txt", b"bye".to_vec())]);
        let handler = HttpFsHandler::new(fs_ref(fs.clone()));

        let response = handler
            .handle(request(Method::DELETE, "/gone.txt"))
            .unwrap();

        assert_eq!(response.status, 200);
        assert!(!exists(&fs, "gone.txt").unwrap());
    }

    #[test]
    fn httpfs_handler_move_renames_paths_with_full_destination_url() {
        let fs = MemFs::new();
        fs.mkdir("dst", FileMode::from_perm(0o755)).unwrap();
        write_file(&fs, "src.txt", b"move", FileMode::from_perm(0o644)).unwrap();
        let handler = HttpFsHandler::new(fs_ref(fs.clone()));
        let mut request = request(Method::from_bytes(b"MOVE").unwrap(), "/src.txt?from=1");
        request.headers = BTreeMap::from([(
            "Destination".to_string(),
            "https://example.invalid/dst/final.txt?overwrite=T".to_string(),
        )]);

        let response = handler.handle(request).unwrap();

        assert_eq!(response.status, 200);
        assert!(!exists(&fs, "src.txt").unwrap());
        assert_eq!(read_file(&fs, "dst/final.txt").unwrap(), b"move");
    }

    #[test]
    fn httpfs_handler_patch_updates_mode_only() {
        let fs = MemFs::from_entries([("chmod.txt", b"mode".to_vec())]);
        let handler = HttpFsHandler::new(fs_ref(fs.clone()));
        let mut request = request(Method::PATCH, "/chmod.txt");
        request.headers = BTreeMap::from([("Content-Mode".to_string(), "33184".to_string())]);

        let response = handler.handle(request).unwrap();

        assert_eq!(response.status, 200);
        assert_eq!(
            lstat(&fs, "chmod.txt").unwrap().mode,
            FileMode::from_perm(0o640)
        );
    }

    #[test]
    fn httpfs_handler_rejects_unsupported_methods() {
        let handler = HttpFsHandler::new(fs_ref(MemFs::new()));

        let response = handler.handle(request(Method::POST, "/")).unwrap();

        assert_eq!(response.status, 405);
        assert_eq!(
            response.headers["Allow"],
            "GET, HEAD, PUT, DELETE, PATCH, MOVE"
        );
    }

    #[test]
    fn httpfs_handler_patch_tar_via_httpfs_upserts_server_files() {
        let fs = MemFs::from_entries([("dir/file.txt", b"old".to_vec())]);
        let transport = Arc::new(HandlerTransport::new(fs_ref(fs.clone())));
        let httpfs = HttpFs::new("https://example.invalid", transport.clone());
        let patch = tar_patch_bytes([
            ("dir/file.txt", b"new".to_vec()),
            ("dir/nested.txt", b"fresh".to_vec()),
        ]);

        httpfs.patch_tar(".", patch).unwrap();

        assert_eq!(read_file(&fs, "dir/file.txt").unwrap(), b"new");
        assert_eq!(read_file(&fs, "dir/nested.txt").unwrap(), b"fresh");
        let requests = transport.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, Method::PATCH);
        assert_eq!(requests[0].headers["Content-Type"], "application/x-tar");
    }

    #[test]
    fn httpfs_handler_patch_tar_delete_markers_remove_paths_and_subtrees() {
        let fs = MemFs::from_entries([
            ("gone.txt", b"gone".to_vec()),
            ("keep.txt", b"keep".to_vec()),
            ("tree/branch/file.txt", b"leaf".to_vec()),
        ]);
        let transport = Arc::new(HandlerTransport::new(fs_ref(fs.clone())));
        let httpfs = HttpFs::new("https://example.invalid", transport);
        let patch = tar_delete_patch([("gone.txt", false), ("tree", true)]);

        httpfs.patch_tar(".", patch).unwrap();

        assert!(!exists(&fs, "gone.txt").unwrap());
        assert!(!exists(&fs, "tree").unwrap());
        assert_eq!(read_file(&fs, "keep.txt").unwrap(), b"keep");
    }

    #[test]
    fn httpfs_handler_invalid_tar_patch_returns_http_error_to_httpfs() {
        let fs = MemFs::new();
        let handler = HttpFsHandler::new(fs_ref(fs.clone()));
        let mut raw_request = request(Method::PATCH, "/dir");
        raw_request.headers =
            BTreeMap::from([("Content-Type".to_string(), "application/x-tar".to_string())]);
        raw_request.body = b"not a tar archive".to_vec();

        let response = handler.handle(raw_request).unwrap();
        assert!(!(200..=299).contains(&response.status));

        let transport = Arc::new(HandlerTransport::new(fs_ref(fs)));
        let httpfs = HttpFs::new("https://example.invalid", transport);
        let err = httpfs
            .patch_tar("dir", b"not a tar archive".to_vec())
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            format!("httpfs dir returned HTTP {}", response.status)
        );
    }
}
