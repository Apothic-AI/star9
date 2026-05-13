use std::collections::BTreeMap;
use std::io::Read;

use crate::{Error, HttpRequest, HttpResponse, HttpTransport, Result};

pub struct NativeHttpTransport;

impl NativeHttpTransport {
    pub fn new() -> Self {
        Self
    }
}

impl Default for NativeHttpTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpTransport for NativeHttpTransport {
    fn request(&self, request: HttpRequest) -> Result<HttpResponse> {
        let mut builder = ureq::request(request.method.as_str(), &request.url);
        for (name, value) in &request.headers {
            builder = builder.set(name, value);
        }

        let response = if request.body.is_empty() {
            ureq::OrAnyStatus::or_any_status(builder.call())
        } else {
            ureq::OrAnyStatus::or_any_status(builder.send_bytes(&request.body))
        }
        .map_err(|err| {
            Error::Message(format!(
                "native http transport failed for {} {}: {err}",
                request.method, request.url
            ))
        })?;

        let status = response.status();
        let headers = response
            .headers_names()
            .into_iter()
            .filter_map(|name| {
                response
                    .header(&name)
                    .map(|value| (name, value.to_string()))
            })
            .collect::<BTreeMap<_, _>>();

        let mut body = Vec::new();
        response
            .into_reader()
            .read_to_end(&mut body)
            .map_err(|err| {
                Error::Message(format!(
                    "native http transport failed reading response body for {} {}: {err}",
                    request.method, request.url
                ))
            })?;

        Ok(HttpResponse {
            status,
            headers,
            body,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::io::{self, Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::Arc;
    use std::thread;

    use http::Method;

    use super::NativeHttpTransport;
    use crate::{read_file, ErrorKind, HttpFs, HttpRequest, HttpTransport};

    #[derive(Debug)]
    struct CapturedRequest {
        method: String,
        path: String,
        headers: BTreeMap<String, String>,
        body: Vec<u8>,
    }

    #[test]
    fn native_http_transport_round_trips_request_and_response() {
        let (base_url, server) = spawn_test_server(
            concat!(
                "HTTP/1.1 201 Created\r\n",
                "Content-Type: application/octet-stream\r\n",
                "X-Reply: transport-ok\r\n",
                "Content-Length: 5\r\n",
                "Connection: close\r\n",
                "\r\n",
                "hello"
            )
            .as_bytes()
            .to_vec(),
        );

        let response = NativeHttpTransport::new()
            .request(HttpRequest {
                method: Method::PATCH,
                url: format!("{base_url}/tree/file.txt?mode=append"),
                headers: BTreeMap::from([
                    ("Content-Type".to_string(), "text/plain".to_string()),
                    ("X-Test".to_string(), "native-http".to_string()),
                ]),
                body: b"delta".to_vec(),
            })
            .unwrap();

        let request = server.join().unwrap().unwrap();
        assert_eq!(request.method, "PATCH");
        assert_eq!(request.path, "/tree/file.txt?mode=append");
        assert_eq!(header(&request.headers, "content-type"), Some("text/plain"));
        assert_eq!(header(&request.headers, "x-test"), Some("native-http"));
        assert_eq!(request.body, b"delta");

        assert_eq!(response.status, 201);
        assert_eq!(
            header(&response.headers, "content-type"),
            Some("application/octet-stream")
        );
        assert_eq!(header(&response.headers, "x-reply"), Some("transport-ok"));
        assert_eq!(response.body, b"hello");
    }

    #[test]
    fn native_http_transport_get_preserves_response_headers_and_body() {
        let (base_url, server) = spawn_test_server(
            concat!(
                "HTTP/1.1 200 OK\r\n",
                "Content-Type: application/octet-stream\r\n",
                "Content-Mode: 33188\r\n",
                "X-Reply: get-ok\r\n",
                "Content-Length: 5\r\n",
                "Connection: close\r\n",
                "\r\n",
                "hello"
            )
            .as_bytes()
            .to_vec(),
        );

        let response = NativeHttpTransport::new()
            .request(HttpRequest {
                method: Method::GET,
                url: format!("{base_url}/hello.txt"),
                headers: BTreeMap::from([("Accept".to_string(), "*/*".to_string())]),
                body: Vec::new(),
            })
            .unwrap();

        let request = server.join().unwrap().unwrap();
        assert_eq!(request.method, "GET");
        assert_eq!(request.path, "/hello.txt");
        assert_eq!(header(&request.headers, "accept"), Some("*/*"));
        assert_eq!(request.body, Vec::<u8>::new());

        assert_eq!(response.status, 200);
        assert_eq!(header(&response.headers, "content-mode"), Some("33188"));
        assert_eq!(header(&response.headers, "x-reply"), Some("get-ok"));
        assert_eq!(response.body, b"hello");
    }

    #[test]
    fn native_http_transport_404_maps_to_httpfs_not_found() {
        let (base_url, server) = spawn_test_server(
            concat!(
                "HTTP/1.1 404 Not Found\r\n",
                "Content-Type: text/plain\r\n",
                "Content-Length: 9\r\n",
                "Connection: close\r\n",
                "\r\n",
                "not here\n"
            )
            .as_bytes()
            .to_vec(),
        );

        let fs = HttpFs::new(base_url, Arc::new(NativeHttpTransport::new()));
        let err = read_file(&fs, "missing.txt").unwrap_err();
        assert_eq!(err.kind(), ErrorKind::NotFound);

        let request = server.join().unwrap().unwrap();
        assert_eq!(request.method, "GET");
        assert_eq!(request.path, "/missing.txt");
    }

    fn spawn_test_server(
        response: Vec<u8>,
    ) -> (String, thread::JoinHandle<io::Result<CapturedRequest>>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let handle = thread::spawn(move || -> io::Result<CapturedRequest> {
            let (mut stream, _) = listener.accept()?;
            stream.set_read_timeout(Some(std::time::Duration::from_secs(5)))?;
            let request = read_http_request(&mut stream)?;
            stream.write_all(&response)?;
            stream.flush()?;
            Ok(request)
        });
        (format!("http://{address}"), handle)
    }

    fn read_http_request(stream: &mut TcpStream) -> io::Result<CapturedRequest> {
        let mut buffer = Vec::new();
        let mut chunk = [0_u8; 1024];
        let header_end = loop {
            let read = stream.read(&mut chunk)?;
            if read == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "connection closed before request headers",
                ));
            }
            buffer.extend_from_slice(&chunk[..read]);
            if let Some(index) = find_bytes(&buffer, b"\r\n\r\n") {
                break index + 4;
            }
        };

        let header_text = std::str::from_utf8(&buffer[..header_end]).map_err(|err| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("request headers were not valid utf-8: {err}"),
            )
        })?;
        let mut lines = header_text.split("\r\n");
        let request_line = lines
            .next()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing request line"))?;
        let mut parts = request_line.split_whitespace();
        let method = parts
            .next()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing method"))?
            .to_string();
        let path = parts
            .next()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing path"))?
            .to_string();

        let mut headers = BTreeMap::new();
        let mut content_length = 0usize;
        for line in lines {
            if line.is_empty() {
                continue;
            }
            let (name, value) = line.split_once(':').ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "malformed request header")
            })?;
            let value = value.trim().to_string();
            if name.eq_ignore_ascii_case("content-length") {
                content_length = value.parse().map_err(|err| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("invalid content-length header: {err}"),
                    )
                })?;
            }
            headers.insert(name.to_ascii_lowercase(), value);
        }

        let mut body = buffer[header_end..].to_vec();
        while body.len() < content_length {
            let read = stream.read(&mut chunk)?;
            if read == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "connection closed before request body was complete",
                ));
            }
            body.extend_from_slice(&chunk[..read]);
        }
        body.truncate(content_length);

        Ok(CapturedRequest {
            method,
            path,
            headers,
            body,
        })
    }

    fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack
            .windows(needle.len())
            .position(|window| window == needle)
    }

    fn header<'a>(headers: &'a BTreeMap<String, String>, name: &str) -> Option<&'a str> {
        headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}
